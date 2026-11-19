import fs from "node:fs";
import path from "node:path";
import { expect, test } from "@playwright/test";
import type { AgentChatPage } from "../../src/types";
import captures from "../../src/features/chat/__fixtures__/chat-first-turn-reset.json" with { type: "json" };
import { installWorkbenchIpcMock, makeWorkbenchDocument, makeWorkbenchSurface } from "../fixtures/workbenchIpcMock";

// Real desktop UI and shared hook; replayed native envelopes do not exercise
// provider spawning or filesystem capture, which produced the original audit.
for (const provider of ["claude", "codex"] as const) {
  test(`${provider} first turn appears in Chat opened before the source exists`, async ({ page }) => {
    const { pending, completed } = captures[provider] as { pending: AgentChatPage; completed: AgentChatPage };
    const document = makeWorkbenchDocument({ surfaces: [makeWorkbenchSurface("overview", "agents-overview")] });
    await page.addInitScript(() => localStorage.setItem("wardian-settings", JSON.stringify({
      state: { gridCardDisplayMode: "chat" }, version: 2,
    })));
    const mock = await installWorkbenchIpcMock(page, {
      agents: [{ session_id: pending.session_id, session_name: `${provider} First Turn`, agent_class: "Coder",
        provider, folder: "/workspace/sample", is_off: false }],
      responses: { load_agent_chat_page: pending },
      load_result: { source: "primary", document, notice: null, durable_revision: document.revision, durable_token: "first-turn-state" },
    });
    await page.goto("/", { waitUntil: "domcontentloaded" });
    const chat = page.locator(".agent-chat-view");
    await expect(chat).toBeVisible();
    await expect.poll(async () => (await mock.calls("load_agent_chat_page")).length).toBeGreaterThan(0);
    const mountedChat = await chat.elementHandle();
    const answer = page.getByText(`First turn completed for ${provider}.`, { exact: true });
    await expect(answer).toHaveCount(0);
    await page.evaluate((completed) => {
      const runtime = window as unknown as {
        __TAURI_INTERNALS__: { invoke: (command: string, args?: Record<string, unknown>) => Promise<unknown> };
        firstTurnRevisions: unknown[];
      };
      const invoke = runtime.__TAURI_INTERNALS__.invoke;
      runtime.firstTurnRevisions = [];
      runtime.__TAURI_INTERNALS__.invoke = (command, args) => {
        if (command !== "load_agent_chat_page") return invoke(command, args);
        runtime.firstTurnRevisions.push(args?.revision);
        return Promise.resolve(completed);
      };
    }, completed);
    await expect(answer).toBeVisible({ timeout: 15_000 });
    await expect(answer).toHaveCount(1);
    await expect(page.getByText("Unable to load transcript", { exact: true })).toHaveCount(0);
    const revisions = () => page.evaluate(() => (window as unknown as { firstTurnRevisions: unknown[] }).firstTurnRevisions);
    expect(await revisions()).toContain(pending.revision);
    await expect.poll(revisions).toContain(completed.revision);
    expect(await mountedChat!.evaluate((element) => element.isConnected)).toBe(true);
    const screenshotDir = process.env.WARDIAN_FIRST_TURN_SCREENSHOT_DIR;
    if (screenshotDir) {
      fs.mkdirSync(screenshotDir, { recursive: true });
      await chat.screenshot({ path: path.join(screenshotDir, `${provider}-first-turn.png`), animations: "disabled" });
    }
  });
}
