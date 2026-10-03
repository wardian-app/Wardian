import fs from "node:fs";
import path from "node:path";
import { expect, test } from "@playwright/test";

// Browser evidence for the remote read UI; does not qualify a native/device read.
test("mobile Chat retains rows through a local error and retries without re-pairing", async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.addInitScript(() => localStorage.setItem("wardian.remote.agentDefaultViewMode", "chat"));
  const agent = {
    session_id: "chat-fixture", session_name: "Chat Fixture", agent_class: "Coder",
    provider: "codex", workspace: "<agent-workspace>", status: "Idle", latest_text: null,
  };
  let reads = 0;
  await page.route("**/remote/api/**", async (route) => {
    const endpoint = new URL(route.request().url()).pathname;
    if (endpoint.endsWith("/chat")) {
      reads += 1;
      if (reads === 2) {
        await route.fulfill({ status: 400, json: { code: "agent_chat_provenance_failed", detail: "Private diagnostic must stay hidden" } });
        return;
      }
      await route.fulfill({ json: {
        events: [{
          id: reads === 1 ? "earlier" : "recovered", session_id: agent.session_id, provider: "codex",
          kind: "message", role: "assistant", text: reads === 1 ? "Earlier reply remains available." : "History loaded after retry.",
          title: null, status: null, turn_id: "fixture-turn", source: "provider_log", command: null,
          exit_code: null, path: null, language: null, created_at: "2026-01-01T00:00:00Z", sequence: reads, metadata: {},
        }], has_older: false, next_before: null,
      } });
      return;
    }
    const responses: Record<string, unknown> = {
      "/remote/api/session": { csrf_nonce: "fixture-csrf", expires_at: "2099-01-01T00:00:00Z", absolute_expires_at: "2099-01-01T00:00:00Z" },
      "/remote/api/agents": { agents: [agent] },
      "/remote/api/watchlists": { watchlists: [], teams: [], prefs: null },
      "/remote/api/queue": { items: [] },
      "/remote/api/automations": { automations: [] },
      "/remote/api/ws-ticket": { ticket: "fixture-ticket", expires_at: "2099-01-01T00:00:00Z" },
    };
    await route.fulfill({ json: responses[endpoint] ?? {} });
  });
  await page.routeWebSocket("**/remote/api/status-stream", (socket) => {
    socket.onMessage(() => socket.send(JSON.stringify({ type: "agent_status", agents: [agent] })));
  });
  await page.goto("/remote", { waitUntil: "domcontentloaded" });
  await page.getByRole("button", { name: "Open Chat Fixture details" }).click();
  await expect(page.getByText("Earlier reply remains available.", { exact: true })).toBeVisible();
  await page.getByRole("button", { name: "Refresh chat", exact: true }).click();
  await expect(page.getByRole("alert")).toContainText("agent_chat_provenance_failed");
  await expect(page.getByText("Earlier reply remains available.", { exact: true })).toBeVisible();
  await expect(page.getByText("No chat transcript yet.")).toHaveCount(0);
  await expect(page.getByText("Private diagnostic must stay hidden")).toHaveCount(0);
  const screenshotDir = process.env.WARDIAN_CHAT_RECOVERY_SCREENSHOT_DIR;
  if (screenshotDir) {
    fs.mkdirSync(screenshotDir, { recursive: true });
    await page.screenshot({ path: path.join(screenshotDir, "chat-local-error.png"), animations: "disabled" });
  }
  await page.getByRole("button", { name: "Retry Chat", exact: true }).click();
  await expect(page.getByRole("alert")).toHaveCount(0);
  await expect(page.getByText("History loaded after retry.", { exact: true })).toBeVisible();
  expect(reads).toBe(3);
});
