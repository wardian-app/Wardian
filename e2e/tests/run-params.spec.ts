import { expect, test, type Page } from "@playwright/test";

const blueprint = {
  schema: 2,
  id: "wf",
  name: "Parameterized WF",
  nodes: [
    {
      id: "trigger",
      type: "manual_trigger",
      name: "Trigger",
      fields: {
        input_schema: JSON.stringify({
          type: "object",
          properties: { symbol: { type: "string" } },
        }),
      },
      position: { x: 0, y: 80 },
    },
    {
      id: "plan",
      type: "task",
      name: "Plan",
      fields: { agent: "role:planner", prompt: "Plan {{trigger.output.symbol}}" },
      position: { x: 320, y: 80 },
    },
  ],
  edges: [{ from: "trigger", to: "plan", from_port: "out", to_port: "in" }],
};

async function installRunParamsIpcMock(page: Page) {
  await page.addInitScript(({ blueprintFixture }) => {
    let callbackId = 1;
    const callbacks = new Map<number, unknown>();
    const tauriWindow = window as Window & {
      __runParamsInvokes?: Array<{ command: string; args?: Record<string, unknown> }>;
      __TAURI_INTERNALS__?: Record<string, unknown>;
      __TAURI_EVENT_PLUGIN_INTERNALS__?: Record<string, unknown>;
    };

    tauriWindow.__runParamsInvokes = [];
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
        tauriWindow.__runParamsInvokes?.push({ command, args });

        if (command === "list_agents") return [];
        if (command === "list_agent_classes") return [];
        if (command === "list_provider_readiness") {
          return [
            { provider: "claude", display_name: "Claude", available: true, executable: "claude", reason: null },
            { provider: "codex", display_name: "Codex", available: true, executable: "codex", reason: null },
            { provider: "gemini", display_name: "Gemini", available: true, executable: "gemini", reason: null },
            { provider: "antigravity", display_name: "Antigravity", available: true, executable: "agy", reason: null },
            { provider: "opencode", display_name: "OpenCode", available: true, executable: "opencode", reason: null },
          ];
        }
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

        if (command === "workflow_list_blueprints") {
          return [{ id: "wf", name: "Parameterized WF", path: "/x/wf.md" }];
        }
        if (command === "workflow_parse") return { blueprint: blueprintFixture, diagnostics: [] };
        if (command === "workflow_validate") return { ok: true, diagnostics: [] };
        if (command === "workflow_run_v2") {
          return { ok: true, run_id: "run-params-1", blueprint_id: "wf", run_dir: "/runs/run-params-1" };
        }
        if (command === "workflow_list_runs") return [];
        if (command === "workflow_read_run") {
          return {
            state: {
              run_id: "run-params-1",
              blueprint_id: "wf",
              status: "completed",
              nodes: { plan: "completed" },
              registry: { nodes: { plan: { output: { ok: true } } }, trigger: { output: { symbol: "SPY" } } },
              loop_iter: {},
              delivered: {},
              skipped_edges: [],
              next_seq: 3,
              failure: null,
            },
            events: [
              { seq: 0, ts: "t0", kind: "run_started", blueprint_id: "wf", schema: 2, trigger: { symbol: "SPY" } },
              { seq: 1, ts: "t1", kind: "node_completed", node: "plan", output: { ok: true } },
              { seq: 2, ts: "t2", kind: "run_completed" },
            ],
            blueprint: blueprintFixture,
          };
        }

        return null;
      },
    };
  }, { blueprintFixture: blueprint });
}

test("parameterized run dialog sends entry input to workflow_run_v2", async ({ page }) => {
  await installRunParamsIpcMock(page);
  await page.setViewportSize({ width: 1700, height: 980 });
  await page.goto("/", { waitUntil: "domcontentloaded" });
  await page.locator('[data-testid="app-shell"]').waitFor({ timeout: 15_000 });

  const titlebar = page.locator(".titlebar-center");
  await titlebar.getByRole("button", { name: "Workflows" }).click();
  await page.evaluate(async () => {
    const { useSettingsStore } = await import("/src/store/useSettingsStore.ts");
    useSettingsStore.setState({ default_provider: "codex" });
  });

  await page.getByTestId("blueprint-selector").getByRole("combobox").selectOption("/x/wf.md");
  await page.getByTestId("workflows-view").getByRole("button", { name: /^Run$/ }).click();

  const dialog = page.getByRole("dialog");
  await expect(dialog).toBeVisible();
  await expect(dialog.getByLabel(/provider/i)).toHaveValue("codex");
  await expect(dialog.getByLabel(/symbol/i)).toBeVisible();
  await dialog.getByLabel(/symbol/i).fill("SPY");
  await dialog.screenshot({ path: "e2e/screenshots/run-params/param-form.png" });
  await dialog.getByRole("button", { name: /^Run$/ }).click();

  await page.waitForFunction(() => window.__runParamsInvokes?.some((call) => call.command === "workflow_run_v2"));
  const runCall = await page.evaluate(() => window.__runParamsInvokes?.find((call) => call.command === "workflow_run_v2"));
  expect(runCall?.args).toMatchObject({
    path: "/x/wf.md",
    provider: "codex",
    input: { symbol: "SPY" },
  });
});
