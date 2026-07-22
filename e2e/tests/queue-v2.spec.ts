import { expect, test, type Page } from "@playwright/test";
import { openSurface } from "../fixtures/workbench";
import { makeWorkbenchDocument } from "../fixtures/workbenchIpcMock";

async function installQueueV2IpcMock(page: Page) {
  const workbenchDocument = makeWorkbenchDocument();
  await page.addInitScript((workbenchDocument) => {
    type QueueItem = {
      id: string;
      type: "action_needed" | "agent_completed" | "workflow_completed";
      timestamp: number;
      read: boolean;
      agent_session_id?: string;
      agent_name?: string;
      workflow_name?: string;
      status?: "completed" | "failed";
      summary?: string;
      error?: string;
    };

    const now = Date.now();
    let queueItems: QueueItem[] = [
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
        id: "workflow-failed-1",
        type: "workflow_completed",
        timestamp: now - 180_000,
        read: false,
        workflow_name: "Release Drill",
        status: "failed",
        error: "Verifier returned a non-zero exit code.",
      },
    ];
    let queuePreferences = {};
    const submittedPrompts: Array<{ sessionId: string; prompt: string }> = [];
    let callbackId = 1;
    const callbacks = new Map<number, unknown>();
    const tauriWindow = window as Window & {
      __TAURI_INTERNALS__?: Record<string, unknown>;
      __TAURI_EVENT_PLUGIN_INTERNALS__?: Record<string, unknown>;
      __WARDIAN_E2E_SUBMITTED_PROMPTS__?: Array<{ sessionId: string; prompt: string }>;
    };

    tauriWindow.__WARDIAN_E2E_SUBMITTED_PROMPTS__ = submittedPrompts;
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
          return [{
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
          }];
        }
        if (command === "list_workflow_inbox_approvals") return [];
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
        if (command === "load_onboarding_hints") return { dismissed_hint_ids: ["spawn-agent-first-run:v1"] };
        if (command === "dismiss_onboarding_hint") return { dismissed_hint_ids: ["spawn-agent-first-run:v1"] };
        if (command === "list_workflows") return [];
        if (command === "list_scheduled_runs") return [];
        if (command === "load_workflow_library") return { folders: [], rootWorkflowIds: [] };
        if (command === "get_library_tree") return { type: "Folder", path: "", name: "Root", children: [] };
        if (command === "list_deployed_skills") return [];
        if (command === "plugin:event|listen") return callbackId++;
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

    await expect(page.getByText("Action needed", { exact: true })).toBeVisible();
    await expect(page.getByText("Production deployment", { exact: true })).toBeVisible();
    await expect(page.getByText("Migration update", { exact: true })).toBeVisible();
    await expect(page.getByText("Approve the generated patch before continuing.")).toBeVisible();
    await expect(page.getByRole("button", { name: "Filter Inbox events" })).toContainText("Filter: All events");
    await expect(page.getByLabel("Desktop alert for action needed")).toBeHidden();
    await expect(page.getByLabel("Sound alert for action needed")).toBeHidden();
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
});
