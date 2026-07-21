import { expect, test, type Locator, type Page } from "@playwright/test";

import type { WorkbenchDocumentV1 } from "../../src/types";
import type { WorkbenchLoadResult } from "../../src/features/workbench/workbenchPersistence";
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

const surfaceTypes = [
  "new-tab",
  "dashboard",
  "inbox",
  "graph",
  "garden",
  "library",
  "workflows",
] as const;

type ResponsiveSurfaceType = typeof surfaceTypes[number];

const rootSelector: Record<ResponsiveSurfaceType, string> = {
  "new-tab": ".wardian-workbench-home",
  dashboard: ".dashboard-view",
  queue: ".queue-view",
  graph: "[data-testid=graph-view]",
  garden: ".garden-view",
  library: "[data-testid=library-view]",
  workflows: "[data-testid=workflows-view]",
};

function responsiveDocument(): WorkbenchDocumentV1 {
  const surfaces = surfaceTypes.map((surfaceType) => makeWorkbenchSurface(
    `${surfaceType}-responsive`,
    surfaceType,
  ));
  const overview = makeWorkbenchSurface("overview-reference", "agents-overview", {
    state: {
      mode: "auto",
      last_multi_agent_mode: "auto",
      focused_agent_id: null,
      search_query: "",
      status_filter: [],
    },
  });

  return makeWorkbenchDocument({
    revision: 4,
    root: {
      kind: "split",
      node_id: "responsive-split",
      direction: "horizontal",
      ratio: 0.5,
      first: { kind: "group", group_id: "group-surfaces" },
      second: { kind: "group", group_id: "group-reference" },
    },
    groups: {
      "group-surfaces": {
        group_id: "group-surfaces",
        surface_ids: surfaces.map(({ surface_id }) => surface_id),
        active_surface_id: surfaces[0].surface_id,
      },
      "group-reference": {
        group_id: "group-reference",
        surface_ids: [overview.surface_id],
        active_surface_id: overview.surface_id,
      },
    },
    surfaces: [...surfaces, overview],
    active_group_id: "group-surfaces",
    shell: {
      left_sidebar_collapsed: true,
      right_sidebar_collapsed: true,
    },
  });
}

function primaryLoad(document: WorkbenchDocumentV1): WorkbenchLoadResult {
  return {
    source: "primary",
    document,
    notice: null,
    durable_revision: document.revision,
    durable_token: `responsive-token-${document.revision}`,
  };
}

function surfaceTab(page: Page, surfaceType: ResponsiveSurfaceType): Locator {
  return page.getByRole("tab").and(page.locator(
    `[data-surface-type=${JSON.stringify(surfaceType)}]`,
  ));
}

function surfacePanel(page: Page, surfaceType: ResponsiveSurfaceType): Locator {
  return page.getByTestId("surface-panel").and(page.locator(
    `[data-surface-type=${JSON.stringify(surfaceType)}]`,
  ));
}

async function expectNoHorizontalOverflow(locator: Locator): Promise<void> {
  await expect(locator).toBeVisible();
  await expect.poll(() => locator.evaluate((element) => ({
    clientWidth: element.clientWidth,
    scrollWidth: element.scrollWidth,
  }))).toEqual(expect.objectContaining({
    clientWidth: expect.any(Number),
    scrollWidth: expect.any(Number),
  }));
  const sizes = await locator.evaluate((element) => ({
    clientWidth: element.clientWidth,
    scrollWidth: element.scrollWidth,
  }));
  expect(sizes.clientWidth).toBeGreaterThan(0);
  expect(sizes.scrollWidth).toBeLessThanOrEqual(sizes.clientWidth + 1);
}

test("keeps every core surface usable in a half-width Workbench pane", async ({ page }, testInfo) => {
  const document = responsiveDocument();
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.addInitScript(() => {
    localStorage.setItem("wardian-settings", JSON.stringify({
      state: { gridCardDisplayMode: "chat" },
      version: 2,
    }));
  });
  await installWorkbenchIpcMock(page, {
    agents,
    load_result: primaryLoad(document),
  });

  await page.goto("/");
  await expect(page.getByTestId("workbench-group")).toHaveCount(2);

  for (const surfaceType of surfaceTypes) {
    await surfaceTab(page, surfaceType).click();
    const panel = surfacePanel(page, surfaceType);
    const root = panel.locator(rootSelector[surfaceType]);
    await expectNoHorizontalOverflow(root);

    const panelBounds = await panel.boundingBox();
    const rootBounds = await root.boundingBox();
    expect(panelBounds).not.toBeNull();
    expect(rootBounds).not.toBeNull();
    expect(rootBounds!.x).toBeGreaterThanOrEqual(panelBounds!.x - 1);
    expect(rootBounds!.x + rootBounds!.width).toBeLessThanOrEqual(
      panelBounds!.x + panelBounds!.width + 1,
    );
  }

  await surfaceTab(page, "graph").click();
  const graphPanel = surfacePanel(page, "graph");
  const graphCanvas = graphPanel.locator(".graph-canvas-shell");
  await expect(graphCanvas).toBeVisible();
  const graphBounds = await graphCanvas.boundingBox();
  expect(graphBounds?.width).toBeGreaterThan(0);
  expect(graphBounds?.height).toBeGreaterThan(0);
  await graphPanel.getByRole("button", { name: "Hide inspector" }).click();
  await expect(graphPanel.locator(".graph-inspector")).toHaveCount(0);

  await surfaceTab(page, "garden").click();
  const gardenBounds = await surfacePanel(page, "garden").locator(".garden-canvas").boundingBox();
  expect(gardenBounds?.width).toBeGreaterThan(0);
  expect(gardenBounds?.height).toBeGreaterThan(0);

  await surfaceTab(page, "workflows").click();
  const workflowsPanel = surfacePanel(page, "workflows");
  const toolbar = workflowsPanel.locator(".workflows-toolbar");
  const primaryBounds = await toolbar.locator(".workflows-toolbar__primary").boundingBox();
  const actionBounds = await toolbar.locator(".workflows-toolbar__actions").boundingBox();
  expect(primaryBounds).not.toBeNull();
  expect(actionBounds).not.toBeNull();
  expect(actionBounds!.y).toBeGreaterThanOrEqual(primaryBounds!.y + primaryBounds!.height - 1);

  await workflowsPanel.getByRole("button", { name: "Show Runs" }).click();
  await expect(workflowsPanel.locator(".workflows-run-drawer")).toBeVisible();

  const screenshotDir = process.env.WARDIAN_RESPONSIVE_SCREENSHOT_DIR;
  if (screenshotDir) {
    const workflowsPath = `${screenshotDir}/workflows-compact.png`;
    const agentsPath = `${screenshotDir}/agents-auto-preferred.png`;
    await workflowsPanel.screenshot({ path: workflowsPath, animations: "disabled" });
    const overviewPanel = page.getByTestId("surface-panel").and(page.locator(
      '[data-surface-type="agents-overview"]',
    ));
    await overviewPanel.screenshot({ path: agentsPath, animations: "disabled" });
    await testInfo.attach("workflows-compact", { path: workflowsPath, contentType: "image/png" });
    await testInfo.attach("agents-auto-preferred", { path: agentsPath, contentType: "image/png" });
  }

  await workflowsPanel.getByRole("button", { name: "Hide Runs" }).click();
  await expect(workflowsPanel.locator(".workflows-run-drawer")).toHaveCount(0);
});
