import fs from "node:fs";
import path from "node:path";
import { expect, test, type Locator } from "@playwright/test";
import { committedChatFixture } from "../../src/test/committedChatFixture";
import type { AgentChatPage, ChatPresentationV1, ProviderModelCatalog } from "../../src/types";
import { coldSourcePreviewIdentityFixture, previewIdentityFixture } from "../fixtures/chatPreviewIdentity";
import { qualifiedChatPreviewBridge, sourceLegacyBridgeCoversWindow } from "../../src/features/chat/chatPreviewIdentity";
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

function previewBrowserPages() {
  const fixture = previewIdentityFixture();
  const native = coldSourcePreviewIdentityFixture();
  const payload = fixture.provisional.presentation!.payload as ChatPresentationV1;
  const note = { ...fixture.provisional.events.find((event) => event.id === "source:note")!, id: "source:running-note",
    text: "The focused check is still running." };
  const item = { ...payload.items.find((entry) => entry.kind === "narration")!, item_key: "item:running-note",
    order_key: "00035", event_refs: [note.id], preview: { text: note.text, attachment_refs: [] } };
  const running: AgentChatPage = { ...fixture.provisional, revision: "browser-running-H1", aliases: [],
    events: [...fixture.provisional.events, note], presentation: { version: 1, payload: { ...payload, items: [...payload.items, item] } } };
  const closed = fixture.closed.presentation!.payload as ChatPresentationV1;
  const canonical: AgentChatPage = { ...fixture.closed, revision: "browser-canonical-H2",
    events: [...fixture.closed.events.slice(0, -1), { ...note, metadata: { ...note.metadata, chat_provisional: false } }, fixture.closed.events.at(-1)!],
    presentation: { version: 1, payload: { ...closed, preview_bridge: { ...closed.preview_bridge!, canonical_head: "b".repeat(64) },
      items: [...closed.items, { ...item, provenance: "canonical", admission_id: null, preview: null }] } } };
  const context = (position: "before" | "after", paragraphs: number) => ({ ...native.cold.events[0],
    id: `source:context:${position}`, role: "assistant" as const,
    text: Array.from({ length: paragraphs }, (_, index) => `Synthetic ${position} context ${index + 1}.`).join("\n\n"),
    metadata: { chat_provisional: true, chat_source_epoch: native.cold.source_epoch } });
  const before = context("before", 8);
  const after = context("after", 32);
  const wrap = (current: AgentChatPage): AgentChatPage => {
    const value = current.presentation?.payload as ChatPresentationV1 | undefined;
    const provisional = current.recent_admission != null || !value;
    const contextRows = [before, after].map((event) => ({ ...event,
      metadata: { ...event.metadata, chat_provisional: provisional } }));
    return { ...current, events: [contextRows[0], ...current.events, contextRows[1]],
      ...(value ? { presentation: { version: 1, payload: { ...value, legacy_turns: [...value.legacy_turns,
        ...contextRows.map((event) => ({ turn_key: `legacy:${event.id}`, order_key: event.id,
          kind: "earlier_activity" as const, membership_complete: false, event_refs: [event.id] }))] } } } : {}) };
  };
  const extensionEvents = [0, 1, 2].map((index) => ({ ...fixture.provisional.events[index + 1], id: `source:uncertified:${index}`,
    title: `Extension tool ${index + 1}`, metadata: { chat_provisional: true, chat_source_epoch: fixture.provisional.source_epoch } }));
  const extensionTail = { ...context("after", 32), id: "source:extension-tail" };
  const tailGroup = { turn_key: "legacy:extension-tail", order_key: "extension-tail", kind: "earlier_activity" as const,
    membership_complete: false, event_refs: [extensionTail.id] };
  const runningPage = wrap(running);
  // Context is literal admitted activity. Later source rows append after it.
  runningPage.events = [...wrap(fixture.provisional).events, note];
  const runningValue = running.presentation!.payload as ChatPresentationV1;
  const extension: AgentChatPage = { ...runningPage, revision: "browser-uncertified-extension-H1",
    events: [...runningPage.events, ...extensionEvents, extensionTail],
    presentation: { version: 1, payload: { ...runningValue, turns: runningValue.turns.map((turn) => ({ ...turn, status: "incomplete" })),
      legacy_turns: [...(runningPage.presentation!.payload as ChatPresentationV1).legacy_turns, tailGroup,
        { turn_key: "legacy:uncertified-extension", order_key: "00036", kind: "earlier_activity",
          membership_complete: false, event_refs: extensionEvents.map((event) => event.id) }] } } };
  const provenEvents = extensionEvents.map((event, index) => ({ ...event, id: `source:proven-extension:${index}`,
    metadata: { ...event.metadata, chat_provisional: false, chat_turn_key: closed.turns[0].turn_key } }));
  const canonicalValue = canonical.presentation!.payload as ChatPresentationV1;
  const proven = wrap({ ...canonical,
    aliases: extensionEvents.map((event, index) => ({ observation_id: event.id, canonical_id: provenEvents[index].id })),
    presentation: { version: 1, payload: { ...canonicalValue, turns: canonicalValue.turns.map((turn) => ({ ...turn, step_count: 7 })),
      legacy_turns: [tailGroup],
      items: [...canonicalValue.items, ...provenEvents.map((event, index) => ({ ...closed.items[1], item_key: `item:proven-extension:${index}`,
        order_key: `0003${index + 6}`, event_refs: [event.id], step: { ...closed.items[1].step!, description: `Proven extension ${index + 1}`,
          invocation_ref: event.id, detail_ref: `step:proven-extension:${index}` } }))] } } });
  const canonicalEvents = new Map(canonical.events.map((event) => [event.id, event]));
  const provenByObservation = new Map(extensionEvents.map((event, index) => [event.id, provenEvents[index]]));
  proven.events = [...extension.events.map((event) => provenByObservation.get(event.id) ?? canonicalEvents.get(event.id)
    ?? { ...event, metadata: { ...event.metadata, chat_provisional: false } }), canonical.events.at(-1)!];
  return { legacy: wrap(native.cold), bridge: wrap({ ...native.bridge, revision: "browser-bridge-H1" }), running: runningPage, extension, canonical: proven };
}

// Browser transport only. These phases prove rendering and state continuity,
// not producer ownership, native acquisition, or time to first content.
for (const surface of ["desktop", "remote"] as const) {
  test(`${surface} synthetic cold-source preview preserves measured anchors through an uncertified extension and canonical upgrade`, async ({ page }, testInfo) => {
    const pages = previewBrowserPages();
    for (const current of [pages.bridge, pages.running, pages.extension, pages.canonical]) {
      expect(qualifiedChatPreviewBridge(current)).not.toBeNull();
    }
    expect(sourceLegacyBridgeCoversWindow(pages.legacy, pages.bridge, pages.legacy.events)).toBe(true);
    type Phase = keyof typeof pages;
    let phase: Phase = "legacy";
    type Read = { phase: Phase; detail: string | null; cursor: string | null; revision?: string; previewBackingIds?: string[] };
    const reads: Read[] = [];
    const agent = { session_id: pages.legacy.session_id, session_name: "Synthetic preview identity", agent_class: "Coder",
      provider: "codex", workspace: "Sample", status: "Processing", latest_text: null };
    await page.setViewportSize(surface === "desktop" ? { width: 1920, height: 1080 } : { width: 390, height: 844 });
    if (surface === "desktop") {
      await page.addInitScript(() => localStorage.setItem("wardian-settings", JSON.stringify({ state: { gridCardDisplayMode: "chat" }, version: 2 })));
      const document = makeWorkbenchDocument({ surfaces: [makeWorkbenchSurface("overview", "agents-overview")],
        shell: { left_sidebar_collapsed: true, right_sidebar_collapsed: true } });
      await installWorkbenchIpcMock(page, { agents: [{ session_id: agent.session_id, session_name: agent.session_name,
        agent_class: agent.agent_class, provider: agent.provider, folder: "/workspace/sample", is_off: false }],
        responses: { load_agent_chat_page: pages.legacy,
          load_shell_settings: { shell_id: "auto", custom_executable: null, custom_args: null,
            agent_session_persistence: "resume", default_provider: "auto", default_workspace: "", conversation_logging: "enabled",
            codex_runtime_policy: { sandbox_mode: "workspace-write", approval_policy: "on-request", full_auto: false, trust_workspaces: false } },
          list_provider_model_catalog: { provider: "codex", version: null, source: "provider_aliases", refresh_error: null,
            models: [{ id: "synthetic-model", display_name: "Fixture model", effort_options: [], default_effort: null, is_default: true }],
          } satisfies ProviderModelCatalog },
        load_result: { source: "primary", document, notice: null, durable_revision: document.revision, durable_token: "synthetic-preview-state" } });
      await page.goto("/", { waitUntil: "domcontentloaded" });
      await page.evaluate((initial) => {
        const runtime = window as unknown as {
          __TAURI_INTERNALS__: { invoke: (command: string, args?: Record<string, unknown>) => Promise<unknown> };
          previewPage: AgentChatPage; previewPhase: Phase; previewReads: Read[];
        };
        const original = runtime.__TAURI_INTERNALS__.invoke;
        runtime.previewPage = initial;
        runtime.previewPhase = "legacy";
        runtime.previewReads = [];
        runtime.__TAURI_INTERNALS__.invoke = (command, args) => {
          if (command !== "load_agent_chat_page") return original(command, args);
          runtime.previewReads.push({ phase: runtime.previewPhase, detail: typeof args?.detailRef === "string" ? args.detailRef : null,
            cursor: typeof args?.cursor === "string" ? args.cursor : null,
            revision: typeof args?.revision === "string" ? args.revision : undefined,
            previewBackingIds: Array.isArray(args?.previewBackingIds) ? args.previewBackingIds as string[] : undefined });
          return Promise.resolve(structuredClone(runtime.previewPage));
        };
      }, pages.legacy);
    } else {
      await page.addInitScript(() => localStorage.setItem("wardian.remote.agentDefaultViewMode", "chat"));
      await page.route("**/remote/api/**", async (route) => {
        const url = new URL(route.request().url());
        if (url.pathname.endsWith("/chat")) {
          const backing = url.searchParams.get("preview_backing_ids");
          reads.push({ phase, detail: url.searchParams.get("detail"), cursor: url.searchParams.get("before"),
            revision: url.searchParams.get("revision") ?? undefined, previewBackingIds: backing === null ? undefined : JSON.parse(backing) as string[] });
          await route.fulfill({ json: pages[phase] });
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
      await page.getByRole("button", { name: "Open Synthetic preview identity details" }).click();
    }
    const capture = async (state: Phase) => {
      await expect(page.getByText("Provider returned an invalid model catalogue.", { exact: true })).toHaveCount(0);
      await expect(page.getByText("Chat history could not be updated. Retry.", { exact: true })).toHaveCount(0);
      const directory = process.env.WARDIAN_PREVIEW_IDENTITY_SCREENSHOT_DIR;
      if (!directory) return;
      fs.mkdirSync(directory, { recursive: true });
      const filename = `${surface}-pr3-${state}-${testInfo.workerIndex}-${testInfo.retry}.png`;
      const screenshot = path.join(directory, filename);
      await page.screenshot({ path: screenshot, animations: "disabled" });
      await testInfo.attach(`${surface}-${state}`, { path: screenshot, contentType: "image/png" });
    };
    const advance = async (next: Phase) => {
      phase = next;
      if (surface === "desktop") await page.evaluate(({ next, current }) => {
        const runtime = window as unknown as { previewPage: AgentChatPage; previewPhase: string };
        runtime.previewPage = current;
        runtime.previewPhase = next;
      }, { next, current: pages[next] });
    };
    const relativeTop = (anchor: Locator) => anchor.evaluate((element) => {
      const scroll = element.closest<HTMLElement>(".chat-transcript-scroll");
      if (!scroll) throw new Error("Transcript scroll surface missing");
      return element.getBoundingClientRect().top - scroll.getBoundingClientRect().top;
    });
    const pinToViewportTop = async (anchor: Locator) => {
      await anchor.evaluate((element) => {
        const scroll = element.closest<HTMLElement>(".chat-transcript-scroll");
        if (!scroll) throw new Error("Transcript scroll surface missing");
        scroll.scrollTop += element.getBoundingClientRect().top - scroll.getBoundingClientRect().top;
        scroll.dispatchEvent(new Event("scroll", { bubbles: true }));
      });
      await expect.poll(async () => Math.abs(await relativeTop(anchor))).toBeLessThanOrEqual(2);
      const position = await anchor.evaluate((element) => {
        const scroll = element.closest<HTMLElement>(".chat-transcript-scroll")!;
        return { top: scroll.scrollTop, remaining: scroll.scrollHeight - scroll.scrollTop - scroll.clientHeight };
      });
      expect(position.top).toBeGreaterThan(0);
      expect(position.remaining).toBeGreaterThan(48);
      return relativeTop(anchor);
    };
    const turn = page.getByTestId("chat-committed-turn");
    const legacy = page.getByTestId("chat-work-group");
    // The legacy work sits inside the turn's collapsed work summary until opened.
    await page.getByTestId("chat-turn-work-summary").getByRole("button").first().click({ timeout: 15_000 });
    await expect(legacy).toBeVisible({ timeout: 15_000 });
    await expect(turn).toHaveCount(0);
    await legacy.getByTitle("Show all work").click();
    const legacyKeys = JSON.parse(await legacy.locator("[data-chat-anchor-keys]").getAttribute("data-chat-anchor-keys") ?? "[]") as string[];
    expect(legacyKeys.length).toBeGreaterThan(0);
    const initialAnchor = await pinToViewportTop(legacy.locator("[data-chat-anchor-keys]"));
    await capture("legacy");
    await advance("bridge");
    await expect(turn).toBeVisible({ timeout: 15_000 });
    await expect(legacy).toHaveCount(0);
    const shared = await turn.elementHandle();
    await expect(turn.getByRole("button", { name: /Working.*In progress/ })).toHaveAttribute("aria-expanded", "true");
    await expect.poll(async () => Math.abs(await relativeTop(turn.locator("button[data-chat-anchor-keys]")) - initialAnchor)).toBeLessThanOrEqual(2);
    const bridgeAnchor = await relativeTop(turn.locator("button[data-chat-anchor-keys]"));
    await expect(turn.getByText("Provisional · history is updating")).toBeVisible();
    const sharedKeys = JSON.parse(await turn.locator("button[data-chat-anchor-keys]").getAttribute("data-chat-anchor-keys") ?? "[]") as string[];
    expect(sharedKeys).toEqual(expect.arrayContaining(legacyKeys));
    const promptId = pages.legacy.events.find((event) => event.role === "user")!.id;
    await expect(turn.locator("[data-chat-anchor-key]").first()).toHaveAttribute("data-chat-anchor-key", promptId);
    await expect(turn.getByRole("button", { name: "Show output", exact: true })).toHaveCount(0);
    await expect(turn.getByText(/4 steps/)).toHaveCount(0);
    await expect(turn.getByText("All 42 tests passed.")).toHaveCount(0);
    await capture("bridge");
    await advance("running");
    await expect(turn.getByText("The focused check is still running.")).toBeVisible({ timeout: 15_000 });
    expect(await turn.evaluate((element, original) => element === original, shared)).toBe(true);
    await expect(legacy).toHaveCount(0);
    await expect(turn.getByRole("button", { name: /Working.*In progress/ })).toHaveAttribute("aria-expanded", "true");
    await expect(turn.getByText("All 42 tests passed.")).toHaveCount(0);
    await capture("running");
    const certifiedKeys = await turn.locator("[data-step-key]").evaluateAll((elements) => elements.map((element) => element.getAttribute("data-step-key")));
    await advance("extension");
    await page.getByTestId("chat-turn-work-summary").getByRole("button").first().click({ timeout: 15_000 });
    await expect(legacy).toBeVisible({ timeout: 15_000 });
    expect(await turn.evaluate((element, original) => element === original, shared)).toBe(true);
    await expect(turn.getByRole("button", { name: /Incomplete.*Incomplete history/ })).toHaveAttribute("aria-expanded", "true");
    await expect(turn.getByText("No final answer recorded")).toBeVisible();
    expect(await turn.locator("[data-step-key]").evaluateAll((elements) => elements.map((element) => element.getAttribute("data-step-key")))).toEqual(certifiedKeys);
    await expect(turn.getByText("All 42 tests passed.")).toHaveCount(0);
    await legacy.getByTitle("Show all work").click();
    const extensionKeys = JSON.parse(await legacy.locator("[data-chat-anchor-keys]").getAttribute("data-chat-anchor-keys") ?? "[]") as string[];
    await capture("extension");
    const extensionAnchor = await pinToViewportTop(legacy.locator("[data-chat-anchor-keys]"));
    await advance("canonical");
    // Waiting for this shared section's final state catches fallback to a legacy Worklog.
    await expect(turn.getByRole("button", { name: /Worked.*7 steps/ })).toHaveAttribute("aria-expanded", "true", { timeout: 15_000 });
    await expect.poll(async () => Math.abs(await relativeTop(turn.locator("button[data-chat-anchor-keys]")) - extensionAnchor)).toBeLessThanOrEqual(2);
    const canonicalAnchor = await relativeTop(turn.locator("button[data-chat-anchor-keys]"));
    await expect(turn.getByText("All 42 tests passed.")).toBeVisible();
    await expect(turn.getByText("Provisional · history is updating")).toHaveCount(0);
    await expect(legacy).toHaveCount(0);
    expect(await turn.evaluate((element, original) => element === original, shared)).toBe(true);
    await expect(turn.locator("[data-chat-anchor-key]").first()).toHaveAttribute("data-chat-anchor-key", promptId);
    const provenKeys = JSON.parse(await turn.locator("button[data-chat-anchor-keys]").getAttribute("data-chat-anchor-keys") ?? "[]") as string[];
    expect(provenKeys).toEqual(expect.arrayContaining(extensionKeys));
    await expect(turn.locator('[data-step-key^="item:proven-extension:"]')).toHaveCount(3);
    const observed = surface === "desktop" ? await page.evaluate(() => (window as unknown as { previewReads: typeof reads }).previewReads) : reads;
    expect(observed.some((read) => read.phase === "canonical")).toBe(true);
    expect(observed.some((read) => read.phase === "bridge" && read.revision === pages.legacy.revision
      && JSON.stringify(read.previewBackingIds) === JSON.stringify(pages.legacy.events.map((event) => event.id)))).toBe(true);
    expect(observed.every((read) => read.detail === null && read.cursor === null)).toBe(true);
    expect(await turn.evaluate((element) => element.scrollWidth <= element.clientWidth)).toBe(true);
    await capture("canonical");
    const measurements = JSON.stringify({ surface, synthetic: true,
      coldHandoff: { before: initialAnchor, after: bridgeAnchor, delta: bridgeAnchor - initialAnchor, expanded: true },
      uncertifiedExtensionUpgrade: { before: extensionAnchor, after: canonicalAnchor, delta: canonicalAnchor - extensionAnchor,
        expanded: true, sameTurnElement: true, retainedCertifiedStepKeys: certifiedKeys, mappedExtensionKeys: extensionKeys },
    }, null, 2);
    await testInfo.attach(`${surface}-anchor-measurements`, { body: measurements, contentType: "application/json" });
    const directory = process.env.WARDIAN_PREVIEW_IDENTITY_SCREENSHOT_DIR;
    if (directory) fs.writeFileSync(path.join(directory, `${surface}-anchor-measurements-${testInfo.workerIndex}-${testInfo.retry}.json`), measurements);
  });
}
