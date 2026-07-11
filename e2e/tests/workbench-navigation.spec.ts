import { expect, test, type Page } from "@playwright/test";

import type { WorkbenchDocumentV1 } from "../../src/types";
import type { WorkbenchLoadResult } from "../../src/features/workbench/workbenchPersistence";
import {
  installWorkbenchIpcMock,
  makeWorkbenchDocument,
  makeWorkbenchSurface,
  type WorkbenchAgentFixture,
} from "../fixtures/workbenchIpcMock";
import {
  activeWorkbenchGroup,
  openSurface,
  surfaceTab,
  type CoreWorkbenchSurfaceType,
} from "../fixtures/workbench";

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

const CORE_SINGLETON_SURFACES = [
  "agents-overview",
  "dashboard",
  "queue",
  "graph",
  "garden",
  "library",
  "workflows",
] as const satisfies readonly CoreWorkbenchSurfaceType[];

function primaryLoad(document: WorkbenchDocumentV1): WorkbenchLoadResult {
  return {
    source: "primary",
    document,
    notice: null,
    durable_revision: document.revision,
    durable_token: `mock-token-${document.revision}`,
  };
}

async function bootWorkbench(
  page: Page,
  document: WorkbenchDocumentV1,
  agents: WorkbenchAgentFixture[] = [],
): Promise<void> {
  // Browser navigation tests do not make terminal-runtime claims. Keeping the
  // Overview preference on Chat avoids coupling this suite to native PTY IPC.
  await page.addInitScript(() => {
    localStorage.setItem("wardian-settings", JSON.stringify({
      state: { gridCardDisplayMode: "chat" },
      version: 2,
    }));
  });
  await installWorkbenchIpcMock(page, {
    load_result: primaryLoad(document),
    agents,
  });
  await page.goto("/");
  await expect(page.getByTestId("workbench-host")).toBeVisible();
  await expect(page.getByTestId("workbench-group")).toHaveCount(
    Object.keys(document.groups).length,
  );
}

function twoGroupDocument(): WorkbenchDocumentV1 {
  const dashboard = makeWorkbenchSurface("dashboard-1", "dashboard");
  const queue = makeWorkbenchSurface("queue-1", "queue");
  const graph = makeWorkbenchSurface("graph-1", "graph");
  return makeWorkbenchDocument({
    root: {
      kind: "split",
      node_id: "split-1",
      direction: "horizontal",
      ratio: 0.5,
      first: { kind: "group", group_id: "group-1" },
      second: { kind: "group", group_id: "group-2" },
    },
    groups: {
      "group-1": {
        group_id: "group-1",
        surface_ids: [dashboard.surface_id, queue.surface_id],
        active_surface_id: dashboard.surface_id,
      },
      "group-2": {
        group_id: "group-2",
        surface_ids: [graph.surface_id],
        active_surface_id: graph.surface_id,
      },
    },
    surfaces: [dashboard, queue, graph],
    active_group_id: "group-1",
  });
}

test("opens every migrated surface and focuses an existing singleton", async ({ page }) => {
  await bootWorkbench(page, makeWorkbenchDocument(), [ALPHA_AGENT]);

  for (const surfaceType of CORE_SINGLETON_SURFACES) {
    await openSurface(page, surfaceType);
  }

  await page.getByLabel("Agent Alpha", { exact: true }).click();
  await openSurface(page, "agent-session", ALPHA_AGENT.session_id);

  for (const surfaceType of CORE_SINGLETON_SURFACES) {
    await expect(surfaceTab(page, surfaceType)).toHaveCount(1);
  }
  await expect(surfaceTab(page, "agent-session", ALPHA_AGENT.session_id)).toHaveCount(1);

  await surfaceTab(page, "queue").click();
  await openSurface(page, "queue");
  await expect(surfaceTab(page, "queue")).toHaveCount(1);
  await expect(surfaceTab(page, "queue")).toHaveAttribute("aria-selected", "true");
});

test("splits, moves, zooms, joins, closes, and reopens through semantic controls", async ({ page }) => {
  const dashboard = makeWorkbenchSurface("dashboard-1", "dashboard");
  const queue = makeWorkbenchSurface("queue-1", "queue");
  await bootWorkbench(page, makeWorkbenchDocument({ surfaces: [dashboard, queue] }));

  const groups = page.getByTestId("workbench-group");
  const groupOne = groups.and(page.locator('[data-group-id="group-1"]'));
  await groupOne.getByLabel("Split group-1 right", { exact: true }).click();
  await expect(groups).toHaveCount(2);

  await expect.poll(async () => activeWorkbenchGroup(page).getAttribute("data-group-id"))
    .not.toBe("group-1");
  const newGroupId = await activeWorkbenchGroup(page).getAttribute("data-group-id");
  expect(newGroupId).toBeTruthy();

  await surfaceTab(page, "dashboard").click();
  await expect(groupOne).toHaveAttribute("data-active", "true");
  await page.getByRole("button", { name: "Move Tab to Next Group", exact: true }).click();

  const newGroup = groups.and(page.locator(`[data-group-id=${JSON.stringify(newGroupId)}]`));
  await expect(newGroup.getByRole("tab", { name: "Dashboard", exact: true })).toBeVisible();
  await expect(newGroup).toHaveAttribute("data-active", "true");

  await page.getByRole("button", { name: "Toggle Group Zoom", exact: true }).click();
  await expect(page.getByTestId("workbench-host")).toHaveAttribute(
    "data-zoomed-group-id",
    newGroupId!,
  );
  await expect(groups).toHaveCount(2);
  await page.getByRole("button", { name: "Toggle Group Zoom", exact: true }).click();
  await expect(page.getByTestId("workbench-host")).toHaveAttribute(
    "data-zoomed-group-id",
    "none",
  );

  await newGroup.getByLabel(`Join ${newGroupId} into group-1`, { exact: true }).click();
  await expect(groups).toHaveCount(1);
  await expect(groupOne.getByRole("tab")).toHaveCount(2);

  await groupOne.getByLabel("Close group-1", { exact: true }).click();
  await expect(groupOne.getByRole("tab")).toHaveCount(0);
  await expect(groupOne.getByRole("heading", { name: "New Surface" })).toBeVisible();

  await groupOne.getByRole("button", { name: /^Reopen / }).click();
  await expect(groupOne.getByRole("tab")).toHaveCount(1);
});

test("traverses tabs and groups with workbench keyboard commands", async ({ page }) => {
  await bootWorkbench(page, twoGroupDocument());

  const dashboardTab = surfaceTab(page, "dashboard");
  const queueTab = surfaceTab(page, "queue");
  const graphTab = surfaceTab(page, "graph");

  await dashboardTab.focus();
  // Dispatch the exact browser key value used by the command router. Physical
  // `Control+]` is keyboard-layout dependent on Windows CI.
  await dashboardTab.dispatchEvent("keydown", {
    key: "]",
    code: "BracketRight",
    ctrlKey: true,
    bubbles: true,
    cancelable: true,
  });
  await expect(queueTab).toHaveAttribute("aria-selected", "true");
  await expect(queueTab).toBeFocused();

  await page.keyboard.press("F6");
  await expect(graphTab).toHaveAttribute("aria-selected", "true");
  await expect(graphTab).toBeFocused();
  await expect(activeWorkbenchGroup(page)).toHaveAttribute("data-group-id", "group-2");

  await page.keyboard.press("Shift+F6");
  await expect(queueTab).toBeFocused();
  await expect(activeWorkbenchGroup(page)).toHaveAttribute("data-group-id", "group-1");
});

test("keeps the left rail auxiliary while routing its object action to a surface", async ({ page }) => {
  const dashboard = makeWorkbenchSurface("dashboard-1", "dashboard");
  await bootWorkbench(page, makeWorkbenchDocument({ surfaces: [dashboard] }));

  await page.getByTestId("sidebar-tab-workflows").click();
  await expect(surfaceTab(page, "dashboard")).toHaveAttribute("aria-selected", "true");
  await expect(surfaceTab(page, "workflows")).toHaveCount(0);

  await page.getByRole("button", { name: "Monitor", exact: true }).click();
  await expect(surfaceTab(page, "workflows")).toHaveCount(1);
  await expect(surfaceTab(page, "workflows")).toHaveAttribute("aria-selected", "true");
});

test("separates roster selection from open and duplicates Agent Session only to side", async ({ page }) => {
  const dashboard = makeWorkbenchSurface("dashboard-1", "dashboard");
  await bootWorkbench(
    page,
    makeWorkbenchDocument({ surfaces: [dashboard] }),
    [ALPHA_AGENT, BETA_AGENT],
  );

  const alphaRow = page.getByLabel("Agent Alpha", { exact: true });
  await alphaRow.click();
  await expect(surfaceTab(page, "agent-session", ALPHA_AGENT.session_id)).toHaveCount(0);

  await activeWorkbenchGroup(page).getByLabel("Open Surface", { exact: true }).click();
  const launcher = page.getByRole("dialog", { name: "Open Surface", exact: true });
  const agentSessionOption = launcher
    .getByRole("option")
    .and(page.locator('[data-surface-type="agent-session"]'));
  await expect(agentSessionOption).toHaveAttribute("aria-disabled", "false");
  await page.keyboard.press("Escape");

  await alphaRow.dblclick();
  await expect(surfaceTab(page, "agent-session", ALPHA_AGENT.session_id)).toHaveCount(1);

  await alphaRow.click();
  await activeWorkbenchGroup(page).getByLabel("Open Surface", { exact: true }).click();
  await page.getByRole("button", { name: "Open Agent Session to Side", exact: true }).click();
  await expect(surfaceTab(page, "agent-session", ALPHA_AGENT.session_id)).toHaveCount(2);
  await expect(page.getByTestId("workbench-group")).toHaveCount(2);
});

test("cancelling one dirty close guard leaves the complete group unchanged", async ({ page }) => {
  const dashboard = makeWorkbenchSurface("dashboard-1", "dashboard");
  const workflows = makeWorkbenchSurface("workflows-1", "workflows");
  const document = makeWorkbenchDocument({
    surfaces: [dashboard, workflows],
    groups: {
      "group-1": {
        group_id: "group-1",
        surface_ids: [dashboard.surface_id, workflows.surface_id],
        active_surface_id: workflows.surface_id,
      },
    },
  });
  await bootWorkbench(page, document);

  await page.getByRole("textbox", { name: "Workflow name" }).fill("Edited workflow");
  const group = activeWorkbenchGroup(page);
  await group.getByLabel("Close group-1", { exact: true }).click();

  const prompt = page.getByRole("dialog", { name: "Unsaved Workflows changes" });
  await expect(prompt).toBeVisible();
  await prompt.getByRole("button", { name: "Cancel", exact: true }).click();
  await expect(group.getByRole("tab")).toHaveCount(2);
  await expect(surfaceTab(page, "workflows")).toHaveAttribute("aria-selected", "true");

  await group.getByLabel("Close group-1", { exact: true }).click();
  await page.getByRole("dialog", { name: "Unsaved Workflows changes" })
    .getByRole("button", { name: "Discard", exact: true })
    .click();
  await expect(group.getByRole("tab")).toHaveCount(0);
  await expect(group.getByRole("heading", { name: "New Surface" })).toBeVisible();
});
