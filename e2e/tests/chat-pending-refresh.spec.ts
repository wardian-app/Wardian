import fs from "node:fs";
import path from "node:path";
import { expect, test } from "@playwright/test";
import { committedChatFixture } from "../../src/test/committedChatFixture";
import { installWorkbenchIpcMock, makeWorkbenchDocument, makeWorkbenchSurface } from "../fixtures/workbenchIpcMock";

test("Desktop pending Chat keeps its answer in view when earlier history arrives", async ({ page }) => {
  const fixture = committedChatFixture();
  const answer = fixture.events[fixture.events.length - 1];
  const message = (id: string, text: string) => ({ ...answer, id, text, metadata: {} });
  const recent = Array.from({ length: 24 }, (_, index) =>
    message(`recent-${index}`, `Recent report note ${index + 1}.`));
  const initial = { ...fixture.page, presentation: null, progress: "provisional" as const, events: [
    ...recent, { ...message("prompt", "Review the synthetic report."), role: "user" as const },
    message("answer", "The synthetic report is ready."),
  ] };
  const history = Array.from({ length: 28 }, (_, index) =>
    message(`history-${index}`, `Earlier history entry ${index + 1}.`));
  const refresh = { ...initial, revision: "captured-earlier-history", progress: "indexing" as const, events: [...history, ...initial.events] };
  await page.addInitScript(() => localStorage.setItem("wardian-settings", JSON.stringify({ state: { gridCardDisplayMode: "chat" }, version: 2 })));
  const document = makeWorkbenchDocument({ surfaces: [makeWorkbenchSurface("overview", "agents-overview")],
    shell: { left_sidebar_collapsed: true, right_sidebar_collapsed: true } });
  await installWorkbenchIpcMock(page, { agents: [{ session_id: initial.session_id, session_name: "Synthetic report review",
    agent_class: "Coder", provider: "codex", folder: "/workspace/sample", is_off: false }], responses: {
      load_agent_chat_page: initial,
      list_provider_model_catalog: { provider: "codex", models: [], default_effort: null, refresh_error: null },
    },
    load_result: { source: "primary", document, notice: null, durable_revision: document.revision, durable_token: "synthetic-state" } });
  await page.goto("/", { waitUntil: "domcontentloaded" });
  const scroll = page.getByTestId("agent-chat-scroll-region");
  const target = scroll.getByText("The synthetic report is ready.", { exact: true });
  await expect(target).toBeAttached({ timeout: 20_000 });
  // The initial card layout can put the composer partly below the viewport.
  // Wait for it to fit before setting the scroll position we will preserve.
  await expect(page.getByTestId("chat-composer")).toBeInViewport({ ratio: 1 });
  await scroll.evaluate((element) => { element.scrollTop = element.scrollHeight; });
  await expect(target).toBeInViewport({ ratio: 0.95 });
  expect(await scroll.evaluate((element) => element.scrollTop)).toBeGreaterThan(0);
  const before = await target.evaluate((element) => element.getBoundingClientRect().top);
  await page.evaluate((reply) => {
    const { __TAURI_INTERNALS__: ipc } = window as unknown as {
      __TAURI_INTERNALS__: { invoke: (command: string, args?: Record<string, unknown>) => Promise<unknown> };
    };
    const invoke = ipc.invoke;
    ipc.invoke = (command, args) => command === "load_agent_chat_page" ? Promise.resolve(reply) : invoke(command, args);
  }, refresh);
  await expect(scroll).toHaveAttribute("data-chat-page-scope", /captured-earlier-history/);
  await expect(scroll.locator("[data-chat-row-key]").last()).toHaveAttribute("data-chat-row-key", "answer");
  await expect(target).toBeInViewport({ ratio: 0.95 });
  const after = await target.evaluate((element) => element.getBoundingClientRect().top);
  expect(Math.abs(after - before)).toBeLessThanOrEqual(2);
  const directory = process.env.WARDIAN_PENDING_REFRESH_SCREENSHOT_DIR;
  if (directory) {
    fs.mkdirSync(directory, { recursive: true });
    await page.getByTestId("agent-chat-view").screenshot({ path: path.join(directory, "desktop-pending-refresh.png"), animations: "disabled" });
  }
});

test("Desktop Chat keeps the prompt in the composer while sending", async ({ page }) => {
  const { page: initial } = committedChatFixture();
  await page.addInitScript(() => localStorage.setItem("wardian-settings", JSON.stringify({ state: { gridCardDisplayMode: "chat" }, version: 2 })));
  const workbench = makeWorkbenchDocument({ surfaces: [makeWorkbenchSurface("overview", "agents-overview")],
    shell: { left_sidebar_collapsed: true, right_sidebar_collapsed: true } });
  await installWorkbenchIpcMock(page, { agents: [{ session_id: initial.session_id, session_name: "Sample report review",
    agent_class: "Coder", provider: "codex", folder: "/workspace/sample", is_off: false }], responses: {
      load_agent_chat_page: initial,
      list_provider_model_catalog: { provider: "codex", models: [], default_effort: null, refresh_error: null },
    },
    load_result: { source: "primary", document: workbench, notice: null, durable_revision: workbench.revision, durable_token: "synthetic-state" } });
  await page.goto("/", { waitUntil: "domcontentloaded" });
  const chat = page.getByTestId("agent-chat-view");
  const input = chat.getByLabel("Message agent");
  await expect(input).toBeEditable();
  await page.evaluate(() => {
    const { __TAURI_INTERNALS__: ipc } = window as unknown as {
      __TAURI_INTERNALS__: { invoke: (command: string, args?: Record<string, unknown>) => Promise<unknown> };
    };
    const invoke = ipc.invoke;
    ipc.invoke = (command, args) => command === "submit_prompt_to_agent" ? new Promise((resolve) => {
      document.addEventListener("accept-synthetic-send", () => resolve({ delivery_state: "provider_accepted" }), { once: true });
    }) : invoke(command, args);
  });
  const prompt = "Summarize the sample report in three bullets.";
  await input.fill(prompt);
  await chat.getByRole("button", { name: "Send message" }).click();
  await expect(chat.getByRole("button", { name: "Sending message" })).toBeVisible();
  await expect(input).toHaveValue(prompt);
  await expect(chat.getByTestId("agent-chat-scroll-region").getByText(prompt, { exact: true })).toHaveCount(0);
  const directory = process.env.WARDIAN_PENDING_REFRESH_SCREENSHOT_DIR;
  if (directory) {
    fs.mkdirSync(directory, { recursive: true });
    await chat.screenshot({ path: path.join(directory, "desktop-prompt-sending.png"), animations: "disabled" });
  }
  await page.evaluate(() => document.dispatchEvent(new Event("accept-synthetic-send")));
  await expect(input).toHaveValue("");
});
