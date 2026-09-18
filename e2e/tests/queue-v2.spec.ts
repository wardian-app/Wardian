import { expect, test, type Page } from "@playwright/test";
import { openSurface } from "../fixtures/workbench";
import { makeWorkbenchDocument } from "../fixtures/workbenchIpcMock";

async function installQueueV2IpcMock(page: Page) {
  const workbenchDocument = makeWorkbenchDocument();
  await page.addInitScript((workbenchDocument) => {
    type QueueItem = {
      id: string;
      type: "action_needed" | "agent_completed" | "automation_completed";
      timestamp: number;
      read: boolean;
      agent_session_id?: string;
      agent_name?: string;
      automation_name?: string;
      status?: "completed" | "failed";
      summary?: string;
      error?: string;
      provider_question?: {
        provider: "codex" | "claude";
        call_id: string;
        questions: Array<{
          id?: string;
          header?: string;
          prompt: string;
          options: Array<{ label: string; description?: string }>;
        }>;
      };
    };

    const now = Date.now();
    const useQueueBacklog = (window as Window & { __WARDIAN_E2E_QUEUE_BACKLOG__?: boolean })
      .__WARDIAN_E2E_QUEUE_BACKLOG__ === true;
    const useStructuredQuestion = (window as Window & { __WARDIAN_E2E_QUEUE_STRUCTURED_QUESTION__?: boolean })
      .__WARDIAN_E2E_QUEUE_STRUCTURED_QUESTION__ === true;
    let queueItems: QueueItem[] = useQueueBacklog ? Array.from({ length: 120 }, (_, index) => ({
      id: `backlog-${index}`,
      type: "agent_completed",
      timestamp: now - index,
      read: false,
      agent_name: `Inbox history ${index}`,
      summary: `Completed queued task ${index}.`,
    })) : useStructuredQuestion ? [] : [
      {
        id: "action-needed-1",
        type: "action_needed",
        timestamp: now,
        read: false,
        agent_session_id: "mock-session-e2e-001",
        agent_name: "E2E Coder",
        summary: "Approve the generated patch before continuing.\n1. Yes\n2. No",
      },
      {
        id: "agent-complete-1",
        type: "agent_completed",
        timestamp: now - 90_000,
        read: false,
        agent_session_id: "mock-session-e2e-001",
        agent_name: "E2E Coder",
        summary: "Finished the test summary.",
      },
      {
        id: "automation-failed-1",
        type: "automation_completed",
        timestamp: now - 180_000,
        read: false,
        automation_name: "Release Drill",
        status: "failed",
        error: "Verifier returned a non-zero exit code.",
      },
    ];
    let queuePreferences = {};
    let automationApprovals: Array<Record<string, unknown>> = [];
    const automationTerminalRuns: Array<Record<string, unknown>> = Array.isArray(
      (window as Window & { __WARDIAN_E2E_AUTOMATION_TERMINAL_RUNS__?: unknown })
        .__WARDIAN_E2E_AUTOMATION_TERMINAL_RUNS__,
    )
      ? (window as Window & { __WARDIAN_E2E_AUTOMATION_TERMINAL_RUNS__: Array<Record<string, unknown>> })
        .__WARDIAN_E2E_AUTOMATION_TERMINAL_RUNS__
      : [];
    const submittedPrompts: Array<{ sessionId: string; prompt: string }> = [];
    let callbackId = 1;
    const callbacks = new Map<number, unknown>();
    const eventHandlers = new Map<string, number>();
    const tauriWindow = window as Window & {
      __TAURI_INTERNALS__?: Record<string, unknown>;
      __TAURI_EVENT_PLUGIN_INTERNALS__?: Record<string, unknown>;
      __WARDIAN_E2E_SUBMITTED_PROMPTS__?: Array<{ sessionId: string; prompt: string }>;
      __WARDIAN_E2E_AUTOMATION_INBOX_UPDATE__?: (payload: Record<string, unknown>) => void;
      __WARDIAN_E2E_QUEUE_RUNTIME__?: { emit: (event: string, payload: unknown) => void };
    };

    tauriWindow.__WARDIAN_E2E_SUBMITTED_PROMPTS__ = submittedPrompts;
    tauriWindow.__WARDIAN_E2E_AUTOMATION_INBOX_UPDATE__ = (payload) => {
      automationApprovals = payload.status === "awaiting_approval" ? [{
        blueprint_id: payload.automation_id,
        blueprint_path: "/automations/release.md",
        run_id: payload.run_instance_id,
        node: "approve-release",
        title: payload.automation_name,
        prompt: "Approve the release automation?",
        created_at: new Date().toISOString(),
      }] : [];
      const handlerId = eventHandlers.get("automation-inbox-updated");
      const handler = handlerId === undefined
        ? undefined
        : callbacks.get(handlerId) as ((event: unknown) => void) | undefined;
      handler?.({ payload });
    };
    tauriWindow.__WARDIAN_E2E_QUEUE_RUNTIME__ = {
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
        if (command === "list_agents") {
          return [{
            session_id: "mock-session-e2e-001",
            session_name: "E2E Coder",
            agent_class: "TestClass",
            folder: "<absolute-workspace-path>",
            provider: "mock",
            is_off: false,
          }];
        }
        if (command === "list_agent_classes") {
          return [{ name: "TestClass", description: "E2E test class", is_default: true }];
        }
        if (command === "list_provider_readiness") return [];
        if (command === "load_watchlists") return [];
        if (command === "load_watchlist_prefs") return null;
        if (command === "load_agent_interactions") return {};
        if (command === "get_workbench_boot_config") return { safe_mode: false };
        if (command === "load_workbench_state") {
          return {
            source: "default",
            document: workbenchDocument,
            notice: null,
            durable_revision: workbenchDocument.revision,
            durable_token: `mock-token-${workbenchDocument.revision}`,
          };
        }
        if (command === "save_workbench_state") {
          const document = args?.document as { revision?: number } | undefined;
          const revision = document?.revision ?? workbenchDocument.revision;
          return {
            outcome: "saved",
            durable_revision: revision,
            durable_token: `mock-token-${revision}`,
            request_id: args?.request_id,
          };
        }
        if (command === "load_queue_items") return queueItems;
        if (command === "list_inbox_notifications") {
          return { notifications: [{
            id: "important-update-1",
            kind: "update",
            sender_session_id: "mock-session-e2e-001",
            status: "completed",
            title: "Migration update",
            body: "The Inbox migration is ready for review.",
            choices: [],
            created_at: new Date(now - 30_000).toISOString(),
          }, {
            id: "approval-request-1",
            kind: "approval",
            sender_session_id: "mock-session-e2e-001",
            status: "awaiting_reply",
            title: "Production deployment",
            body: "Choose whether this deployment may proceed.",
            proposed_action: "Deploy the approved release to production",
            risk: "This changes live traffic and may require rollback.",
            choices: ["Deploy", "Do not deploy"],
            created_at: new Date(now).toISOString(),
          }], truncated: false, next_offset: null };
        }
        if (command === "list_automation_inbox_approvals") return automationApprovals;
        if (command === "list_automation_inbox_terminal_runs") return automationTerminalRuns;
        if (command === "save_queue_items") {
          queueItems = args?.items as QueueItem[];
          return null;
        }
        if (command === "load_queue_preferences") return queuePreferences;
        if (command === "save_queue_preferences") {
          queuePreferences = args?.preferences ?? {};
          return null;
        }
        if (command === "submit_prompt_to_agent") {
          submittedPrompts.push({
            sessionId: String(args?.sessionId ?? ""),
            prompt: String(args?.prompt ?? ""),
          });
          return null;
        }
        if (command === "submit_inbox_provider_choice") {
          submittedPrompts.push({
            sessionId: String(args?.sessionId ?? ""),
            prompt: String(args?.prompt ?? ""),
          });
          return null;
        }
        if (command === "load_onboarding_hints") return { dismissed_hint_ids: ["spawn-agent-first-run:v1"] };
        if (command === "dismiss_onboarding_hint") return { dismissed_hint_ids: ["spawn-agent-first-run:v1"] };
        if (command === "list_automations") return [];
        if (command === "list_scheduled_runs") return [];
        if (command === "load_automation_library") return { folders: [], rootAutomationIds: [] };
        if (command === "get_library_tree") return { type: "Folder", path: "", name: "Root", children: [] };
        if (command === "list_deployed_skills") return [];
        if (command === "plugin:event|listen") {
          eventHandlers.set(String(args?.event), Number(args?.handler));
          return callbackId++;
        }
        if (command === "plugin:event|unlisten") return null;
        if (command === "sync_provider_theme_settings") return null;
        return null;
      },
    };
  }, workbenchDocument);
}

test.describe("Inbox", () => {
  test("shows notifications, action-needed cards, header filtering, and clickable action choices", async ({ page }) => {
    await installQueueV2IpcMock(page);
    await page.goto("/", { waitUntil: "domcontentloaded" });
    await page.locator('[data-testid="app-shell"]').waitFor({ timeout: 15_000 });

    await openSurface(page, "inbox");

    await expect(page.getByText("Action required", { exact: true })).toBeVisible();
    await expect(page.getByText("Production deployment", { exact: true })).toBeVisible();
    await expect(page.getByText("Migration update", { exact: true })).toBeVisible();
    await expect(page.getByText("Approve the generated patch before continuing.")).toBeVisible();
    await expect(page.getByRole("button", { name: "Filter Inbox events" })).toContainText("Filter: All events");
    await expect(page.getByLabel("Desktop alert for action required")).toBeHidden();
    await expect(page.getByLabel("Sound alert for action required")).toBeHidden();
    await expect(page.getByRole("button", { name: "Send action response 1: Yes" })).toBeVisible();

    if (process.env.WARDIAN_INBOX_SCREENSHOT) {
      await page
        .locator('[data-testid="surface-panel"][data-surface-type="inbox"]')
        .screenshot({ path: process.env.WARDIAN_INBOX_SCREENSHOT, animations: "disabled" });
    }

    await page.getByRole("button", { name: "Filter Inbox events" }).click();
    await expect(page.getByLabel("Show agent completions")).toBeChecked();
    await page.getByLabel("Show agent completions").uncheck();
    await expect(page.getByText("Finished the test summary.")).toBeHidden();

    await expect(page.getByRole("textbox", { name: "Quick response" })).toBeHidden();
    await page.getByRole("button", { name: "Send action response 1: Yes" }).click();
    await expect.poll(async () =>
      page.evaluate(() => window.__WARDIAN_E2E_SUBMITTED_PROMPTS__?.[0]?.prompt ?? ""),
    ).toBe("1");
  });

  test("projects live Codex questions through agent-json-event and keeps them read-only", async ({ page }, testInfo) => {
    await page.addInitScript(() => {
      (window as Window & { __WARDIAN_E2E_QUEUE_STRUCTURED_QUESTION__?: boolean })
        .__WARDIAN_E2E_QUEUE_STRUCTURED_QUESTION__ = true;
    });
    await installQueueV2IpcMock(page);
    await page.goto("/", { waitUntil: "domcontentloaded" });
    await page.locator('[data-testid="app-shell"]').waitFor({ timeout: 15_000 });
    await openSurface(page, "inbox");

    const emit = (event: string, payload: unknown) => page.evaluate(
      ({ eventName, eventPayload }) => {
        (window as Window & {
          __WARDIAN_E2E_QUEUE_RUNTIME__?: { emit: (event: string, payload: unknown) => void };
        }).__WARDIAN_E2E_QUEUE_RUNTIME__?.emit(eventName, eventPayload);
      },
      { eventName: event, eventPayload: payload },
    );
    const call = (callId: string) => ({
      type: "response_item",
      payload: {
        type: "function_call",
        name: "request_user_input_async",
        call_id: callId,
        arguments: JSON.stringify({
          questions: [{ title: "Which environment should receive this change?", options: ["Staging", "Production"] }],
        }),
      },
    });

    await emit("agent-status-updated", { session_id: "mock-session-e2e-001", current_status: "Processing..." });
    await emit("agent-json-event", { session_id: "mock-session-e2e-001", data: call("codex-call-1") });
    await expect(page.getByText("Which environment should receive this change?", { exact: true })).toBeVisible();
    await expect(page.getByText("Staging", { exact: true })).toBeVisible();
    await expect(page.getByRole("button", { name: "Open agent terminal" }).first()).toBeVisible();
    await expect(page.getByRole("button", { name: /send action response/i })).toHaveCount(0);

    await emit("agent-json-event", { session_id: "mock-session-e2e-001", data: call("codex-call-1") });
    await emit("agent-json-event", { session_id: "mock-session-e2e-001", data: call("codex-call-2") });
    await emit("agent-json-event", {
      session_id: "mock-session-e2e-001",
      data: {
        type: "response_item",
        payload: {
          type: "function_call_output",
          call_id: "codex-call-1",
          output: JSON.stringify({ accepted: true }),
        },
      },
    });
    await expect(page.getByTestId("provider-question-details")).toHaveCount(2);

    const screenshotPath = process.env.WARDIAN_ISSUE1367_SCREENSHOT
      ?? testInfo.outputPath("provider-question-inbox.png");
    await page.locator('[data-testid="surface-panel"][data-surface-type="inbox"]')
      .screenshot({ path: screenshotPath, animations: "disabled" });
    await testInfo.attach("issue1367-provider-question-inbox", {
      path: screenshotPath,
      contentType: "image/png",
    });
  });

  test("projects automation approval and completion events into Inbox", async ({ page }) => {
    await installQueueV2IpcMock(page);
    await page.goto("/", { waitUntil: "domcontentloaded" });
    await page.locator('[data-testid="app-shell"]').waitFor({ timeout: 15_000 });
    await openSurface(page, "inbox");

    await page.evaluate(() => {
      window.__WARDIAN_E2E_AUTOMATION_INBOX_UPDATE__?.({
        automation_id: "release-automation",
        run_instance_id: "run-42",
        automation_name: "Release approval",
        status: "awaiting_approval",
      });
    });
    await expect(page.getByText("Release approval", { exact: true })).toBeVisible();
    await expect(page.getByText("Approve the release automation?", { exact: true })).toBeVisible();
    await expect(page.getByRole("button", { name: "Approve", exact: true })).toBeVisible();

    if (process.env.WARDIAN_AUTOMATION_INBOX_SCREENSHOT) {
      await page
        .locator('[data-testid="surface-panel"][data-surface-type="inbox"]')
        .screenshot({ path: process.env.WARDIAN_AUTOMATION_INBOX_SCREENSHOT, animations: "disabled" });
    }

    await page.evaluate(() => {
      window.__WARDIAN_E2E_AUTOMATION_INBOX_UPDATE__?.({
        automation_id: "release-automation",
        run_instance_id: "run-42",
        automation_name: "Release approval",
        status: "completed",
        summary: "Release automation completed successfully.",
      });
    });
    await expect(page.getByText("Automation completed", { exact: true })).toBeVisible();
    await expect(page.getByText("Release automation completed successfully.", { exact: true })).toBeVisible();
    await expect(page.getByRole("button", { name: "Approve", exact: true })).toBeHidden();
  });

  test("loads older desktop Inbox history when the user scrolls to the end", async ({ page }) => {
    await page.addInitScript(() => {
      (window as Window & { __WARDIAN_E2E_QUEUE_BACKLOG__?: boolean }).__WARDIAN_E2E_QUEUE_BACKLOG__ = true;
    });
    await installQueueV2IpcMock(page);
    await page.goto("/", { waitUntil: "domcontentloaded" });
    await page.locator('[data-testid="app-shell"]').waitFor({ timeout: 15_000 });
    await openSurface(page, "inbox");

    await expect(page.getByText("Inbox history 0", { exact: true })).toBeVisible();
    await expect(page.getByText("Inbox history 100", { exact: true })).toBeHidden();

    const scrollRegion = page.getByTestId("inbox-scroll-region");
    await scrollRegion.evaluate((element) => {
      element.scrollTop = element.scrollHeight;
      element.dispatchEvent(new Event("scroll", { bubbles: true }));
    });

    await expect(page.getByText("Inbox history 119", { exact: true })).toBeVisible();
    await scrollRegion.evaluate((element) => {
      element.scrollTop = element.scrollHeight;
      element.dispatchEvent(new Event("scroll", { bubbles: true }));
    });

    if (process.env.WARDIAN_INBOX_LAZY_SCREENSHOT) {
      await page
        .locator('[data-testid="surface-panel"][data-surface-type="inbox"]')
        .screenshot({ path: process.env.WARDIAN_INBOX_LAZY_SCREENSHOT, animations: "disabled" });
    }
  });

  test("reconciles completed and failed automation runs that predate the Inbox listener", async ({ page }) => {
    await page.addInitScript(() => {
      (window as Window & { __WARDIAN_E2E_AUTOMATION_TERMINAL_RUNS__?: Array<Record<string, unknown>> })
        .__WARDIAN_E2E_AUTOMATION_TERMINAL_RUNS__ = [{
          automation_id: "completed-scheduled-automation",
          run_instance_id: "run-completed-before-inbox",
          automation_name: "Completed scheduled automation",
          status: "completed",
          summary: "The scheduled automation finished before Inbox opened.",
          updated_at: new Date().toISOString(),
        }, {
          automation_id: "missing-scheduled-automation",
          run_instance_id: "run-before-inbox",
          automation_name: "Missing scheduled automation",
          status: "failed",
          error: "The scheduled automation blueprint was removed.",
          updated_at: new Date().toISOString(),
        }];
    });
    await installQueueV2IpcMock(page);
    await page.goto("/", { waitUntil: "domcontentloaded" });
    await page.locator('[data-testid="app-shell"]').waitFor({ timeout: 15_000 });
    await openSurface(page, "inbox");

    await expect(page.getByText("Completed scheduled automation", { exact: true })).toBeVisible();
    await expect(page.getByText("The scheduled automation finished before Inbox opened.", { exact: true })).toBeVisible();
    await expect(page.getByText("Missing scheduled automation", { exact: true })).toBeVisible();
    await expect(page.getByText("The scheduled automation blueprint was removed.", { exact: true })).toBeVisible();

    if (process.env.WARDIAN_AUTOMATION_INBOX_RECONCILIATION_SCREENSHOT) {
      await page
        .locator('[data-testid="surface-panel"][data-surface-type="inbox"]')
        .screenshot({ path: process.env.WARDIAN_AUTOMATION_INBOX_RECONCILIATION_SCREENSHOT, animations: "disabled" });
    }
  });

  test("keeps automation read and clear actions stable across Inbox refreshes", async ({ page }) => {
    await page.addInitScript(() => {
      (window as Window & { __WARDIAN_E2E_AUTOMATION_TERMINAL_RUNS__?: Array<Record<string, unknown>> })
        .__WARDIAN_E2E_AUTOMATION_TERMINAL_RUNS__ = [{
          automation_id: "release-automation",
          run_instance_id: "run-triage",
          automation_name: "Release automation",
          status: "completed",
          summary: "The release automation completed successfully.",
          updated_at: new Date().toISOString(),
        }];
    });
    await installQueueV2IpcMock(page);
    await page.goto("/", { waitUntil: "domcontentloaded" });
    await page.locator('[data-testid="app-shell"]').waitFor({ timeout: 15_000 });
    await openSurface(page, "inbox");

    const automationCard = page.locator(".group").filter({ hasText: "Release automation" }).first();
    await expect(automationCard).toBeVisible();
    await automationCard.click();
    await expect(page.getByRole("button", { name: /clear read/i })).toBeEnabled();

    if (process.env.WARDIAN_AUTOMATION_TRIAGE_SCREENSHOT) {
      await page
        .locator('[data-testid="surface-panel"][data-surface-type="inbox"]')
        .screenshot({ path: process.env.WARDIAN_AUTOMATION_TRIAGE_SCREENSHOT, animations: "disabled" });
    }

    await page.getByRole("button", { name: /clear read/i }).click();
    await expect(page.getByText("Release automation", { exact: true })).toBeHidden();

    await page.waitForTimeout(5_500);
    await expect(page.getByText("Release automation", { exact: true })).toBeHidden();
  });
});
