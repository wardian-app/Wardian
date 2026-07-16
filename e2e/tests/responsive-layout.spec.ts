import { expect, test, type Page } from "@playwright/test";

import { surfacePanel } from "../fixtures/workbench";
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
