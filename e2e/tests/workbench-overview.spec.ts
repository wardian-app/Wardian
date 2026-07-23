import { expect, test, type Locator, type Page } from "@playwright/test";

import type { AgentsOverviewMode, WorkbenchDocumentV1 } from "../../src/types";
import type { WorkbenchLoadResult } from "../../src/features/workbench/workbenchPersistence";
import {
  installWorkbenchIpcMock,
  makeWorkbenchDocument,
  makeWorkbenchSurface,
  type WorkbenchAgentFixture,
  type WorkbenchIpcMockController,
  type WorkbenchIpcMockSnapshot,
} from "../fixtures/workbenchIpcMock";
import { surfacePanel, surfaceTab } from "../fixtures/workbench";

const ALPHA_AGENT: WorkbenchAgentFixture = {
  session_id: "agent-alpha",
  session_name: "Alpha",
  agent_class: "Coder",
  folder: "/workspace/alpha",
  provider: "mock",
  is_off: false,
};

const BETA_AGENT: WorkbenchAgentFixture = {
  session_id: "agent-beta",
  session_name: "Beta",
  agent_class: "Reviewer",
  folder: "/workspace/beta",
  provider: "mock",
  is_off: false,
};

function overviewDocument(
  mode: AgentsOverviewMode,
  focusedAgentId: string | null = null,
): WorkbenchDocumentV1 {
  const overview = makeWorkbenchSurface("overview-1", "agents-overview", {
    state: {
      mode,
      focused_agent_id: focusedAgentId,
      search_query: "",
      status_filter: [],
    },
  });
  return makeWorkbenchDocument({ surfaces: [overview] });
}

function primaryLoad(document: WorkbenchDocumentV1): WorkbenchLoadResult {
  return {
    source: "primary",
    document,
    notice: null,
    durable_revision: document.revision,
    durable_token: `mock-token-${document.revision}`,
  };
}

async function bootOverview(
  page: Page,
  options: {
    document: WorkbenchDocumentV1;
    agents: WorkbenchAgentFixture[];
    viewport: { width: number; height: number };
  },
): Promise<WorkbenchIpcMockController> {
  await page.setViewportSize(options.viewport);
  // Overview layout is presentation behavior. Chat cards keep this browser
  // suite below the native terminal/PTY boundary while retaining real floors.
  await page.addInitScript(() => {
    localStorage.setItem("wardian-settings", JSON.stringify({
      state: { gridCardDisplayMode: "chat" },
      version: 2,
    }));
  });
  const controller = await installWorkbenchIpcMock(page, {
    load_result: primaryLoad(options.document),
    agents: options.agents,
  });
  await page.goto("/");
  await expect(surfaceTab(page, "agents-overview")).toHaveAttribute("aria-selected", "true");
  await expect(surfacePanel(page, "agents-overview")).toBeVisible();
  return controller;
}

/** Task 16's sole public signal for computed Auto/Grid/Single presentation. */
function overviewMode(page: Page): Locator {
  return surfacePanel(page, "agents-overview").locator("[data-overview-mode]");
}

function visibleAgentCards(page: Page): Locator {
  return page.locator('[data-testid="agent-card"]:visible');
}

function persistedFocusedAgent(snapshot: WorkbenchIpcMockSnapshot): string | null {
  const state = snapshot.load_result.document?.surfaces["overview-1"]?.state;
  if (typeof state !== "object" || state === null || Array.isArray(state)) return null;
  const focused = (state as Record<string, unknown>).focused_agent_id;
  return typeof focused === "string" ? focused : null;
}

test("derives Auto from the measured container and keeps explicit Grid/Single stable", async ({ page }) => {
  const controller = await bootOverview(page, {
    document: overviewDocument("auto"),
    agents: [ALPHA_AGENT, BETA_AGENT],
    viewport: { width: 1920, height: 1080 },
  });
  const mode = overviewMode(page);
  const grid = page.getByTestId("agent-grid");

  await expect(mode).toHaveAttribute("data-overview-mode", "grid");
  const columnsBefore = await grid.evaluate((element) =>
    getComputedStyle(element).gridTemplateColumns.split(" ").filter(Boolean).length);

  await page.setViewportSize({ width: 1840, height: 1040 });
  await page.waitForTimeout(160);
  await expect(mode).toHaveAttribute("data-overview-mode", "grid");
  const columnsAfterSubThresholdResize = await grid.evaluate((element) =>
    getComputedStyle(element).gridTemplateColumns.split(" ").filter(Boolean).length);
  expect(columnsAfterSubThresholdResize).toBe(columnsBefore);

  await page.setViewportSize({ width: 900, height: 600 });
  await page.waitForTimeout(60);
  await expect(mode).toHaveAttribute("data-overview-mode", "grid");
  await expect.poll(async () => grid.evaluate((element) =>
    getComputedStyle(element).gridTemplateColumns.split(" ").filter(Boolean).length,
  )).toBe(1);

  await page.setViewportSize({ width: 960, height: 600 });
  await page.waitForTimeout(180);
  await expect(mode).toHaveAttribute("data-overview-mode", "grid");
  await page.setViewportSize({ width: 1920, height: 1080 });
  await expect(mode).toHaveAttribute("data-overview-mode", "grid", { timeout: 2_000 });

  await page.setViewportSize({ width: 900, height: 600 });
  await page.getByRole("button", { name: "Grid", exact: true }).click();
  await expect(mode).toHaveAttribute("data-overview-mode", "grid");
  await expect(visibleAgentCards(page)).toHaveCount(2);
  await expect.poll(async () => grid.evaluate((element) =>
    getComputedStyle(element).gridTemplateColumns.split(" ").filter(Boolean).length,
  )).toBe(2);

  await page.getByRole("button", { name: "Single", exact: true }).click();
  await expect(mode).toHaveAttribute("data-overview-mode", "single");
  await expect(visibleAgentCards(page)).toHaveCount(1);

  await expect.poll(async () => {
    const state = (await controller.snapshot()).load_result.document
      ?.surfaces["overview-1"]?.state;
    return typeof state === "object" && state !== null && !Array.isArray(state)
      ? (state as Record<string, unknown>).mode
      : null;
  }).toBe("single");
  await page.reload({ waitUntil: "domcontentloaded" });
  await expect(overviewMode(page)).toHaveAttribute("data-overview-mode", "single");
  await expect(visibleAgentCards(page)).toHaveCount(1);
});

test("restores persisted Single independently of roster targets and falls back when focus disappears", async ({ page }) => {
  const controller = await bootOverview(page, {
    document: overviewDocument("single", BETA_AGENT.session_id),
    agents: [ALPHA_AGENT, BETA_AGENT],
    viewport: { width: 1400, height: 800 },
  });
  const mode = overviewMode(page);

  await expect(mode).toHaveAttribute("data-overview-mode", "single");
  await expect(page.getByRole("button", { name: "Single", exact: true }))
    .toHaveAttribute("aria-pressed", "true");
  await expect(visibleAgentCards(page)).toHaveCount(1);
  await expect(page.locator("#agent-card-agent-beta")).toBeVisible();

  await page.getByLabel("Agent Alpha", { exact: true }).click({ modifiers: ["Control"] });
  await expect(surfaceTab(page, "agent-session", ALPHA_AGENT.session_id)).toHaveCount(0);
  await expect(page.getByLabel("Agent Alpha", { exact: true })).toHaveAttribute("data-selected", "true");
  await expect(page.locator("#agent-card-agent-beta")).toBeVisible();
  await expect(visibleAgentCards(page)).toHaveCount(1);

  await controller.setAgents([ALPHA_AGENT]);
  await expect(page.getByLabel("Agent Beta", { exact: true })).toHaveCount(0);
  await expect(page.locator("#agent-card-agent-alpha")).toBeVisible();
  await expect(visibleAgentCards(page)).toHaveCount(1);
  await expect(mode).toHaveAttribute("data-overview-mode", "single");

  await expect.poll(async () => persistedFocusedAgent(await controller.snapshot()))
    .toBe(ALPHA_AGENT.session_id);
  await page.reload({ waitUntil: "domcontentloaded" });
  await expect(overviewMode(page)).toHaveAttribute("data-overview-mode", "single");
  await expect(page.locator("#agent-card-agent-alpha")).toBeVisible();
  await expect(visibleAgentCards(page)).toHaveCount(1);
});

test("keeps later Grid row gutters aligned after scrolling", async ({ page }) => {
  const agents = [ALPHA_AGENT, BETA_AGENT, ...Array.from({ length: 4 }, (_, index) => ({
    session_id: `agent-extra-${index + 1}`,
    session_name: `Extra ${index + 1}`,
    agent_class: "Coder",
    folder: `/workspace/extra-${index + 1}`,
    provider: "mock",
    is_off: false,
  }))];
  await bootOverview(page, {
    document: overviewDocument("grid"),
    agents,
    viewport: { width: 1200, height: 650 },
  });

  const container = page.getByTestId("agents-overview-container");
  const grid = page.getByTestId("agent-grid");
  await expect(visibleAgentCards(page)).toHaveCount(6);
  const columns = await grid.evaluate((element) =>
    getComputedStyle(element).gridTemplateColumns.split(" ").filter(Boolean).length);
  const nextRowCard = visibleAgentCards(page).nth(columns * 2);
  const secondRowGutter = grid.locator('[data-resize-handle="v"]').nth(1);

  await container.evaluate((element) => {
    element.scrollTop = 760;
  });
  await page.evaluate(() => new Promise<void>((resolve) => requestAnimationFrame(() => resolve())));

  const cardBounds = await nextRowCard.boundingBox();
  const gutterBounds = await secondRowGutter.boundingBox();
  expect(cardBounds).not.toBeNull();
  expect(gutterBounds).not.toBeNull();
  const gutterCenter = gutterBounds!.y + (gutterBounds!.height / 2);
  expect(Math.abs(cardBounds!.y - gutterCenter - 6)).toBeLessThanOrEqual(1);
});
