import fs from "node:fs";
import path from "node:path";
import { expect, test } from "@playwright/test";
import type { AgentChatEvent, AgentChatPage } from "../../src/types";

// The Rust committed-page regression proves the dual-source reconciliation.
// These pages prove that the shared browser reader replaces an already shown
// live row when that backend alias arrives.
const agent = {
  session_id: "narration-fixture", session_name: "Claude Narration", agent_class: "Coder",
  provider: "claude", workspace: "<agent-workspace>", status: "Idle", latest_text: null,
};
const narration = "I'll start by reading `audit.txt`.";
const liveId = "narration-fixture:0000000000000003:stream_json";
const nativeId = "source:narration-fixture:epoch:100:record:0";
const event = (id: string, values: Partial<AgentChatEvent>): AgentChatEvent => ({
  id, session_id: agent.session_id, provider: "claude", kind: "message", role: "assistant",
  text: null, title: null, status: null, turn_id: null, source: "stream_json", command: null,
  exit_code: null, path: null, language: null, created_at: null, sequence: null, metadata: {}, ...values,
});

for (const viewport of [{ name: "desktop", width: 1280, height: 900 }, { name: "mobile", width: 390, height: 844 }]) {
  test(`${viewport.name} Claude narration keeps one row when its provider log arrives`, async ({ page }) => {
    await page.setViewportSize(viewport);
    await page.addInitScript(() => localStorage.setItem("wardian.remote.agentDefaultViewMode", "chat"));
    let captured = false;
    const dto = (): AgentChatPage => ({
      session_id: agent.session_id, conversation_id: "conversation", generation: "generation", source_epoch: "epoch",
      revision: captured ? "captured" : "live", unchanged: false, reset: false, progress: "ready",
      aliases: captured ? [{ observation_id: liveId, canonical_id: nativeId }] : [], removed_ids: [],
      next_before: null, detail: null, bytes_read: 0, records_decoded: 0,
      events: [event("prompt", { role: "user", text: "Read audit.txt and report the result.", sequence: 1 }),
        event(captured ? nativeId : liveId, { text: narration, turn_id: "msg-narration", sequence: 2,
          metadata: captured ? { provider_log: true, chat_source_ref: nativeId, chat_source_epoch: "epoch",
            chat_display_member_ids: [nativeId, liveId] } : {} }),
        ...(captured ? [
          event("read-call", { kind: "tool_call", role: "tool", title: "Read", turn_id: "tool-read", sequence: 3,
            metadata: { tool_name: "Read", tool_input: { file_path: "audit.txt" } } }),
          event("read-result", { kind: "tool_result", role: "tool", text: "Audit complete.", turn_id: "tool-read", sequence: 4, status: "succeeded" }),
          event("answer", { text: "The file reports that the audit is complete.", turn_id: "msg-answer", sequence: 5 }),
        ] : []),
      ],
    });
    await page.route("**/remote/api/**", async (route) => {
      const url = new URL(route.request().url());
      if (url.pathname.endsWith("/chat")) {
        const ids = url.searchParams.get("revalidate_ids");
        const reply = dto();
        if (ids) {
          const selected = JSON.parse(ids) as string[];
          reply.revalidated_ids = selected;
          reply.events = reply.events.filter((row) => selected.includes(row.id)
            || (row.id === nativeId && selected.includes(liveId)));
        }
        await route.fulfill({ json: reply });
        return;
      }
      const responses: Record<string, unknown> = {
        "/remote/api/session": { csrf_nonce: "fixture-csrf", expires_at: "2099-01-01T00:00:00Z", absolute_expires_at: "2099-01-01T00:00:00Z" },
        "/remote/api/agents": { agents: [agent] }, "/remote/api/watchlists": { watchlists: [], teams: [], prefs: null },
        "/remote/api/queue": { items: [] }, "/remote/api/automations": { automations: [] },
        "/remote/api/ws-ticket": { ticket: "fixture-ticket", expires_at: "2099-01-01T00:00:00Z" },
      };
      await route.fulfill({ json: responses[url.pathname] ?? {} });
    });
    await page.routeWebSocket("**/remote/api/status-stream", (socket) => {
      socket.onMessage(() => socket.send(JSON.stringify({ type: "agent_status", agents: [agent] })));
    });
    await page.goto("/remote", { waitUntil: "domcontentloaded" });
    await page.getByRole("button", { name: "Open Claude Narration details" }).click();
    const paragraph = page.getByText("I'll start by reading", { exact: false });
    await expect(paragraph).toHaveCount(1);
    captured = true;
    await expect(page.getByText("The file reports that the audit is complete.", { exact: true })).toBeVisible();
    // Once captured, the narration belongs to the turn's collapsed work.
    await page.getByTestId("chat-turn-work-summary").getByRole("button").click();
    await expect(paragraph).toHaveCount(1);
    await page.getByRole("button", { name: "Show output", exact: true }).click();
    await expect(page.getByText("Audit complete.", { exact: true })).toBeVisible();
    await expect(paragraph).toHaveCount(1);
    const screenshotDir = process.env.WARDIAN_CLAUDE_NARRATION_SCREENSHOT_DIR;
    if (screenshotDir) {
      fs.mkdirSync(screenshotDir, { recursive: true });
      await page.screenshot({ path: path.join(screenshotDir, `${viewport.name}-one-narration.png`), animations: "disabled" });
    }
  });
}
