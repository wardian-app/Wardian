// @tier ci — Pure harness error propagation; no native app or provider.
import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import vm from "node:vm";
import { createRequire } from "node:module";
import { createHash } from "node:crypto";
import ts from "typescript";
import { invokeTauri } from "../lib/harness.mjs";
import { chatBodyWitness } from "../lib/chatSavedFixture.mjs";

// CI uses Node20. Execute the actual type-erased module rather than relying on
// newer Node's native TS loader or reimplementing the app facade in this fixture.
const observationSource = fs.readFileSync(new URL("../../src/features/chat/chatPageObservation.ts", import.meta.url), "utf8");
const observationModule = {};
const observationJs = ts.transpileModule(observationSource, {
  compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 },
}).outputText;
vm.runInNewContext(observationJs, { exports: observationModule });
const { createChatPageObservation } = observationModule;

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
  const timers = new Map();
  let nextTimer = 0;
  let admitted = { session: "session", conversation: "conversation", generation: "generation", epoch: "epoch",
    revision: "opaque-revision", admission: null };
  let identity = { id: "public-id", members: null };
  let key = "public-id";
  const rawBody = options.body ?? chatAcceptanceFixture().latestBody;
  const row = { isConnected: true, parentElement: null,
    getAttribute: (name) => name === "data-chat-row-key" ? key
      : name === "data-chat-row-identity" ? JSON.stringify(identity) : null };
  const content = { textContent: options.renderedText ?? rawBody, isConnected: true, parentElement: null,
    getAttribute: () => null,
    closest: () => row };
  let candidates = [content];
  let composers = [];
  const scroll = { scrollTop: 100, scrollHeight: 1000, clientHeight: 500,
    contains(node) { for (let current = node; current; current = current.parentElement) if (current === this) return true; return false; },
    getBoundingClientRect: () => ({ top: 0, bottom: 500, left: 0, right: 500 }),
    getAttribute: () => JSON.stringify(admitted) };
  const element = (attributes, parentElement = null) => ({
    attributes, parentElement, isConnected: true, hidden: false,
    style: { visibility: "visible", display: "block", opacity: "1" },
    getAttribute(name) { return this.attributes[name] ?? null; },
    getBoundingClientRect: () => ({ top: 0, bottom: 500, left: 0, right: 500, width: 500, height: 500 }),
    matches(selector) {
      if (selector.startsWith('.')) return (this.attributes.class ?? '').split(/\s+/).includes(selector.slice(1));
      return [...selector.matchAll(/\[([\w-]+)(?:="([^"]*)")?\]/g)]
        .every(([, name, value]) => value === undefined ? this.getAttribute(name) !== null : this.getAttribute(name) === value);
    },
    closest(selector) {
      if (this.matches?.(selector)) return this;
      for (let node = this.parentElement; node; node = node.parentElement) if (node.matches?.(selector)) return node;
      return null;
    },
    contains(node) {
      if (typeof node.owned === "boolean") return node.owned;
      for (let current = node; current; current = current.parentElement) if (current === this) return true;
      return false;
    },
    isDisplayed() {
      if (!this.isConnected || this.hidden || this.style.display === "none" || this.style.visibility === "hidden") return false;
      for (let node = this.parentElement; node; node = node.parentElement) {
        if (!node.isConnected || node.hidden || node.style.display === "none" || node.style.visibility === "hidden") return false;
      }
      return true;
    },
  });
  const host = element({ "data-testid": "workbench-host", "data-reset-pending": "false" });
  const routes = [];
  const addContainer = (kind = "agent-session", owner = "session") => {
    const surfaceId = `surface-${routes.length}`;
    const dockRoot = options.portal ? element({ class: "dv-shell" }, host) : null;
    const portal = options.portal ? element({ class: "dv-render-overlay" }, dockRoot) : null;
    const tab = element({ id: `tab-${surfaceId}`, role: "tab", "aria-selected": "true",
      "data-surface-id": surfaceId, "data-surface-type": kind, "data-resource-key": owner,
      "aria-controls": `panel-${surfaceId}` }, dockRoot ?? host);
    const tabpanel = element({ id: `panel-${surfaceId}`, role: "tabpanel", "aria-labelledby": tab.getAttribute("id") }, dockRoot ?? host);
    const panel = element({ "data-testid": "surface-panel", "data-surface-id": surfaceId,
      "data-surface-type": kind, "data-resource-key": owner }, portal ?? tabpanel);
    const semantic = element({ "data-testid": kind === "agents-overview" ? "agents-overview-surface" : "agent-session-surface",
      "data-surface-id": surfaceId, "data-resource-key": owner }, panel);
    const root = kind === "agents-overview" ? element({ "data-agent-grid-card-id": owner }, semantic) : semantic;
    root.querySelector = (selector) => selector.includes("Switch to Terminal") ? (chatMode ? {} : null)
      : selector.includes("Switch to Chat") ? (chatMode ? null : {})
        : !shown || !chatMode ? null : selector.includes("scroll-region") ? scroll
          : selector.includes("textarea") ? composers[0] ?? null : null;
    root.querySelectorAll = (selector = "") => !shown || !chatMode ? [] : selector.includes("textarea") ? composers : candidates;
    root.findElement = async (selector) => {
      assert.equal(selector, "ancestor::*[@role='tabpanel'][1]");
      return root.closest('[role="tabpanel"]');
    };
    const close = element({ "data-tab-close": "" }, tab);
    close.click = async () => { tab.closed = true; tab.isConnected = false; tabpanel.isConnected = false; root.isConnected = false; };
    tab.findElements = async (selector) => close.matches(selector) ? [close] : [];
    const route = { root, semantic, panel, tabpanel, tab, host, portal, dockRoot, kind };
    routes.push(route);
    return route;
  };
  let route = addContainer(options.route);
  row.parentElement = scroll;
  content.parentElement = row;
  scroll.parentElement = route.root;
  const composer = (settings = {}) => Object.assign(element({ "aria-label": "Message agent" }, route.root), {
    value: "", disabled: false, readOnly: false,
    getBoundingClientRect: () => ({ top: 400, bottom: 440, left: 10, right: 400, width: 390, height: 40 }), ...settings,
  });
  const document = { documentElement: {}, activeElement: null, visibilityState: "visible",
    querySelectorAll: (selector) => {
      const nodes = selector.includes('role="tab"') ? routes.map((item) => item.tab)
        : selector.includes('data-testid="workbench-host"') ? [host] : routes.map((item) => item.root);
      return nodes.filter((node) => selector.split(",").some((part) => node.matches(part)));
    },
    getElementById: (id) => routes.flatMap((item) => [item.tab, item.tabpanel])
      .find((node) => node.getAttribute("id") === id) ?? null,
    addEventListener: (type, callback) => listeners.set(type, callback),
    removeEventListener: (type, callback) => { if (listeners.get(type) === callback) listeners.delete(type); },
    createTreeWalker: (root) => {
      let offset = 0;
      const nodes = (options.textSegments ?? [root.textContent]).map((text) => {
        const node = { textContent: text, parentElement: root, offset };
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
          : start.node.parentElement.textContent.slice(start.node.offset + start.offset, end.node.offset + end.offset),
        getClientRects: () => [whole ? rect : needleRect ?? rect] };
    } };
  const bridge = createChatPageObservation();
  let disconnected = false;
  const context = { window: { __TAURI_INTERNALS__: { invoke },
    __wardianChatPageObservation: Object.hasOwn(options, "facade") ? options.facade : bridge.facade }, document,
    performance: { now: () => clock }, innerHeight: 500, innerWidth: 500,
    setTimeout: (callback, ms) => { const id = ++nextTimer; timers.set(id, { callback, at: clock + ms }); return id; },
    clearTimeout: (id) => timers.delete(id),
    MutationObserver: class { observe() {} disconnect() { disconnected = true; } }, requestAnimationFrame: (callback) => frames.push(callback) };
  if (options.immutableInvoke) Object.defineProperty(context.window.__TAURI_INTERNALS__, "invoke", {
    value: invoke, writable: false, configurable: false,
  });
  vm.runInNewContext(`${source.slice(start, end)}\ninstallChatObservation(config);`,
    { ...context, TextEncoder, getComputedStyle: (node) => node.style ?? { visibility: "visible", display: "block", opacity: "1" },
      config: { name: "Owned", body: rawBody, witness: options.witness, probe: "native-input-probe" } });
  const observer = context.window.__chatAcceptance;
  observer.bind("session");
  observer.arm("cold");
  return {
    observer, document, content, row, scroll, route, window: context.window, bridge,
    timerCount: () => timers.size,
    expire() { for (const [id, timer] of [...timers]) { clock = Math.max(clock, timer.at); timers.delete(id); timer.callback(); } },
    duplicate(id) {
      const twinRow = { isConnected: true, parentElement: scroll,
        getAttribute: (name) => name === "data-chat-row-key" ? id
          : name === "data-chat-row-identity" ? JSON.stringify({ id, members: null }) : null };
      return { textContent: content.textContent, isConnected: true, parentElement: twinRow,
        getAttribute: () => null, closest: () => twinRow };
    },
    candidates(value) { candidates = value; },
    composer(settings) { composers = settings === null ? [] : [composer(settings)]; return composers[0] ?? null; },
    composers(settings) { composers = settings.map(composer); return composers; },
    listenerCount: () => listeners.size, disconnected: () => disconnected,
    wrapper(...args) {
      const result = Reflect.apply(context.window.__TAURI_INTERNALS__.invoke, this, args);
      if (args[0] === "load_agent_chat_page") bridge.notifyRequest(args[1], result);
      return result;
    },
    addContainer,
    replaceContainer() {
      route.root.isConnected = false;
      route = addContainer(route.kind);
      for (const candidate of candidates) candidate.closest().parentElement = scroll;
      for (const input of composers) input.parentElement = route.root;
      scroll.parentElement = route.root;
      return route;
    },
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
      const target = composer({ owned, value: "native-input-probe" });
      if (owned) composers = [target];
      document.activeElement = target;
      listeners.get("input")({ isTrusted: trusted, target });
      return target;
    },
    wheel(trusted, nextTop, timing = "after-handler") {
      // A real driver can read this baseline before dispatch; old observation
      // has no preparation boundary and instead samples after compositor scroll.
      observer.prepareInteraction?.("session", scroll);
      if (timing === "before-handler") scroll.scrollTop = nextTop;
      listeners.get("wheel")({ isTrusted: trusted, target: scroll, deltaY: -20 });
      if (timing === "after-handler") scroll.scrollTop = nextTop;
    },
    menu(owned = true) {
      const button = { owned, textContent: "Message actions" };
      listeners.get("click")({ isTrusted: true,
        target: { closest: (selector) => selector === "button" ? button : null } });
    },
    frame() { clock += 16; const pending = frames; frames = []; for (const callback of pending) callback(); },
  };
}

/** Run the actual maintained driver selector against the same observer/DOM. */
function actualSurfaceOperations(harness) {
  const source = fs.readFileSync(new URL("./chat-turn-change-native.test.mjs", import.meta.url), "utf8");
  const start = source.indexOf("async function visible(driver, selector");
  const end = source.indexOf("/** Wait only for the real raw producer", start);
  const closeStart = source.indexOf("async function closeTerminalSurface(driver,");
  const closeEnd = source.indexOf("function canonicalHistoryReceipts(", closeStart);
  assert.ok(start >= 0 && end > start && closeStart >= 0 && closeEnd > closeStart);
  const driver = {
    findElements: async (selector) => harness.document.querySelectorAll(selector),
    findElement: async (id) => harness.document.getElementById(id),
    executeScript: async (script, ...args) => script(...args),
    wait: async (predicate, timeout) => {
      assert.equal(timeout, 10_000);
      const value = await predicate();
      if (!value) throw new Error("owned container unavailable");
      return value;
    },
  };
  return vm.runInNewContext(`${source.slice(start, end)}\n${source.slice(closeStart, closeEnd)}
    ({ lookup: () => ownedSurface(driver, "session"), close: () => closeTerminalSurface(driver, "session") });`,
    { driver, assert, window: harness.window, By: { css: (selector) => selector, xpath: (selector) => selector, id: (id) => id } });
}

const actualOwnedSurface = (harness) => actualSurfaceOperations(harness).lookup();

test("the maintained observer witnesses original page calls with an immutable native invoke descriptor", async () => {
  const original = () => Promise.resolve(actualPage());
  const harness = observationHarness(original, { immutableInvoke: true });
  const descriptor = Object.getOwnPropertyDescriptor(harness.window.__TAURI_INTERNALS__, "invoke");
  assert.equal(descriptor.writable, false);
  assert.equal(descriptor.configurable, false);
  assert.equal(harness.window.__TAURI_INTERNALS__.invoke, original);
  await harness.wrapper("load_agent_chat_page", { sessionId: "session" });
  assert.equal(harness.observer.state.calls.length, 1);
  assert.equal(harness.observer.state.receipts.length, 1);
  assert.equal(harness.observer.state.activation.owner, "session");
  assert.equal(harness.observer.subscriptionStatus().active, true);
});

test("page-only source routing inventory is separate from broader unobserved runtime IPC", () => {
  const root = fileURLToPath(new URL("../../src/", import.meta.url));
  const files = fs.readdirSync(root, { recursive: true }).filter((name) => /\.[jt]sx?$/.test(name)
    && !/\.test\.[jt]sx?$/.test(name) && !name.replaceAll("\\", "/").startsWith("test/"));
  const sources = files.map((name) => ({ name: name.replaceAll("\\", "/"), text: fs.readFileSync(path.join(root, name), "utf8") }));
  assert.deepEqual(sources.filter(({ text }) => /["']load_agent_chat_page["']/.test(text)).map(({ name }) => name),
    ["features/chat/chatPageLoader.ts"]);
  assert.deepEqual(sources.filter(({ text }) => /["']load_agent_chat_transcript["']/.test(text)), []);
  const view = sources.find(({ name }) => name === "features/grid/AgentChatView.tsx").text;
  assert.match(view, /import \{ loadChatPage \} from "\.\.\/chat\/chatPageLoader"/);
  assert.match(view, /useChatPages\(sessionId, loadChatPage,/);
  const main = sources.find(({ name }) => name === "main.tsx").text;
  assert.ok(main.indexOf("installChatPageObservation(window)") < main.indexOf("ReactDOM.createRoot"));
});

test("observer activation fails explicitly for missing, incompatible, duplicate and wrong-owner subscriptions", () => {
  const original = () => Promise.resolve(actualPage());
  for (const facade of [null, { version: 2 }]) {
    assert.throws(() => observationHarness(original, { facade }), /unavailable\/incompatible/);
  }
  const bridge = createChatPageObservation();
  bridge.facade.subscribe({ version: 1, owner: "session", onRequest() {} });
  assert.throws(() => observationHarness(original, { facade: bridge.facade }), /activation failed/);
  let released = false;
  assert.throws(() => observationHarness(original, { facade: { version: 1, subscribe: () => ({
    active: true, version: 1, owner: "foreign", token: 1, status: () => ({ active: true }),
    unsubscribe() { released = true; },
  }) } }), /activation failed/);
  assert.equal(released, true);
});

test("subscriber faults are explicit while the original product Promise survives", async () => {
  const original = Promise.resolve(actualPage());
  const harness = observationHarness(() => original);
  const args = { sessionId: "session", get cursor() { throw new Error("observer metadata fault"); } };
  assert.equal(harness.wrapper("load_agent_chat_page", args), original);
  await original;
  assert.equal(harness.observer.subscriptionStatus().fault_count, 1);
  assert.throws(() => harness.observer.arm("broken"), /not active\/healthy/);
});

test("disposal retires pending settlements, queued frames and DOM listeners without removing a newer subscription", async () => {
  for (const rejects of [false, true]) {
    let resolve; let reject;
    const pending = new Promise((yes, no) => { resolve = yes; reject = no; });
    let calls = 0;
    const harness = observationHarness(() => ++calls === 1 ? Promise.resolve(actualPage()) : pending);
    harness.mode("chat"); harness.roster();
    await harness.wrapper("load_agent_chat_page", { sessionId: "session" });
    harness.observer.sample(); harness.frame();
    assert.equal(harness.observer.state.phase.checking, true);
    assert.equal(harness.observer.state.phase.paintAt, null);
    const result = harness.wrapper("load_agent_chat_page", { sessionId: "session" });
    harness.observer.dispose();
    const snapshot = JSON.stringify(harness.observer.state);
    assert.equal(harness.disconnected(), true);
    assert.equal(harness.listenerCount(), 0);
    const next = harness.bridge.facade.subscribe({ version: 1, owner: "session", onRequest() {} });
    assert.equal(next.active, true);
    harness.observer.dispose();
    assert.equal(next.status().active, true);
    if (rejects) { const assertion = assert.rejects(result, /original/); reject(new Error("original")); await assertion; }
    else { resolve(actualPage()); await result; }
    harness.observer.sample(); harness.frame(); harness.frame();
    assert.equal(JSON.stringify(harness.observer.state), snapshot);
    assert.throws(() => harness.observer.bind("session"), /after disposal/);
    next.unsubscribe();
  }
});

test("the maintained oracle and close helper resolve real Dockview overlay placement through its current tab", async () => {
  for (const route of ["agents-overview", "agent-session"]) {
    const harness = observationHarness(() => Promise.resolve(actualPage()), { route, portal: true });
    assert.equal(harness.route.root.closest('[role="tabpanel"]'), null);
    assert.equal(await actualOwnedSurface(harness), harness.route.root);
    assert.equal(harness.observer.containerTab("session"), harness.route.tab);
    harness.mode("chat"); harness.clock(100); harness.roster();
    harness.clock(110); await harness.wrapper("load_agent_chat_page", { sessionId: "session" });
    harness.observer.sample(); harness.frame(); harness.frame();
    assert.equal(harness.observer.state.phase.rosterAt, 100);
    assert.equal(harness.observer.state.phase.paintAt, 142);
    harness.input(); harness.frame(); harness.frame();
    assert.equal(harness.observer.state.phase.usableAt, 174);
    await actualSurfaceOperations(harness).close();
    assert.equal(harness.route.tab.closed, true);
  }
});

test("Dockview portal placement still requires the same root and bidirectional current-tab ownership", async () => {
  for (const change of [
    (h) => { h.route.tabpanel.attributes["aria-labelledby"] = "foreign-tab"; },
    (h) => { h.route.tab.attributes["aria-controls"] = "foreign-panel"; },
    (h) => { h.route.tab.attributes["aria-selected"] = "false"; },
    (h) => { h.route.portal.parentElement = h.route.host; },
    (h) => { h.route.tabpanel.parentElement = h.route.host; },
    (h) => { h.route.tab.parentElement = h.route.host; },
    (h) => { h.route.portal.style.visibility = "hidden"; },
    (h) => { h.route.root.attributes["data-agent-grid-card-id"] = "foreign-owner"; },
  ]) {
    const harness = observationHarness(() => Promise.resolve(actualPage()), { route: "agents-overview", portal: true });
    change(harness);
    await assert.rejects(actualOwnedSurface(harness), /owned container unavailable/);
    assert.equal(harness.observer.containerTab("session"), null);
  }
});

test("normal Roster Overview and actual AgentSession share the maintained driver and observer container", async () => {
  for (const route of ["agents-overview", "agent-session"]) {
    const harness = observationHarness(() => Promise.resolve(actualPage()), { route });
    assert.equal(await actualOwnedSurface(harness), harness.route.root);
    harness.mode("chat"); harness.clock(100); harness.roster();
    harness.clock(110); await harness.wrapper("load_agent_chat_page", { sessionId: "session" });
    harness.observer.sample(); harness.frame(); harness.frame();
    assert.equal(harness.observer.state.phase.rosterAt, 100);
    assert.equal(harness.observer.state.phase.paintAt, 142);
    harness.input(); harness.frame(); harness.frame();
    assert.equal(harness.observer.state.phase.usableAt, 174);
  }
});

test("maintained surface oracle rejects foreign hidden detached stale and ambiguous containers", async () => {
  for (const route of ["agents-overview", "agent-session"]) {
    for (const change of [
      (h) => { h.route.root.attributes[route === "agents-overview" ? "data-agent-grid-card-id" : "data-resource-key"] = "foreign"; },
      (h) => { h.route.root.style.visibility = "hidden"; },
      (h) => { h.route.root.hidden = true; },
      (h) => { h.document.visibilityState = "hidden"; },
      (h) => { h.route.root.isConnected = false; },
      (h) => { h.route.tab.attributes["aria-selected"] = "false"; },
      (h) => { h.route.tab.attributes["aria-controls"] = "stale-panel"; },
      (h) => { h.route.panel.attributes["data-surface-id"] = "foreign-surface"; },
      (h) => { h.addContainer(route); },
      (h) => { h.route.host.attributes["data-reset-pending"] = "true"; },
    ]) {
      const harness = observationHarness(() => Promise.resolve(actualPage()), { route });
      change(harness);
      await assert.rejects(actualOwnedSurface(harness));
      harness.click(); await harness.wrapper("load_agent_chat_page", { sessionId: "session" });
      harness.observer.sample(); harness.frame(); harness.frame();
      assert.equal(harness.observer.state.phase.paintAt, null);
    }
  }
});

test("a changed container cannot reuse body and scroll nodes across either two-frame witness", async () => {
  for (const boundary of ["paint", "input"]) {
    const harness = observationHarness(() => Promise.resolve(actualPage()));
    harness.click(); await harness.wrapper("load_agent_chat_page", { sessionId: "session" });
    harness.observer.sample();
    if (boundary === "input") { harness.frame(); harness.frame(); harness.input(); }
    harness.frame(); harness.replaceContainer(); harness.frame();
    assert.equal(harness.observer.state.phase[boundary === "paint" ? "paintAt" : "usableAt"], null);
  }
});

test("the maintained close route uses the actual parent tab for Overview and AgentSession before normal reopen", async () => {
  for (const route of ["agents-overview", "agent-session"]) {
    const harness = observationHarness(() => Promise.resolve(actualPage()), { route });
    const operations = actualSurfaceOperations(harness);
    await operations.close();
    assert.equal(harness.route.tab.closed, true);
    const reopened = harness.replaceContainer();
    assert.equal(await operations.lookup(), reopened.root);
    harness.observer.arm("warm"); harness.mode("chat"); harness.clock(1000); harness.roster();
    await harness.wrapper("load_agent_chat_page", { sessionId: "session" });
    harness.observer.sample(); harness.frame(); harness.frame();
    assert.equal(harness.observer.state.phase.rosterAt, 1000);
    assert.equal(harness.observer.state.phase.paintAt, 1032);
  }
});

function actualPage(overrides = {}) {
  return { session_id: "session", conversation_id: "conversation", generation: "generation", source_epoch: "epoch",
    revision: "opaque-revision", recent_admission: null, next_before: null, progress: "ready",
    events: [{ id: "public-id", session_id: "session", provider: "mock", kind: "message", role: "assistant",
      turn_id: chatAcceptanceFixture().latestTurn, text: chatAcceptanceFixture().latestBody, metadata: {} }],
    ...overrides };
}

function actualPreviewPage(overrides = {}) {
  return actualPage({ conversation_id: null, generation: null, source_epoch: null,
    recent_admission: "preview-owner", progress: "provisional", reset: false, unchanged: false,
    older_page: false, aliases: [], removed_ids: [], revalidated_ids: null, detail: null, ...overrides });
}

test("the actual observer paints an admitted recent preview with null canonical IDs", async () => {
  const page = actualPreviewPage();
  const harness = observationHarness(() => Promise.resolve(page));
  harness.admitted({ conversation: null, generation: null, epoch: null, admission: page.recent_admission });
  harness.click();
  await harness.wrapper("load_agent_chat_page", { sessionId: "session" });
  harness.observer.sample(); harness.frame(); harness.frame();
  assert.notEqual(harness.observer.state.phase.paintAt, null, "legal admission-backed preview must qualify paint");
  harness.input(); harness.frame(); harness.frame();
  assert.notEqual(harness.observer.state.phase.usableAt, null);
  assert.equal(harness.observer.state.phase.body.generation, null);
  assert.equal(harness.observer.state.phase.body.conversation, null);
  assert.equal(harness.observer.state.phase.body.admission, "preview-owner");
  assert.equal(harness.observer.state.phase.body.scopeMode, "recent-preview");
});

test("recent preview preserves every actual nullable canonical hint without fabricating a durable scope", async () => {
  for (const conversation of [null, "conversation-hint"]) for (const generation of [null, "generation-hint"])
    for (const epoch of [null, "epoch-hint"]) {
      const page = actualPreviewPage({ conversation_id: conversation, generation, source_epoch: epoch });
      const harness = observationHarness(() => Promise.resolve(page));
      harness.admitted({ conversation, generation, epoch, admission: page.recent_admission });
      harness.click(); await harness.wrapper("load_agent_chat_page", { sessionId: "session" });
      harness.observer.sample(); harness.frame(); harness.frame();
      const body = harness.observer.state.phase.body;
      assert.ok(body);
      assert.equal(body.conversation, conversation); assert.equal(body.generation, generation); assert.equal(body.epoch, epoch);
      assert.equal(body.scopeMode, "recent-preview");
      assert.equal(actualCanonicalHistory(harness.observer.state.receipts).length, 0, "preview cannot grant Older/detail renewal authority");
    }
});

test("a matching empty unchanged preview retains the original current target receipt", async () => {
  let page = actualPreviewPage();
  const harness = observationHarness(() => Promise.resolve(page));
  harness.admitted({ conversation: null, generation: null, epoch: null, admission: page.recent_admission });
  harness.click(); await harness.wrapper("load_agent_chat_page", { sessionId: "session" });
  harness.observer.sample(); harness.frame(); harness.frame();
  const originalReceipt = harness.observer.state.phase.body.receiptAt;
  page = actualPreviewPage({ unchanged: true, events: [] });
  harness.clock(200); await harness.wrapper("load_agent_chat_page", { sessionId: "session", revision: page.revision });
  harness.observer.sample(); harness.input(); harness.frame(); harness.frame();
  assert.notEqual(harness.observer.state.phase.usableAt, null);
  assert.equal(harness.observer.state.phase.body.receiptAt, originalReceipt);
  assert.equal(harness.observer.state.receipts.length, 2);
  assert.equal(actualCanonicalHistory(harness.observer.state.receipts).length, 0);
});

test("preview scope requires exact owner admission revision and nullable hints in the admitted DOM", async () => {
  const page = actualPreviewPage();
  for (const mismatch of [{ session: "foreign" }, { admission: null }, { admission: undefined }, { admission: "" },
    { admission: "other-preview" }, { revision: "other-window" }, { conversation: "different" },
    { generation: "different" }, { epoch: "different" }, { generation: undefined }]) {
    const harness = observationHarness(() => Promise.resolve(page));
    harness.admitted({ conversation: null, generation: null, epoch: null, admission: page.recent_admission, ...mismatch });
    harness.click(); await harness.wrapper("load_agent_chat_page", { sessionId: "session" });
    harness.observer.sample(); harness.frame(); harness.frame();
    assert.equal(harness.observer.state.phase.paintAt, null);
  }
});

test("malformed preview capability cannot supply an observer receipt", async () => {
  const normal = actualPreviewPage();
  const event = normal.events[0];
  for (const invalid of [{ progress: "ready" }, { next_before: "older" }, { detail: {} }, { older_page: true },
    { revalidated_ids: [] }, { aliases: [{ observation_id: "a", canonical_id: "b" }] }, { removed_ids: ["removed"] },
    { reset: undefined }, { unchanged: undefined }, { conversation_id: " " }, { generation: undefined },
    { source_epoch: undefined }, { recent_admission: "" }, { recent_admission: "x".repeat(257) },
    { events: [event, { ...event }] }, { events: [{ ...event, session_id: "foreign" }] },
    { events: [{ ...event, text: "é".repeat(513) }] },
    { events: Array.from({ length: 81 }, (_, index) => ({ ...event, id: `preview-${index}` })) }]) {
    const page = actualPreviewPage(invalid);
    const harness = observationHarness(() => Promise.resolve(page));
    harness.admitted({ conversation: page.conversation_id, generation: page.generation, epoch: page.source_epoch,
      admission: page.recent_admission });
    harness.click(); await harness.wrapper("load_agent_chat_page", { sessionId: "session" });
    harness.observer.sample(); harness.frame(); harness.frame();
    assert.equal(harness.observer.state.phase.paintAt, null);
    assert.equal(harness.observer.state.receipts.length, 0);
    assert.equal(harness.observer.state.errors.length, 1);
  }
});

test("preview admission or mode changes cannot qualify a pending or painted subject", async () => {
  for (const change of [{ admission: "new-preview" }, { revision: "new-window" }, { generation: "new-hint" },
    { admission: null, conversation: "canonical-conversation", generation: "canonical-generation", epoch: "canonical-epoch" }]) {
    for (const painted of [false, true]) {
      const page = actualPreviewPage();
      const harness = observationHarness(() => Promise.resolve(page));
      harness.admitted({ conversation: null, generation: null, epoch: null, admission: page.recent_admission });
      harness.click(); await harness.wrapper("load_agent_chat_page", { sessionId: "session" });
      harness.observer.sample();
      if (painted) { harness.frame(); harness.frame(); }
      harness.admitted(change);
      harness.input(); harness.frame(); harness.frame();
      if (!painted) assert.equal(harness.observer.state.phase.paintAt, null);
      assert.equal(harness.observer.state.phase.usableAt, null);
    }
  }
});

test("failure-only candidate facts preserve actual preview scope and the first rejected gate without endpoints or body text", async () => {
  const page = actualPreviewPage();
  const harness = observationHarness(() => Promise.resolve(page));
  harness.admitted({ conversation: null, generation: null, epoch: null, admission: page.recent_admission });
  harness.click(); await harness.wrapper("load_agent_chat_page", { sessionId: "session" });
  const valid = harness.observer.candidateStatus();
  assert.equal(valid.scopeMode, "recent-preview"); assert.equal(valid.actualScope.generation, null);
  assert.equal(valid.candidateValid, true); assert.equal(valid.firstRejectionGate, null);
  assert.equal(valid.targetPresent, true); assert.equal(valid.targetIndex, 0); assert.equal(valid.candidateIndex, 0);
  assert.equal(valid.range.intersects, true); assert.equal(valid.range.count, 1);
  assert.equal(valid.scroll.top, 100); assert.equal(valid.scroll.clientHeight, 500);
  assert.equal(harness.observer.state.phase.paintAt, null, "diagnostics cannot set an endpoint");
  harness.admitted({ generation: undefined });
  const invalid = harness.observer.candidateStatus();
  assert.equal(invalid.scopeMode, "invalid"); assert.equal(invalid.firstRejectionGate, "admitted-scope");
  assert.equal(invalid.scopeFieldsPresent.generation, false);
  harness.admitted({ generation: null });
  harness.rectangle({ top: -80, bottom: -20, left: 0, right: 400, width: 400, height: 60 });
  const outside = harness.observer.candidateStatus();
  assert.equal(outside.firstRejectionGate, "target-viewport"); assert.equal(outside.range.intersects, false);
  assert.equal(harness.observer.state.calls.length, 1);
  const redacted = actualRedaction(outside);
  assert.equal(redacted.scopeMode, "recent-preview"); assert.equal(redacted.firstRejectionGate, "target-viewport");
  assert.equal(redacted.actualScope.admission, createHash("sha256").update("preview-owner").digest("hex"));
  assert.equal(JSON.stringify(redacted).includes(chatAcceptanceFixture().latestBody), false);
  assert.equal(harness.observer.state.phase.paintAt, null);
});

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

/** Execute the maintained failure callback without importing its native case. */
function rosterDiagnosticHarness(invoke, roster = null, observer = null) {
  const source = fs.readFileSync(new URL("./chat-turn-change-native.test.mjs", import.meta.url), "utf8");
  const start = source.indexOf("function captureRosterDiagnostics(sessionId, name, done) {");
  const end = source.indexOf("async function visible(driver, selector", start);
  assert.ok(start >= 0 && end > start);
  let expire;
  let completions = 0;
  const result = new Promise((resolve) => {
    vm.runInNewContext(`${source.slice(start, end)}\ncaptureRosterDiagnostics("session", "Owned Saved", done);`, {
      window: { __TAURI_INTERNALS__: { invoke }, __chatAcceptance: observer,
        getComputedStyle: () => ({ display: "block", visibility: "visible" }) },
      document: { querySelector: () => roster },
      done: (value) => { completions += 1; resolve(value); },
      setTimeout: (callback, ms) => { assert.equal(ms, 2_000); expire = callback; return 1; },
      clearTimeout: () => {},
    });
  });
  return { result, expire: () => expire(), completions: () => completions };
}

function rosterMeasurement(driver, evidence, visible, ownedSurface, extensions = {}) {
  const source = fs.readFileSync(new URL("./chat-turn-change-native.test.mjs", import.meta.url), "utf8");
  const start = source.indexOf("function waitForChatEndpoint(binding,");
  const end = source.indexOf("async function closeTerminalSurface(driver,", start);
  assert.ok(start >= 0 && end > start);
  return vm.runInNewContext(`${source.slice(start, end)}\nmeasureChat(driver, "session", "test-phase", "Owned Saved", evidence, requireTerminalToggle);`,
    { driver, evidence, visible, ownedSurface, performance, captureRosterDiagnostics() {}, assert, requireTerminalToggle: false, ...extensions });
}

test("passive timing retains the actual command trace, result and rejected command object", async (t) => {
  for (const scenario of [null, "wheel-success", "remaining-budget", "controller-expiry", "foreign-ack", "paintWait", "scrollLookup", "prepareInteraction", "inputDisplayed", "inputEnabled",
    "inputValue", "sendKeys", "usableWait", "wheelPerform", "readObservation", "timeout", "clock", "sink"]) {
    const failure = ["wheel-success", "remaining-budget"].includes(scenario) ? null : scenario;
    await t.test(scenario ?? "keyboard-success", async () => {
      const original = new Error("original command rejection");
      const trace = [];
      let clock = 0;
      let wallClock = 100;
      let secondFailure = false;
      const evidence = { rosterFailures: [] };
      if (failure === "sink") Object.defineProperty(evidence, "commandTiming", { set() { throw new Error("diagnostic sink unavailable"); } });
      const binding = { owner: "session", version: 1, token: 1, ordinal: 1, label: "test-phase" };
      const phase = { label: "test-phase", ordinal: 1, rosterAt: 0, chatAt: null, exposureAt: 5, exposureKind: "automatic",
        commitAt: 10, paintAt: 20, usableAt: 30, body: { receiptAt: 4 } };
      const state = { owner: "session", phase, phases: [phase], errors: [], subscription: { active: true, fault_count: 0 },
        activation: { active: true, version: 1, token: 1, owner: "session", at: 0 }, coverage: "PAGE_LOADER_ONLY", broaderIpcRuntime: "UNOBSERVED",
        calls: [{ session: "session", at: 1 }], receipts: [{ session: "session", requestSession: "session", at: 4 }] };
      const command = (step, value) => {
        trace.push(step);
        if (secondFailure && step === (wheel ? "wheelPerform" : "sendKeys")) throw original;
        if (failure === step || (["clock", "sink"].includes(failure) && step === "sendKeys")) throw original;
        if (scenario === "remaining-budget" && step === "sendKeys") wallClock += 80;
        return value;
      };
      const input = { isDisplayed: async () => command("inputDisplayed", true),
        isEnabled: async () => command("inputEnabled", true),
        getAttribute: async (key) => { assert.equal(key, "value"); return command("inputValue", ""); },
        sendKeys: async (...keys) => { assert.deepEqual(keys, ["native-input-probe"]); command("sendKeys"); } };
      const surface = {};
      const scroll = {};
      const wheel = scenario === "wheel-success" || failure === "wheelPerform";
      const driver = {
        executeScript: async (fn, ...args) => {
          if (fn.name === "captureChatFailureMetadata") {
            trace.push("failureCapture"); throw new Error("secondary capture rejection");
          }
          if (args.length === 1) { assert.deepEqual(args, ["test-phase"]); return command("arm", binding); }
          assert.deepEqual(args, [scroll, "session"]);
          return command("prepareInteraction", { control: wheel ? "wheel" : "keyboard", input: wheel ? null : input,
            top: 100, plannedAtPage: 24 });
        },
        executeAsyncScript: async (fn, actualBinding, field, budget) => {
          assert.equal(fn.name, "waitForChatEndpoint"); assert.equal(actualBinding, binding);
          assert.ok(["paintAt", "usableAt"].includes(field));
          assert.equal(budget, scenario === "remaining-budget" && field === "usableAt" ? 4720 : 5000);
          command(field === "paintAt" ? "paintWait" : "usableWait");
          if (failure === "timeout") throw original;
          if (scenario === "remaining-budget") wallClock += 200;
          if (scenario === "controller-expiry") wallClock += 5000;
          return { status: "ACCEPTED", ...binding, token: scenario === "foreign-ack" ? 2 : binding.token,
            field, endpointAt: phase[field], captureAtPage: 40 };
        },
        wait: () => { throw new Error("endpoint polling must not run"); },
        actions: (options) => {
          assert.deepEqual({ ...options }, { async: true });
          return { scroll: (...args) => {
            assert.deepEqual(args, [0, 0, 0, -20, scroll]);
            return { perform: async () => command("wheelPerform") };
          } };
        },
      };
      const visible = async (actualDriver, selector, root) => {
        assert.equal(actualDriver, driver);
        if (selector === '[aria-label="Agent Owned Saved"]') {
          assert.equal(root, undefined); command("rosterLookup"); return { click: async () => command("rosterClick") };
        }
        assert.equal(root, surface);
        if (selector === 'button[title="Switch to Chat"], button[title="Switch to Terminal"]') {
          command("modeLookup"); return { getAttribute: async (key) => {
            assert.equal(key, "title"); return command("modeTitle", "Switch to Terminal");
          } };
        }
        assert.equal(selector, '[data-testid="agent-chat-scroll-region"]'); return command("scrollLookup", scroll);
      };
      const result = rosterMeasurement(driver, evidence, visible, async (...args) => {
        assert.deepEqual(args, [driver, "session"]); return command("ownedSurface", surface);
      }, { Date: { now: () => wallClock },
        // WebDriver JSON arrays enter the controller realm in production. The
        // extracted VM otherwise gives its empty expected array a foreign brand.
        assert: { ...assert, deepEqual: (actual, expected, message) => assert.deepEqual(Array.from(actual), Array.from(expected), message) },
        performance: { now: () => {
        if (failure === "clock") throw new Error("diagnostic clock unavailable");
        return ++clock;
      } }, captureChatFailureMetadata() {}, readObservation: async () => {
        command("readObservation");
        return state;
      } });
      if (failure) await assert.rejects(result, (error) => failure === "controller-expiry" ? /original deadline/.test(error.message)
        : failure === "foreign-ack" ? /binding failed/.test(error.message) : error === original);
      else assert.equal(await result, phase, "return the original phase object");
      const expected = ["arm", "rosterLookup", "rosterClick", "ownedSurface", "modeLookup", "modeTitle",
        "paintWait", "scrollLookup", "prepareInteraction",
        ...(wheel ? ["wheelPerform"] : ["inputDisplayed", "inputEnabled", "inputValue", "sendKeys"]),
        "usableWait", "readObservation"];
      if (failure) {
        const step = ["clock", "sink"].includes(failure) ? "sendKeys"
          : ["controller-expiry", "foreign-ack"].includes(failure) ? "paintWait" : failure;
        const prefix = failure === "timeout" ? expected.slice(0, expected.indexOf("scrollLookup")) : expected.slice(0, expected.indexOf(step) + 1);
        assert.deepEqual(trace, [...prefix, "failureCapture"]);
      } else assert.deepEqual(trace, expected);
      if (failure === "sink") return;
      const timing = evidence.commandTiming;
      assert.ok(timing, "retain bounded partial command timing even on rejection");
      if (failure === "clock") assert.equal(timing.spans.sendKeys.finishedAt, null);
      else {
        const span = timing.spans[["readObservation", "timeout", "controller-expiry", "foreign-ack"].includes(failure) ? "paintWait" : failure ?? (wheel ? "wheelPerform" : "sendKeys")];
        assert.ok(Number.isFinite(span.startedAt) && Number.isFinite(span.finishedAt));
        assert.equal(span.durationMs, span.finishedAt - span.startedAt);
      }
      if (["readObservation", "timeout"].includes(failure)) {
        assert.equal(timing.waits.paintAt.attempts, 1);
        assert.equal(timing.waits.paintAt.timedAttempts, timing.waits.paintAt.attempts);
        assert.ok(timing.waits.paintAt.elapsedMs > 0, "finally includes rejected wait work");
      }
      if (!failure) {
        assert.equal(phase.controllerTiming, timing);
        assert.equal(timing.endpointObservation, "EVENT_NOTIFICATION");
        assert.equal(timing.waits.paintAt.attempts, 1); assert.equal(timing.waits.usableAt.attempts, 1);
        assert.equal(timing.waits.paintAt.timedAttempts, 1); assert.ok(timing.waits.paintAt.elapsedMs > 0);
        assert.equal(timing.page.plannedAtPage, 24); assert.equal(timing.page.paintCaptureAtPage, 40);
        assert.equal(timing.spans[wheel ? "sendKeys" : "wheelPerform"].startedAt, null);
        assert.deepEqual(JSON.parse(JSON.stringify(actualRedaction(timing))), JSON.parse(JSON.stringify(timing)), "fixed scalar diagnostics survive one redaction");
        assert.equal(evidence.phaseTimings["test-phase"].controllerTiming, timing, "retain success before returning to the next phase");
        secondFailure = true;
        await assert.rejects(rosterMeasurement(driver, evidence, visible, async () => surface, {
          Date: { now: () => wallClock }, readObservation: async () => state,
          captureChatFailureMetadata() {},
          assert: { ...assert, deepEqual: (actual, expected, message) => assert.deepEqual(Array.from(actual), Array.from(expected), message) },
        }), (error) => error === original);
        assert.equal(evidence.phaseTimings["test-phase"].controllerTiming, timing, "a later failure must not erase successful timing");
        assert.notEqual(evidence.commandTiming, timing, "retain the second phase's partial current timing separately");
        assert.equal(Object.keys(evidence.phaseTimings).length, 1);
      }
    });
  }
});

test("passive timing observation capture shares its original RPC and preserves rejection identity", async () => {
  const source = fs.readFileSync(new URL("./chat-turn-change-native.test.mjs", import.meta.url), "utf8");
  const start = source.indexOf("async function readObservation(driver) {");
  const end = source.indexOf("/** Return the acknowledgement", start);
  assert.ok(start >= 0 && end > start);
  const phase = { label: "cold" };
  const state = { phase, disposed: false };
  let calls = 0;
  let clockFails = false;
  const subscription = { active: true };
  const read = vm.runInNewContext(`${source.slice(start, end)}\nreadObservation;`, {
    window: { __chatAcceptance: { state, subscriptionStatus: () => subscription } },
    performance: { now: () => { if (clockFails) throw new Error("clock failed"); return 42; } },
  });
  const driver = { executeScript: async (fn, ...args) => { calls++; assert.deepEqual(args, []); return fn(); } };
  const observation = await read(driver);
  assert.equal(calls, 1); assert.equal(observation.phase, phase); assert.equal(observation.subscription, subscription);
  assert.equal(observation.captureAtPage, 42);
  assert.equal(Object.hasOwn(state, "captureAtPage"), false, "capture is appended to the existing result, not mutable observer state");
  clockFails = true; assert.equal((await read(driver)).captureAtPage, null);
  clockFails = false; state.disposed = true; assert.equal((await read(driver)).captureAtPage, null);
  const original = new Error("original RPC rejection");
  await assert.rejects(read({ executeScript: () => Promise.reject(original) }), (error) => error === original);
});

test("passive timing page stamps use only the accepted full probe and existing two frames", async (t) => {
  for (const ending of ["keyboard", "wheel", "wheel-event-first", "reset", "rearm", "disposed"]) {
    await t.test(ending, async () => {
      const harness = observationHarness(() => Promise.resolve(actualPage()));
      harness.mode("chat"); harness.roster();
      await harness.wrapper("load_agent_chat_page", { sessionId: "session" });
      harness.observer.sample(); harness.frame(); harness.frame();
      const phase = harness.observer.state.phase;
      const plan = harness.observer.prepareInteraction("session", harness.scroll);
      assert.ok(Number.isFinite(plan.plannedAtPage));
      assert.equal(phase.completeTrustedProbeAt, null);
      assert.throws(() => harness.observer.prepareInteraction("foreign", harness.scroll), /owner/);
      if (ending.startsWith("wheel")) {
        harness.composer({ value: "owned draft" });
        harness.wheel(true, 60, ending === "wheel" ? "before-handler" : "after-handler");
      } else {
        harness.input(false); assert.equal(phase.completeTrustedProbeAt, null);
        harness.input();
      }
      assert.equal(phase.usableFrame1At, null); assert.equal(phase.usableFrame2At, null);
      harness.frame();
      if (ending === "wheel-event-first") {
        assert.equal(phase.usableAt, null, "the existing event-first queued check precedes both witness frames");
        harness.frame();
      }
      assert.equal(phase.usableAt, null, "one frame cannot qualify usability");
      if (ending === "reset") harness.admitted({ revision: "replacement" });
      if (ending === "rearm") harness.observer.arm("warm");
      if (ending === "disposed") harness.observer.dispose();
      harness.frame();
      if (["reset", "rearm", "disposed"].includes(ending)) {
        assert.equal(phase.usableAt, null); assert.equal(phase.usableFrame1At, null); assert.equal(phase.usableFrame2At, null);
        if (ending === "rearm") assert.equal(harness.observer.state.phase.completeTrustedProbeAt, null);
      } else {
        assert.ok(Number.isFinite(phase.usableFrame1At)); assert.equal(phase.usableFrame2At, phase.usableAt);
        assert.ok(phase.usableFrame1At < phase.usableFrame2At);
        if (ending === "keyboard") assert.ok(phase.completeTrustedProbeAt <= phase.usableFrame1At);
        else { assert.equal(phase.completeTrustedProbeAt, null); assert.ok(Number.isFinite(phase.trustedWheelAt)); }
      }
    });
  }
});

test("Saved overflow prefers its already-safe empty composer over wheel input", async () => {
  const actions = [];
  const input = { isEnabled: async () => true, isDisplayed: async () => true,
    getAttribute: async () => "", sendKeys: async (...keys) => actions.push(["keyboard", ...keys]) };
  const binding = { owner: "session", version: 1, token: 1, ordinal: 1, label: "test-phase" };
  const phase = { label: "test-phase", ordinal: 1, rosterAt: 0, chatAt: null, exposureAt: 5, exposureKind: "automatic", commitAt: 10,
    paintAt: 20, usableAt: 30, body: { receiptAt: 4 } };
  const state = { owner: "session", errors: [], subscription: { active: true, fault_count: 0 }, phase, phases: [phase],
    activation: { active: true, version: 1, token: 1, owner: "session", at: 0 },
    coverage: "PAGE_LOADER_ONLY", broaderIpcRuntime: "UNOBSERVED",
    calls: [{ session: "session", at: 1 }], receipts: [{ session: "session", requestSession: "session", at: 4 }] };
  const driver = {
    executeScript: async (_fn, ...args) => args.length === 1 ? binding
      : ({ overflow: true, top: 100, input, control: "keyboard", reason: "safe_empty_composer" }),
    executeAsyncScript: async (_fn, actualBinding, field) => ({ status: "ACCEPTED", ...actualBinding, field, endpointAt: phase[field] }),
    actions: () => ({ scroll: () => ({ perform: async () => actions.push(["wheel"]) }) }),
  };
  const visible = async (_driver, selector) => selector.includes("Switch to Chat")
    ? { getAttribute: async () => "Switch to Terminal" }
    : selector.includes("textarea") ? input : { click: async () => {} };
  await rosterMeasurement(driver, { rosterFailures: [] }, visible, async () => ({}), { readObservation: async () => state });
  assert.deepEqual(actions, [["keyboard", "native-input-probe"]], "send only the probe, without submitting or clearing a draft");
});

test("a draft appearing after planning fails without typing, clearing or trying wheel", async () => {
  const actions = [];
  const input = { isEnabled: async () => true, isDisplayed: async () => true,
    getAttribute: async () => "existing-owned-draft", sendKeys: async () => actions.push("keyboard") };
  const driver = { executeScript: async () => ({ owner: "session", label: "test-phase", version: 1, token: 1, ordinal: 1,
    control: "keyboard", input, overflow: true, top: 100 }),
    executeAsyncScript: async (_fn, binding, field) => ({ status: "ACCEPTED", ...binding, field, endpointAt: 20 }),
    actions: () => ({ scroll: () => ({ perform: async () => actions.push("wheel") }) }) };
  const visible = async (_driver, selector) => selector.includes("Switch to Chat")
    ? { getAttribute: async () => "Switch to Terminal" } : { click: async () => {} };
  await assert.rejects(rosterMeasurement(driver, { rosterFailures: [] }, visible, async () => ({})),
    /fixture must not replace an existing draft/);
  assert.deepEqual(actions, []);
});

test("live measurements still require the actual Terminal to Chat toggle before any probe", async () => {
  let probes = 0;
  const driver = { executeScript: async () => {}, actions: () => { probes++; } };
  const visible = async (_driver, selector) => selector.includes("Switch to Chat")
    ? { getAttribute: async () => "Switch to Terminal" } : { click: async () => {} };
  await assert.rejects(rosterMeasurement(driver, { rosterFailures: [] }, visible, async () => ({}),
    { requireTerminalToggle: true }), /live Terminal measurement requires its real Chat toggle/);
  assert.equal(probes, 0);
});

test("Roster failure metadata distinguishes registry membership from bounded visible rows without changing results", async () => {
  const agents = Array.from({ length: 70 }, (_, index) => ({
    session_id: index === 69 ? "session" : "other-" + index,
    session_name: index === 69 ? "Owned Saved" : "Other " + index,
    provider: "codex", agent_class: "TestClass", is_off: true, folder: "private-folder",
  }));
  const before = JSON.stringify(agents);
  const rows = agents.map((agent) => ({
    isConnected: true, getAttribute: () => "Agent " + agent.session_name,
    getBoundingClientRect: () => ({ width: 240, height: 30 }),
  }));
  const roster = { isConnected: true, getBoundingClientRect: () => ({ width: 0, height: 600 }),
    querySelectorAll: () => rows,
    querySelector: (selector) => selector === "input" ? { value: "private-filter" } : { textContent: "All Agents" } };
  const calls = [];
  const harness = rosterDiagnosticHarness(async (command, args) => {
    calls.push({ command, args }); return agents;
  }, roster);
  const metadata = await harness.result;
  assert.equal(calls.length, 1); assert.equal(calls[0].command, "list_agents");
  assert.equal(Object.keys(calls[0].args).length, 0);
  assert.equal(metadata.registry.status, "OBSERVED_REGISTRY");
  assert.equal(metadata.registry.count, 70); assert.equal(metadata.registry.agents.length, 64);
  assert.equal(metadata.registry.expectedOwnerPresent, true);
  assert.equal(metadata.registry.expectedNamePresent, true);
  assert.equal(metadata.roster.geometry.hasLayout, false);
  assert.equal(metadata.roster.rowCount, 70); assert.equal(metadata.roster.rows.length, 64);
  assert.equal(metadata.roster.expectedNamePresent, true);
  assert.equal(JSON.stringify(agents), before);
  assert.equal(JSON.stringify(metadata).includes("private-folder"), false);
  const redacted = actualRedaction(metadata);
  assert.equal(redacted.registry.status, "OBSERVED_REGISTRY");
  assert.notEqual(redacted.registry.agents[0].session_name, agents[0].session_name);
  assert.notEqual(redacted.roster.rows[0].aria_label, rows[0].getAttribute());
  assert.notEqual(redacted.roster.filter, "private-filter");
});

test("Roster metadata records rejection, malformed registry and a bounded timeout without accepting late results", async () => {
  const rejected = await rosterDiagnosticHarness(async () => { throw { code: "registry_unavailable", message: "private-error" }; }).result;
  assert.equal(rejected.registry.status, "REGISTRY_ERROR");
  assert.equal(rejected.registry.code, "registry_unavailable");
  const malformed = await rosterDiagnosticHarness(async () => ({})).result;
  assert.equal(malformed.registry.status, "MALFORMED_REGISTRY");
  let release;
  const pending = rosterDiagnosticHarness(() => new Promise((resolve) => { release = resolve; }));
  await Promise.resolve();
  pending.expire();
  assert.equal((await pending.result).registry.status, "REGISTRY_TIMEOUT");
  release([]);
  await Promise.resolve(); await Promise.resolve();
  assert.equal(pending.completions(), 1);
});

test("measurement captures click and surface failures only after rejection, preserving the original error", async () => {
  const original = new Error("original Roster lookup failed");
  for (const captureFails of [false, true]) {
    const evidence = { rosterFailures: [] };
    let captures = 0;
    const driver = { executeScript: async () => {}, executeAsyncScript: async () => {
      captures += 1;
      if (captureFails) throw new Error("secondary capture failed");
      return { registry: { status: "OBSERVED_REGISTRY", count: 0 } };
    } };
    await assert.rejects(rosterMeasurement(driver, evidence, async () => { throw original; },
      async () => { throw new Error("must not activate"); }), (error) => error === original);
    assert.equal(captures, 1); assert.equal(evidence.rosterFailures.length, 1);
    assert.equal(evidence.rosterFailures[0].label, "test-phase");
  }
  let captures = 0;
  const afterActivation = new Error("later surface failure");
  const evidence = { rosterFailures: [] };
  await assert.rejects(rosterMeasurement({
    executeScript: async () => {},
    executeAsyncScript: async () => { captures += 1; },
  }, evidence, async () => ({ click: async () => {} }),
  async () => { throw afterActivation; }), (error) => error === afterActivation);
  assert.equal(captures, 1);
  assert.equal(evidence.rosterFailures[0].stage, "surface-resolution");
});

test("late toggle, paint, input, usability and assertion failures preserve their original error without registry RPC", async () => {
  for (const stage of ["chat-toggle", "paint", "input", "usability", "assertions"]) {
    for (const captureFails of [false, true]) {
      const original = new Error(`original ${stage}`);
      const evidence = { rosterFailures: [] };
      let waits = 0; let captures = 0; let registryCalls = 0;
      const input = { isEnabled: async () => true, isDisplayed: async () => true, getAttribute: async () => "",
        sendKeys: async () => { if (stage === "input") throw original; } };
      const driver = {
        executeScript: async (fn) => {
          if (fn.name === "captureChatFailureMetadata") {
            captures++;
            if (captureFails) throw new Error("secondary capture");
            return { status: "OBSERVED_LATE_DOM" };
          }
          return { owner: "session", label: "test-phase", version: 1, token: 1, ordinal: 1,
            overflow: false, top: 0, input, control: "keyboard", reason: "safe_empty_composer" };
        },
        executeAsyncScript: async (fn, binding, field, timeout) => {
          if (fn.name !== "waitForChatEndpoint") { registryCalls++; throw new Error("late registry RPC forbidden"); }
          assert.ok(timeout > 0 && timeout <= 5000);
          waits++;
          if ((stage === "paint" && waits === 1) || (stage === "usability" && waits === 2)) throw original;
          return { status: "ACCEPTED", ...binding, field, endpointAt: 20 };
        },
      };
      const visible = async (_driver, selector) => selector.includes("Switch to Chat")
        ? { getAttribute: async () => "Switch to Chat", click: async () => { if (stage === "chat-toggle") throw original; } }
        : selector.includes("textarea") ? input : { click: async () => {} };
      await assert.rejects(rosterMeasurement(driver, evidence, visible, async () => ({}), {
        captureChatFailureMetadata() {},
        readObservation: async () => { if (stage === "assertions") throw original; },
      }), (error) => error === original);
      assert.equal(registryCalls, 0);
      assert.equal(captures, 1);
      assert.equal(evidence.rosterFailures.length, 0);
      assert.equal(evidence.measurementFailures[0].stage, stage);
    }
  }
});

test("actual late DOM capture is bounded, leaves unmeasured endpoints null and never reads body or IPC", () => {
  const source = fs.readFileSync(new URL("./chat-turn-change-native.test.mjs", import.meta.url), "utf8");
  const start = source.indexOf("function captureChatFailureMetadata(sessionId, stage) {");
  const end = source.indexOf("async function measureChat(driver,", start);
  assert.ok(start >= 0 && end > start);
  const privateText = "private-body-must-never-be-read";
  const region = {
    getAttribute: () => "present-scope", getBoundingClientRect: () => ({ top: 0, left: 0, width: 100, height: 50 }),
    querySelectorAll: () => Array.from({ length: 100 }, () => ({ get textContent() { throw new Error(privateText); } })),
  };
  const root = { querySelector: (selector) => selector.includes("scroll-region") ? region
    : selector.includes("Switch to Terminal") ? {} : null };
  const window = { get __TAURI_INTERNALS__() { throw new Error("no IPC"); }, __chatAcceptance: {
    container: () => root, describeContainers: () => ({ count: 1, ownedMatchCount: 1 }),
    subscriptionStatus: () => ({ active: true, fault_count: 0, request_count: 1 }),
    state: { activation: { owner: "session", active: true }, calls: [{ privateText }], receipts: [], errors: [], rejections: 0,
      phase: { rosterAt: 10, chatAt: 11, exposureAt: null, commitAt: null, paintAt: null, usableAt: null } },
  } };
  const metadata = vm.runInNewContext(`${source.slice(start, end)}\ncaptureChatFailureMetadata("session", "paint");`,
    { window, document: { visibilityState: "visible" } });
  assert.equal(metadata.memberRows, 64);
  assert.equal(metadata.totalMemberRows, 100); assert.equal(metadata.capturedMemberRows, 64);
  assert.equal(metadata.memberRowsTruncated, true);
  assert.equal(metadata.mode, "chat");
  assert.equal(metadata.observer.endpoints.paintAt, null);
  assert.equal(metadata.observer.endpoints.usableAt, null);
  assert.equal(JSON.stringify(metadata).includes(privateText), false);
  const redacted = actualRedaction({ ...metadata, coverage: "PAGE_LOADER_ONLY", broaderIpcRuntime: "UNOBSERVED" });
  assert.equal(redacted.status, "OBSERVED_LATE_DOM");
  assert.equal(redacted.stage, "paint");
  assert.equal(redacted.mode, "chat");
  assert.equal(redacted.visibility, "visible");
  assert.equal(redacted.coverage, "PAGE_LOADER_ONLY");
  assert.equal(redacted.broaderIpcRuntime, "UNOBSERVED");
});

test("failure-only container metadata is bounded and contains identities and visibility without body text", async () => {
  const harness = observationHarness(() => Promise.resolve(actualPage()), { route: "agents-overview" });
  const metadata = await rosterDiagnosticHarness(async () => [], null, harness.observer).result;
  assert.equal(metadata.containers.status, "OBSERVED_CONTAINERS");
  assert.equal(metadata.containers.count, 1);
  assert.equal(metadata.containers.containers[0].eligible, true);
  assert.equal(metadata.containers.ownedMatchCount, 1);
  assert.equal(metadata.containers.containers[0].kind, "agents-overview");
  assert.equal(JSON.stringify(metadata).includes(chatAcceptanceFixture().latestBody), false);
  const redacted = actualRedaction(metadata);
  assert.equal(redacted.containers.status, "OBSERVED_CONTAINERS");
  assert.notEqual(redacted.containers.containers[0].owner, "session");
  for (let index = 0; index < 70; index += 1) harness.addContainer("agents-overview", `foreign-${index}`);
  const bounded = await rosterDiagnosticHarness(async () => [], null, harness.observer).result;
  assert.equal(bounded.containers.truncated, true);
  assert.equal(bounded.containers.containers.length, 64);
  harness.addContainer("agents-overview");
  const ambiguous = await rosterDiagnosticHarness(async () => [], null, harness.observer).result;
  assert.equal(ambiguous.containers.ambiguous, true);
  assert.equal(ambiguous.containers.ownedMatchCount, 2);
});

function endpointBinding(harness) {
  const { activation, phase, phases } = harness.observer.state;
  return { owner: activation.owner, version: activation.version, token: activation.token, ordinal: phases.length, label: phase.label };
}

test("endpoint notification accepts only original publication, handles ready-before-wait and preserves both trusted frames", async () => {
  for (const ready of [false, true]) {
    const harness = observationHarness(() => Promise.resolve(actualPage()));
    const binding = endpointBinding(harness);
    harness.click(); await harness.wrapper("load_agent_chat_page", { sessionId: "session" });
    const paint = [];
    if (ready) { harness.observer.sample(); harness.frame(); harness.frame(); }
    harness.observer.waitForEndpoint(binding, "paintAt", 5000, (ack) => paint.push(ack));
    if (!ready) {
      assert.equal(paint.length, 0);
      harness.observer.sample(); harness.frame(); assert.equal(paint.length, 0);
      harness.frame();
    }
    assert.equal(paint.length, 1); assert.equal(paint[0].status, "ACCEPTED");
    assert.equal(paint[0].endpointAt, harness.observer.state.phase.paintAt);
    assert.equal(harness.timerCount(), 0);
    const usable = [];
    harness.observer.waitForEndpoint(binding, "usableAt", 4000, (ack) => usable.push(ack));
    harness.input(false); harness.frame(); harness.frame(); assert.equal(usable.length, 0);
    harness.input(); harness.frame(); assert.equal(usable.length, 0); harness.frame();
    assert.equal(usable.length, 1); assert.equal(usable[0].status, "ACCEPTED");
    harness.observer.sample(); harness.frame(); harness.frame(); harness.expire();
    assert.equal(usable.length, 1); assert.equal(harness.timerCount(), 0);
    assert.equal(JSON.stringify(paint).includes(chatAcceptanceFixture().latestBody), false);
  }
});

test("endpoint notification rejects stale bindings and retires every pending callback exactly once", async () => {
  for (const scenario of ["owner", "version", "token", "ordinal", "label", "field", "budget", "rearm", "dispose", "fault", "timeout", "duplicate", "callback-fault"]) {
    const harness = observationHarness(() => Promise.resolve(actualPage()));
    const binding = endpointBinding(harness);
    const acks = [];
    if (["owner", "label"].includes(scenario)) binding[scenario] = "foreign";
    if (["version", "token", "ordinal"].includes(scenario)) binding[scenario]++;
    const done = (ack) => { acks.push(ack); if (scenario === "callback-fault") throw new Error("callback failed"); };
    harness.observer.waitForEndpoint(binding, scenario === "field" ? "body" : "paintAt", scenario === "budget" ? 0 : 5000, done);
    if (scenario === "rearm") harness.observer.arm("next");
    if (scenario === "dispose") harness.observer.dispose();
    if (scenario === "fault") await harness.wrapper("load_agent_chat_page", { sessionId: "session", get cursor() { throw new Error("fault"); } });
    if (scenario === "duplicate") harness.observer.waitForEndpoint(binding, "paintAt", 4000, (ack) => acks.push(ack));
    if (scenario === "timeout" || scenario === "callback-fault") harness.expire();
    assert.equal(acks.length, scenario === "duplicate" ? 2 : 1, scenario);
    assert.ok(acks.every((ack) => ack.status !== "ACCEPTED"), scenario);
    assert.equal(harness.timerCount(), 0, scenario);
    if (scenario === "callback-fault") assert.equal(harness.observer.state.errors.length, 1);
    if (scenario !== "dispose") {
      harness.click(); await harness.wrapper("load_agent_chat_page", { sessionId: "session" });
      harness.observer.sample(); harness.frame(); harness.frame();
    }
    harness.expire(); assert.equal(acks.length, scenario === "duplicate" ? 2 : 1, `${scenario}: late publication`);
  }
});

test("pending notification rejects facade retirement and late publication even before its timer dispatches", async () => {
  for (const change of ["facade", "deadline"]) {
    const harness = observationHarness(() => Promise.resolve(actualPage()));
    const binding = endpointBinding(harness);
    const acks = [];
    harness.observer.waitForEndpoint(binding, "paintAt", 100, (ack) => acks.push(ack));
    harness.click(); await harness.wrapper("load_agent_chat_page", { sessionId: "session" });
    harness.observer.sample(); harness.frame();
    if (change === "facade") harness.window.__wardianChatPageObservation = { version: 1 };
    else harness.clock(100);
    harness.frame();
    assert.equal(acks.length, 1); assert.notEqual(acks[0].status, "ACCEPTED");
    assert.equal(harness.timerCount(), 0);
    harness.expire(); assert.equal(acks.length, 1);
  }
});

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
  assert.equal(harness.wrapper("load_agent_chat_page", { sessionId: "session" }), rejected);
  await assert.rejects(rejected, (error) => error === failure);
  assert.equal(harness.observer.state.receipts.length, 0);
  const sync = observationHarness(() => { throw failure; });
  assert.throws(() => sync.wrapper("load_agent_chat_page", { sessionId: "session" }), (error) => error === failure);
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

for (const schedule of ["different-id becomes first", "diagnostic population changes"]) {
  test(`usability keeps the valid painted subject when ${schedule}`, async () => {
    const page = actualPage();
    page.events.push({ ...page.events[0], id: "different-id", turn_id: "different-turn" });
    const harness = observationHarness(() => Promise.resolve(page));
    const twin = harness.duplicate("different-id");
    harness.candidates([harness.content, twin]);
    harness.mode("chat"); harness.roster();
    await harness.wrapper("load_agent_chat_page", { sessionId: "session" });
    harness.observer.sample(); harness.frame(); harness.frame();
    const painted = harness.observer.state.phase;
    assert.equal(painted.body.eventId, "public-id");
    assert.equal(painted.body.matchingBodySlots, 2);
    const paintAt = painted.paintAt;
    if (schedule === "different-id becomes first") harness.candidates([twin, harness.content]);
    else twin.textContent = "Unrelated synthetic diagnostic population.";
    harness.observer.sample();
    harness.input(); harness.frame(); harness.frame();
    assert.notEqual(painted.usableAt, null, "valid original subject must qualify regardless of another matching row");
    assert.equal(painted.paintAt, paintAt);
    assert.equal(painted.body.eventId, "public-id", "no replacement duplicate can become the measured body");
  });
}

test("two-frame paint validates its original candidate when a different matching ID becomes first", async () => {
  const page = actualPage();
  page.events.push({ ...page.events[0], id: "different-id", turn_id: "different-turn" });
  const harness = observationHarness(() => Promise.resolve(page));
  const twin = harness.duplicate("different-id");
  harness.candidates([harness.content, twin]);
  harness.mode("chat"); harness.roster();
  await harness.wrapper("load_agent_chat_page", { sessionId: "session" });
  harness.observer.sample(); harness.frame();
  harness.candidates([twin, harness.content]);
  harness.frame();
  assert.notEqual(harness.observer.state.phase.paintAt, null);
  assert.equal(harness.observer.state.phase.body.eventId, "public-id");
});

test("a compositor scroll before the trusted wheel handler uses the pre-action baseline", async () => {
  const harness = observationHarness(() => Promise.resolve(actualPage()));
  harness.mode("chat"); harness.roster();
  await harness.wrapper("load_agent_chat_page", { sessionId: "session" });
  harness.observer.sample(); harness.frame(); harness.frame();
  const baseline = harness.scroll.scrollTop;
  harness.wheel(true, baseline - 20, "before-handler");
  harness.frame(); harness.frame();
  assert.notEqual(harness.observer.state.phase.usableAt, null,
    "the unchanged valid subject and a real trusted offset change must not lose the earlier baseline");
  const wheel = harness.observer.interactionStatus().wheel;
  assert.equal(wheel.before, baseline);
  assert.equal(wheel.eventTop, baseline - 20);
  assert.equal(wheel.after, baseline - 20);
  assert.equal(wheel.received, true);
  assert.equal(wheel.changed, true);
});

test("event-first scrolling waits for the actual change and then the full two-frame witness", async () => {
  const harness = observationHarness(() => Promise.resolve(actualPage()));
  harness.mode("chat"); harness.roster();
  await harness.wrapper("load_agent_chat_page", { sessionId: "session" });
  harness.observer.sample(); harness.frame(); harness.frame();
  const before = harness.scroll.scrollTop;
  harness.wheel(true, before - 20, "after-handler");
  harness.frame(); harness.frame();
  assert.equal(harness.observer.state.phase.usableAt, null);
  harness.frame();
  assert.notEqual(harness.observer.state.phase.usableAt, null);
  const wheel = harness.observer.interactionStatus().wheel;
  assert.equal(wheel.before, before);
  assert.equal(wheel.eventTop, before);
  assert.equal(wheel.after, before - 20);
});

for (const invalid of ["hidden", "detached", "member", "row-id", "window", "container", "unproved-alias"]) {
  test(`a valid duplicate cannot replace the painted subject after ${invalid} changes`, async () => {
    const page = actualPage();
    page.events.push({ ...page.events[0], id: "different-id", turn_id: "different-turn" });
    const harness = observationHarness(() => Promise.resolve(page));
    const twin = harness.duplicate("different-id");
    harness.candidates([harness.content, twin]);
    harness.mode("chat"); harness.roster();
    await harness.wrapper("load_agent_chat_page", { sessionId: "session" });
    harness.observer.sample(); harness.frame(); harness.frame();
    const paintAt = harness.observer.state.phase.paintAt;
    if (invalid === "hidden") harness.content.style = { visibility: "hidden", display: "block", opacity: "1" };
    if (invalid === "detached") harness.content.isConnected = false;
    if (invalid === "member") harness.identity({ id: "public-id", members: ["unadmitted-member"] });
    if (invalid === "row-id") harness.identity({ id: "foreign-id", members: null });
    if (invalid === "window") harness.admitted({ revision: "new-window" });
    if (invalid === "container") harness.replaceContainer();
    if (invalid === "unproved-alias") harness.key("unproved-slot");
    harness.observer.sample(); harness.input(); harness.frame(); harness.frame();
    assert.equal(harness.observer.state.phase.usableAt, null);
    assert.equal(harness.observer.state.phase.paintAt, paintAt);
    assert.equal(harness.observer.state.phase.body.eventId, "public-id");
    assert.equal(harness.observer.subjectStatus().latchedValid, false);
  });
}

test("the actual planner prefers a safe empty composer with overflow and never changes an unsafe draft", async () => {
  const harness = observationHarness(() => Promise.resolve(actualPage()));
  harness.mode("chat"); harness.roster();
  await harness.wrapper("load_agent_chat_page", { sessionId: "session" });
  harness.observer.sample(); harness.frame(); harness.frame();
  const safe = harness.composer({});
  const plan = harness.observer.prepareInteraction("session", harness.scroll);
  assert.equal(plan.overflow, true);
  assert.equal(plan.control, "keyboard");
  assert.equal(plan.input, safe);
  assert.equal(safe.value, "");
  assert.equal(harness.observer.state.phase.usableAt, null, "planning is not trusted input");
  for (const settings of [{ value: "pre-existing-private-draft" }, { disabled: true }, { readOnly: true },
    { hidden: true }, { style: { visibility: "hidden", display: "block", opacity: "1" } }]) {
    const node = harness.composer(settings);
    const before = node.value;
    const fallback = harness.observer.prepareInteraction("session", harness.scroll);
    assert.equal(fallback.control, "wheel");
    assert.equal(fallback.input, null);
    assert.equal(node.value, before);
    assert.equal(JSON.stringify(harness.observer.interactionStatus()).includes("pre-existing-private-draft"), false);
  }
  harness.composers([{}, {}]);
  assert.equal(harness.observer.prepareInteraction("session", harness.scroll).reason, "composer_ambiguous");
  harness.composer(null); harness.overflow(false);
  assert.equal(harness.observer.prepareInteraction("session", harness.scroll).control, "none");
});

test("actual wheel acknowledgement requires both the trusted owned event and an offset change", async () => {
  for (const [trusted, change] of [[false, true], [true, false]]) {
    const harness = observationHarness(() => Promise.resolve(actualPage()));
    harness.mode("chat"); harness.roster();
    await harness.wrapper("load_agent_chat_page", { sessionId: "session" });
    harness.observer.sample(); harness.frame(); harness.frame();
    harness.wheel(trusted, harness.scroll.scrollTop - (change ? 20 : 0), "before-handler");
    harness.frame(); harness.frame(); harness.frame();
    assert.equal(harness.observer.state.phase.usableAt, null);
    const wheel = harness.observer.interactionStatus().wheel;
    assert.equal(wheel.received, trusted);
    assert.equal(wheel.changed, change);
  }
});

test("post-disposal acknowledgement comes from the actual unsubscribe and leaves the pre-disposal observation intact", async () => {
  const harness = observationHarness(() => Promise.resolve(actualPage()));
  await harness.wrapper("load_agent_chat_page", { sessionId: "session" });
  const before = JSON.parse(JSON.stringify(harness.observer.state));
  const token = before.activation.token;
  const ack = harness.observer.dispose();
  assert.equal(ack.status, "OBSERVED_POST_DISPOSE");
  assert.equal(ack.disposed, true);
  assert.equal(ack.token, token);
  assert.equal(ack.subscription.active, false);
  assert.equal(harness.observer.subscriptionStatus().active, false);
  assert.equal(ack.callback_count, 1);
  assert.equal(ack.callback_fault_count, 0);
  assert.equal(ack.request_count, 1);
  assert.equal(before.disposed, false);
  assert.equal(before.activation.token, token);
  assert.equal(JSON.stringify(ack).includes(chatAcceptanceFixture().latestBody), false);
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
  assert.equal(harness.wrapper("load_agent_chat_page", { sessionId: "session" }), promise);
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
  let dto = actualPreviewPage({ events: repeated("preview") });
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
  await harness.wrapper("load_agent_chat_page", { sessionId: "session" });
  assert.equal(harness.observer.state.receipts.length, 0);
  assert.match(harness.observer.state.errors[0], /invalid recent admission/);
});
