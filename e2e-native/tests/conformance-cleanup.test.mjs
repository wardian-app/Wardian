// @tier nightly — Deterministic cleanup and owned-lock checks; no app/provider/ports.
import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { acquireHomeLock, readHomeLock, releaseHomeLock, HOME_LOCK_FILE } from "../lib/sessionHome.mjs";
import { cleanupConformanceSession, closeConformanceSession, pauseConformanceAgents,
  pauseConformanceWork } from "../lib/conformance-cleanup.mjs";

function fixture(t) {
  const isolatedHome = fs.mkdtempSync(path.join(os.tmpdir(), "wardian-conformance-cleanup-"));
  const harness = { isolatedHome, runId: `cleanup-${path.basename(isolatedHome)}` };
  acquireHomeLock({ home: isolatedHome, runId: harness.runId });
  t.after(() => fs.rmSync(isolatedHome, { recursive: true, force: true }));
  const calls = [];
  const session = {
    tauriDriver: { exitCode: null, signalCode: null },
    close: async () => { calls.push("close"); session.tauriDriver.exitCode = 0; },
  };
  const options = {
    harness, session, startupAttempted: true,
    pause: async () => { calls.push("pause"); },
    save: async (result) => { calls.push("save"); assert.equal(result.home_lock_released, readHomeLock(isolatedHome) === null); },
  };
  return { harness, session, calls, options };
}

test("report-write failure still pauses, shuts down and releases a proven owned home", async (t) => {
  const { options, calls, harness } = fixture(t);
  const failure = new Error("report disk full");
  options.save = async () => { calls.push("save"); throw failure; };
  await assert.rejects(cleanupConformanceSession(options), (error) =>
    error.cleanupConfirmed === true && error.errors.includes(failure));
  assert.deepEqual(calls, ["pause", "close", "save"]);
  assert.equal(readHomeLock(harness.isolatedHome), null);
});

test("pause rejection cannot bypass close and retains ownership despite driver exit", async (t) => {
  const { options, calls, harness } = fixture(t);
  options.pause = async () => { calls.push("pause"); throw new Error("pause rejected"); };
  await assert.rejects(cleanupConformanceSession(options), (error) => error.cleanupConfirmed === false);
  assert.deepEqual(calls, ["pause", "close", "save"]);
  assert.equal(readHomeLock(harness.isolatedHome).runId, harness.runId);
});

test("bounded close returning with a live driver retains the lock", async (t) => {
  const { options, session, harness } = fixture(t);
  session.close = async () => {};
  await assert.rejects(cleanupConformanceSession(options), (error) =>
    error.cleanupConfirmed === false && error.errors.some((item) => /exit is unconfirmed/.test(item.message)));
  assert.equal(readHomeLock(harness.isolatedHome).runId, harness.runId);
});

test("missing exit metadata cannot masquerade as an observed process exit", async (t) => {
  const { options, session, harness } = fixture(t);
  session.tauriDriver = {};
  session.close = async () => {};
  await assert.rejects(cleanupConformanceSession(options));
  assert.ok(readHomeLock(harness.isolatedHome));
});

test("shutdown and report failures are both retained without releasing the home", async (t) => {
  const { options, session, harness, calls } = fixture(t);
  session.close = async () => { calls.push("close"); throw new Error("quit failed"); };
  options.save = async () => { calls.push("save"); throw new Error("save failed"); };
  await assert.rejects(cleanupConformanceSession(options), (error) =>
    error.cleanupConfirmed === false && error.errors.length === 2);
  assert.deepEqual(calls, ["pause", "close", "save"]);
  assert.ok(readHomeLock(harness.isolatedHome));
});

test("failed startup without a session handle retains the lock for supervised cleanup", async (t) => {
  const { options, harness } = fixture(t);
  options.session = undefined;
  await assert.rejects(cleanupConformanceSession(options), (error) => error.cleanupConfirmed === false);
  assert.ok(readHomeLock(harness.isolatedHome));
});

test("fixture failure before startup can release its claim even if reporting fails", async (t) => {
  const { options, harness } = fixture(t);
  options.session = undefined;
  options.startupAttempted = false;
  options.pause = async () => assert.fail("no runtime exists to pause");
  options.save = async () => { throw new Error("fixture/report failed"); };
  await assert.rejects(cleanupConformanceSession(options), (error) => error.cleanupConfirmed === true);
  assert.equal(readHomeLock(harness.isolatedHome), null);
});

test("foreign home lock is never released after successful session cleanup", async (t) => {
  const { options, harness } = fixture(t);
  releaseHomeLock({ home: harness.isolatedHome, runId: harness.runId });
  acquireHomeLock({ home: harness.isolatedHome, runId: "foreign" });
  await assert.rejects(cleanupConformanceSession(options), (error) => error.cleanupConfirmed === false);
  assert.equal(readHomeLock(harness.isolatedHome).runId, "foreign");
});

test("observed signal exit releases only the suite claim after successful quit", async (t) => {
  const { options, session, harness } = fixture(t);
  session.close = async () => { session.tauriDriver.signalCode = "SIGTERM"; };
  assert.deepEqual(await cleanupConformanceSession(options), { shutdown_confirmed: true, home_lock_released: true });
  assert.equal(readHomeLock(harness.isolatedHome), null);
});

test("archive restart closes with proof without releasing the home between sessions", async (t) => {
  const { session, harness } = fixture(t);
  await closeConformanceSession(session);
  assert.equal(readHomeLock(harness.isolatedHome).runId, harness.runId);
});

test("owner-aware serial lock refuses live contention and reclaims dead metadata", async (t) => {
  const { harness } = fixture(t);
  assert.throws(() => acquireHomeLock({ home: harness.isolatedHome, runId: "another" }), /still using it/);
  // Invalid/nonlive PID is inert fixture metadata; no process is launched or killed.
  fs.writeFileSync(path.join(harness.isolatedHome, HOME_LOCK_FILE), JSON.stringify({ runId: "stale", pid: -1 }));
  acquireHomeLock({ home: harness.isolatedHome, runId: "next" });
  releaseHomeLock({ home: harness.isolatedHome, runId: "stale" });
  assert.equal(readHomeLock(harness.isolatedHome).runId, "next");
  releaseHomeLock({ home: harness.isolatedHome, runId: "next" });
  assert.equal(readHomeLock(harness.isolatedHome), null);
});


test("owned roster cleanup pauses off native owners as well as live agents", async () => {
  const calls = [];
  await pauseConformanceAgents(async (command, args) => {
    calls.push([command, args]);
    if (command === "list_agents") return [{ session_id: "live", is_off: false }, { session_id: "native", is_off: true }];
  });
  assert.deepEqual(calls, [["list_agents", undefined], ["pause_agent", { sessionId: "live" }], ["pause_agent", { sessionId: "native" }]]);
});

test("one agent pause failure still attempts the other agents and retains the lock", async (t) => {
  const { options, session, harness, calls } = fixture(t);
  const paused = [];
  options.pause = () => pauseConformanceAgents(async (command, args) => {
    if (command === "list_agents") return [{ session_id: "first" }, { session_id: "second" }];
    paused.push(args.sessionId);
    if (args.sessionId === "first") throw new Error("pause unconfirmed");
  });
  await assert.rejects(cleanupConformanceSession(options), (error) => error.cleanupConfirmed === false);
  assert.deepEqual(paused, ["first", "second"]);
  assert.deepEqual(calls, ["close", "save"]);
  assert.equal(session.tauriDriver.exitCode, 0);
  assert.equal(readHomeLock(harness.isolatedHome).runId, harness.runId);
});

test("unavailable or malformed owned roster cannot authorize lock release", async (t) => {
  const { options, harness } = fixture(t);
  for (const roster of [null, {}, [{ session_id: "" }], [{}], { ok: false }]) {
    options.pause = () => pauseConformanceAgents(async (command) => {
      assert.equal(command, "list_agents");
      return roster;
    });
    await assert.rejects(cleanupConformanceSession(options), (error) => error.cleanupConfirmed === false);
    assert.equal(readHomeLock(harness.isolatedHome).runId, harness.runId);
  }
});

test("empty owned roster permits cleanup only with observed driver exit", async (t) => {
  const { options, harness } = fixture(t);
  options.pause = () => pauseConformanceAgents(async () => []);
  assert.deepEqual(await cleanupConformanceSession(options), { shutdown_confirmed: true, home_lock_released: true });
  assert.equal(readHomeLock(harness.isolatedHome), null);
});

test("unsettled temporary task retains lock even with no registered agents", async (t) => {
  const { options, harness, calls } = fixture(t);
  options.pause = async () => {
    await pauseConformanceAgents(async () => []);
    throw new Error("Temporary provider completion is unconfirmed");
  };
  await assert.rejects(cleanupConformanceSession(options), (error) => error.cleanupConfirmed === false);
  assert.deepEqual(calls, ["close", "save"]);
  assert.equal(readHomeLock(harness.isolatedHome).runId, harness.runId);
});

test("chat/context unreturned spawn pauses roster but retains lock after driver exit", async (t) => {
  const { options, harness } = fixture(t);
  for (const roster of [[], [{ session_id: "late-owner" }]]) {
    const paused = [];
    options.pause = () => pauseConformanceWork(async (command, args) => {
      if (command === "list_agents") return roster;
      paused.push(args.sessionId);
    }, { spawnAttempted: true, sessionId: undefined });
    await assert.rejects(cleanupConformanceSession(options), error => error.cleanupConfirmed === false);
    assert.deepEqual(paused, roster.map(agent => agent.session_id));
    assert.ok(readHomeLock(harness.isolatedHome));
  }
});

test("context unsettled automation retains lock even after returned agent pauses", async (t) => {
  const { options, harness } = fixture(t);
  options.pause = () => pauseConformanceWork(async command =>
    command === "list_agents" ? [{ session_id: "owned" }] : undefined,
  { spawnAttempted: true, sessionId: "owned", automationUnsettled: true });
  await assert.rejects(cleanupConformanceSession(options), error => error.cleanupConfirmed === false);
  assert.ok(readHomeLock(harness.isolatedHome));
});

test("returned owner and settled automation permit proven cleanup", async (t) => {
  const { options, harness } = fixture(t);
  options.pause = () => pauseConformanceWork(async command =>
    command === "list_agents" ? [{ session_id: "owned" }] : undefined,
  { spawnAttempted: true, sessionId: "owned", automationUnsettled: false });
  assert.equal((await cleanupConformanceSession(options)).shutdown_confirmed, true);
  assert.equal(readHomeLock(harness.isolatedHome), null);
});
