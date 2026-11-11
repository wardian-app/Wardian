// @tier ci — Pure harness error propagation; no native app or provider.
import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import vm from "node:vm";
import { createRequire } from "node:module";
import { createHash } from "node:crypto";
import { invokeTauri } from "../lib/harness.mjs";
import { chatBodyWitness } from "../lib/chatSavedFixture.mjs";

const { chatAcceptanceFixture, chatAcceptanceEvents } = createRequire(import.meta.url)("../../scripts/mock-agent.cjs");

/** Extract the actual browser function without registering or running a native case. */
function observationHarness(invoke, options = {}) {
  const source = fs.readFileSync(new URL("./chat-turn-change-native.test.mjs", import.meta.url), "utf8");
  const start = source.indexOf("function installChatObservation(config) {");
  const end = source.indexOf("async function readObservation(driver) {", start);
  assert.ok(start >= 0 && end > start);
  assert.equal(source.indexOf("function installChatObservation(config) {", start + 1), -1);
  let clock = 0;
  let shown = true;
  let chatMode = false;
  let rect = { top: 20, bottom: 40, left: 10, right: 400, width: 390, height: 20 };
  let needleRect = null;
  const listeners = new Map();
  let frames = [];
  let admitted = { session: "session", conversation: "conversation", generation: "generation", epoch: "epoch",
    revision: "opaque-revision", admission: null };
  let identity = { id: "public-id", members: null };
  let key = "public-id";
  const rawBody = options.body ?? chatAcceptanceFixture().latestBody;
  const content = { textContent: options.renderedText ?? rawBody, isConnected: true, parentElement: null,
    getAttribute: () => null,
    closest: () => ({ getAttribute: (name) => name === "data-chat-row-key" ? key : JSON.stringify(identity) }) };
  const scroll = { scrollTop: 100, scrollHeight: 1000, clientHeight: 500,
    getBoundingClientRect: () => ({ top: 0, bottom: 500, left: 0, right: 500 }),
    getAttribute: () => JSON.stringify(admitted) };
  const surface = { getAttribute: () => "session", contains: (node) => node.owned === true,
    querySelector: (selector) => selector.includes("Switch to Terminal") ? (chatMode ? {} : null)
      : shown && chatMode ? scroll : null,
    querySelectorAll: () => shown && chatMode ? [content] : [] };
  const document = { documentElement: {}, activeElement: null, visibilityState: "visible",
    querySelectorAll: () => [surface], addEventListener: (type, callback) => listeners.set(type, callback),
    createTreeWalker: () => {
      let offset = 0;
      const nodes = (options.textSegments ?? [content.textContent]).map((text) => {
        const node = { textContent: text, parentElement: content, offset };
        offset += text.length;
        return node;
      });
      let index = 0;
      return { nextNode: () => nodes[index++] ?? null };
    },
    createRange: () => {
      let whole = false;
      let start = null;
      let end = null;
      return { selectNodeContents() { whole = true; },
        setStart(node, offset) { start = { node, offset }; }, setEnd(node, offset) { end = { node, offset }; },
        toString: () => start?.node === end?.node ? start.node.textContent.slice(start.offset, end.offset)
          : content.textContent.slice(start.node.offset + start.offset, end.node.offset + end.offset),
        getClientRects: () => [whole ? rect : needleRect ?? rect] };
    } };
  const context = { window: { __TAURI_INTERNALS__: { invoke } }, document,
    performance: { now: () => clock }, innerHeight: 500, innerWidth: 500,
    MutationObserver: class { observe() {} }, requestAnimationFrame: (callback) => frames.push(callback) };
  vm.runInNewContext(`${source.slice(start, end)}\ninstallChatObservation(config);`,
    { ...context, getComputedStyle: () => ({ visibility: "visible", display: "block", opacity: "1" }),
      config: { name: "Owned", body: rawBody, witness: options.witness, probe: "native-input-probe" } });
  const observer = context.window.__chatAcceptance;
  observer.bind("session");
  observer.arm("cold");
  return {
    observer, document, content, wrapper: context.window.__TAURI_INTERNALS__.invoke,
    clock(value) { clock = value; }, shown(value) { shown = value; }, rectangle(value) { rect = value; },
    needleRectangle(value) { needleRect = value; }, mode(value) { chatMode = value === "chat"; },
    overflow(value) { scroll.scrollHeight = value ? 1000 : scroll.clientHeight; },
    admitted(value) { admitted = { ...admitted, ...value }; },
    identity(value) { identity = value; }, key(value) { key = value; },
    roster(trusted = true, name = "Owned", type = "pointerdown") {
      const row = { getAttribute: () => `Agent ${name}` };
      listeners.get(type)({ type, isTrusted: trusted,
        target: { closest: (selector) => selector === `[aria-label="Agent ${name}"]` ? row : null } });
    },
    click(trusted = true, owned = true) {
      // The normal manual route starts with Roster; explicit-origin tests can
      // supply its earlier pointer event and verify that it is never replaced.
      if (trusted && owned && observer.state.phase.rosterAt === null) {
        const row = { getAttribute: () => "Agent Owned" };
        listeners.get("pointerdown")({ type: "pointerdown", isTrusted: true,
          target: { closest: () => row } });
      }
      const button = { owned };
      const target = { closest: (selector) => selector.includes("Switch to Chat") ? button : null };
      listeners.get("click")({ isTrusted: trusted, target });
      if (trusted && owned) chatMode = true;
    },
    input(trusted = true, owned = true) {
      const target = { owned, value: "native-input-probe", getAttribute: () => "Message agent" };
      document.activeElement = target;
      listeners.get("input")({ isTrusted: trusted, target });
      return target;
    },
    wheel(trusted, nextTop) {
      listeners.get("wheel")({ isTrusted: trusted, target: { owned: true }, deltaY: -20 });
      scroll.scrollTop = nextTop;
    },
    menu(owned = true) {
      const button = { owned, textContent: "Message actions" };
      listeners.get("click")({ isTrusted: true,
        target: { closest: (selector) => selector === "button" ? button : null } });
    },
    frame() { clock += 16; const pending = frames; frames = []; for (const callback of pending) callback(); },
  };
}

function actualPage(overrides = {}) {
  return { session_id: "session", conversation_id: "conversation", generation: "generation", source_epoch: "epoch",
    revision: "opaque-revision", recent_admission: null, next_before: null, progress: "ready",
    events: [{ id: "public-id", session_id: "session", provider: "mock", kind: "message", role: "assistant",
      turn_id: chatAcceptanceFixture().latestTurn, text: chatAcceptanceFixture().latestBody, metadata: {} }],
    ...overrides };
}

function actualCanonicalHistory(receipts) {
  const source = fs.readFileSync(new URL("./chat-turn-change-native.test.mjs", import.meta.url), "utf8").replaceAll("\r\n", "\n");
  const start = source.indexOf("function canonicalHistoryReceipts(receipts, sessionId) {");
  const end = source.indexOf("\n/** Real scroll/click demand only;", start);
  assert.ok(start >= 0 && end > start);
  return vm.runInNewContext(`${source.slice(start, end)}\ncanonicalHistoryReceipts(receipts, "session");`, { receipts });
}

function actualRedaction(value) {
  const source = fs.readFileSync(new URL("./chat-turn-change-native.test.mjs", import.meta.url), "utf8");
  const start = source.indexOf("function redactObservation(value) {");
  const end = source.indexOf("// Native execution is intentionally separate", start);
  assert.ok(start >= 0 && end > start);
  return vm.runInNewContext(`${source.slice(start, end)}\nredactObservation(value);`, { value, createHash });
}

function driverResult(result) {
  return { executeAsyncScript: async () => result };
}

test("invokeTauri preserves a structured backend error code", async () => {
  const driver = driverResult({
    ok: false,
    error: { code: "provider_input_not_ready", message: "input is still booting" },
  });

  await assert.rejects(invokeTauri(driver, "submit_prompt_to_agent"), (error) => {
    assert.equal(error.code, "provider_input_not_ready");
    assert.equal(error.message, "submit_prompt_to_agent failed: input is still booting");
    return true;
  });
});

test("invokeTauri retains rejection details without promoting non-string codes", async () => {
  for (const code of [undefined, null, 408, { nested: "code" }]) {
    const driver = driverResult({ ok: false, error: { code, message: "read failed" } });
    await assert.rejects(invokeTauri(driver, "load_agent_chat_page"), (error) => {
      assert.equal(error.code, undefined);
      assert.equal(error.message, "load_agent_chat_page failed: read failed");
      return true;
    });
  }
});

test("invokeTauri returns a successful response unchanged", async () => {
  const value = { session_id: "session", events: [] };
  assert.equal(await invokeTauri(driverResult({ ok: true, value }), "load_agent_chat_page"), value);
});

test("passive Chat observation preserves this, arguments, physical call, Promise and result identity", async () => {
  const page = actualPage();
  const promise = Promise.resolve(page);
  const receiver = {};
  const payload = { sessionId: "session", cursor: "opaque-cursor" };
  const calls = [];
  const harness = observationHarness(function (...args) { calls.push({ receiver: this, args }); return promise; });
  const returned = harness.wrapper.call(receiver, "load_agent_chat_page", payload);
  assert.equal(returned, promise);
  assert.equal(await returned, page);
  assert.equal(calls.length, 1);
  assert.equal(calls[0].receiver, receiver);
  assert.equal(calls[0].args[1], payload);
  assert.equal(harness.observer.state.receipts[0].before, "opaque-cursor");
});

test("passive Chat observation preserves original synchronous and asynchronous failures", async () => {
  const failure = new Error("original rejection");
  const rejected = Promise.reject(failure);
  const harness = observationHarness(() => rejected);
  assert.equal(harness.wrapper("load_agent_chat_page", {}), rejected);
  await assert.rejects(rejected, (error) => error === failure);
  assert.equal(harness.observer.state.receipts.length, 0);
  const sync = observationHarness(() => { throw failure; });
  assert.throws(() => sync.wrapper("load_agent_chat_page", {}), (error) => error === failure);
});

test("only trusted owned Chat input starts the original selection clock", () => {
  const harness = observationHarness(() => Promise.resolve(actualPage()));
  harness.click(false);
  harness.click(true, false);
  assert.equal(harness.observer.state.phase.chatAt, null);
  harness.clock(41);
  harness.click();
  harness.clock(900);
  harness.click();
  assert.equal(harness.observer.state.phase.chatAt, 41);
});

test("readable body qualification requires actual receipt identity and two animation frames", async () => {
  const harness = observationHarness(() => Promise.resolve(actualPage()));
  harness.click();
  harness.observer.sample();
  harness.frame(); harness.frame();
  assert.equal(harness.observer.state.phase.paintAt, null, "DOM text alone is not proof");
  await harness.wrapper("load_agent_chat_page", { sessionId: "session" });
  harness.observer.sample();
  assert.equal(harness.observer.state.phase.paintAt, null);
  harness.frame();
  assert.equal(harness.observer.state.phase.paintAt, null);
  harness.frame();
  assert.ok(harness.observer.state.phase.paintAt > harness.observer.state.phase.commitAt);
  assert.equal(harness.observer.state.phase.body.revision, "opaque-revision");
});

test("rich Saved text uses a precomputed rendered range while the RPC proof retains the entire independent raw Markdown", async () => {
  const body = "**An independently rendered assistant witness.**";
  const renderedText = "An independently rendered assistant witness.";
  const witness = chatBodyWitness(body);
  const options = { body, renderedText, witness, textSegments: ["An independently ", "rendered assistant witness."] };
  const page = (text) => actualPage({ events: [{ ...actualPage().events[0], text }] });
  const harness = observationHarness(() => Promise.resolve(page(body)), options);
  harness.click(); await harness.wrapper("load_agent_chat_page", { sessionId: "session" });
  harness.observer.sample(); harness.frame(); harness.frame();
  assert.equal(harness.observer.state.phase.body.witnessKind, "needle");
  assert.equal(harness.observer.state.phase.body.bodySha256, createHash("sha256").update(body).digest("hex"));
  assert.equal(harness.observer.state.receipts[0].rows[0].target, true);
  assert.equal(harness.observer.state.receipts[0].rows[0].length, body.length);
  for (const change of ["raw", "hidden-range", "stale-scope"]) {
    const invalid = observationHarness(() => Promise.resolve(page(change === "raw" ? renderedText : body)), options);
    invalid.click(); await invalid.wrapper("load_agent_chat_page", { sessionId: "session" });
    if (change === "hidden-range") invalid.needleRectangle({ top: 600, bottom: 620, left: 10, right: 400, width: 390, height: 20 });
    invalid.observer.sample(); invalid.frame();
    if (change === "stale-scope") invalid.admitted({ revision: "replaced-window" });
    invalid.frame();
    assert.equal(invalid.observer.state.phase.paintAt, null, change);
  }
});

test("automatic Off Chat preserves the first trusted Roster clock and a null explicit-toggle time", async () => {
  const preselected = observationHarness(() => Promise.resolve(actualPage()));
  preselected.mode("chat");
  await preselected.wrapper("load_agent_chat_page", { sessionId: "session" });
  preselected.observer.sample(); preselected.frame(); preselected.frame();
  assert.equal(preselected.observer.state.phase.paintAt, null, "an already visible view needs its original Roster action");
  const harness = observationHarness(() => Promise.resolve(actualPage()));
  harness.mode("chat");
  harness.clock(50); harness.roster(false);
  harness.observer.sample(); harness.frame(); harness.frame();
  assert.equal(harness.observer.state.phase.exposureAt, null);
  harness.clock(100); harness.roster();
  harness.clock(105); await harness.wrapper("load_agent_chat_page", { sessionId: "session" });
  harness.clock(110); harness.observer.sample(); harness.frame(); harness.frame();
  const phase = harness.observer.state.phase;
  assert.equal(phase.rosterAt, 100);
  assert.equal(phase.chatAt, null);
  assert.equal(phase.exposureAt, 110);
  assert.equal(phase.exposureKind, "automatic");
  assert.equal(phase.paintAt, 142);
  assert.ok(harness.observer.state.calls.every((call) => call.at >= phase.rosterAt));
  harness.input(); harness.frame(); harness.frame();
  assert.equal(phase.usableAt, 174);
  harness.clock(900); harness.roster();
  assert.equal(phase.rosterAt, 100);
  const hiddenMode = observationHarness(() => Promise.resolve(actualPage()));
  hiddenMode.mode("chat"); hiddenMode.roster();
  await hiddenMode.wrapper("load_agent_chat_page", { sessionId: "session" });
  hiddenMode.observer.sample(); hiddenMode.frame(); hiddenMode.mode("terminal"); hiddenMode.frame();
  assert.equal(hiddenMode.observer.state.phase.paintAt, null);
});

test("a non-overflowing Off view accepts only its trusted unsent composer with a stable accepted body", async () => {
  const harness = observationHarness(() => Promise.resolve(actualPage()));
  harness.overflow(false); harness.mode("chat"); harness.roster();
  await harness.wrapper("load_agent_chat_page", { sessionId: "session" });
  harness.observer.sample(); harness.frame(); harness.frame();
  assert.ok(harness.observer.state.phase.paintAt !== null);
  harness.menu(false); harness.menu(true); harness.frame(); harness.frame();
  assert.equal(harness.observer.state.phase.usableAt, null, "unrelated or foreign menus are not an interaction witness");
  harness.wheel(true, 60); harness.frame(); harness.frame(); harness.frame();
  assert.equal(harness.observer.state.phase.usableAt, null, "no overflow cannot produce a real wheel state change");
  harness.input(true, false); harness.frame(); harness.frame();
  assert.equal(harness.observer.state.phase.usableAt, null);
  harness.input(); harness.frame(); harness.admitted({ revision: "stale-composer-window" }); harness.frame();
  assert.equal(harness.observer.state.phase.usableAt, null);
  harness.admitted({ revision: "opaque-revision" });
  harness.input(); harness.frame(); harness.frame();
  assert.ok(harness.observer.state.phase.usableAt !== null);
  assert.equal(harness.observer.state.phase.chatAt, null);
});

test("foreign scope, user echo, spinner and wrong public slot cannot qualify assistant paint", async () => {
  const normal = actualPage();
  for (const page of [actualPage({ session_id: "foreign" }), actualPage({ generation: null }),
    actualPage({ events: [{ ...normal.events[0], role: "user" }] }),
    actualPage({ events: [{ ...normal.events[0], text: "Processing your request..." }] }),
    actualPage({ events: [{ ...normal.events[0], id: "another-public-slot" }] })]) {
    const harness = observationHarness(() => Promise.resolve(page));
    harness.click();
    await harness.wrapper("load_agent_chat_page", { sessionId: "session" });
    harness.observer.sample(); harness.frame(); harness.frame();
    assert.equal(harness.observer.state.phase.paintAt, null);
  }
});

test("body anchor outside the owned Chat viewport cannot qualify even when its row exists", async () => {
  const harness = observationHarness(() => Promise.resolve(actualPage()));
  harness.rectangle({ top: -80, bottom: -20, left: 0, right: 400, width: 400, height: 60 });
  harness.click();
  await harness.wrapper("load_agent_chat_page", { sessionId: "session" });
  harness.observer.sample(); harness.frame(); harness.frame();
  assert.equal(harness.observer.state.phase.paintAt, null);
});

test("late detached body and synthetic composer edits cannot qualify paint or usability", async () => {
  const harness = observationHarness(() => Promise.resolve(actualPage()));
  harness.click();
  await harness.wrapper("load_agent_chat_page", { sessionId: "session" });
  harness.observer.sample();
  harness.shown(false);
  harness.frame(); harness.frame();
  assert.equal(harness.observer.state.phase.paintAt, null);
  harness.shown(true);
  harness.observer.sample(); harness.frame(); harness.frame();
  harness.input(false); harness.frame(); harness.frame();
  assert.equal(harness.observer.state.phase.usableAt, null);
  const input = harness.input();
  input.value = "changed before paint";
  harness.frame(); harness.frame();
  assert.equal(harness.observer.state.phase.usableAt, null);
  harness.input(); harness.frame(); harness.frame();
  assert.ok(harness.observer.state.phase.usableAt !== null);
});

test("observer bounds diagnostics without changing the original result", async () => {
  const page = actualPage({ events: Array(385).fill(actualPage().events[0]) });
  const promise = Promise.resolve(page);
  const harness = observationHarness(() => promise);
  assert.equal(harness.wrapper("load_agent_chat_page", {}), promise);
  assert.equal(await promise, page);
  assert.equal(harness.observer.state.receipts.length, 0);
  assert.match(harness.observer.state.errors[0], /bound exceeded/);
});

test("typing before delayed body arrival cannot qualify a body that was absent at the trusted interaction", async () => {
  const harness = observationHarness(() => Promise.resolve(actualPage()));
  harness.shown(false);
  harness.clock(10); harness.click();
  harness.clock(30); harness.input();
  harness.frame(); harness.frame();
  assert.equal(harness.observer.state.phase.usableAt, null);
  harness.clock(400);
  await harness.wrapper("load_agent_chat_page", { sessionId: "session" });
  harness.shown(true); harness.observer.sample();
  harness.frame(); harness.frame();
  assert.equal(harness.observer.state.phase.chatAt, 10);
  assert.equal(harness.observer.state.phase.usableAt, null);
  assert.equal(harness.observer.state.phase.paintAt, 432);
  harness.input(); harness.frame(); harness.frame();
  assert.equal(harness.observer.state.phase.usableAt, 464);
});

test("saved-Off usability needs a trusted wheel and an actual viewport state change", async () => {
  const harness = observationHarness(() => Promise.resolve(actualPage()));
  harness.click();
  await harness.wrapper("load_agent_chat_page", { sessionId: "session" });
  harness.observer.sample(); harness.frame(); harness.frame();
  harness.wheel(false, 80); harness.frame(); harness.frame(); harness.frame();
  assert.equal(harness.observer.state.phase.usableAt, null);
  harness.wheel(true, 80); harness.frame(); harness.frame(); harness.frame();
  assert.equal(harness.observer.state.phase.usableAt, null, "wheel receipt alone is not state change");
  harness.wheel(true, 60); harness.frame(); harness.frame(); harness.frame();
  assert.ok(harness.observer.state.phase.usableAt !== null);
});

test("the original trusted Roster action precedes Chat and is never reset by a later toggle", () => {
  const harness = observationHarness(() => Promise.resolve(actualPage()));
  harness.roster(false); harness.roster(true, "Foreign");
  assert.equal(harness.observer.state.phase.rosterAt, null);
  harness.clock(7); harness.roster();
  harness.clock(700); harness.click();
  harness.clock(900); harness.roster(); harness.click();
  assert.equal(harness.observer.state.phase.rosterAt, 7);
  assert.equal(harness.observer.state.phase.chatAt, 700);
});

test("returned but unadmitted scope, missing epoch and mismatched actual members cannot qualify paint", async () => {
  for (const changed of [{ conversation: "new" }, { generation: "new" }, { epoch: null },
    { revision: "new-window" }, { admission: "new-preview" }]) {
    const harness = observationHarness(() => Promise.resolve(actualPage()));
    harness.click(); await harness.wrapper("load_agent_chat_page", { sessionId: "session" });
    harness.admitted(changed); harness.observer.sample(); harness.frame(); harness.frame();
    assert.equal(harness.observer.state.phase.paintAt, null);
  }
  const missing = observationHarness(() => Promise.resolve(actualPage({ source_epoch: undefined })));
  missing.admitted({ epoch: null }); missing.click();
  await missing.wrapper("load_agent_chat_page", { sessionId: "session" });
  missing.observer.sample(); missing.frame(); missing.frame();
  assert.equal(missing.observer.state.phase.paintAt, null);
  const members = observationHarness(() => Promise.resolve(actualPage()));
  members.identity({ id: "public-id", members: ["fabricated-member"] }); members.click();
  await members.wrapper("load_agent_chat_page", { sessionId: "session" });
  members.observer.sample(); members.frame(); members.frame();
  assert.equal(members.observer.state.phase.paintAt, null);
});

test("null source epochs remain null, and actual scope changes across either paint or interaction reject stale success", async () => {
  const harness = observationHarness(() => Promise.resolve(actualPage({ source_epoch: null })));
  harness.admitted({ epoch: null }); harness.click();
  await harness.wrapper("load_agent_chat_page", { sessionId: "session" });
  harness.observer.sample(); harness.frame(); harness.frame();
  assert.equal(harness.observer.state.phase.body.epoch, null);
  harness.input(); harness.frame(); harness.admitted({ generation: "replacement" }); harness.frame();
  assert.equal(harness.observer.state.phase.usableAt, null);
  const paint = observationHarness(() => Promise.resolve(actualPage()));
  paint.click(); await paint.wrapper("load_agent_chat_page", { sessionId: "session" });
  paint.observer.sample(); paint.frame(); paint.admitted({ revision: "replacement-window" }); paint.frame();
  assert.equal(paint.observer.state.phase.paintAt, null);
});

test("an old display slot is accepted only with the response's actual canonical alias", async () => {
  let page = actualPage();
  const harness = observationHarness(() => Promise.resolve(page));
  harness.key("old-display-slot"); harness.click();
  await harness.wrapper("load_agent_chat_page", { sessionId: "session" });
  harness.observer.sample(); harness.frame(); harness.frame();
  assert.equal(harness.observer.state.phase.paintAt, null);
  page = actualPage({ aliases: [{ observation_id: "old-display-slot", canonical_id: "public-id" }] });
  await harness.wrapper("load_agent_chat_page", { sessionId: "session" });
  harness.observer.sample(); harness.frame(); harness.frame();
  assert.equal(harness.observer.state.phase.body.key, "old-display-slot");
  assert.equal(harness.observer.state.phase.body.eventId, "public-id");
});

test("hidden or detached body text cannot qualify a readable endpoint", async () => {
  for (const hide of [(h) => { h.document.visibilityState = "hidden"; }, (h) => { h.content.isConnected = false; }]) {
    const harness = observationHarness(() => Promise.resolve(actualPage()));
    harness.click(); await harness.wrapper("load_agent_chat_page", { sessionId: "session" });
    hide(harness); harness.observer.sample(); harness.frame(); harness.frame();
    assert.equal(harness.observer.state.phase.paintAt, null);
  }
});

test("partial failure-path evidence is redacted once while bounded public status and digest tokens survive", () => {
  const digest = createHash("sha256").update("fixture-manifest").digest("hex");
  const value = { timings: [{ label: "saved-off-cold", body: { eventId: "private-placeholder", members: ["private-placeholder"] } }],
    closure: { status: "UNPROVEN", reason: "private-path-placeholder" }, manifestSha256: digest, agentPolicy: "default" };
  const result = actualRedaction(value);
  assert.equal(result.timings[0].body.eventId, createHash("sha256").update("private-placeholder").digest("hex"));
  assert.equal(result.timings[0].body.members[0], result.timings[0].body.eventId);
  assert.equal(result.manifestSha256, digest); assert.equal(result.agentPolicy, "default");
  assert.equal(result.closure.status, "UNPROVEN"); assert.equal(result.timings[0].label, "saved-off-cold");
  assert.equal(JSON.stringify(result).includes("private-placeholder"), false);
  assert.equal(JSON.stringify(result).includes("private-path-placeholder"), false);
});

test("the new opt-in Mock fixture declares genuine paged history, full body, repeats and completed owner", () => {
  const fixture = chatAcceptanceFixture();
  const frames = [...chatAcceptanceEvents()];
  const assistants = frames.filter((frame) => frame.type === "model");
  assert.ok(assistants.length > 80);
  assert.equal(Buffer.byteLength(fixture.longBody), 60000);
  assert.equal(fixture.longBody.trim(), fixture.longBody);
  assert.equal(assistants.filter((frame) => frame.content === fixture.longBody).length, 1);
  const repeats = assistants.filter((frame) => frame.content === fixture.repeatBody);
  assert.equal(repeats.length, 2);
  assert.equal(new Set(repeats.map((frame) => frame.turn_id)).size, 2);
  assert.equal(assistants.at(-1).content, fixture.latestBody);
  assert.equal(assistants.at(-1).turn_id, fixture.latestTurn);
  assert.equal(frames.at(-1).type, "result");
  assert.equal(frames.at(-1).turn_id, fixture.latestTurn);
  assert.equal(frames.at(-1).status, "success");
  assert.equal(frames.some((frame) => frame.type === "user" && frame.content === fixture.latestBody), false);
  assert.ok(frames.length + 32 <= fixture.maxRawFrames);
});

test("mixed preview/canonical receipts retain exactly two canonical repeated turns after handoff reset", async () => {
  const body = chatAcceptanceFixture().repeatBody;
  const repeated = (prefix) => ["a", "b"].map((turn) => ({ ...actualPage().events[0],
    id: `${prefix}-${turn}`, turn_id: `repeated-${turn}`, text: body }));
  let dto = actualPage({ recent_admission: "preview-owner", events: repeated("preview") });
  const harness = observationHarness(() => Promise.resolve(dto));
  await harness.wrapper("load_agent_chat_page", { sessionId: "session" });
  dto = actualPage({ reset: true, events: repeated("canonical") });
  await harness.wrapper("load_agent_chat_page", { sessionId: "session" });
  const rows = new Map(actualCanonicalHistory(harness.observer.state.receipts)
    .flatMap((receipt) => receipt.rows).map((row) => [row.key, row]));
  assert.equal(rows.size, 2, "preview identities are not durable canonical history");
  assert.deepEqual([...rows.values()].map((row) => row.id), ["canonical-a", "canonical-b"]);
  assert.equal(new Set([...rows.values()].map((row) => row.turn)).size, 2);
});

test("canonical history clears old cohorts on reset and scope changes without interpreting opaque revision", () => {
  const receipt = (id, overrides = {}) => ({ session: "session", conversation: "conversation", generation: "generation",
    epoch: "epoch", revision: "opaque-recent", admission: null, progress: "ready", detail: false,
    rows: [{ id, key: id, role: "assistant", turn: id }], ...overrides });
  const ids = (entries) => Array.from(actualCanonicalHistory(entries).flatMap((entry) => entry.rows), (row) => row.id);
  for (const scope of [{ conversation: "other" }, { generation: "new" }, { epoch: "new-source" }]) {
    assert.deepEqual(ids([receipt("stale"), receipt("new", scope)]), ["new"]);
  }
  assert.deepEqual(ids([receipt("stale"), receipt("after-reset", { reset: true })]), ["after-reset"]);
  assert.deepEqual(ids([receipt("recent"), receipt("older", { older: true, revision: "a-different-opaque-page-token" })]),
    ["recent", "older"]);
  assert.deepEqual(ids([receipt("owned"), receipt("foreign", { session: "foreign", reset: true })]), ["owned"]);
});

test("preview and unknown body capabilities cannot supply canonical full-body owners", async () => {
  const receipt = { session: "session", conversation: "conversation", generation: "generation", epoch: null,
    revision: "opaque", admission: null, progress: "ready", rows: [{ id: "body-owner", turn: "chat-acceptance-long" }] };
  for (const overrides of [{ admission: "preview" }, { progress: "indexing" }, { generation: null },
    { revision: null }, { conversation: null }, { epoch: undefined }, { detail: true }]) {
    assert.equal(actualCanonicalHistory([{ ...receipt, ...overrides }]).length, 0);
  }
  const harness = observationHarness(() => Promise.resolve(actualPage({ recent_admission: 42 })));
  await harness.wrapper("load_agent_chat_page", {});
  assert.equal(harness.observer.state.receipts.length, 0);
  assert.match(harness.observer.state.errors[0], /invalid recent admission/);
});
