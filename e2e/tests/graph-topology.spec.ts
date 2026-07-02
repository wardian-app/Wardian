import { test, expect, type Page } from "@playwright/test";
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
}

async function installGraphTopologyIpcMock(
  page: Page,
  topology: {
    edges: Array<{ a: string; b: string; origin: string }>;
    ignored_pairs: [string, string][];
    fallback_groups: string[][];
  },
  agents: MockAgent[],
) {
  await page.addInitScript(({ topologyFixture, agentsFixture }) => {
    let callbackId = 1;
    const callbacks = new Map<number, unknown>();
    const tauriWindow = window as Window & {
      __TAURI_INTERNALS__?: Record<string, unknown>;
      __TAURI_EVENT_PLUGIN_INTERNALS__?: Record<string, unknown>;
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
        if (command === "list_agent_classes") {
          return [{ name: "TestClass", description: "Graph test class", is_default: true }];
        }
        if (command === "list_provider_readiness") {
          return [
            { provider: "claude", display_name: "Claude", available: true, executable: "C:/tools/claude.cmd", reason: null },
          ];
        }
        if (command === "load_watchlists") return [];
        if (command === "load_watchlist_prefs") return null;
        if (command === "load_agent_interactions") return {};
        if (command === "load_queue_items") return [];
        if (command === "load_queue_preferences") return {};
        if (command === "load_onboarding_hints") {
          return { dismissed_hint_ids: ["spawn-agent-first-run:v1"] };
        }
        if (command === "dismiss_onboarding_hint") {
          return { dismissed_hint_ids: ["spawn-agent-first-run:v1"] };
        }
        if (command === "list_workflows") return [];
        if (command === "list_scheduled_runs") return [];
        if (command === "load_workflow_library") return { folders: [], rootWorkflowIds: [] };
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
        if (command === "get_pair_activity") return [];
        if (command === "plugin:event|listen") return callbackId++;
        if (command === "plugin:event|unlisten") return null;
        if (command === "sync_provider_theme_settings") return null;
        return null;
      },
    };
  }, { topologyFixture: topology, agentsFixture: agents });
}

async function openGraphView(page: Page) {
  await page.setViewportSize({ width: 1400, height: 900 });
  await page.goto("/", { waitUntil: "domcontentloaded" });
  await page.locator('[data-testid="app-shell"]').waitFor({ timeout: 15_000 });

  // Click the Graph tab in the titlebar
  await page.getByRole("button", { name: "Graph", exact: true }).click();
  await expect(page.locator('[data-testid="graph-view"]')).toBeVisible({ timeout: 10_000 });
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

    // Wait for and verify the neighbors panel is visible
    await expect(page.locator(".graph-neighbors-list")).toBeVisible();

    // Verify the neighbor (Beta) is listed with "manual" origin tag
    const neighborsRow = page.locator(".graph-neighbors-row").first();
    await expect(neighborsRow).toContainText("Beta");
    await expect(neighborsRow.locator(".graph-neighbors-origin")).toContainText("manual");
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

  test("neighbors panel shows unmapped badge for ghost edges", async () => {
    test.skip(
      true,
      "@native-only: Ghost edges require pair activity data from backend, which is not available in browser mock layer. Test in native E2E with real IPC."
    );
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
