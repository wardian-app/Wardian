import fs from "node:fs";
import path from "node:path";
import { expect, test } from "@playwright/test";
import type { AgentChatEvent, AgentChatPage } from "../../src/types";

// Browser presentation evidence for normalized Codex records, not provider execution.
test("Codex custom exec calls retain their input details beside output", async ({ page }) => {
  const agent = { session_id: "exec-fixture", session_name: "Codex Work", agent_class: "Coder",
    provider: "codex", workspace: "<agent-workspace>", status: "Off", latest_text: null };
  const input = 'const result = await tools.exec_command({ cmd: "cat audit.txt" }); text(result);';
  const call: AgentChatEvent = {
    id: "exec-call", session_id: agent.session_id, provider: "codex", kind: "tool_call", role: null,
    text: null, title: "exec", status: "running", turn_id: "turn-1", source: "provider_log", command: null,
    exit_code: null, path: null, language: null, created_at: "2026-01-01T00:00:00Z", sequence: 1,
    metadata: { raw_type: "custom_tool_call", tool_name: "exec", tool_input_text: input,
      call_id: "exec-1", chat_detail_ref: "exec-input:0" },
  };
  const result: AgentChatEvent = { ...call, id: "exec-output", kind: "tool_result", title: "Tool result",
    text: "audit.txt contains the expected checkpoint.", status: "succeeded", sequence: 2,
    metadata: { raw_type: "custom_tool_call_output", call_id: "exec-1" } };
  const initial: AgentChatPage = {
    session_id: agent.session_id, conversation_id: "exec-conversation", generation: "exec-generation",
    source_epoch: null, revision: "exec-revision", unchanged: false,
    reset: false, progress: "ready", aliases: [], removed_ids: [], detail: null,
    bytes_read: 0, records_decoded: 0, events: [call, result], next_before: null,
  };
  await page.addInitScript(() => localStorage.setItem("wardian.remote.agentDefaultViewMode", "chat"));
  await page.route("**/remote/api/**", async (route) => {
    const url = new URL(route.request().url());
    if (url.pathname.endsWith("/chat")) {
      const response = url.searchParams.has("detail")
        ? { ...initial, events: [], detail: { event_id: call.id, text: input, next: null, complete: true } }
        : initial;
      await route.fulfill({ json: response });
      return;
    }
    const responses: Record<string, unknown> = {
      "/remote/api/session": { csrf_nonce: "fixture-csrf", expires_at: "2099-01-01T00:00:00Z", absolute_expires_at: "2099-01-01T00:00:00Z" },
      "/remote/api/agents": { agents: [agent] },
      "/remote/api/watchlists": { watchlists: [], teams: [], prefs: null },
      "/remote/api/queue": { items: [] }, "/remote/api/automations": { automations: [] },
      "/remote/api/ws-ticket": { ticket: "fixture-ticket", expires_at: "2099-01-01T00:00:00Z" },
    };
    await route.fulfill({ json: responses[url.pathname] ?? {} });
  });
  await page.routeWebSocket("**/remote/api/status-stream", (socket) => {
    socket.onMessage(() => socket.send(JSON.stringify({ type: "agent_status", agents: [agent] })));
  });
  await page.goto("/remote", { waitUntil: "domcontentloaded" });
  await page.getByRole("button", { name: "Open Codex Work details" }).click();
  await page.getByTestId("chat-turn-work-summary").getByRole("button").click();
  await expect(page.getByText("exec", { exact: true })).toBeVisible();
  await page.getByRole("button", { name: "Show output", exact: true }).click();
  await expect(page.getByText(result.text!, { exact: true })).toBeVisible();
  await page.getByRole("button", { name: "Show full details", exact: true }).click();
  await expect(page.locator("pre").filter({ hasText: input })).toBeVisible();
  const screenshotDir = process.env.WARDIAN_EXEC_SCREENSHOT_DIR;
  if (screenshotDir) {
    fs.mkdirSync(screenshotDir, { recursive: true });
    await page.screenshot({ path: path.join(screenshotDir, "codex-exec-input-and-output.png"), animations: "disabled" });
  }
});
