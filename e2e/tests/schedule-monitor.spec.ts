import { expect, test, type Page } from "@playwright/test";
import { openSurface } from "../fixtures/workbench";
import { mkdir } from "node:fs/promises";

const adaptiveCardScreenshotDirectory =
  "e2e/screenshots/workflow-monitor-adaptive-cards/2026-07-16T06-18-35Z";
const fixedBrowserTime = "2026-07-16T16:00:00.000Z";

test.use({ locale: "en-US", timezoneId: "America/New_York" });

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
  await page.addInitScript(({ blueprintFixture, agentFixtures, completedRunFixture }) => {
    let callbackId = 1;
    const callbacks = new Map<number, unknown>();
    let schedules: Array<Record<string, unknown>> = [{
      id: "schedule-script-only",
      blueprint_id: "script-only",
      name: "Trident LEAPS Scan",
      provider: null,
      workspace: null,
      input: {},
      bindings: {},
      schedule: { schedule_type: "weekly", days_of_week: ["Mon", "Tue", "Wed", "Thu", "Fri"], time_of_day: "09:35", active: true },
      next_run_epoch_ms: Date.parse("2026-07-17T13:35:00.000Z"),
      paused_remaining_ms: null,
      is_paused: false,
      last_run_status: null,
      last_run_error: null,
      last_run_epoch_ms: null,
    }];
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

        if (command === "list_agents") return agentFixtures;
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
        if (command === "workflow_list_runs") return [completedRunFixture];
        if (command === "workflow_read_run") return { state: null, events: [], blueprint: null };

        if (command === "schedule_list") return schedules;
        if (command === "schedule_create") {
          const schedule = {
            id: "schedule-1",
            blueprint_id: args?.blueprintId,
            name: args?.name,
            provider: args?.provider ?? null,
            workspace: args?.workspace ?? null,
            input: args?.input ?? {},
            bindings: args?.bindings ?? {},
            assignments: {
              analyst: { target_type: "agent", agent_id: "agent-analyst", conversation: "current" },
              reviewer: { target_type: "agent", agent_id: "agent-reviewer", conversation: "fresh_background" },
              writer: { target_type: "agent", agent_id: "agent-writer", conversation: "current" },
            },
            schedule: args?.schedule,
            next_run_epoch_ms: Date.parse("2026-07-17T13:30:00.000Z"),
            paused_remaining_ms: null,
            is_paused: false,
            last_run_status: "completed",
            last_run_error: null,
            last_run_epoch_ms: Date.parse("2026-07-15T16:32:00.000Z"),
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
  }, {
    blueprintFixture: blueprint,
    agentFixtures: [
      {
        session_id: "agent-analyst",
        session_name: "Analyst Ada",
        agent_class: "Analyst",
        folder: "/workspace",
        is_off: false,
        provider: "claude",
      },
      {
        session_id: "agent-reviewer",
        session_name: "Reviewer Rui",
        agent_class: "Reviewer",
        folder: "/workspace",
        is_off: false,
        provider: "codex",
      },
      {
        session_id: "agent-writer",
        session_name: "Writer Wren",
        agent_class: "Writer",
        folder: "/workspace",
        is_off: false,
        provider: "opencode",
      },
    ],
    completedRunFixture: {
      run_id: "run-completed",
      blueprint_id: "wf",
      schedule_id: "schedule-1",
      status: "completed",
      node_count: 2,
      failure: null,
      path: "/runs/wf/run-completed",
      started_at: "2026-07-15T16:30:00.000Z",
      updated_at: "2026-07-15T16:32:00.000Z",
      completed_at: "2026-07-15T16:32:00.000Z",
    },
  });
}

test("schedule a blueprint and prove adaptive Monitor cards", async ({ page }) => {
  await mkdir(adaptiveCardScreenshotDirectory, { recursive: true });
  await installScheduleMonitorIpcMock(page);
  await page.setViewportSize({ width: 1700, height: 980 });
  await page.clock.setFixedTime(fixedBrowserTime);
  await page.goto("/", { waitUntil: "domcontentloaded" });
  await page.locator('[data-testid="app-shell"]').waitFor({ timeout: 15_000 });
  await expect(page.evaluate(() => ({
    now: new Date().toISOString(),
    locale: Intl.DateTimeFormat().resolvedOptions().locale,
    timeZone: Intl.DateTimeFormat().resolvedOptions().timeZone,
  }))).resolves.toEqual({
    now: fixedBrowserTime,
    locale: "en-US",
    timeZone: "America/New_York",
  });

  await openSurface(page, "workflows");
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
  const monitor = page.getByTestId("workflow-monitor");
  await expect(monitor).toBeVisible();

  const scheduledCard = monitor.getByTestId("workflow-activity-row-wf");
  await expect(scheduledCard).toHaveAttribute("data-mode", "scheduled");
  await expect(scheduledCard).toContainText("Next run");
  await expect(scheduledCard).toContainText("Schedule");
  await expect(scheduledCard).toContainText("Tomorrow, 9:30 AM");
  await expect(scheduledCard).toContainText("Wed, Jul 15 · 12:32 PM");
  await expect(scheduledCard).toContainText("Analyst Ada");
  await expect(scheduledCard).toContainText("Reviewer Rui");
  await expect(scheduledCard).not.toContainText("Blueprint wf");
  await expect(scheduledCard).not.toContainText("run-completed");
  await expect(scheduledCard.getByRole("button", { name: "Show 1 more agents for E2E Nightly" })).toHaveText("+1 agents");
  await scheduledCard.screenshot({ path: `${adaptiveCardScreenshotDirectory}/monitor-adaptive-cards.png` });

  const scriptOnlyCard = monitor.getByTestId("workflow-activity-row-script-only");
  await expect(scriptOnlyCard).toContainText("Trident LEAPS Scan");
  await expect(scriptOnlyCard).not.toContainText("Default assignment");
  await scriptOnlyCard.screenshot({ path: `${adaptiveCardScreenshotDirectory}/script-only-card.png` });

  await monitor.getByRole("button", { name: "History" }).click();
  const historyCard = monitor.getByTestId("workflow-history-run-run-completed");
  await expect(historyCard).toContainText("Ran");
  await expect(historyCard).toContainText("Outcome");
  await expect(historyCard).toContainText("Wed, Jul 15 · 12:32 PM");
  await expect(historyCard).not.toContainText("Next run");
  await expect(historyCard).not.toContainText("Blueprint wf");
  await expect(historyCard).not.toContainText("run-completed");

  await page.getByTestId("sidebar-tab-workflows").click();
  const sidebarCard = page.getByTestId("workflow-glance-row-schedule-1");
  await expect(sidebarCard).toContainText("Analyst Ada");
  await expect(sidebarCard).toContainText("Reviewer Rui");
  await expect(sidebarCard.getByRole("button", { name: "Show 1 more agents for E2E Nightly" })).toHaveText("+1 agents");
  await page.evaluate(async () => {
    const { useLayoutStore } = await import("/src/store/useLayoutStore.ts");
    useLayoutStore.getState().setLeftSidebarWidth(360);
  });
  await expect.poll(() => sidebarCard.evaluate((element) => (
    element.closest("aside")?.getBoundingClientRect().width ?? 0
  ))).toBeGreaterThan(359);
  await page.evaluate(() => new Promise<void>((resolve) => {
    requestAnimationFrame(() => requestAnimationFrame(() => resolve()));
  }));
  await sidebarCard.screenshot({ path: `${adaptiveCardScreenshotDirectory}/sidebar-multi-agent-card.png` });

  await monitor.getByRole("button", { name: "Scheduled" }).click();
  await expect(monitor.getByRole("button", { name: "Pause E2E Nightly" }).first()).toBeVisible();

  await page.getByRole("button", { name: "Edit E2E Nightly" }).first().click();
  await expect(page.getByRole("dialog")).toBeVisible();
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
