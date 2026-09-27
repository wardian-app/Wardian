import { test, expect, type Page } from "@playwright/test";
import { openSurface, surfacePanel } from "../fixtures/workbench";
import { makeWorkbenchDocument, makeWorkbenchSurface } from "../fixtures/workbenchIpcMock";
import { Buffer } from "buffer";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";
import { seedTopology } from "../fixtures/mockAgent";

/**
 * Graph topology browser E2E tests.
 *
 * These tests verify the browser-layer rendering and interaction of the graph view's
 * communication topology feature: manual edges, neighbors panels, and the add-connection picker.
 * Tests that require real Tauri IPC or filesystem operations are marked @native-only.
 */

interface MockAgent {
  session_id: string;
  session_name: string;
  agent_class: string;
  folder: string;
  provider: string;
  is_off: boolean;
  description?: string;
}

interface PairActivity {
  a: string;
  b: string;
  last_message_at: string;
  active_ask: boolean;
}

const GRAPH_WORKBENCH_DOCUMENT = makeWorkbenchDocument({
  surfaces: [makeWorkbenchSurface("graph-surface", "graph")],
});

const GRAPH_STATUS_WATCHLIST_PREFS: Record<string, unknown> = {
  columns: [
    { id: "status_label", visible: true },
    { id: "query_count", visible: false },
    { id: "uptime", visible: false },
    { id: "provider_model", visible: false },
    { id: "last_queried", visible: false },
  ],
  sort: null,
  preserve_team_grouping_when_sorted: false,
  collapsed_team_ids: [],
  collapsed_team_ids_by_list: {},
};

async function installGraphTopologyIpcMock(
  page: Page,
  topology: {
    edges: Array<{ a: string; b: string; origin: string }>;
    ignored_pairs: [string, string][];
    fallback_groups: string[][];
  },
  agents: MockAgent[],
  pairActivity: PairActivity[] = [],
  watchlistPrefs: Record<string, unknown> | null = null,
) {
  await page.addInitScript(({
    topologyFixture,
    agentsFixture,
    activityFixture,
    workbenchDocument,
    watchlistPrefsFixture,
  }) => {
    let callbackId = 1;
    const callbacks = new Map<number, unknown>();
    const eventHandlers = new Map<string, number>();
    const tauriWindow = window as Window & {
      __TAURI_INTERNALS__?: Record<string, unknown>;
      __TAURI_EVENT_PLUGIN_INTERNALS__?: Record<string, unknown>;
      __WARDIAN_E2E_GRAPH_RUNTIME__?: {
        hasListener: (event: string) => boolean;
        emit: (event: string, payload: unknown) => void;
      };
    };

    tauriWindow.__WARDIAN_E2E_GRAPH_RUNTIME__ = {
      hasListener: (event) => eventHandlers.has(event),
      emit: (event, payload) => {
        const handlerId = eventHandlers.get(event);
        const handler = handlerId === undefined
          ? undefined
          : callbacks.get(handlerId) as ((event: unknown) => void) | undefined;
        handler?.({ event, id: 0, payload });
      },
    };

    tauriWindow.__TAURI_EVENT_PLUGIN_INTERNALS__ = {
      unregisterListener: () => undefined,
    };

    tauriWindow.__TAURI_INTERNALS__ = {
      metadata: {
        currentWindow: { label: "main" },
        currentWebview: { label: "main" },
      },
      transformCallback: (callback: unknown) => {
        const id = callbackId++;
        callbacks.set(id, callback);
        return id;
      },
      unregisterCallback: (id: number) => {
        callbacks.delete(id);
      },
      convertFileSrc: (filePath: string) => filePath,
      invoke: async (command: string, args?: Record<string, unknown>) => {
        if (command === "list_agents") return agentsFixture;
        if (command === "get_workbench_boot_config") return { safe_mode: false };
        if (command === "load_workbench_state") {
          return {
            source: "default",
            document: workbenchDocument,
            notice: null,
            durable_revision: workbenchDocument.revision,
            durable_token: "graph-test-token",
          };
        }
        if (command === "save_workbench_state") {
          const proposedRevision = (args?.document as { revision?: number } | undefined)?.revision
            ?? workbenchDocument.revision;
          return {
            outcome: "saved",
            durable_revision: proposedRevision,
            durable_token: "graph-test-token",
            request_id: String(args?.request_id ?? "graph-save-request"),
          };
        }
        if (command === "list_agent_classes") {
          return [{ name: "TestClass", description: "Graph test class", is_default: true }];
        }
        if (command === "list_provider_readiness") {
          return [
            { provider: "claude", display_name: "Claude", available: true, executable: "C:/tools/claude.cmd", reason: null },
          ];
        }
        if (command === "load_watchlists") return [];
        if (command === "load_watchlist_prefs") return watchlistPrefsFixture;
        if (command === "load_agent_interactions") return {};
        if (command === "load_queue_items") return [];
        if (command === "load_queue_preferences") return {};
        if (command === "load_onboarding_hints") {
          return { dismissed_hint_ids: ["spawn-agent-first-run:v1"] };
        }
        if (command === "dismiss_onboarding_hint") {
          return { dismissed_hint_ids: ["spawn-agent-first-run:v1"] };
        }
        if (command === "list_automations") return [];
        if (command === "list_scheduled_runs") return [];
        if (command === "load_automation_library") return { folders: [], rootAutomationIds: [] };
        if (command === "get_library_tree") {
          return { type: "Folder", path: "", name: "Root", children: [] };
        }
        if (command === "list_deployed_skills") return [];
        if (command === "load_app_settings") return null;
        if (command === "load_shell_settings") {
          return {
            shell_id: "auto",
            custom_executable: null,
            custom_args: null,
            agent_session_persistence: "resume",
            default_provider: "claude",
          };
        }
        if (command === "list_available_shells") return [];
        if (command === "get_topology") {
          return topologyFixture;
        }
        // Callers pass a plain array; the command returns a page.
        if (command === "get_pair_activity") {
          return { pairs: activityFixture, truncated: false, next_offset: null };
        }
        if (command === "plugin:event|listen") {
          eventHandlers.set(String(args?.event), Number(args?.handler));
          return callbackId++;
        }
        if (command === "plugin:event|unlisten") return null;
        if (command === "sync_provider_theme_settings") return null;
        return null;
      },
    };
  }, {
    topologyFixture: topology,
    agentsFixture: agents,
    activityFixture: pairActivity,
    workbenchDocument: GRAPH_WORKBENCH_DOCUMENT,
    watchlistPrefsFixture: watchlistPrefs,
  });
}

async function openGraphView(page: Page) {
  await page.setViewportSize({ width: 1400, height: 900 });
  await page.goto("/", { waitUntil: "domcontentloaded" });
  await page.locator('[data-testid="app-shell"]').waitFor({ timeout: 15_000 });

  await openSurface(page, "graph");
  await expect(surfacePanel(page, "graph").locator('[data-testid="graph-view"]')).toBeVisible({ timeout: 10_000 });
}

async function countScreenshotPixelsMatchingColor(
  page: Page,
  screenshot: Buffer,
  cssColor: string,
): Promise<number> {
  return page.evaluate(async ({ screenshotBase64, targetColor }) => {
    const channels = targetColor.match(/[\d.]+/g)?.slice(0, 3).map(Number);
    if (!channels || channels.length !== 3) {
      throw new Error(`Could not parse rendered status color: ${targetColor}`);
    }

    const image = new Image();
    image.src = `data:image/png;base64,${screenshotBase64}`;
    await image.decode();

    const canvas = document.createElement("canvas");
    canvas.width = image.naturalWidth;
    canvas.height = image.naturalHeight;
    const context = canvas.getContext("2d", { willReadFrequently: true });
    if (!context) throw new Error("Could not read Graph canvas screenshot pixels.");
    context.drawImage(image, 0, 0);

    const { data } = context.getImageData(0, 0, canvas.width, canvas.height);
    const tolerance = 20;
    let matchingPixels = 0;
    for (let index = 0; index < data.length; index += 4) {
      if (
        data[index + 3] > 240
        && Math.abs(data[index] - channels[0]) <= tolerance
        && Math.abs(data[index + 1] - channels[1]) <= tolerance
        && Math.abs(data[index + 2] - channels[2]) <= tolerance
      ) {
        matchingPixels += 1;
      }
    }
    return matchingPixels;
  }, {
    screenshotBase64: screenshot.toString("base64"),
    targetColor: cssColor,
  });
}

test.describe("Graph Topology", () => {
  test.describe.configure({ mode: "serial" });

  let page: Page;

  test.beforeAll(async ({ browser }) => {
    page = await browser.newPage();
  });

  test.afterAll(async () => {
    await page.close();
  });

  test("renders seeded manual edge in neighbors panel", async () => {
    const agent1: MockAgent = {
      session_id: "test-agent-1",
      session_name: "Alpha",
      agent_class: "TestClass",
      folder: "/test/alpha",
      provider: "claude",
      is_off: false,
      description: "Coordinates the alpha workstream",
    };

    const agent2: MockAgent = {
      session_id: "test-agent-2",
      session_name: "Beta",
      agent_class: "TestClass",
      folder: "/test/beta",
      provider: "claude",
      is_off: false,
    };

    const topology = {
      edges: [
        {
          a: "test-agent-1",
          b: "test-agent-2",
          origin: "manual",
        },
      ],
      ignored_pairs: [],
      fallback_groups: [],
    };

    await installGraphTopologyIpcMock(page, topology, [agent1, agent2]);
    await openGraphView(page);

    // Wait for canvas and inspector to render
    await expect(page.locator(".graph-canvas-shell")).toBeVisible();
    await expect(page.locator(".graph-inspector")).toBeVisible({ timeout: 5_000 });

    // The inspector defaults to the first agent in the graph (Alpha)
    // Verify the inspector header shows Alpha's info
    const inspectorHeader = page.locator(".graph-inspector h2");
    await expect(inspectorHeader).toContainText("Alpha");
    await expect(page.locator(".graph-inspector")).toContainText("Coordinates the alpha workstream");

    // Wait for and verify the neighbors panel is visible
    await expect(page.locator(".graph-neighbors-list")).toBeVisible();

    // Verify the neighbor (Beta) is listed; persisted edges carry no origin
    // tag (all are manual) — only ghost pairs get an "Unmapped" badge
    const neighborsRow = page.locator(".graph-neighbors-row").first();
    await expect(neighborsRow).toContainText("Beta");
    await expect(neighborsRow.locator(".graph-inspector-unmapped")).toHaveCount(0);
    await page.locator(".graph-inspector").screenshot({
      path: path.join("e2e", "screenshots", "graph", "2026-08-04", "agent-description-inspector.png"),
      animations: "disabled",
    });
  });

  test("shows an existing agent's status event in both Graph and Watchlist", async ({ page }, testInfo) => {
    const alpha: MockAgent = {
      session_id: "graph-status-alpha",
      session_name: "Alpha",
      agent_class: "TestClass",
      folder: "/test/alpha",
      provider: "claude",
      is_off: false,
    };
    await installGraphTopologyIpcMock(page, {
      edges: [], ignored_pairs: [], fallback_groups: [],
    }, [alpha], [], GRAPH_STATUS_WATCHLIST_PREFS);
    await openGraphView(page);

    const alphaRow = page.getByLabel("Agent Alpha", { exact: true });
    const graphInspector = surfacePanel(page, "graph").locator(".graph-inspector");
    const graphCanvas = surfacePanel(page, "graph").locator(".graph-canvas-shell");
    await expect(alphaRow).toBeVisible();
    const idleStatusDot = alphaRow.locator(".bg-wardian-success");
    await expect(idleStatusDot).toBeVisible();
    const idleStatus = alphaRow.locator('[aria-label^="Status: Idle"]');
    await expect(idleStatus).toHaveText("Idle");
    await expect(graphInspector).toContainText("Idle");
    const idleColor = await idleStatusDot.evaluate((element) => getComputedStyle(element).backgroundColor);
    await expect.poll(async () => countScreenshotPixelsMatchingColor(
      page,
      await graphCanvas.screenshot({ animations: "disabled" }),
      idleColor,
    )).toBeGreaterThan(0);
    await expect.poll(() => page.evaluate(() => (
      window as Window & {
        __WARDIAN_E2E_GRAPH_RUNTIME__?: { hasListener: (event: string) => boolean };
      }
    ).__WARDIAN_E2E_GRAPH_RUNTIME__?.hasListener("agent-status-updated"))).toBe(true);

    await page.evaluate(() => {
      (window as Window & {
        __WARDIAN_E2E_GRAPH_RUNTIME__?: { emit: (event: string, payload: unknown) => void };
      }).__WARDIAN_E2E_GRAPH_RUNTIME__?.emit("agent-status-updated", {
        session_id: "graph-status-alpha",
        current_status: "Action Needed",
      });
    });

    const actionNeededStatusDot = alphaRow.locator(".bg-wardian-warning");
    await expect(actionNeededStatusDot).toBeVisible();
    const actionRequiredStatus = alphaRow.locator('[aria-label^="Status: Action Required"]');
    await expect(actionRequiredStatus).toHaveText("Action Required");
    await expect(actionRequiredStatus).toHaveClass(/text-wardian-warning/);
    await expect(graphInspector).toContainText("Action Required");
    const actionNeededColor = await actionNeededStatusDot.evaluate((element) => getComputedStyle(element).backgroundColor);
    await expect.poll(async () => countScreenshotPixelsMatchingColor(
      page,
      await graphCanvas.screenshot({ animations: "disabled" }),
      actionNeededColor,
    )).toBeGreaterThan(0);
    const actionNeededScreenshot = await graphCanvas.screenshot({ animations: "disabled" });
    expect(await countScreenshotPixelsMatchingColor(page, actionNeededScreenshot, idleColor)).toBe(0);

    const screenshotPath = testInfo.outputPath("graph-and-watchlist-action-required.png");
    const graphView = surfacePanel(page, "graph").locator('[data-testid="graph-view"]');
    const watchlist = page.getByTestId("agent-watchlist");
    const [graphBounds, watchlistBounds] = await Promise.all([
      graphView.boundingBox(),
      watchlist.boundingBox(),
    ]);
    expect(graphBounds).not.toBeNull();
    expect(watchlistBounds).not.toBeNull();
    const clipX = Math.floor(Math.min(graphBounds!.x, watchlistBounds!.x));
    const clipY = Math.floor(Math.min(graphBounds!.y, watchlistBounds!.y));
    const clipRight = Math.ceil(Math.max(
      graphBounds!.x + graphBounds!.width,
      watchlistBounds!.x + watchlistBounds!.width,
    ));
    const clipBottom = Math.ceil(Math.max(
      graphBounds!.y + graphBounds!.height,
      watchlistBounds!.y + watchlistBounds!.height,
    ));
    await page.screenshot({
      path: screenshotPath,
      animations: "disabled",
      clip: { x: clipX, y: clipY, width: clipRight - clipX, height: clipBottom - clipY },
    });
    await testInfo.attach("graph-and-watchlist-action-required", {
      path: screenshotPath,
      contentType: "image/png",
    });
  });

  test("add-connection picker opens and filters agents", async () => {
    const agent1: MockAgent = {
      session_id: "add-test-1",
      session_name: "Creator",
      agent_class: "TestClass",
      folder: "/test/creator",
      provider: "claude",
      is_off: false,
    };

    const agent2: MockAgent = {
      session_id: "add-test-2",
      session_name: "Candidate",
      agent_class: "TestClass",
      folder: "/test/candidate",
      provider: "claude",
      is_off: false,
    };

    const agent3: MockAgent = {
      session_id: "add-test-3",
      session_name: "Already Connected",
      agent_class: "TestClass",
      folder: "/test/connected",
      provider: "claude",
      is_off: false,
    };

    const topology = {
      edges: [
        {
          a: "add-test-1",
          b: "add-test-3",
          origin: "manual",
        },
      ],
      ignored_pairs: [],
      fallback_groups: [],
    };

    await installGraphTopologyIpcMock(page, topology, [agent1, agent2, agent3]);
    await openGraphView(page);

    // Wait for inspector to be visible with the first agent (Creator)
    await expect(page.locator(".graph-inspector")).toBeVisible();
    const inspectorHeader = page.locator(".graph-inspector h2");
    await expect(inspectorHeader).toContainText("Creator");

    // The "Add connection…" button should be visible after the neighbors list
    const addBtn = page.locator(".graph-neighbors-add-btn").first();
    await expect(addBtn).toBeVisible();
    await addBtn.click();

    // Verify picker opens
    const picker = page.locator(".graph-neighbors-picker");
    await expect(picker).toBeVisible();

    // Verify input field is focused and ready
    const pickerInput = picker.locator(".graph-neighbors-picker-input");
    await expect(pickerInput).toBeFocused();

    // Type to filter agents; the assertion below auto-waits for the filter
    await pickerInput.fill("Candidate");

    // Verify "Candidate" appears in the list
    const pickerList = picker.locator(".graph-neighbors-picker-list");
    await expect(pickerList).toContainText("Candidate");

    // Close the picker without selection
    await pickerInput.press("Escape");
    await expect(picker).toBeHidden();
  });

  test("streamlines scope and toggles graph labels from the canvas menu", async () => {
    const agents: MockAgent[] = ["Alpha", "Beta", "Gamma"].map((session_name, index) => ({
      session_id: `label-test-${index}`,
      session_name,
      agent_class: "TestClass",
      folder: `/test/${session_name.toLowerCase()}`,
      provider: "claude",
      is_off: false,
    }));

    await installGraphTopologyIpcMock(page, {
      edges: [],
      ignored_pairs: [],
      fallback_groups: [],
    }, agents);
    await openGraphView(page);

    const graphPanel = surfacePanel(page, "graph");
    await expect(graphPanel.locator(".graph-scope-label")).toHaveText("All agents");
    await expect(graphPanel.locator(".graph-scope-count")).toHaveCount(0);
    await expect(graphPanel.locator(".graph-toolbar").getByText("Shift-drag to connect", { exact: true })).toHaveCount(0);
    await expect(graphPanel.locator(".graph-onboarding-hint")).toContainText("Shift-drag");

    const canvasShell = graphPanel.locator(".graph-canvas-shell");
    await expect(canvasShell.locator("canvas").last()).toBeVisible();
    const canvasBox = await canvasShell.boundingBox();
    expect(canvasBox).not.toBeNull();
    await page.mouse.click(
      canvasBox!.x + canvasBox!.width - 24,
      canvasBox!.y + canvasBox!.height - 24,
      { button: "right" },
    );

    await expect(page.getByRole("menuitem", { name: "Show selected agent names only" })).toBeVisible();
    const screenshotDir = process.env.WARDIAN_GRAPH_LABEL_SCREENSHOT_DIR;
    if (screenshotDir) {
      fs.mkdirSync(screenshotDir, { recursive: true });
      const screenshotPath = path.join(screenshotDir, "graph-label-display-menu.png");
      await graphPanel.screenshot({ path: screenshotPath, animations: "disabled" });
      await test.info().attach("graph-label-display-menu", {
        path: screenshotPath,
        contentType: "image/png",
      });
    }

    await page.getByRole("menuitem", { name: "Show selected agent names only" }).click();
    await page.mouse.click(
      canvasBox!.x + canvasBox!.width - 24,
      canvasBox!.y + canvasBox!.height - 24,
      { button: "right" },
    );
    await expect(page.getByRole("menuitem", { name: "Show all agent names" })).toBeVisible();
  });

  test("neighbors panel shows unmapped badge for ghost edges", async () => {
    test.skip(
      true,
      "@native-only: Ghost edges require pair activity data from backend, which is not available in browser mock layer. Test in native E2E with real IPC."
    );
  });

  test("unmapped neighbor actions fit the inspector row", async () => {
    const agent1: MockAgent = {
      session_id: "ghost-style-1",
      session_name: "Source",
      agent_class: "TestClass",
      folder: "/test/source",
      provider: "claude",
      is_off: false,
    };
    const agent2: MockAgent = {
      session_id: "ghost-style-2",
      session_name: "BionicFace-PCB",
      agent_class: "TestClass",
      folder: "/test/target",
      provider: "claude",
      is_off: false,
    };
    const topology = {
      edges: [],
      ignored_pairs: [],
      fallback_groups: [],
    };

    await installGraphTopologyIpcMock(page, topology, [agent1, agent2], [
      {
        a: agent1.session_id,
        b: agent2.session_id,
        last_message_at: new Date(Date.now() - 60_000).toISOString(),
        active_ask: false,
      },
    ]);
    await openGraphView(page);

    const neighborsRow = page.locator(".graph-neighbors-row").first();
    await expect(neighborsRow).toContainText("BionicFace-PCB");
    await expect(neighborsRow.locator(".graph-inspector-unmapped")).toHaveText("Unmapped");

    const formalize = neighborsRow.locator(".graph-neighbors-action-formalize");
    const ignore = neighborsRow.locator(".graph-neighbors-action-ignore");
    await expect(formalize).toHaveText("Formalize");
    await expect(ignore).toHaveText("Ignore");

    const rowBox = await neighborsRow.boundingBox();
    const formalizeBox = await formalize.boundingBox();
    const ignoreBox = await ignore.boundingBox();
    expect(rowBox).not.toBeNull();
    expect(formalizeBox).not.toBeNull();
    expect(ignoreBox).not.toBeNull();
    expect(formalizeBox!.x + formalizeBox!.width).toBeLessThanOrEqual(rowBox!.x + rowBox!.width + 1);
    expect(ignoreBox!.x + ignoreBox!.width).toBeLessThanOrEqual(rowBox!.x + rowBox!.width + 1);

    await neighborsRow.screenshot({ path: "e2e/screenshots/graph/20260804-unmapped-neighbor-actions.png" });
  });

  test("formalize and ignore actions on ghost edges", async () => {
    test.skip(
      true,
      "@native-only: Ghost edge formalize/ignore requires real Tauri invoke(add_topology_edge, ignore_topology_pair), which cannot be verified in browser mock layer."
    );
  });

  test("manual edge shows delete button", async () => {
    const agent1: MockAgent = {
      session_id: "delete-test-1",
      session_name: "Source",
      agent_class: "TestClass",
      folder: "/test/source",
      provider: "claude",
      is_off: false,
    };

    const agent2: MockAgent = {
      session_id: "delete-test-2",
      session_name: "Target",
      agent_class: "TestClass",
      folder: "/test/target",
      provider: "claude",
      is_off: false,
    };

    const topology = {
      edges: [
        {
          a: "delete-test-1",
          b: "delete-test-2",
          origin: "manual",
        },
      ],
      ignored_pairs: [],
      fallback_groups: [],
    };

    await installGraphTopologyIpcMock(page, topology, [agent1, agent2]);
    await openGraphView(page);

    // Wait for inspector and neighbors panel
    await expect(page.locator(".graph-inspector")).toBeVisible();
    await expect(page.locator(".graph-neighbors-list")).toBeVisible();

    // Verify edge is shown with delete button (× symbol)
    const neighborsRow = page.locator(".graph-neighbors-row").first();
    await expect(neighborsRow).toContainText("Target");
    const deleteBtn = neighborsRow.locator(".graph-neighbors-action-btn");
    await expect(deleteBtn).toContainText("×");
  });

  test("keeps the graph surface stable during repeated wheel zoom", async ({ page }, testInfo) => {
    const runtimeErrors: string[] = [];
    const onPageError = (error: Error) => runtimeErrors.push(error.message);
    page.on("pageerror", onPageError);

    const zoomAgents: MockAgent[] = Array.from({ length: 12 }, (_, index) => ({
      session_id: `zoom-test-${index}`,
      session_name: `Zoom Agent ${index}`,
      agent_class: "TestClass",
      folder: `/test/zoom-${index}`,
      provider: "claude",
      is_off: false,
    }));
    const topology = {
      edges: zoomAgents.slice(1).map((agent, index) => ({
        a: zoomAgents[index].session_id,
        b: agent.session_id,
        origin: "manual",
      })),
      ignored_pairs: [] as [string, string][],
      fallback_groups: [] as string[][],
    };

    try {
      await installGraphTopologyIpcMock(page, topology, zoomAgents);
      await openGraphView(page);

      const canvasShell = surfacePanel(page, "graph").locator(".graph-canvas-shell");
      const before = await canvasShell.boundingBox();
      expect(before).not.toBeNull();
      await page.mouse.move(
        before!.x + before!.width / 2,
        before!.y + before!.height / 2,
      );

      for (let index = 0; index < 8; index += 1) {
        await page.mouse.wheel(0, index < 5 ? -180 : 180);
        await page.waitForTimeout(20);
      }

      await expect(canvasShell).toBeVisible();
      expect(await canvasShell.locator("canvas").count()).toBeGreaterThan(1);
      const after = await canvasShell.boundingBox();
      expect(after).not.toBeNull();
      expect(after!.width).toBeCloseTo(before!.width, 0);
      expect(after!.height).toBeCloseTo(before!.height, 0);
      expect(runtimeErrors).toEqual([]);

      const screenshotDir = process.env.WARDIAN_GRAPH_ZOOM_SCREENSHOT_DIR;
      if (screenshotDir) {
        fs.mkdirSync(screenshotDir, { recursive: true });
        const screenshotPath = path.join(screenshotDir, "graph-after-wheel-zoom.png");
        await canvasShell.screenshot({ path: screenshotPath, animations: "disabled" });
        await testInfo.attach("graph-after-wheel-zoom", {
          path: screenshotPath,
          contentType: "image/png",
        });
      }
    } finally {
      page.off("pageerror", onPageError);
    }
  });

  test("delete button triggers remove_topology_edge command", async () => {
    test.skip(
      true,
      "@native-only: Delete button click invokes remove_topology_edge, which requires real Tauri IPC to persist state changes. Browser mock cannot verify the backend effect."
    );
  });
});

test.describe("seedTopology fixture", () => {
  test("writes canonically ordered topology.json the Rust loader can parse", () => {
    // Runs in the Playwright Node context: the browser layer never reads
    // topology.json (the backend does), so the helper is verified by its
    // on-disk output here and consumed for real by native E2E tests.
    const home = fs.mkdtempSync(path.join(os.tmpdir(), "wardian-topo-"));
    try {
      seedTopology(home, [["zeta", "alpha"]], [["mike", "kilo"]]);

      const written = JSON.parse(
        fs.readFileSync(path.join(home, "topology.json"), "utf8"),
      );
      expect(written.version).toBe(1);
      expect(written.edges).toHaveLength(1);
      expect(written.edges[0].a).toBe("alpha");
      expect(written.edges[0].b).toBe("zeta");
      expect(typeof written.edges[0].created_at).toBe("string");
      expect(Date.parse(written.edges[0].created_at)).not.toBeNaN();
      expect(written.ignored_pairs).toEqual([{ a: "kilo", b: "mike" }]);
    } finally {
      fs.rmSync(home, { recursive: true, force: true });
    }
  });
});
