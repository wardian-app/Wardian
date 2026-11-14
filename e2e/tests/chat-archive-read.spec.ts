import fs from "node:fs";
import path from "node:path";
import { expect, test } from "@playwright/test";
import type { AgentChatEvent, AgentChatPage } from "../../src/types";

// Exercises the real remote UI and shared archive classifier with synthetic HTTP
// pages. Native import/ownership and body durability are backend regression tests.
test("stopped capture automatically opens a complete archive with Older and owned detail", async ({ page }) => {
  const agent = {
    session_id: "archive-fixture", session_name: "Saved Conversation", agent_class: "Coder",
    provider: "codex", workspace: "<agent-workspace>", status: "Off", latest_text: null,
  };
  const row = (id: string, text: string, sequence: number): AgentChatEvent => ({
    id, session_id: agent.session_id, provider: "codex", kind: "message", role: "assistant",
    text, title: null, status: null, turn_id: `turn-${sequence}`, source: "provider_log",
    command: null, exit_code: null, path: null, language: null,
    created_at: "2026-01-01T00:00:00Z", sequence, metadata: {},
  });
  const answer = row("archive-display:answer", "The authorized interval is saved in conversation history.", 10);
  answer.metadata = {
    chat_display_member_ids: ["source:historical-public", "source:historical-completion"],
    chat_detail_ref: "owned-answer:0", chat_body_binding: "owned-answer-body",
  };
  const initial: AgentChatPage = {
    session_id: agent.session_id, conversation_id: "saved-conversation", generation: "saved-generation",
    source_epoch: null, archive_read_ready: true, revision: "saved-revision", unchanged: false,
    reset: false, progress: "capture_source_replaced", aliases: [
      { observation_id: "source:historical-public", canonical_id: answer.id },
      { observation_id: "source:historical-completion", canonical_id: answer.id },
    ], removed_ids: [], detail: null, bytes_read: 0, records_decoded: 0,
    events: [row("archive:prompt", "Continue from the saved checkpoint.", 9), answer],
    next_before: "owned-older-frontier",
  };
  initial.events[0].role = "user";
  const traffic: URL[] = [];
  await page.addInitScript(() => localStorage.setItem("wardian.remote.agentDefaultViewMode", "chat"));
  await page.route("**/remote/api/**", async (route) => {
    const url = new URL(route.request().url());
    if (url.pathname.endsWith("/chat")) {
      traffic.push(url);
      expect(url.searchParams.has("revalidate_ids")).toBe(false);
      expect(route.request().postData()).toBeNull();
      let response: AgentChatPage;
      if (traffic.length === 1) {
        response = { ...initial, conversation_id: null, generation: null, archive_read_ready: false,
          revision: "pending", events: [], aliases: [], next_before: null };
      } else if (url.searchParams.has("detail")) {
        expect(url.searchParams.get("detail")).toBe("owned-answer:0");
        response = { ...initial, events: [], aliases: [], next_before: null,
          detail: { event_id: answer.id, text: "Owned archive body. No native source read is needed.", next: null, complete: true } };
      } else if (url.searchParams.has("before")) {
        expect(url.searchParams.get("before")).toBe("owned-older-frontier");
        response = { ...initial, events: [row("archive:older", "Earlier authorized archive entry.", 1)],
          aliases: [], next_before: null, older_page: true };
      } else if (url.searchParams.get("revision") === initial.revision) {
        response = { ...initial, events: [], aliases: [], unchanged: true };
      } else {
        response = initial;
      }
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
  await page.getByRole("button", { name: "Open Saved Conversation details" }).click();
  await expect(page.getByText(answer.text!, { exact: true })).toBeVisible({ timeout: 15_000 });
  await expect(page.getByText(/History capture is paused because the provider log was replaced/)).toBeVisible();
  await expect(page.getByText(answer.text!, { exact: true })).toHaveCount(1);
  expect(traffic.some((url) => url.searchParams.has("detail"))).toBe(false);
  await page.getByRole("button", { name: "Show full details", exact: true }).click();
  await expect(page.getByText("Owned archive body. No native source read is needed.", { exact: true })).toBeVisible();
  await page.locator(".chat-transcript-scroll").evaluate((element) => {
    element.scrollTop = 0;
    element.dispatchEvent(new Event("scroll"));
  });
  await expect(page.getByText("Earlier authorized archive entry.", { exact: true })).toBeVisible();
  await expect(page.getByRole("alert")).toHaveCount(0);
  expect(traffic.filter((url) => url.searchParams.has("before"))).toHaveLength(1);
  expect(traffic.filter((url) => url.searchParams.has("detail"))).toHaveLength(1);
  const screenshotDir = process.env.WARDIAN_ARCHIVE_READ_SCREENSHOT_DIR;
  if (screenshotDir) {
    fs.mkdirSync(screenshotDir, { recursive: true });
    await page.screenshot({ path: path.join(screenshotDir, "archive-readable-capture-stopped.png"), animations: "disabled" });
  }
});
