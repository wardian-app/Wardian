import fs from "node:fs";
import path from "node:path";
import { expect, test } from "@playwright/test";
import type { AgentChatEvent, AgentChatPage } from "../../src/types";

for (const viewport of [{ width: 390, height: 844 }, { width: 768, height: 1024 }]) {
  test(`older Chat application failures stay local at ${viewport.width}`, async ({ page }) => {
    await page.setViewportSize(viewport);
    await page.addInitScript(() => localStorage.setItem("wardian.remote.agentDefaultViewMode", "chat"));
    const agent = { session_id: "older-chat", session_name: "Older Chat", agent_class: "Coder",
      provider: "codex", workspace: "<agent-workspace>", status: "Off", latest_text: null };
    const event = (index: number): AgentChatEvent => ({
      id: `message-${index}`, session_id: agent.session_id, provider: "codex", kind: "message",
      role: "assistant", text: `Saved reply ${index}`, title: null, status: null, turn_id: `turn-${index}`,
      source: "provider_log", command: null, exit_code: null, path: null, language: null,
      created_at: null, sequence: index, metadata: {},
    });
    let olderReads = 0;
    let failTransport = false;
    const errors: string[] = [];
    page.on("pageerror", (error) => errors.push(error.message));
    await page.route("**/remote/api/**", async (route) => {
      const url = new URL(route.request().url());
      if (url.pathname.endsWith("/chat")) {
        if (failTransport) { await route.abort("failed"); return; }
        const older = url.searchParams.has("before");
        if (older) { olderReads += 1; expect(url.searchParams.get("before")).toBe("older-cursor"); }
        const rejected = older && olderReads === 1;
        const dto: AgentChatPage = {
          session_id: agent.session_id, conversation_id: rejected ? null : "conversation",
          generation: "generation", source_epoch: null, revision: "revision", progress: older ? "provisional" : "ready",
          reset: false, unchanged: false, aliases: [], removed_ids: [], detail: null,
          bytes_read: 0, records_decoded: 0,
          events: older ? Array.from({ length: 32 }, (_, i) => event(i + 1)) : [event(33)],
          next_before: older ? null : "older-cursor",
        };
        await route.fulfill({ status: 200, json: dto }); return;
      }
      const responses: Record<string, unknown> = {
        "/remote/api/session": { csrf_nonce: "csrf", expires_at: "2099-01-01T00:00:00Z", absolute_expires_at: "2099-01-01T00:00:00Z" },
        "/remote/api/agents": { agents: [agent] },
        "/remote/api/watchlists": { watchlists: [], teams: [], prefs: null },
        "/remote/api/queue": { items: [] }, "/remote/api/automations": { automations: [] },
        "/remote/api/ws-ticket": { ticket: "ticket", expires_at: "2099-01-01T00:00:00Z" },
      };
      await route.fulfill({ json: responses[url.pathname] ?? {} });
    });
    await page.routeWebSocket("**/remote/api/status-stream", (socket) => {
      socket.onMessage(() => socket.send(JSON.stringify({ type: "agent_status", agents: [agent] })));
    });
    await page.goto("/remote", { waitUntil: "domcontentloaded" });
    await page.getByRole("button", { name: "Open Older Chat details" }).click();
    await expect(page.getByText("Saved reply 33", { exact: true })).toBeVisible();
    await page.getByRole("button", { name: "Load older transcript" }).click();
    await expect(page.getByRole("alert")).toContainText("Chat history could not be updated. Retry.");
    await expect(page.getByText("Saved reply 33", { exact: true })).toBeVisible();
    await expect(page.getByText(/Desktop unreachable/)).toHaveCount(0);
    const screenshotDir = process.env.WARDIAN_OLDER_CHAT_SCREENSHOT_DIR;
    if (screenshotDir) {
      fs.mkdirSync(screenshotDir, { recursive: true });
      await page.screenshot({ path: path.join(screenshotDir, `${viewport.width}-local-error.png`), animations: "disabled" });
    }
    await page.getByRole("button", { name: "Load older transcript" }).click();
    await expect(page.getByText("Saved reply 1", { exact: true })).toBeVisible();
    await expect(page.getByText("Saved reply 33", { exact: true })).toHaveCount(1);
    await expect(page.getByRole("alert")).toHaveCount(0);
    expect(olderReads).toBe(2);
    if (screenshotDir) {
      await page.screenshot({ path: path.join(screenshotDir, `${viewport.width}-older-loaded.png`), animations: "disabled" });
    }
    expect(errors).toEqual([]);
    failTransport = true;
    await page.getByRole("button", { name: "Refresh chat", exact: true }).click();
    await expect(page.getByText(/Desktop unreachable/)).toBeVisible();
  });
}
