import { expect, test, type Locator, type Page } from "@playwright/test";

import type { WorkbenchDocumentV1 } from "../../src/types";
import type { AppSettings, WorkbenchNewTabAction } from "../../src/types/settings";
import type { WorkbenchLoadResult } from "../../src/features/workbench/workbenchPersistence";
import {
  installWorkbenchIpcMock,
  makeWorkbenchDocument,
  makeWorkbenchSurface,
  type WorkbenchAgentFixture,
  type WorkbenchIpcMockController,
} from "../fixtures/workbenchIpcMock";
import {
  activeWorkbenchGroup,
  dragSurfaceTab,
  openSurface,
  surfaceTab,
  waitForStableBoundingBoxes,
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
  newTabAction: WorkbenchNewTabAction = "home",
): Promise<WorkbenchIpcMockController> {
  // Browser navigation tests do not make terminal-runtime claims. Keeping the
  // Overview preference on Chat avoids coupling this suite to native PTY IPC.
  await page.addInitScript(() => {
    localStorage.setItem("wardian-settings", JSON.stringify({
      state: { gridCardDisplayMode: "chat" },
      version: 2,
    }));
  });
  const ipc = await installWorkbenchIpcMock(page, {
    load_result: primaryLoad(document),
    agents,
    responses: {
      load_app_settings: {
        schema_version: 2,
        settings: {
          theme: "system",
          auto_patch_gemini: false,
          terminal_font_size: 14,
          terminal_font_family: null,
          grid_card_display_mode: "chat",
          watchlist_new_agent_position: "top",
          titlebar_telemetry_visible: true,
          external_editor: "system",
          external_editor_custom_executable: null,
          explorer_file_click_action: "preview",
          workbench_new_tab_action: newTabAction,
        } satisfies AppSettings,
        overrides: newTabAction === "palette"
          ? { workbench_new_tab_action: "palette" }
          : {},
        persisted: true,
      },
    },
  });
  await page.goto("/");
  await expect(page.getByTestId("workbench-host")).toBeVisible();
  await expect(page.getByTestId("workbench-group")).toHaveCount(
    Object.keys(document.groups).length,
  );
  return ipc;
}

function workbenchGroup(page: Page, groupId: string): Locator {
  return page.getByTestId("workbench-group")
    .and(page.locator(`[data-group-id=${JSON.stringify(groupId)}]`));
}

function tabSurfaceIds(group: Locator): Promise<string[]> {
  return group.getByRole("tab").evaluateAll((tabs) => tabs.map((tab) => (
    (tab as HTMLElement).dataset.surfaceId ?? ""
  )));
}

function surfaceOwner(page: Page, surfaceId: string): Locator {
  return page.getByRole("tab")
    .and(page.locator(`[data-surface-id=${JSON.stringify(surfaceId)}]`))
    .locator('xpath=ancestor::*[@data-testid="workbench-group"][1]');
}

function installDragErrorMonitor(page: Page): () => string[] {
  const pageErrors: string[] = [];
  const consoleErrors: string[] = [];
  page.on("pageerror", (error) => pageErrors.push(error.message));
  page.on("console", (message) => {
    if (message.type() === "error") consoleErrors.push(message.text());
  });
  return () => [
    ...pageErrors.map((message) => `pageerror: ${message}`),
    ...consoleErrors
      .filter((message) => /Dockview group projection failed|uncaught|fatal/i.test(message))
      .map((message) => `console: ${message}`),
  ];
}

async function expectPersistedTopology(
  ipc: WorkbenchIpcMockController,
  assertion: (document: WorkbenchDocumentV1) => boolean,
): Promise<void> {
  await expect.poll(async () => {
    const snapshot = await ipc.snapshot();
    return assertion(snapshot.load_result.document);
  }).toBe(true);
}

function noEmptyNonFinalGroup(document: WorkbenchDocumentV1): boolean {
  return Object.keys(document.groups).length === 1
    || Object.values(document.groups).every((group) => group.surface_ids.length > 0);
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
    const tab = surfaceTab(page, surfaceType);
    await expect(tab).toHaveCount(1);
    await expect(tab.locator(`[data-surface-icon=${JSON.stringify(surfaceType)}]`)).toBeVisible();
  }
  const agentTab = surfaceTab(page, "agent-session", ALPHA_AGENT.session_id);
  await expect(agentTab).toHaveCount(1);
  await expect(agentTab.locator('[data-surface-icon="agent-session"]')).toBeVisible();

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

test("opens an inline New Tab in its captured pane and transitions Browse all to search", async ({ page }) => {
  const dashboard = makeWorkbenchSurface("dashboard-1", "dashboard");
  const graph = makeWorkbenchSurface("graph-1", "graph");
  const document = makeWorkbenchDocument({
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
        surface_ids: [dashboard.surface_id],
        active_surface_id: dashboard.surface_id,
      },
      "group-2": {
        group_id: "group-2",
        surface_ids: [graph.surface_id],
        active_surface_id: graph.surface_id,
      },
    },
    surfaces: [dashboard, graph],
    active_group_id: "group-1",
  });
  await bootWorkbench(page, document);

  const targetGroup = workbenchGroup(page, "group-2");
  await targetGroup.getByLabel("Open Surface", { exact: true }).click();
  await expect(targetGroup.getByRole("tab", { name: "New Tab", exact: true }))
    .toHaveAttribute("aria-selected", "true");
  await expect(targetGroup.getByRole("heading", { name: "Choose a surface" })).toBeVisible();
  await expect(targetGroup.getByLabel("Available surfaces").getByRole("button")).toHaveCount(7);
  await targetGroup.getByRole("button", { name: /^Queue:/ }).click();

  const queueTab = surfaceTab(page, "queue");
  await expect(queueTab).toBeVisible();
  await expect(queueTab.locator('xpath=ancestor::*[@data-testid="workbench-group"][1]'))
    .toHaveAttribute("data-group-id", "group-2");

  await targetGroup.getByLabel("Open Surface", { exact: true }).click();
  await targetGroup.getByRole("button", { name: "Browse all surfaces", exact: true }).click();
  const searchable = page.getByRole("dialog", { name: "Open Surface", exact: true });
  await expect(searchable).toBeVisible();
  await expect(searchable.getByRole("combobox", { name: "Open a surface" })).toBeFocused();

  await page.keyboard.press("Escape");
  await expect(searchable).toHaveCount(0);
  await page.keyboard.press(process.platform === "darwin" ? "Meta+p" : "Control+p");
  await expect(searchable).toBeVisible();
  await expect(searchable.getByRole("combobox", { name: "Open a surface" })).toBeFocused();
  await expect(page.getByRole("dialog", { name: "Choose a surface", exact: true })).toHaveCount(0);
});

test("honors the persisted palette plus preference while Quick Open remains searchable", async ({ page }) => {
  const dashboard = makeWorkbenchSurface("dashboard-1", "dashboard");
  await bootWorkbench(
    page,
    makeWorkbenchDocument({ surfaces: [dashboard] }),
    [],
    "palette",
  );

  const group = activeWorkbenchGroup(page);
  await group.getByLabel("Open Surface", { exact: true }).click();
  const searchable = page.getByRole("dialog", { name: "Open Surface", exact: true });
  await expect(searchable).toBeVisible();
  await expect(page.getByRole("dialog", { name: "Choose a surface", exact: true })).toHaveCount(0);
  const search = searchable.getByRole("combobox", { name: "Open a surface" });
  await search.fill("Queue");
  await expect(searchable.getByRole("option", { name: "Queue", exact: true })).toBeVisible();
  await page.keyboard.press("Escape");

  await page.keyboard.press(process.platform === "darwin" ? "Meta+p" : "Control+p");
  await expect(searchable).toBeVisible();
  await expect(searchable.getByRole("combobox", { name: "Open a surface" })).toBeFocused();
});

test("reorders tabs with real pointer coordinates and persists canonical order", async ({ page }) => {
  const fatalDragErrors = installDragErrorMonitor(page);
  const dashboard = makeWorkbenchSurface("dashboard-1", "dashboard");
  const queue = makeWorkbenchSurface("queue-1", "queue");
  const ipc = await bootWorkbench(page, makeWorkbenchDocument({
    groups: {
      "group-1": {
        group_id: "group-1",
        surface_ids: [dashboard.surface_id, queue.surface_id],
        active_surface_id: dashboard.surface_id,
      },
    },
    surfaces: [dashboard, queue],
  }));

  const group = workbenchGroup(page, "group-1");
  const initialGroupBounds = await group.boundingBox();
  const dashboardBounds = await surfaceTab(page, "dashboard").boundingBox();
  expect(initialGroupBounds).not.toBeNull();
  expect(dashboardBounds).not.toBeNull();

  await dragSurfaceTab(page, surfaceTab(page, "queue"), {
    x: dashboardBounds!.x + 4,
    y: dashboardBounds!.y + dashboardBounds!.height / 2,
  });

  await expect.poll(() => tabSurfaceIds(group)).toEqual(["queue-1", "dashboard-1"]);
  await expect(page.getByTestId("workbench-group")).toHaveCount(1);
  const finalGroupBounds = await group.boundingBox();
  expect(finalGroupBounds).not.toBeNull();
  expect(Math.abs(finalGroupBounds!.x - initialGroupBounds!.x)).toBeLessThanOrEqual(2);
  expect(Math.abs(finalGroupBounds!.width - initialGroupBounds!.width)).toBeLessThanOrEqual(2);
  await expectPersistedTopology(ipc, (document) => (
    document.groups["group-1"]?.surface_ids.join(",") === "queue-1,dashboard-1"
    && noEmptyNonFinalGroup(document)
  ));
  expect(fatalDragErrors()).toEqual([]);
});

test("moves a sole source tab to another pane center and collapses the source pane", async ({ page }) => {
  const fatalDragErrors = installDragErrorMonitor(page);
  const dashboard = makeWorkbenchSurface("dashboard-1", "dashboard");
  const queue = makeWorkbenchSurface("queue-1", "queue");
  const document = makeWorkbenchDocument({
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
        surface_ids: [dashboard.surface_id],
        active_surface_id: dashboard.surface_id,
      },
      "group-2": {
        group_id: "group-2",
        surface_ids: [queue.surface_id],
        active_surface_id: queue.surface_id,
      },
    },
    surfaces: [dashboard, queue],
    active_group_id: "group-2",
  });
  const ipc = await bootWorkbench(page, document);

  const targetGroup = workbenchGroup(page, "group-1");
  const sourceGroup = workbenchGroup(page, "group-2");
  const [targetBounds, sourceBounds] = await Promise.all([
    targetGroup.boundingBox(),
    sourceGroup.boundingBox(),
  ]);
  expect(targetBounds).not.toBeNull();
  expect(sourceBounds).not.toBeNull();

  await dragSurfaceTab(page, surfaceTab(page, "queue"), {
    x: targetBounds!.x + targetBounds!.width / 2,
    y: targetBounds!.y + targetBounds!.height * 0.6,
  });

  await expect(page.getByTestId("workbench-group")).toHaveCount(1);
  await expect(surfaceOwner(page, "queue-1")).toHaveAttribute("data-group-id", "group-1");
  await expect.poll(() => tabSurfaceIds(targetGroup)).toEqual(["dashboard-1", "queue-1"]);
  await expect(sourceGroup).toHaveCount(0);
  const finalBounds = await targetGroup.boundingBox();
  expect(finalBounds).not.toBeNull();
  const combinedLeft = Math.min(targetBounds!.x, sourceBounds!.x);
  const combinedRight = Math.max(
    targetBounds!.x + targetBounds!.width,
    sourceBounds!.x + sourceBounds!.width,
  );
  expect(Math.abs(finalBounds!.x - combinedLeft)).toBeLessThanOrEqual(4);
  expect(Math.abs(finalBounds!.x + finalBounds!.width - combinedRight)).toBeLessThanOrEqual(4);
  await expectPersistedTopology(ipc, (saved) => (
    saved.root.kind === "group"
    && saved.root.group_id === "group-1"
    && Object.keys(saved.groups).length === 1
    && saved.groups["group-1"]?.surface_ids.join(",") === "dashboard-1,queue-1"
    && noEmptyNonFinalGroup(saved)
  ));
  expect(fatalDragErrors()).toEqual([]);
});

test("rejects an impossible narrow edge split while retaining center movement", async ({ page }) => {
  const fatalDragErrors = installDragErrorMonitor(page);
  const dashboard = makeWorkbenchSurface("dashboard-1", "dashboard");
  const queue = makeWorkbenchSurface("queue-1", "queue");
  const graph = makeWorkbenchSurface("graph-1", "graph");
  const ipc = await bootWorkbench(page, makeWorkbenchDocument({
    root: {
      kind: "split",
      node_id: "split-1",
      direction: "horizontal",
      ratio: 0.1,
      first: { kind: "group", group_id: "group-1" },
      second: { kind: "group", group_id: "group-2" },
    },
    groups: {
      "group-1": {
        group_id: "group-1",
        surface_ids: [dashboard.surface_id],
        active_surface_id: dashboard.surface_id,
      },
      "group-2": {
        group_id: "group-2",
        surface_ids: [queue.surface_id, graph.surface_id],
        active_surface_id: queue.surface_id,
      },
    },
    surfaces: [dashboard, queue, graph],
    active_group_id: "group-2",
  }));

  const targetGroup = workbenchGroup(page, "group-1");
  const contentTarget = targetGroup.locator(":scope > .dv-content-container");
  const queueTab = surfaceTab(page, "queue");
  const [, targetBounds, contentBounds] = await waitForStableBoundingBoxes(
    page,
    [queueTab, targetGroup, contentTarget],
  );
  expect(targetBounds.width).toBeLessThan(200);

  await dragSurfaceTab(page, queueTab, {
    x: contentBounds.x + contentBounds.width - 4,
    y: contentBounds.y + contentBounds.height * 0.6,
  }, async () => {
    await expect(page.locator(".dv-drop-target-selection:visible")).toHaveCount(0);
  });

  await expect(page.getByTestId("workbench-group")).toHaveCount(2);
  await expect(surfaceOwner(page, "queue-1")).toHaveAttribute("data-group-id", "group-2");
  await expectPersistedTopology(ipc, (saved) => (
    saved.root.kind === "split"
    && Object.keys(saved.groups).length === 2
    && saved.groups["group-1"]?.surface_ids.join(",") === "dashboard-1"
    && saved.groups["group-2"]?.surface_ids.join(",") === "queue-1,graph-1"
  ));

  const [, centerBounds] = await waitForStableBoundingBoxes(page, [queueTab, contentTarget]);
  await dragSurfaceTab(page, queueTab, {
    x: centerBounds.x + centerBounds.width / 2,
    y: centerBounds.y + centerBounds.height * 0.6,
  });

  await expect(page.getByTestId("workbench-group")).toHaveCount(2);
  await expect(surfaceOwner(page, "queue-1")).toHaveAttribute("data-group-id", "group-1");
  await expectPersistedTopology(ipc, (saved) => (
    saved.root.kind === "split"
    && Object.keys(saved.groups).length === 2
    && saved.groups["group-1"]?.surface_ids.join(",") === "dashboard-1,queue-1"
    && saved.groups["group-2"]?.surface_ids.join(",") === "graph-1"
    && noEmptyNonFinalGroup(saved)
  ));
  expect(fatalDragErrors()).toEqual([]);
});

test("keeps a sole-tab self-edge preview center-only without creating a group", async ({ page }) => {
  const fatalDragErrors = installDragErrorMonitor(page);
  const dashboard = makeWorkbenchSurface("dashboard-1", "dashboard");
  const ipc = await bootWorkbench(page, makeWorkbenchDocument({
    groups: {
      "group-1": {
        group_id: "group-1",
        surface_ids: [dashboard.surface_id],
        active_surface_id: dashboard.surface_id,
      },
    },
    surfaces: [dashboard],
  }));

  const targetGroup = workbenchGroup(page, "group-1");
  const contentTarget = targetGroup.locator(":scope > .dv-content-container");
  const dashboardTab = surfaceTab(page, "dashboard");
  const [, targetBounds, contentBounds] = await waitForStableBoundingBoxes(
    page,
    [dashboardTab, targetGroup, contentTarget],
  );

  await dragSurfaceTab(page, dashboardTab, {
    x: contentBounds.x + contentBounds.width - 6,
    y: contentBounds.y + contentBounds.height * 0.6,
  }, async () => {
    const selection = page.locator(".dv-drop-target-selection:visible");
    await expect(selection).toHaveCount(1);
    const [selectionBounds, actualContentBounds] = await waitForStableBoundingBoxes(
      page,
      [selection, contentTarget],
    );
    const tolerance = 5;
    expect(Math.abs(selectionBounds.x - actualContentBounds.x)).toBeLessThanOrEqual(tolerance);
    expect(Math.abs(selectionBounds.width - actualContentBounds.width)).toBeLessThanOrEqual(tolerance);
    expect(selectionBounds.width).toBeGreaterThan(actualContentBounds.width * 0.9);
  });

  await expect(page.locator(".dv-drop-target-selection:visible")).toHaveCount(0);
  await expect(page.getByTestId("workbench-group")).toHaveCount(1);
  await expect(surfaceOwner(page, "dashboard-1")).toHaveAttribute("data-group-id", "group-1");
  const finalBounds = await targetGroup.boundingBox();
  expect(finalBounds).not.toBeNull();
  expect(Math.abs(finalBounds!.x - targetBounds.x)).toBeLessThanOrEqual(2);
  expect(Math.abs(finalBounds!.width - targetBounds.width)).toBeLessThanOrEqual(2);
  await expectPersistedTopology(ipc, (saved) => (
    saved.root.kind === "group"
    && saved.root.group_id === "group-1"
    && Object.keys(saved.groups).length === 1
    && saved.groups["group-1"]?.surface_ids.join(",") === "dashboard-1"
    && noEmptyNonFinalGroup(saved)
  ));
  expect(fatalDragErrors()).toEqual([]);
});

test("shows an accurate half-pane edge preview and splits with a real pointer drop", async ({ page }, testInfo) => {
  const fatalDragErrors = installDragErrorMonitor(page);
  const dashboard = makeWorkbenchSurface("dashboard-1", "dashboard");
  const queue = makeWorkbenchSurface("queue-1", "queue");
  const ipc = await bootWorkbench(page, makeWorkbenchDocument({
    groups: {
      "group-1": {
        group_id: "group-1",
        surface_ids: [dashboard.surface_id, queue.surface_id],
        active_surface_id: queue.surface_id,
      },
    },
    surfaces: [dashboard, queue],
  }));

  const targetGroup = workbenchGroup(page, "group-1");
  const contentTarget = targetGroup.locator(":scope > .dv-content-container");
  const queueTab = surfaceTab(page, "queue");
  const [, targetBounds, contentBounds] = await waitForStableBoundingBoxes(
    page,
    [queueTab, targetGroup, contentTarget],
  );
  const screenshotPath = testInfo.outputPath("edge-preview.png");

  await dragSurfaceTab(page, queueTab, {
    x: contentBounds.x + contentBounds.width - 6,
    y: contentBounds.y + contentBounds.height * 0.6,
  }, async () => {
    const selection = page.locator(".dv-drop-target-selection:visible");
    await expect(selection).toHaveCount(1);
    const [selectionBounds, actualContentBounds] = await waitForStableBoundingBoxes(
      page,
      [selection, contentTarget],
    );
    const tolerance = 5;
    expect(selectionBounds.x).toBeGreaterThanOrEqual(actualContentBounds.x - tolerance);
    expect(selectionBounds.y).toBeGreaterThanOrEqual(actualContentBounds.y - tolerance);
    expect(selectionBounds.x + selectionBounds.width)
      .toBeLessThanOrEqual(actualContentBounds.x + actualContentBounds.width + tolerance);
    expect(selectionBounds.y + selectionBounds.height)
      .toBeLessThanOrEqual(actualContentBounds.y + actualContentBounds.height + tolerance);
    expect(Math.abs(selectionBounds.width - actualContentBounds.width / 2))
      .toBeLessThanOrEqual(tolerance);
    expect(Math.abs(selectionBounds.height - actualContentBounds.height))
      .toBeLessThanOrEqual(tolerance);
    expect(Math.abs(selectionBounds.x - (actualContentBounds.x + actualContentBounds.width / 2)))
      .toBeLessThanOrEqual(tolerance);
    expect(selectionBounds.width).toBeGreaterThan(actualContentBounds.width * 0.4);
    await page.screenshot({ path: screenshotPath, animations: "disabled" });
  });

  await expect(page.locator(".dv-drop-target-selection:visible")).toHaveCount(0);
  await expect(page.getByTestId("workbench-group")).toHaveCount(2);
  const dashboardOwner = surfaceOwner(page, "dashboard-1");
  const queueOwner = surfaceOwner(page, "queue-1");
  const [dashboardGroupId, queueGroupId] = await Promise.all([
    dashboardOwner.getAttribute("data-group-id"),
    queueOwner.getAttribute("data-group-id"),
  ]);
  expect(dashboardGroupId).toBeTruthy();
  expect(queueGroupId).toBeTruthy();
  expect(queueGroupId).not.toBe(dashboardGroupId);
  const [dashboardBounds, queueBounds] = await Promise.all([
    dashboardOwner.boundingBox(),
    queueOwner.boundingBox(),
  ]);
  expect(dashboardBounds).not.toBeNull();
  expect(queueBounds).not.toBeNull();
  for (const bounds of [dashboardBounds!, queueBounds!]) {
    expect(bounds.width).toBeGreaterThan(targetBounds.width * 0.4);
    expect(bounds.width).toBeLessThan(targetBounds.width * 0.6);
    expect(Math.abs(bounds.height - targetBounds.height)).toBeLessThanOrEqual(4);
  }
  const finalLeft = Math.min(dashboardBounds!.x, queueBounds!.x);
  const finalRight = Math.max(
    dashboardBounds!.x + dashboardBounds!.width,
    queueBounds!.x + queueBounds!.width,
  );
  expect(Math.abs(finalLeft - targetBounds.x)).toBeLessThanOrEqual(4);
  expect(Math.abs(finalRight - (targetBounds.x + targetBounds.width))).toBeLessThanOrEqual(4);
  await expectPersistedTopology(ipc, (saved) => {
    if (saved.root.kind !== "split" || saved.root.direction !== "horizontal") return false;
    const dashboardGroup = Object.values(saved.groups)
      .find((group) => group.surface_ids.includes("dashboard-1"));
    const queueGroup = Object.values(saved.groups)
      .find((group) => group.surface_ids.includes("queue-1"));
    return Object.keys(saved.groups).length === 2
      && dashboardGroup !== undefined
      && queueGroup !== undefined
      && dashboardGroup.group_id !== queueGroup.group_id
      && Math.abs(saved.root.ratio - 0.5) <= 0.02
      && noEmptyNonFinalGroup(saved);
  });
  expect(fatalDragErrors()).toEqual([]);
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
  const newTabPanel = activeWorkbenchGroup(page);
  await expect(newTabPanel.getByRole("heading", { name: "Choose a surface" })).toBeVisible();
  await newTabPanel.getByRole("button", { name: "Browse all surfaces", exact: true }).click();
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
  const visibleAgentMenus = page.locator(
    ".context-menu:visible, [data-testid='agent-context-menu']:visible",
  );
  await expect(visibleAgentMenus).toHaveCount(1);
  const agentMenu = page.getByTestId("agent-context-menu").filter({ visible: true });
  await expect(agentMenu).toHaveCount(1);
  await expect(agentMenu.getByRole("button", { name: "Open", exact: true })).toBeVisible();
  await expect(agentMenu.getByRole("button", { name: "Open to Side", exact: true })).toBeVisible();
  await expect(agentMenu.getByRole("button", { name: "Rename", exact: true })).toBeVisible();
  await agentMenu.getByRole("button", { name: "Open", exact: true }).click();
  await expect(surfaceTab(page, "agent-session", ALPHA_AGENT.session_id)).toHaveCount(1);

  await alphaRow.click({ button: "right" });
  await page.getByTestId("agent-context-menu")
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
