import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { stripTypeScriptTypes } from "node:module";
import { setImmediate as turn } from "node:timers/promises";
import test from "node:test";

const sourceUrl = new URL("../src-tauri/resources/claude-delivery/hooks/register.ts", import.meta.url);
const javascript = stripTypeScriptTypes(await readFile(sourceUrl, "utf8"), {
  mode: "strip", sourceUrl: sourceUrl.href,
});
let fixtureId = 0;
const inbox = "/owned/delivery/7/inbox";
const recordPath = (kind, id) => `${inbox}/${kind}/${id}.json`;
const deferred = () => Promise.withResolvers();

/** Load the shipped mod with only its documented host surface doubled.
 * Timer callbacks are driven independently, with an event-loop turn draining
 * the delivery callback's deliberately detached promise (no wall-clock sleeps).
 */
async function fixture() {
  const { register } = await import(`data:text/javascript;base64,${Buffer.from(javascript).toString("base64")}#fixture-${fixtureId++}`);
  const files = new Map([["/owned/mod/launch.json", JSON.stringify({
    schema: 1, agent_id: "agent-A", runtime_generation: 7,
    expected_claude_session_id: "session-A", inbox,
  })]]);
  const hooks = new Map();
  const timers = [];
  const writes = [];
  const submissions = [];
  let writeHook = async () => {};
  let submitHook = async ({ text }) => ({ text });
  let sessionId = "session-A";
  const host = {
    plugin: { root: "/owned/mod" },
    session: { id: async () => sessionId },
    fs: {
      exists: async (file) => files.has(file),
      read: async (file) => files.get(file),
      list: async (directory) => [...files.keys()]
        .filter((file) => file.startsWith(`${directory}/`) && !file.slice(directory.length + 1).includes("/"))
        .map((file) => ({ kind: "file", name: file.slice(directory.length + 1) })),
      write: async (file, text) => {
        await writeHook(file, text);
        files.set(file, text);
        writes.push({ file, value: JSON.parse(text) });
      },
    },
    prompt: { submit: async (request) => {
      submissions.push(structuredClone(request));
      return submitHook(request);
    } },
    clock: { every: (milliseconds, callback) => { timers.push({ milliseconds, callback }); } },
  };
  register((name, handler) => hooks.set(name, handler));
  const dispatch = (name, event) => hooks.get(name)(host, event, async () => "next-result");
  return {
    files, writes, submissions, timers, dispatch,
    start: () => dispatch("session.start", { isInteractive: true }),
    pending(id, sequence, generation = 7) {
      files.set(`${inbox}/pending/${String(sequence).padStart(12, "0")}-${id}.json`,
        JSON.stringify({ schema: 1, interaction_id: id, runtime_generation: generation, text: `body-${id}` }));
    },
    record: (kind, id) => {
      const raw = files.get(recordPath(kind, id));
      return raw === undefined ? undefined : JSON.parse(raw);
    },
    setWrite: (hook) => { writeHook = hook; },
    setSubmit: (hook) => { submitHook = hook; },
    setSession: (id) => { sessionId = id; },
    async tick(index) {
      assert.ok(timers[index], `Missing independent timer ${index}`);
      await timers[index].callback();
      await turn();
    },
  };
}

test("interactive start passes middleware through and binds readiness to the real session", async () => {
  const f = await fixture();
  assert.equal(await f.start(), "next-result");
  const ready = JSON.parse(f.files.get(`${inbox}/ready.json`));
  assert.equal(ready.generation, 7);
  assert.equal(ready.session_id, "session-A");
  assert.equal(typeof ready.mod_version, "string");
  assert.deepEqual(f.timers.map((timer) => timer.milliseconds), [1000, 1000]);
  assert.deepEqual(f.submissions, []);
});

test("noninteractive or invalid launch cannot register delivery timers", async () => {
  const noninteractive = await fixture();
  assert.equal(await noninteractive.dispatch("session.start", { isInteractive: false }), "next-result");
  assert.deepEqual(noninteractive.timers, []);
  const invalid = await fixture();
  invalid.files.set("/owned/mod/launch.json", "not JSON");
  await invalid.start();
  assert.deepEqual(invalid.timers, []);
  assert.deepEqual(invalid.writes, []);
});

test("poll sorts pending sequence, serializes submissions, and never replays settled claims", async () => {
  const f = await fixture();
  const first = deferred();
  f.pending("second", 2);
  f.pending("first", 1);
  f.pending("third", 3);
  f.setSubmit(({ text }) => text === "body-first" ? first.promise : Promise.resolve({ text }));
  await f.start();
  await f.tick(1);
  assert.deepEqual(f.submissions, [{ text: "body-first" }]);
  await f.tick(1); // Reentrant poll while submit has not resolved.
  assert.deepEqual(f.submissions, [{ text: "body-first" }]);
  first.resolve({ text: "body-first" });
  await turn();
  assert.deepEqual(f.submissions.map((request) => request.text), ["body-first", "body-second", "body-third"]);
  for (const id of ["first", "second", "third"]) {
    assert.equal(f.record("queued", id)?.interaction_id, id);
    assert.equal(f.record("queued", id)?.generation, 7);
  }
  await f.tick(1);
  assert.equal(f.submissions.length, 3);
});

test("foreign generation and malformed payload are rejected without claim or submit", async () => {
  const f = await fixture();
  f.pending("foreign", 1, 8);
  f.files.set(`${inbox}/pending/000000000002-malformed.json`, "{");
  await f.start();
  await f.tick(1);
  assert.deepEqual(f.submissions, []);
  assert.equal(f.record("rejected", "foreign").reason, "generation mismatch");
  assert.equal(f.record("rejected", "foreign").received_generation, 8);
  assert.equal(f.record("rejected", "malformed").reason, "not valid JSON");
  assert.equal(f.record("claimed", "foreign"), undefined);
  assert.equal(f.record("claimed", "malformed"), undefined);
  const writes = f.writes.length;
  await f.tick(1);
  assert.equal(f.writes.length, writes, "Rejected records must be write-once");
});

test("claim write must resolve before submit can cross the provider boundary", async () => {
  const f = await fixture();
  const claim = deferred();
  f.pending("held", 1);
  f.setWrite((file) => file === recordPath("claimed", "held") ? claim.promise : Promise.resolve());
  f.setSubmit(async ({ text }) => {
    assert.equal(f.record("claimed", "held")?.interaction_id, "held", "Claim was not published before submit");
    return { text };
  });
  await f.start();
  await f.tick(1);
  assert.deepEqual(f.submissions, []);
  assert.equal(f.record("claimed", "held"), undefined);
  claim.resolve();
  await turn();
  assert.deepEqual(f.submissions, [{ text: "body-held" }]);
  assert.ok(f.record("queued", "held"));
});

test("failed claim write cannot submit and can retry safely on the next poll", async () => {
  const f = await fixture();
  f.pending("retry", 1);
  f.setWrite(async (file) => { if (file === recordPath("claimed", "retry")) throw new Error("disk unavailable"); });
  await f.start();
  await f.tick(1);
  assert.deepEqual(f.submissions, []);
  assert.equal(f.record("claimed", "retry"), undefined);
  f.setWrite(async () => {});
  await f.tick(1);
  assert.equal(f.submissions.length, 1);
  assert.ok(f.record("queued", "retry"));
});

test("drop records proven non-entry with its exact reason and cannot become queued or replayed", async () => {
  const f = await fixture();
  f.pending("drop", 1);
  f.setSubmit(async () => ({ drop: "Stopped by user" }));
  await f.start();
  await f.tick(1);
  assert.ok(f.record("claimed", "drop"));
  assert.equal(f.record("dropped", "drop").reason, "Stopped by user");
  assert.equal(f.record("queued", "drop"), undefined);
  await f.tick(1);
  assert.equal(f.submissions.length, 1);
});

for (const outcome of ["reject", "unexpected"]) {
  test(`${outcome} submit outcome leaves an uncertain claim and never replays it`, async () => {
    const f = await fixture();
    f.pending("uncertain", 1);
    f.setSubmit(async () => {
      if (outcome === "reject") throw new Error("provider disconnected after possible admission");
      return { other: true };
    });
    await f.start();
    await f.tick(1);
    assert.ok(f.record("claimed", "uncertain"));
    for (const kind of ["queued", "dropped", "rejected"]) assert.equal(f.record(kind, "uncertain"), undefined);
    // Existence alone is authoritative, even if a claim write was torn.
    f.files.set(recordPath("claimed", "uncertain"), "{");
    await f.tick(1);
    assert.equal(f.submissions.length, 1);
  });
}

test("heartbeat write failure is contained and its independent timer runs during an unresolved submit", async () => {
  const f = await fixture();
  const submit = deferred();
  let heartbeatAttempts = 0;
  f.pending("slow", 1);
  f.setSubmit(() => submit.promise);
  f.setWrite(async (file) => {
    if (file === `${inbox}/heartbeat.json`) {
      heartbeatAttempts++;
      throw new Error("Windows sharing violation");
    }
  });
  await f.start();
  await f.tick(1);
  assert.equal(f.submissions.length, 1);
  assert.equal(f.record("queued", "slow"), undefined);
  await f.tick(0);
  await f.tick(0);
  assert.equal(heartbeatAttempts, 2, "Failed heartbeat must not latch off its timer");
  f.setWrite(async () => {});
  f.setSession("session-B");
  await f.tick(0);
  assert.equal(JSON.parse(f.files.get(`${inbox}/heartbeat.json`)).session_id, "session-B");
  assert.equal(f.record("queued", "slow"), undefined, "Heartbeat must not require submit completion");
  submit.resolve({ text: "body-slow" });
  await turn();
  assert.equal(f.record("queued", "slow")?.session_id, "session-B");
  assert.ok(f.files.has(`${inbox}/sessions/session-A.json`));
  assert.ok(f.files.has(`${inbox}/sessions/session-B.json`));
  await f.tick(1);
  assert.equal(f.submissions.length, 1);
});
