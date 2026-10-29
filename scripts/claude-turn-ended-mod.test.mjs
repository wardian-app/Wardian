import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const source = await readFile(new URL("../src-tauri/src/manager/claude_turn_ended/register.js", import.meta.url), "utf8");
const binding = { wardian_session_id: "owned-agent", launch_id: "owned-launch", directory: "/owned/receipts" };
const { register } = await import(`data:text/javascript;base64,${Buffer.from(`const wardianBridge = ${JSON.stringify(binding)};\n${source}`).toString("base64")}`);

/** Exercises the shipped module with explicit provider-API doubles, never a real session. */
function fixture() {
  const hooks = new Map();
  const files = new Map();
  const store = new Map();
  const timers = [];
  let timestamp = 1000;
  let providerSession = "provider-session";
  let failWrite = false;
  let failStore = false;
  const $ = {
    store: {
      get: async (key) => structuredClone(store.get(key)),
      set: async (key, value) => {
        if (failStore) throw new Error("durable store unavailable");
        store.set(key, structuredClone(value));
      },
    },
    fs: {
      exists: async (path) => files.has(path),
      read: async (path) => files.get(path),
      write: async (path, text) => {
        if (failWrite) throw new Error("capture filesystem unavailable");
        files.set(path, text);
      },
    },
    session: { id: async () => providerSession },
    clock: {
      now: async () => timestamp++,
      every: (...args) => timers.push(args[1]),
    },
  };
  register((name, callback) => hooks.set(name, callback));
  const dispatch = async (name, event, handler = async () => ({}), origin = { plugin: "engine", tier: "core" }) => {
    const next = Object.assign(handler, { origin });
    return hooks.get(name)($, Object.freeze(event), next);
  };
  const submit = (text, origin = "composer", handler) => dispatch("prompt.submit", { text, origin: { kind: origin } }, handler);
  const start = (turnId, text, extra = {}) => dispatch("turn.start", { turnId, text, ...extra });
  const complete = (turnId, answer = "Answer.\n", extra = {}, handler) => dispatch("turn.complete", {
    turnId, answer, isAborted: false, reason: "answer", ...extra,
  }, handler);
  return {
    files, store, dispatch, submit, start, complete,
    failWrite: (value) => { failWrite = value; },
    failStore: (value) => { failStore = value; },
    providerSession: (value) => { providerSession = value; },
    tick: async () => { for (const callback of timers) await callback(); },
    receipts: () => [...files].filter(([path]) => path.includes("/notice-") && path.endsWith(".json"))
      .map(([, text]) => JSON.parse(text)),
  };
}

test("native observer passes middleware through and captures the exact immutable answer", async () => {
  const f = fixture();
  await f.submit("First prompt");
  await f.start("turn-A", "First prompt");
  const result = { text: "Another mod's UI annotation" };
  assert.equal(await f.complete("turn-A", "Exact A\n\n", {}, async () => {
    await f.submit("Next prompt");
    await f.start("turn-B", "Next prompt");
    return result;
  }), result);
  assert.equal(f.receipts().length, 1);
  assert.equal(f.receipts()[0].answer, "Exact A\n\n");
  assert.equal(f.receipts()[0].turn_id, "turn-A");
  assert.equal(f.receipts()[0].prompt_origin, "composer");
  assert.equal(f.receipts()[0].provider_session_id, "provider-session");
  assert.equal(f.receipts()[0].reason, "answer");
});

test("subagent lifecycle cannot become a main-agent notice", async () => {
  const f = fixture();
  await f.submit("A prompt");
  await f.start("child", "A prompt", { agentId: "subagent" });
  await f.complete("child", "Child output", { agentId: "subagent" });
  assert.deepEqual(f.receipts(), []);
});

test("unmatched and non-user starts cannot borrow an earlier answer", async () => {
  for (const origin of ["automatic", "plugin", undefined]) {
    const f = fixture();
    await f.submit("A prompt", origin ?? "automatic");
    await f.start("turn-A", "A prompt");
    await f.complete("turn-A", "Not a user answer");
    assert.deepEqual(f.receipts(), []);
  }
  const f = fixture();
  await f.complete("unmatched", "Stale text");
  assert.deepEqual(f.receipts(), []);
});

test("dropped, throwing, and ambiguously duplicated submissions stay unavailable", async () => {
  const dropped = fixture();
  await dropped.submit("A prompt", "composer", async () => ({ drop: "Policy" }));
  await dropped.start("turn-A", "A prompt");
  await dropped.complete("turn-A");
  assert.deepEqual(dropped.receipts(), []);
  const throwing = fixture();
  await assert.rejects(throwing.submit("A prompt", "composer", async () => { throw new Error("provider failure"); }));
  await throwing.start("turn-A", "A prompt");
  await throwing.complete("turn-A");
  assert.deepEqual(throwing.receipts(), []);
  const ambiguous = fixture();
  await ambiguous.submit("Identical prompt");
  await ambiguous.submit("Identical prompt");
  await ambiguous.start("turn-A", "Identical prompt");
  await ambiguous.complete("turn-A");
  assert.deepEqual(ambiguous.receipts(), []);
});

test("lower middleware rewrites must match the provider's actual start text", async () => {
  const f = fixture();
  await f.submit("Original", "bridge", async () => ({ text: "Rewritten" }));
  await f.start("turn-A", "Rewritten");
  await f.complete("turn-A");
  assert.equal(f.receipts()[0].prompt_text, "Rewritten");
  assert.equal(f.receipts()[0].prompt_origin, "bridge");
});

test("empty interrupted answer is preserved instead of substituting stale assistant text", async () => {
  const f = fixture();
  await f.submit("A prompt");
  await f.start("turn-A", "A prompt");
  await f.complete("turn-A", "", { isAborted: true, reason: "aborted" });
  assert.equal(f.receipts()[0].answer, "");
  assert.equal(f.receipts()[0].is_aborted, true);
});

test("duplicate completion/timer delivery keeps one immutable receipt", async () => {
  const f = fixture();
  await f.submit("A prompt");
  await f.start("turn-A", "A prompt");
  await f.complete("turn-A", "First answer");
  const first = structuredClone(f.receipts());
  await f.complete("turn-A", "Conflicting later answer");
  await f.tick();
  assert.deepEqual(f.receipts(), first);
});

test("file failure retries the stored receipt without prompting again", async () => {
  const f = fixture();
  await f.dispatch("session.start", {});
  await f.submit("A prompt");
  await f.start("turn-A", "A prompt");
  f.failWrite(true);
  await f.complete("turn-A", "Captured before disk failed");
  assert.deepEqual(f.receipts(), []);
  assert.equal([...f.store.values()][0][0].answer, "Captured before disk failed");
  f.failWrite(false);
  await f.tick();
  assert.equal(f.receipts()[0].answer, "Captured before disk failed");
});

test("store failure keeps a memory snapshot for the next durable retry", async () => {
  const f = fixture();
  await f.dispatch("session.start", {});
  await f.submit("A prompt");
  await f.start("turn-A", "A prompt");
  f.failStore(true);
  await f.complete("turn-A", "Captured before store failed");
  assert.deepEqual(f.receipts(), []);
  f.failStore(false);
  await f.tick();
  assert.equal(f.receipts()[0].answer, "Captured before store failed");
});

test("only a backend acknowledgement retires the durable outbox", async () => {
  const f = fixture();
  await f.submit("A prompt");
  await f.start("turn-A", "A prompt");
  await f.complete("turn-A");
  assert.equal([...f.store.values()][0].length, 1);
  f.files.set("/owned/receipts/notice-turn-A.ack", JSON.stringify({ id: "backend-owned-id" }));
  await f.tick();
  assert.deepEqual([...f.store.values()][0], []);
});

test("foreign engine origin, changed session, and unsafe native identities cannot publish", async () => {
  const f = fixture();
  await f.dispatch("prompt.submit", { text: "A prompt", origin: { kind: "composer" } }, async () => ({}), { plugin: "other", tier: "user" });
  await f.start("turn-A", "A prompt");
  await f.complete("turn-A");
  assert.deepEqual(f.receipts(), []);
  const moved = fixture();
  await moved.submit("A prompt");
  await moved.start("turn-A", "A prompt");
  moved.providerSession("different-session");
  await moved.complete("turn-A");
  assert.deepEqual(moved.receipts(), []);
  const unsafe = fixture();
  await unsafe.submit("A prompt");
  await unsafe.start("../turn-A", "A prompt");
  await unsafe.complete("../turn-A");
  assert.deepEqual(unsafe.receipts(), []);
});
