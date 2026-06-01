import { expect, test, type Page } from "@playwright/test";

type RunEvent = { seq: number; ts: string; kind: string; [key: string]: unknown };

const blueprint = {
  schema: 2,
  id: "wf-run-e2e",
  name: "Run View E2E",
  nodes: [
    { id: "trigger", type: "manual_trigger", name: "Trigger", fields: {}, position: { x: 0, y: 80 } },
    { id: "a", type: "task", name: "Plan", fields: { agent: "role:planner", prompt: "Plan" }, position: { x: 320, y: 80 } },
    { id: "b", type: "task", name: "Ship", fields: { agent: "role:builder", prompt: "Ship" }, position: { x: 640, y: 80 } },
  ],
  edges: [
    { from: "trigger", to: "a", from_port: "out", to_port: "in" },
    { from: "a", to: "b", from_port: "out", to_port: "in" },
  ],
};

const events: RunEvent[] = [
  { seq: 0, ts: "t0", kind: "run_started", blueprint_id: blueprint.id, schema: 2, trigger: {} },
  { seq: 1, ts: "t1", kind: "node_started", node: "a" },
  { seq: 2, ts: "t2", kind: "node_completed", node: "a", output: { ok: true } },
  { seq: 3, ts: "t3", kind: "node_started", node: "b" },
  { seq: 4, ts: "t4", kind: "node_failed", node: "b", error: "boom" },
];

async function installRunViewIpcMock(page: Page) {
  await page.addInitScript(({ blueprintFixture, eventFixture }) => {
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
      invoke: async (command: string) => {
        if (command === "list_agents") return [];
        if (command === "list_agent_classes") return [];
        if (command === "list_provider_readiness") return [];
        if (command === "load_watchlists") return [];
        if (command === "load_watchlist_prefs") return null;
        if (command === "load_agent_interactions") return {};
        if (command === "load_queue_items") return [];
        if (command === "load_queue_preferences") return {};
        if (command === "load_onboarding_hints") return { dismissed_hint_ids: ["spawn-agent-first-run:v1"] };
        if (command === "dismiss_onboarding_hint") return { dismissed_hint_ids: ["spawn-agent-first-run:v1"] };
        if (command === "list_workflows") return [];
        if (command === "list_scheduled_runs") return [];
        if (command === "load_workflow_library") return { folders: [], rootWorkflowIds: [] };
        if (command === "get_library_tree") return { type: "Folder", path: "", name: "Root", children: [] };
        if (command === "list_deployed_skills") return [];
        if (command === "load_app_settings") return null;
        if (command === "load_shell_settings") {
          return {
            shell_id: "auto",
            custom_executable: null,
            custom_args: null,
            agent_session_persistence: "resume",
            default_provider: "codex",
          };
        }
        if (command === "list_available_shells") return [];
        if (command === "plugin:event|listen") return callbackId++;
        if (command === "plugin:event|unlisten") return null;
        if (command === "sync_provider_theme_settings") return null;

        if (command === "workflow_list_blueprints") return [];
        if (command === "workflow_list_runs") {
          return [{
            run_id: "run-e2e-1",
            blueprint_id: "wf-run-e2e",
            status: "failed",
            node_count: 2,
            failure: "boom",
            path: "<absolute-workspace-path>/logs/workflows/wf-run-e2e/run-e2e-1",
          }];
        }
        if (command === "workflow_read_run") {
          return {
            state: {
              run_id: "run-e2e-1",
              blueprint_id: "wf-run-e2e",
              status: "failed",
              nodes: { a: "completed", b: "failed" },
              registry: { nodes: { a: { output: { ok: true } } }, trigger: { output: {} } },
              loop_iter: {},
              delivered: {},
              skipped_edges: [],
              next_seq: 5,
              failure: "boom",
            },
            events: eventFixture,
            blueprint: blueprintFixture,
          };
        }

        return null;
      },
    };
  }, { blueprintFixture: blueprint, eventFixture: events });
}

async function openRunView(page: Page) {
  await page.setViewportSize({ width: 1700, height: 980 });
  await page.goto("/", { waitUntil: "domcontentloaded" });
  await page.locator('[data-testid="app-shell"]').waitFor({ timeout: 15_000 });
  await page.locator(".titlebar-center").getByRole("button", { name: "Workflows" }).click();
  await page.evaluate(async () => {
    const { useWorkflowsView } = await import("/src/store/useWorkflowsView.ts");
    const { useRunStore } = await import("/src/features/workflows/run/useRunStore.ts");
    useWorkflowsView.getState().observeRun("wf-run-e2e", "run-e2e-1");
    await useRunStore.getState().openRun("wf-run-e2e", "run-e2e-1");
  });
  await expect(page.getByTestId("workflows-observe-mode")).toBeVisible();
  await expect(nodeById(page, "a")).toBeVisible();
  await expect(nodeById(page, "b")).toBeVisible();
}

function nodeById(page: Page, id: string) {
  return page.getByTestId(`run-dag-node-${id}`);
}

async function scrubTo(page: Page, index: number) {
  const input = page.getByLabel("Event scrubber");
  await input.fill(String(index));
  await expect(input).toHaveValue(String(index));
}

test("run view observes, scrubs, and inspects a failed run", async ({ page }) => {
  await installRunViewIpcMock(page);
  await openRunView(page);

  const nodeA = nodeById(page, "a");
  const nodeB = nodeById(page, "b");

  await expect(nodeA).toContainText("completed");
  await expect(nodeB).toContainText("failed");
  await expect(nodeA).toHaveAttribute("data-status", "completed");
  await expect(nodeB).toHaveAttribute("data-status", "failed");

  await scrubTo(page, 1);
  await expect(nodeA).toContainText("running");
  await expect(nodeB).toContainText("pending");
  await expect(nodeA).toHaveAttribute("data-status", "running");
  await expect(nodeB).toHaveAttribute("data-status", "pending");

  if (process.env.WARDIAN_RUN_VIEW_SCREENSHOT_DIR) {
    await page.getByTestId("workflows-observe-mode").screenshot({
      path: `${process.env.WARDIAN_RUN_VIEW_SCREENSHOT_DIR}/run-view-mid-scrub.png`,
    });
  }

  await nodeB.click();
  await expect(page.getByTestId("workflows-observe-mode").locator("aside").last().getByText("boom")).toBeVisible();

  if (process.env.WARDIAN_RUN_VIEW_SCREENSHOT_DIR) {
    await page.getByTestId("workflows-observe-mode").screenshot({
      path: `${process.env.WARDIAN_RUN_VIEW_SCREENSHOT_DIR}/run-view-failed-inspector.png`,
    });
  }
});
