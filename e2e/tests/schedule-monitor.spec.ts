import { expect, test, type Page } from "@playwright/test";

const blueprint = {
  schema: 2,
  id: "wf",
  name: "Scheduled WF",
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

async function installScheduleMonitorIpcMock(page: Page) {
  await page.addInitScript(({ blueprintFixture }) => {
    let callbackId = 1;
    const callbacks = new Map<number, unknown>();
    let schedules: Array<Record<string, unknown>> = [];
    const tauriWindow = window as Window & {
      __scheduleMonitorInvokes?: Array<{ command: string; args?: Record<string, unknown> }>;
      __TAURI_INTERNALS__?: Record<string, unknown>;
      __TAURI_EVENT_PLUGIN_INTERNALS__?: Record<string, unknown>;
    };

    tauriWindow.__scheduleMonitorInvokes = [];
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
        tauriWindow.__scheduleMonitorInvokes?.push({ command, args });

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
          return [{ id: "wf", name: "Scheduled WF", path: "/x/wf.md" }];
        }
        if (command === "workflow_parse") return { blueprint: blueprintFixture, diagnostics: [] };
        if (command === "workflow_validate") return { ok: true, diagnostics: [] };
        if (command === "workflow_list_runs") return [];
        if (command === "workflow_read_run") return { state: null, events: [], blueprint: null };

        if (command === "schedule_list") return schedules;
        if (command === "schedule_create") {
          const schedule = {
            id: `schedule-${schedules.length + 1}`,
            blueprint_id: args?.blueprintId,
            name: args?.name,
            provider: args?.provider ?? null,
            workspace: args?.workspace ?? null,
            input: args?.input ?? {},
            bindings: args?.bindings ?? {},
            schedule: args?.schedule,
            next_run_epoch_ms: 1780174800000,
            paused_remaining_ms: null,
            is_paused: false,
            last_run_status: null,
            last_run_error: null,
            last_run_epoch_ms: null,
          };
          schedules = [...schedules, schedule];
          return schedule;
        }
        if (command === "schedule_pause") {
          schedules = schedules.map((schedule) => (
            schedule.id === args?.id ? { ...schedule, is_paused: true } : schedule
          ));
          return null;
        }
        if (command === "schedule_resume") {
          schedules = schedules.map((schedule) => (
            schedule.id === args?.id ? { ...schedule, is_paused: false } : schedule
          ));
          return null;
        }
        if (command === "schedule_remove") {
          schedules = schedules.filter((schedule) => schedule.id !== args?.id);
          return null;
        }
        if (command === "schedule_run_now") return null;

        return null;
      },
    };
  }, { blueprintFixture: blueprint });
}

test("schedule a blueprint and pause it in Monitor", async ({ page }) => {
  await installScheduleMonitorIpcMock(page);
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
  await dialog.getByRole("radio", { name: /schedule/i }).click();
  await dialog.getByLabel(/schedule name/i).fill("E2E Nightly");
  await dialog.getByRole("button", { name: /save schedule/i }).click();

  await page.getByTestId("workflows-view").getByRole("button", { name: /^monitor$/i }).click();
  await expect(page.getByTestId("workflow-monitor")).toBeVisible();
  await expect(page.getByTestId("workflow-monitor").getByRole("button", { name: "Pause E2E Nightly" }).first()).toBeVisible();

  await page.getByRole("button", { name: "Edit E2E Nightly" }).first().click();
  await expect(page.getByRole("dialog")).toBeVisible();
  await page.screenshot({ path: "e2e/screenshots/schedule-monitor/monitor-schedule-form.png", fullPage: true });
  await page.getByRole("button", { name: /^Cancel$/ }).click();

  await page.getByRole("button", { name: "Pause E2E Nightly" }).first().click();
  await expect(page.getByRole("button", { name: "Resume E2E Nightly" }).first()).toBeVisible();

  const scheduleCall = await page.evaluate(() => (
    (window as Window & {
      __scheduleMonitorInvokes?: Array<{ command: string; args?: Record<string, unknown> }>;
    }).__scheduleMonitorInvokes?.find((call) => call.command === "schedule_create")
  ));
  expect(scheduleCall?.args).toMatchObject({ blueprintId: "wf", name: "E2E Nightly" });
});
