import fs from "node:fs";
import path from "node:path";
import { expect, test } from "@playwright/test";
import type { AgentChatEvent, AgentChatPage } from "../../src/types";

// Sanitized Claude event shapes from the reported conversation. Browser routes
// exercise the shared reader; copied-log acquisition is verified in Rust.
const agent = {
  session_id: "render-fixture", session_name: "Rendering Fixture", agent_class: "Coder",
  provider: "claude", workspace: "<agent-workspace>", status: "Idle", latest_text: null,
};
const prompt = "Check the sections against last year.";
const event = (id: string, values: Partial<AgentChatEvent>): AgentChatEvent => ({
  id, session_id: agent.session_id, provider: "claude", kind: "message", role: "assistant",
  text: null, title: null, status: null, turn_id: null, source: "stream_json", command: null,
  exit_code: null, path: null, language: null, created_at: null, sequence: null, metadata: {}, ...values,
});
const work = Array.from({ length: 4 }, (_, index) => [
  event(`call-${index}`, { kind: "tool_call", role: "tool", title: "mcp__browser__get_tabs",
    turn_id: `tool-${index}`, sequence: 3 + index * 2,
    metadata: { tool_name: "mcp__browser__get_tabs", tool_input: { description: "Inspecting browser tabs" },
      chat_detail_ref: `call-${index}:0`, chat_body_binding: `call-body-${index}` } }),
  event(`result-${index}`, { kind: "tool_result", role: "tool", status: "succeeded",
    turn_id: `tool-${index}`, text: '{"availableTabs":[{"tabId":"example","title":"Reference table"}]}',
    sequence: 4 + index * 2, metadata: { raw_type: "tool_result", chat_detail_ref: `result-${index}:0`,
      chat_body_binding: `result-body-${index}` } }),
]).flat();

for (const viewport of [{ name: "desktop", width: 1280, height: 900 }, { name: "mobile", width: 390, height: 844 }]) {
  test(`${viewport.name} Chat owns work details and renders each projected prompt once`, async ({ page }) => {
    await page.setViewportSize(viewport);
    await page.addInitScript(() => localStorage.setItem("wardian.remote.agentDefaultViewMode", "chat"));
    let sends = 0;
    const details: string[] = [];
    const dto = (): AgentChatPage => ({
      session_id: agent.session_id, conversation_id: "conversation", generation: "generation", source_epoch: null,
      revision: `revision-${sends}`, unchanged: false, reset: false, progress: "ready", aliases: [], removed_ids: [],
      next_before: null, detail: null, bytes_read: 0, records_decoded: 0,
      events: [event("old-prompt", { role: "user", text: prompt, sequence: 1 }),
        event("before-work", { text: "I will compare the reference table.", sequence: 2 }), ...work,
        event("after-work", { text: "The comparison is complete.", sequence: 11 }),
        ...Array.from({ length: sends }, (_, index) => event(`sent-prompt-${index}`, {
          role: "user", text: prompt, sequence: 12 + index,
        })),
      ],
    });
    await page.route("**/remote/api/**", async (route) => {
      const url = new URL(route.request().url());
      if (url.pathname.endsWith("/chat")) {
        const reference = url.searchParams.get("detail");
        if (reference) {
          details.push(reference);
          await route.fulfill({ json: { ...dto(), events: [], detail: {
            event_id: reference.split(":")[0], text: "Full browser tool evidence", complete: true, next: null,
          } } });
        } else await route.fulfill({ json: dto() });
        return;
      }
      if (url.pathname.endsWith("/agents/action")) {
        expect(route.request().postDataJSON()).toMatchObject({ action: "send_prompt", target: agent.session_id, prompt });
        sends += 1;
        await route.fulfill({ json: { ok: true } }); // Accepted delivery without a generated-row receipt.
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
    await page.getByRole("button", { name: "Open Rendering Fixture details" }).click();
    await expect(page.getByText(prompt, { exact: true })).toHaveCount(1);
    const group = page.getByTestId("chat-work-group");
    const screenshotDir = process.env.WARDIAN_CHAT_RENDER_SCREENSHOT_DIR;
    const capture = async (state: string) => {
      if (!screenshotDir) return;
      fs.mkdirSync(screenshotDir, { recursive: true });
      await page.screenshot({ path: path.join(screenshotDir, `${viewport.name}-${state}.png`), animations: "disabled" });
    };
    await capture("collapsed");
    await expect(page.getByRole("button", { name: "Show full details" })).toHaveCount(0);
    await expect(group).not.toContainText("availableTabs");
    expect(details).toEqual([]);
    await group.getByRole("button", { name: "Show all", exact: true }).click();
    const detailButton = group.getByRole("button", { name: "Show full details" }).first();
    await detailButton.click();
    await expect(group.getByText("Full browser tool evidence")).toBeVisible();
    await capture("details");
    expect(details).toHaveLength(1);
    await group.getByRole("button", { name: "Collapse", exact: true }).click();
    await expect(page.getByText("Full browser tool evidence")).toBeHidden();
    await group.getByRole("button", { name: "Show all", exact: true }).click();
    await expect(group.getByText("Full browser tool evidence")).toBeVisible();
    expect(details).toHaveLength(1);
    await group.getByRole("button", { name: "Collapse", exact: true }).click();
    const before = await page.getByText("I will compare the reference table.", { exact: true }).boundingBox();
    const after = await page.getByText("The comparison is complete.", { exact: true }).boundingBox();
    expect(before?.x).toBe(after?.x);
    await page.getByRole("textbox").fill(prompt);
    await page.getByRole("button", { name: "Send prompt", exact: true }).click();
    await expect.poll(() => sends).toBe(1);
    await expect(page.getByText(prompt, { exact: true })).toHaveCount(2);
    await capture("sent-once");
  });
}
