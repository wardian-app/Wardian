import { expect, test, type Locator, type Page } from "@playwright/test";

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

async function choosePaneAction(
  page: Page,
  group: Locator,
  action: string,
): Promise<void> {
  await group.getByLabel("Pane actions", { exact: true }).click();
  await page.getByRole("menu", { name: "Pane actions", exact: true })
    .getByRole("menuitem", { name: action, exact: true })
    .click();
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

test("uses real top-edge tab groups as responsive window chrome", async ({ page }) => {
  await bootWorkbench(page, twoGroupDocument());

  await expect(page.getByTestId("titlebar-center")).toHaveCount(0);
  const groups = page.getByTestId("workbench-group");
  for (const groupId of ["group-1", "group-2"]) {
    const group = groups.and(page.locator(`[data-group-id="${groupId}"]`));
    const header = group.locator(":scope > .dv-tabs-and-actions-container");
    await expect(header).toBeVisible();
    const bounds = await header.boundingBox();
    expect(bounds?.y).toBe(0);
    expect(bounds?.height).toBe(36);
    await expect(header.locator(".dv-void-container"))
      .toHaveAttribute("data-tauri-drag-region", "");
  }

  const firstGroup = groups.and(page.locator('[data-group-id="group-1"]'));
  const finalTab = firstGroup.getByRole("tab").last();
  const newSurface = firstGroup.getByLabel("Open Surface", { exact: true });
  const [tabBounds, actionBounds] = await Promise.all([
    finalTab.boundingBox(),
    newSurface.boundingBox(),
  ]);
  expect(tabBounds).not.toBeNull();
  expect(actionBounds).not.toBeNull();
  const separation = actionBounds!.x - (tabBounds!.x + tabBounds!.width);
  expect(separation).toBeGreaterThanOrEqual(-1);
  expect(separation).toBeLessThanOrEqual(8);
});

test("keeps the first tab clear of collapsed macOS traffic-light chrome", async ({ browser }) => {
  const context = await browser.newContext({
    userAgent: "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36",
  });
  const page = await context.newPage();
  try {
    await bootWorkbench(page, twoGroupDocument());
    await page.getByRole("button", { name: "Hide Left Sidebar", exact: true }).click();

    const titlebar = page.locator(".titlebar");
    await expect(titlebar).toHaveAttribute("data-platform", "mac");
    await expect(titlebar).toHaveAttribute("data-left-collapsed", "true");

    const firstGroup = page.getByTestId("workbench-group")
      .and(page.locator('[data-group-id="group-1"]'));
    const header = firstGroup.locator(":scope > .dv-tabs-and-actions-container");
    const firstTab = firstGroup.getByRole("tab").first();
    await expect(header).toHaveAttribute("data-left-chrome-clearance", "true");

    const [leftChromeBounds, firstTabBounds] = await Promise.all([
      page.locator(".titlebar-left").boundingBox(),
      firstTab.boundingBox(),
    ]);
    expect(leftChromeBounds).not.toBeNull();
    expect(firstTabBounds).not.toBeNull();
    expect(leftChromeBounds!.x + leftChromeBounds!.width)
      .toBeLessThanOrEqual(firstTabBounds!.x + 1);

    await firstGroup.getByRole("tab").nth(1).click();
    await firstTab.click();
    await expect(firstTab).toHaveAttribute("aria-selected", "true");
  } finally {
    await context.close();
  }
});

test("keeps a downward split header local instead of making it window chrome", async ({ page }) => {
  const dashboard = makeWorkbenchSurface("dashboard-1", "dashboard");
  await bootWorkbench(page, makeWorkbenchDocument({ surfaces: [dashboard] }));

  const groups = page.getByTestId("workbench-group");
  const topGroup = groups.and(page.locator('[data-group-id="group-1"]'));
  await choosePaneAction(page, topGroup, "Split pane down");
  await expect(groups).toHaveCount(2);

  const headers = groups.locator(":scope > .dv-tabs-and-actions-container");
  const firstBounds = await headers.nth(0).boundingBox();
  const secondBounds = await headers.nth(1).boundingBox();
  expect(Math.min(firstBounds!.y, secondBounds!.y)).toBe(0);
  expect(Math.max(firstBounds!.y, secondBounds!.y)).toBeGreaterThan(36);

  const lowerHeader = firstBounds!.y > secondBounds!.y ? headers.nth(0) : headers.nth(1);
  await expect(lowerHeader.locator(".dv-void-container"))
    .not.toHaveAttribute("data-tauri-drag-region", "");
});

test("offers a responsive keyboard-accessible launcher in an empty tab", async ({ page }) => {
  await page.setViewportSize({ width: 620, height: 720 });
  await bootWorkbench(page, makeWorkbenchDocument({ surfaces: [] }));

  const group = activeWorkbenchGroup(page);
  const launcher = group.getByLabel("Available surfaces");
  await expect(group.getByRole("heading", { name: "Choose a surface" })).toBeVisible();
  await expect(launcher.getByRole("button")).toHaveCount(7);
  await expect.poll(() => launcher.evaluate((element) => (
    getComputedStyle(element).gridTemplateColumns.split(" ").filter(Boolean).length
  ))).toBe(1);

  const queue = launcher.locator('[data-surface-type="queue"]');
  await queue.focus();
  await page.keyboard.press("Enter");
  await expect(surfaceTab(page, "queue")).toHaveAttribute("aria-selected", "true");
});

test("splits, moves, zooms, joins, closes, and reopens through semantic controls", async ({ page }) => {
  const dashboard = makeWorkbenchSurface("dashboard-1", "dashboard");
  const queue = makeWorkbenchSurface("queue-1", "queue");
  await bootWorkbench(page, makeWorkbenchDocument({ surfaces: [dashboard, queue] }));

  const groups = page.getByTestId("workbench-group");
  const groupOne = groups.and(page.locator('[data-group-id="group-1"]'));
  await choosePaneAction(page, groupOne, "Split pane right");
  await expect(groups).toHaveCount(2);

  await expect.poll(async () => activeWorkbenchGroup(page).getAttribute("data-group-id"))
    .not.toBe("group-1");

  await surfaceTab(page, "dashboard").click();
  await expect(groupOne).toHaveAttribute("data-active", "true");
  await surfaceTab(page, "dashboard").focus();
  await page.keyboard.press("Alt+Shift+ArrowRight");

  await expect(surfaceTab(page, "dashboard")).toHaveAttribute("aria-selected", "true");
  const newGroup = surfaceTab(page, "dashboard")
    .locator('xpath=ancestor::*[@data-testid="workbench-group"][1]');
  await expect(newGroup).toHaveAttribute("data-active", "true");
  const newGroupId = await newGroup.getAttribute("data-group-id");
  expect(newGroupId).toBeTruthy();
  expect(newGroupId).not.toBe("group-1");

  await choosePaneAction(page, newGroup, "Zoom pane");
  await expect(page.getByTestId("workbench-host")).toHaveAttribute(
    "data-zoomed-group-id",
    newGroupId!,
  );
  await expect(groups).toHaveCount(2);
  await choosePaneAction(page, newGroup, "Restore pane");
  await expect(page.getByTestId("workbench-host")).toHaveAttribute(
    "data-zoomed-group-id",
    "none",
  );

  await choosePaneAction(page, newGroup, "Merge into previous pane");
  await expect(groups).toHaveCount(1);
  await expect(groupOne.getByRole("tab")).toHaveCount(2);

  await choosePaneAction(page, groupOne, "Close pane");
  await expect(groupOne.getByRole("tab")).toHaveCount(0);
  await expect(groupOne.getByRole("heading", { name: "Choose a surface" })).toBeVisible();

  const availableSurfaces = groupOne.getByLabel("Available surfaces");
  await expect(availableSurfaces.getByRole("button")).toHaveCount(7);
  await availableSurfaces.locator('[data-surface-type="agents-overview"]').focus();
  await page.keyboard.press("Enter");
  await expect(surfaceTab(page, "agents-overview")).toHaveCount(1);

  await choosePaneAction(page, groupOne, "Close pane");
  await expect(groupOne.getByRole("heading", { name: "Choose a surface" })).toBeVisible();

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

test("reveals roster agents in Agents and reserves tab creation for explicit Open actions", async ({ page }) => {
  const dashboard = makeWorkbenchSurface("dashboard-1", "dashboard");
  await bootWorkbench(
    page,
    makeWorkbenchDocument({ surfaces: [dashboard] }),
    [ALPHA_AGENT, BETA_AGENT],
  );

  const alphaRow = page.getByLabel("Agent Alpha", { exact: true });
  await activeWorkbenchGroup(page).getByLabel("Open Surface", { exact: true }).click();
  const launcher = page.getByRole("dialog", { name: "Open Surface", exact: true });
  await expect(launcher
    .getByRole("option")
    .and(page.locator('[data-surface-type="agent-session"]'))).toHaveCount(0);
  await page.keyboard.press("Escape");

  await alphaRow.click();
  await expect(surfaceTab(page, "agent-session", ALPHA_AGENT.session_id)).toHaveCount(0);

  await alphaRow.dblclick();
  await expect(surfaceTab(page, "agents-overview")).toHaveCount(1);
  await expect(surfaceTab(page, "agents-overview")).toHaveAttribute("aria-selected", "true");
  await expect(surfaceTab(page, "agent-session", ALPHA_AGENT.session_id)).toHaveCount(0);

  await alphaRow.click({ button: "right" });
  await page.getByTestId("agent-open-context-menu")
    .getByRole("button", { name: "Open", exact: true })
    .click();
  await expect(surfaceTab(page, "agent-session", ALPHA_AGENT.session_id)).toHaveCount(1);

  await alphaRow.click({ button: "right" });
  await page.getByTestId("agent-open-context-menu")
    .getByRole("button", { name: "Open to Side", exact: true })
    .click();
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
  await choosePaneAction(page, group, "Close pane");

  const prompt = page.getByRole("dialog", { name: "Unsaved Workflows changes" });
  await expect(prompt).toBeVisible();
  await prompt.getByRole("button", { name: "Cancel", exact: true }).click();
  await expect(group.getByRole("tab")).toHaveCount(2);
  await expect(surfaceTab(page, "workflows")).toHaveAttribute("aria-selected", "true");

  await choosePaneAction(page, group, "Close pane");
  await page.getByRole("dialog", { name: "Unsaved Workflows changes" })
    .getByRole("button", { name: "Discard", exact: true })
    .click();
  await expect(group.getByRole("tab")).toHaveCount(0);
  await expect(group.getByRole("heading", { name: "Choose a surface" })).toBeVisible();
});
