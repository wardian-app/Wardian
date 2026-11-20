import fs from "node:fs";
import path from "node:path";
import { expect, test, type Page } from "@playwright/test";
import type { AgentChatEvent, AgentChatPage } from "../../src/types";

import { surfacePanel, surfaceTab, waitForStableBoundingBoxes } from "../fixtures/workbench";
import {
  installWorkbenchIpcMock,
  makeWorkbenchDocument,
  makeWorkbenchSurface,
  type WorkbenchAgentFixture,
} from "../fixtures/workbenchIpcMock";

const responsiveAgents: WorkbenchAgentFixture[] = [
  {
    session_id: "responsive-agent-1",
    session_name: "Responsive Alpha",
    agent_class: "Coder",
    folder: "/workspace/responsive-alpha",
    provider: "mock",
    is_off: false,
  },
  {
    session_id: "responsive-agent-2",
    session_name: "Responsive Beta",
    agent_class: "Reviewer",
    folder: "/workspace/responsive-beta",
    provider: "mock",
    is_off: false,
  },
];

async function installResponsiveWorkbench(page: Page, mode: "auto" | "grid" | "single") {
  const overview = makeWorkbenchSurface("responsive-overview", "agents-overview", {
    state: {
      mode,
      focused_agent_id: null,
      search_query: "",
      status_filter: [],
    },
  });
  const document = makeWorkbenchDocument({
    surfaces: [overview],
    shell: { left_sidebar_width: 240 },
  });
  await page.addInitScript(() => {
    localStorage.setItem("wardian-settings", JSON.stringify({
      state: { gridCardDisplayMode: "terminal" },
      version: 2,
    }));
  });
  return installWorkbenchIpcMock(page, {
    agents: responsiveAgents,
    load_result: {
      source: "primary",
      document,
      notice: null,
      durable_revision: document.revision,
      durable_token: "responsive-token-0",
    },
  });
}

test.describe("responsive layout", () => {
  test("left sidebar width persists through canonical workbench state", async ({ page }) => {
    const ipc = await installResponsiveWorkbench(page, "auto");
    await page.goto("/", { waitUntil: "domcontentloaded" });
    const handle = page.getByTestId("sidebar-resize-handle").first();
    const sidebar = handle.locator("xpath=ancestor::aside[1]");
    const initial = await sidebar.evaluate((element) => element.getBoundingClientRect().width);

    const box = await handle.boundingBox();
    if (!box) throw new Error("handle not visible");
    await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
    await page.mouse.down();
    await page.mouse.move(box.x + 60, box.y + box.height / 2, { steps: 10 });
    await page.mouse.up();

    const grown = await sidebar.evaluate((element) => element.getBoundingClientRect().width);
    expect(grown).toBeGreaterThan(initial + 30);
    await expect.poll(async () => {
      const calls = await ipc.calls("save_workbench_state");
      const last = calls.at(-1)?.args as {
        document?: { shell?: { left_sidebar_width?: number } };
      } | undefined;
      return last?.document?.shell?.left_sidebar_width ?? null;
    }).toBe(Math.round(grown));

    await page.reload({ waitUntil: "domcontentloaded" });
    await expect.poll(async () => Math.round(await sidebar.evaluate(
      (element) => element.getBoundingClientRect().width,
    ))).toBe(Math.round(grown));
  });

  test("Auto stacks the roster at the hard floor and Minimize restores the full roster", async ({ page }) => {
    await page.setViewportSize({ width: 900, height: 600 });
    await installResponsiveWorkbench(page, "auto");
    await page.goto("/", { waitUntil: "domcontentloaded" });

    const overview = surfacePanel(page, "agents-overview");
    const grid = overview.locator('[data-testid="agent-grid"]');
    const cards = overview.locator('[data-testid="agent-card"]:visible');
    await expect(grid).toHaveAttribute("data-overview-mode", "grid");
    await expect(cards).toHaveCount(2);
    await expect.poll(() => grid.evaluate(
      (element) => getComputedStyle(element).gridTemplateColumns.split(" ").filter(Boolean).length,
    )).toBe(1);

    await overview.getByRole("button", { name: "Single", exact: true }).click();
    await expect(grid).toHaveAttribute("data-overview-mode", "single");
    await expect(cards).toHaveCount(1);

    await overview.getByRole("button", { name: /^Minimize / }).click();
    await expect(overview.getByRole("button", { name: "Auto", exact: true }))
      .toHaveAttribute("aria-pressed", "true");
    await expect(grid).toHaveAttribute("data-overview-mode", "grid");
    await expect(cards).toHaveCount(2);
    await expect.poll(() => grid.evaluate(
      (element) => getComputedStyle(element).gridTemplateColumns.split(" ").filter(Boolean).length,
    )).toBe(1);
  });
});

test("desktop Chat transcript and composer share the technical lane in a split workbench", async ({ page }, testInfo) => {
  const screenshotDir = process.env.WARDIAN_CHAT_RECOVERY_SCREENSHOT_DIR;
  if (screenshotDir) {
    const relative = path.relative(path.resolve("e2e/screenshots/chat"), path.resolve(screenshotDir));
    expect(path.isAbsolute(screenshotDir) && relative.length > 0 && !path.isAbsolute(relative)
      && !relative.split(path.sep).includes(".."), "capture output must be an owned e2e/screenshots/chat/<timestamp> directory").toBe(true);
    fs.mkdirSync(screenshotDir, { recursive: true });
  }
  const overview = makeWorkbenchSurface("chat-lane-overview", "agents-overview", {
    state: { mode: "single", focused_agent_id: responsiveAgents[0].session_id, search_query: "", status_filter: [] },
  });
  const reference = makeWorkbenchSurface("chat-lane-reference", "new-tab");
  const workbench = makeWorkbenchDocument({
    surfaces: [overview, reference],
    root: { kind: "split", node_id: "chat-lane-split", direction: "horizontal", ratio: 0.5,
      first: { kind: "group", group_id: "chat-lane" }, second: { kind: "group", group_id: "reference" } },
    groups: {
      "chat-lane": { group_id: "chat-lane", surface_ids: [overview.surface_id], active_surface_id: overview.surface_id },
      reference: { group_id: "reference", surface_ids: [reference.surface_id], active_surface_id: reference.surface_id },
    },
    active_group_id: "chat-lane",
    shell: { left_sidebar_collapsed: false, right_sidebar_collapsed: false, left_sidebar_width: 240 },
  });
  const response: AgentChatPage = {
    session_id: responsiveAgents[0].session_id, conversation_id: "layout-conversation", generation: "layout-generation",
    source_epoch: null, revision: "layout-revision", next_before: null, unchanged: false, reset: false,
    progress: "ready", aliases: [], removed_ids: [], detail: null, bytes_read: 0, records_decoded: 0,
    events: [{ id: "layout-message", session_id: responsiveAgents[0].session_id, provider: "mock", kind: "message",
      role: "assistant", text: "Technical layout proof. ".repeat(80) + "\n\n```text\n" + "source_".repeat(60) + "\n```",
      title: null, status: null, turn_id: "layout-turn", source: "watch", command: null, exit_code: null,
      path: null, language: null, created_at: "2026-01-01T00:00:00Z", sequence: 1, metadata: {} }],
  };
  if (screenshotDir) {
    response.events[0] = { ...response.events[0], text: [
      "Review the pane measurements and keep the composer aligned with the technical transcript.",
      "```typescript\nconst laneWidth = Math.min(paneWidth, 1152);\nrenderTranscript({ laneWidth });\n```",
      "| Check | Evidence | State |\n| --- | --- | --- |\n| Code | Local overflow | Ready |\n| Composer | Shared lane | Ready |",
    ].join("\n\n") };
    response.events.push(...[
      { title: "Inspect source", text: "Inspected the synthetic Chat layout fixture.", command: "rg --files src/features/chat" },
      { title: "Check layout", text: "Checked the synthetic transcript and composer alignment.", command: "npm run test -- AgentChatView.test.tsx" },
      { title: "Summarize work", text: "Summarized the synthetic capture preparation.", command: "git diff --stat" },
    ].map((tool, index): AgentChatEvent => ({ ...response.events[0], id: `layout-work-${index + 1}`,
      kind: "tool_call", role: null, text: tool.text, title: tool.title, status: "succeeded",
      source: "provider_log", command: tool.command, exit_code: 0, language: "shell", sequence: index + 2 })));
  }
  await page.addInitScript(() => localStorage.setItem("wardian-settings", JSON.stringify({
    state: { gridCardDisplayMode: "chat" }, version: 2,
  })));
  await installWorkbenchIpcMock(page, {
    agents: [responsiveAgents[0]], responses: { load_agent_chat_page: response },
    load_result: { source: "primary", document: workbench, notice: null, durable_revision: workbench.revision, durable_token: "chat-lane-state" },
  });
  await page.goto("/", { waitUntil: "domcontentloaded" });
  const chat = page.locator(".agent-chat-view");
  await expect(chat.locator(".chat-markdown")).toBeVisible();
  const workGroup = chat.getByTestId("chat-work-group");
  if (screenshotDir) {
    await expect(workGroup).toHaveCount(1);
    await workGroup.getByTitle("Show all work", { exact: true }).click();
    await expect(workGroup).toHaveAttribute("data-expanded", "true");
    await expect(workGroup).toContainText("3 events");
  }
  const capture = async (name: string) => {
    if (!screenshotDir) throw new Error("Capture requires the explicit screenshot opt-in");
    const technical = chat.locator(".chat-markdown").first();
    await expect(workGroup).toHaveAttribute("data-expanded", "true");
    for (const content of [technical.locator(".chat-prose-block").first(), technical.locator("pre"),
      technical.locator("table"), workGroup, chat.locator(".chat-composer")]) {
      await expect(content).toBeInViewport({ ratio: 1 });
    }
    const imagePath = path.join(screenshotDir, name);
    await page.screenshot({ path: imagePath, type: "png", animations: "disabled" });
    await testInfo.attach(name, { path: imagePath, contentType: "image/png" });
  };
  const geometry = [];
  let capturedSplit = false;
  for (const width of [1440, 1792, 1920, 2112]) {
    await page.setViewportSize({ width, height: 1080 });
    // The workbench's ResizeObserver commits pane sizes on the next frame.
    await page.evaluate(() => new Promise<void>((resolve) => requestAnimationFrame(() => requestAnimationFrame(() => resolve()))));
    const sample = await chat.evaluate((element) => {
      const box = (selector: string) => {
        const node = element.querySelector(selector);
        if (!(node instanceof HTMLElement)) throw new Error(`Missing ${selector}`);
        const rect = node.getBoundingClientRect();
        return { left: rect.left, width: rect.width };
      };
      return { pane: element.getBoundingClientRect().width,
        lane: box(".chat-transcript-list"), composer: box(".chat-composer"),
        document: document.documentElement.scrollWidth, viewport: innerWidth };
    });
    geometry.push({ width, ...sample });
    expect(sample.pane).toBeLessThan(width / 2);
    expect(sample.lane.width).toBeLessThanOrEqual(1152);
    expect(sample.lane.width).toBeGreaterThan(sample.pane - 40);
    expect(Math.abs(sample.lane.width - sample.composer.width)).toBeLessThanOrEqual(1);
    expect(Math.abs(sample.lane.left - sample.composer.left)).toBeLessThanOrEqual(1);
    expect(sample.document).toBeLessThanOrEqual(sample.viewport + 1);
    if (screenshotDir && !capturedSplit && sample.pane >= 600 && sample.pane <= 760) {
      const [pane, lane, composer] = await waitForStableBoundingBoxes(page,
        [chat, chat.locator(".chat-transcript-list"), chat.locator(".chat-composer")]);
      expect(pane.width).toBeGreaterThanOrEqual(600);
      expect(pane.width).toBeLessThanOrEqual(760);
      await capture("desktop-split-chat-width.png");
      await testInfo.attach("desktop-split-chat-width-geometry", {
        body: JSON.stringify({ outerViewport: page.viewportSize(), pane, lane, composer, loopSample: sample }, null, 2),
        contentType: "application/json",
      });
      capturedSplit = true;
    }
  }
  await testInfo.attach("desktop-split-chat-geometry", { body: JSON.stringify(geometry, null, 2), contentType: "application/json" });
  if (screenshotDir) {
    expect(capturedSplit, "no measured Chat pane in the requested 600–760px range was reached").toBe(true);
    // Surface content is portaled outside its header; the tab identifies its owning pane.
    const group = surfaceTab(page, "agents-overview")
      .locator('xpath=ancestor::*[@data-testid="workbench-group"][1]');
    await group.getByLabel("Pane actions", { exact: true }).click();
    await page.getByRole("menu", { name: "Pane actions", exact: true })
      .getByRole("menuitem", { name: "Zoom pane", exact: true }).click();
    if (await workGroup.getAttribute("data-expanded") !== "true") {
      await workGroup.getByTitle("Show all work", { exact: true }).click();
    }
    const [pane, lane, composer] = await waitForStableBoundingBoxes(page,
      [chat, chat.locator(".chat-transcript-list"), chat.locator(".chat-composer")]);
    expect(page.viewportSize()?.width).toBe(2112);
    expect(pane.width).toBeGreaterThan(1152);
    expect(lane.width).toBeLessThanOrEqual(1152);
    expect(lane.width).toBeCloseTo(1152, 0);
    expect(Math.abs(lane.width - composer.width)).toBeLessThanOrEqual(1);
    expect(Math.abs(lane.x - composer.x)).toBeLessThanOrEqual(1);
    await capture("desktop-wide-chat-width.png");
    await testInfo.attach("desktop-wide-chat-width-geometry", {
      body: JSON.stringify({ outerViewport: page.viewportSize(), pane, lane, composer }, null, 2), contentType: "application/json",
    });
  }
});
