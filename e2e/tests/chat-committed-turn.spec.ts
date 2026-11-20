import fs from "node:fs";
import path from "node:path";
import { expect, test } from "@playwright/test";
import { committedChatFixture } from "../../src/test/committedChatFixture";
import { installWorkbenchIpcMock, makeWorkbenchDocument, makeWorkbenchSurface } from "../fixtures/workbenchIpcMock";

// Synthetic fixtures only: actual desktop and Remote components, not native/provider acquisition.
for (const surface of ["desktop", "mobile"] as const) {
  test(`${surface} committed turn keeps output inside its step`, async ({ page }) => {
    const fixture = committedChatFixture();
    const detailRequests: string[] = [];
    await page.setViewportSize(surface === "desktop" ? { width: 1920, height: 1080 } : { width: 390, height: 844 });
    if (surface === "desktop") {
      await page.addInitScript(() => localStorage.setItem("wardian-settings", JSON.stringify({ state: { gridCardDisplayMode: "chat" }, version: 2 })));
      const document = makeWorkbenchDocument({ surfaces: [makeWorkbenchSurface("overview", "agents-overview")],
        shell: { left_sidebar_collapsed: true, right_sidebar_collapsed: true } });
      await installWorkbenchIpcMock(page, { agents: [{ session_id: fixture.page.session_id, session_name: "Synthetic configuration check",
        agent_class: "Coder", provider: "codex", folder: "/workspace/sample", is_off: false }], responses: { load_agent_chat_page: fixture.page },
        load_result: { source: "primary", document, notice: null, durable_revision: document.revision, durable_token: "synthetic-state" } });
      await page.goto("/", { waitUntil: "domcontentloaded" });
      await page.evaluate((fixture) => {
        const runtime = window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string, args?: Record<string, unknown>) => Promise<unknown> }; stepRequests: string[] };
        const original = runtime.__TAURI_INTERNALS__.invoke;
        runtime.stepRequests = [];
        runtime.__TAURI_INTERNALS__.invoke = (command, args) => {
          if (command !== "load_agent_chat_page" || !args?.detailRef) return original(command, args);
          runtime.stepRequests.push(String(args.detailRef));
          return Promise.resolve({ ...fixture.page, events: [], presentation: { version: 1, payload: { ...fixture.presentation, detail: fixture.detail } } });
        };
      }, fixture);
    } else {
      const agent = { session_id: fixture.page.session_id, session_name: "Synthetic configuration check", agent_class: "Coder",
        provider: "codex", workspace: "Sample", status: "Idle", latest_text: null };
      await page.addInitScript(() => localStorage.setItem("wardian.remote.agentDefaultViewMode", "chat"));
      await page.route("**/remote/api/**", async (route) => {
        const url = new URL(route.request().url());
        if (url.pathname.endsWith("/chat")) {
          const ref = url.searchParams.get("detail");
          if (ref) detailRequests.push(ref);
          await route.fulfill({ json: ref ? { ...fixture.page, events: [], presentation: { version: 1, payload: { ...fixture.presentation, detail: fixture.detail } } } : fixture.page });
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
      await page.routeWebSocket("**/remote/api/status-stream", (socket) => socket.onMessage(() => socket.send(JSON.stringify({ type: "agent_status", agents: [agent] }))));
      await page.goto("/remote", { waitUntil: "domcontentloaded" });
      await page.getByRole("button", { name: "Open Synthetic configuration check details" }).click();
    }
    const turn = page.getByTestId("chat-committed-turn");
    await expect(turn).toBeVisible({ timeout: 15_000 });
    await expect(turn.getByText("All 42 tests passed.")).toBeVisible();
    await expect(turn.getByRole("button", { name: "Show output" })).toHaveCount(0);
    const capture = async (state: string) => {
      const directory = process.env.WARDIAN_COMMITTED_TURN_SCREENSHOT_DIR;
      if (!directory) return;
      fs.mkdirSync(directory, { recursive: true });
      await page.screenshot({ path: path.join(directory, `${surface}-${state}.png`), animations: "disabled" });
    };
    await capture("collapsed");
    await turn.getByRole("button", { name: /Worked.*2 steps/ }).click();
    await expect(turn.getByText(/The configuration looks consistent/)).toBeVisible();
    const narrationColor = await turn.getByText(/The configuration looks consistent/).evaluate((element) => getComputedStyle(element).color);
    const answerColor = await turn.getByText("All 42 tests passed.").evaluate((element) => getComputedStyle(element).color);
    expect(narrationColor).not.toBe(answerColor);
    await turn.getByRole("button", { name: "Show output", exact: true }).first().click();
    await expect(turn.getByText(/"valid": true/)).toBeVisible();
    await expect(turn.getByText("3 checks")).toBeVisible();
    const requests = surface === "desktop" ? await page.evaluate(() => (window as unknown as { stepRequests: string[] }).stepRequests) : detailRequests;
    expect(requests).toEqual(["step:read"]);
    expect(await turn.evaluate((element) => element.scrollWidth <= element.clientWidth)).toBe(true);
    await capture("detail");
  });
}
