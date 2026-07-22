import { expect, test } from "@playwright/test";

import { surfacePanel, surfaceTab } from "../fixtures/workbench";
import {
  installWorkbenchIpcMock,
  makeWorkbenchDocument,
  makeWorkbenchSurface,
  type WorkbenchAgentFixture,
} from "../fixtures/workbenchIpcMock";

const agents: WorkbenchAgentFixture[] = [
  {
    session_id: "agent-alpha",
    session_name: "Alpha",
    agent_class: "Coder",
    folder: "/workspace/alpha",
    provider: "mock",
    is_off: false,
  },
  {
    session_id: "agent-beta",
    session_name: "Beta",
    agent_class: "Reviewer",
    folder: "/workspace/beta",
    provider: "mock",
    is_off: false,
  },
];

test("renders a capture-ready tabs-and-splits workbench", async ({ page }, testInfo) => {
  const overview = makeWorkbenchSurface("overview-evidence", "agents-overview", {
    state: {
      mode: "grid",
      focused_agent_id: null,
      search_query: "",
      status_filter: [],
    },
  });
  const queue = makeWorkbenchSurface("inbox-evidence", "inbox");
  const document = makeWorkbenchDocument({
    revision: 2,
    root: {
      kind: "split",
      node_id: "evidence-split",
      direction: "horizontal",
      ratio: 0.7,
      first: { kind: "group", group_id: "group-overview" },
      second: { kind: "group", group_id: "group-queue" },
    },
    groups: {
      "group-overview": {
        group_id: "group-overview",
        surface_ids: [overview.surface_id],
        active_surface_id: overview.surface_id,
      },
      "group-queue": {
        group_id: "group-queue",
        surface_ids: [queue.surface_id],
        active_surface_id: queue.surface_id,
      },
    },
    surfaces: [overview, queue],
    active_group_id: "group-overview",
  });

  await page.setViewportSize({ width: 1920, height: 1080 });
  await page.addInitScript(() => {
    localStorage.setItem("wardian-settings", JSON.stringify({
      state: { gridCardDisplayMode: "chat" },
      version: 2,
    }));
  });
  await installWorkbenchIpcMock(page, {
    agents,
    load_result: {
      source: "primary",
      document,
      notice: null,
      durable_revision: document.revision,
      durable_token: "evidence-token-2",
    },
  });

  await page.goto("/");
  await expect(page.getByTestId("workbench-group")).toHaveCount(2);
  await expect(surfaceTab(page, "agents-overview")).toBeVisible();
  await expect(surfaceTab(page, "inbox")).toBeVisible();
  await expect(surfacePanel(page, "agents-overview")).toBeVisible();
  await expect(surfacePanel(page, "inbox")).toBeVisible();
  await expect(page.getByTestId("sidebar-icon-rail")).toBeVisible();
  await expect(page.getByTestId("agent-watchlist")).toBeVisible();
  await expect(page.locator('[data-testid="agent-card"]:visible')).toHaveCount(2);
  await expect(page.getByText("Saving workbench changes…", { exact: true })).toBeHidden();
  await page.waitForTimeout(500);

  const path = process.env.WARDIAN_WORKBENCH_SCREENSHOT
    ?? testInfo.outputPath("tabs-and-splits.png");
  await page.screenshot({ path, animations: "disabled" });
  await testInfo.attach("tabs-and-splits", { path, contentType: "image/png" });
});

test("renders a capture-ready new-tab surface launcher", async ({ page }, testInfo) => {
  const dashboard = makeWorkbenchSurface("dashboard-launcher-evidence", "dashboard");
  const document = makeWorkbenchDocument({ revision: 3, surfaces: [dashboard] });
  await page.setViewportSize({ width: 1440, height: 900 });
  await installWorkbenchIpcMock(page, {
    load_result: {
      source: "primary",
      document,
      notice: null,
      durable_revision: document.revision,
      durable_token: "launcher-evidence-token-3",
    },
  });

  await page.goto("/");
  const group = page.getByTestId("workbench-group").filter({
    has: surfaceTab(page, "dashboard"),
  });
  await group.getByLabel("Open Surface", { exact: true }).click();
  await expect(group.getByRole("tab", { name: "New Tab", exact: true }))
    .toHaveAttribute("aria-selected", "true");
  await expect(page.getByRole("heading", { name: "Choose a surface" })).toBeVisible();
  await expect(page.getByLabel("Available surfaces").getByRole("button")).toHaveCount(7);
  await expect(page.getByText("Monitor active agents.", { exact: true })).toBeVisible();
  await page.waitForTimeout(250);

  const path = process.env.WARDIAN_WORKBENCH_LAUNCHER_SCREENSHOT
    ?? testInfo.outputPath("surface-launcher.png");
  await page.screenshot({ path, animations: "disabled" });
  await testInfo.attach("surface-launcher", { path, contentType: "image/png" });
});
