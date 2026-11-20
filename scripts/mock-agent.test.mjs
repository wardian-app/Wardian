import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { randomUUID } from "node:crypto";
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import vm from "node:vm";

const require = createRequire(import.meta.url);
const scriptPath = fileURLToPath(new URL("./mock-agent.cjs", import.meta.url));
const source = readFileSync(scriptPath, "utf8");

/** Runs the shipped emitter with controlled transport failures and no disk writes. */
function emitter({ log = "owned-mock.jsonl", session = "actual-session", failures = 0 } = {}) {
  const writes = [];
  let uuidCalls = 0;
  const context = vm.createContext({
    module: { exports: {} },
    require(name) {
      if (name === "node:crypto") return { randomUUID() { uuidCalls++; return randomUUID(); } };
      if (name === "node:fs") return {
        appendFileSync(path, line) {
          assert.equal(path, log);
          const failed = failures-- > 0;
          writes.push({ transport: "log", line, failed });
          if (failed) throw new Error("controlled append failure");
        },
      };
      return require(name);
    },
    process: {
      argv: [process.execPath, scriptPath],
      env: { WARDIAN_MOCK_LOG: log, WARDIAN_MOCK_SESSION_ID: session },
      stdout: { write(line) { writes.push({ transport: "stdout", line }); } },
    },
  });
  vm.runInContext(source, context, { filename: scriptPath });
  return { emit: vm.runInContext("emit", context), writes, uuidCalls: () => uuidCalls };
}

function receipt(record, session, logged) {
  const mirror = record.wardian_mirror;
  assert.match(mirror.incarnation, /^[a-f0-9]{8}-[a-f0-9]{4}-4[a-f0-9]{3}-[89ab][a-f0-9]{3}-[a-f0-9]{12}$/);
  assert.ok(mirror.observation_id.startsWith(`${mirror.incarnation}:`));
  assert.match(mirror.observation_id.slice(mirror.incarnation.length + 1), /^[1-9][0-9]*$/);
  assert.ok(mirror.observation_id.length <= 128);
  assert.equal(mirror.provider_session_id, session);
  assert.equal(mirror.logged, logged);
  return mirror;
}

test("message/model/info observations log first with identical stdout bytes and independent IDs", () => {
  const fixture = emitter();
  const observations = [
    { type: "message", role: "assistant", turn_id: "same-turn", content: "Equal text" },
    { type: "model", turn_id: "same-turn", content: "Equal text" },
    { type: "info", turn_id: "different-turn", content: "Equal text" },
  ];
  const ids = new Set();
  const incarnations = new Set();
  for (const [index, observation] of observations.entries()) {
    fixture.emit(observation);
    const [log, stdout] = fixture.writes.slice(index * 2);
    assert.equal(log.transport, "log");
    assert.equal(stdout.transport, "stdout");
    assert.equal(log.line, stdout.line);
    const { wardian_mirror, ...payload } = JSON.parse(stdout.line);
    assert.deepEqual(payload, observation);
    assert.equal(observation.wardian_mirror, undefined);
    const mirror = receipt({ wardian_mirror }, "actual-session", true);
    ids.add(mirror.observation_id);
    incarnations.add(mirror.incarnation);
  }
  assert.equal(ids.size, observations.length);
  assert.equal(incarnations.size, 1);
  assert.equal(fixture.uuidCalls(), 1);
});

test("append failure keeps output and identity with logged:false, then recovers", () => {
  const fixture = emitter({ failures: 1 });
  const observation = { type: "model", turn_id: "turn-1", content: "Genuine output" };
  fixture.emit(observation);
  fixture.emit(observation);
  const [failed, fallback, recovered, stdout] = fixture.writes;
  assert.equal(failed.failed, true);
  assert.equal(failed.transport, "log");
  assert.equal(fallback.transport, "stdout");
  const claimed = JSON.parse(failed.line);
  const actual = JSON.parse(fallback.line);
  receipt(claimed, "actual-session", true);
  receipt(actual, "actual-session", false);
  claimed.wardian_mirror.logged = false;
  assert.deepEqual(actual, claimed);
  assert.equal(actual.content, observation.content);
  assert.equal(recovered.failed, false);
  assert.equal(recovered.line, stdout.line);
  assert.notEqual(actual.wardian_mirror.observation_id, JSON.parse(stdout.line).wardian_mirror.observation_id);
  assert.equal(fixture.uuidCalls(), 1);
});

test("no mirror log preserves observations as explicitly unlogged", () => {
  const fixture = emitter({ log: "", session: "resumed-session" });
  for (const type of ["message", "model", "info"]) fixture.emit({ type, content: "Answer" });
  assert.equal(fixture.writes.length, 3);
  for (const write of fixture.writes) {
    assert.equal(write.transport, "stdout");
    receipt(JSON.parse(write.line), "resumed-session", false);
  }
});

test("status, tool, and fixture payloads retain their existing shape", () => {
  const fixture = emitter();
  for (const record of [
    { type: "init", session_id: "actual-session" },
    { type: "user", turn_id: "turn-1", content: "User" },
    { type: "result", status: "success" },
    { type: "action_required", message: "Approve?" },
    { type: "tool_call", call_id: "call-1", tool_name: "Read", input: {} },
    { response: "Headless result", status: "ok" },
  ]) {
    fixture.emit(record);
    const [log, stdout] = fixture.writes.slice(-2);
    assert.equal(log.line, stdout.line);
    assert.deepEqual(JSON.parse(stdout.line), record);
  }
});

function runProvider(session, log = "") {
  const result = spawnSync(process.execPath, [scriptPath], {
    env: {
      ...process.env,
      WARDIAN_MOCK_SCENARIO: "basic", WARDIAN_MOCK_DELAY_MS: "0",
      WARDIAN_MOCK_SESSION_ID: session, WARDIAN_MOCK_LOG: log,
    },
    encoding: "utf8", timeout: 10_000,
  });
  assert.equal(result.error, undefined);
  assert.equal(result.status, 0, result.stderr);
  const rows = result.stdout.trim().split("\n").map((line) => JSON.parse(line));
  assert.deepEqual(rows.map((row) => row.type), ["init", "user", "message", "model", "result"]);
  return rows;
}

test("real processes resuming the same session never reuse observation identity", () => {
  const processes = [runProvider("repeated-session"), runProvider("repeated-session"), runProvider("other-session")];
  const ids = new Set();
  const incarnations = new Set();
  for (const rows of processes) {
    const processIncarnations = new Set();
    for (const row of rows.filter((row) => row.wardian_mirror)) {
      const mirror = receipt(row, rows[0].session_id, false);
      ids.add(mirror.observation_id);
      processIncarnations.add(mirror.incarnation);
    }
    assert.equal(processIncarnations.size, 1);
    incarnations.add([...processIncarnations][0]);
  }
  assert.equal(ids.size, 6);
  assert.equal(incarnations.size, 3);
});

test("real filesystem append failure emits genuine unlogged output", () => {
  // Appending to the existing script directory fails without creating an artifact.
  const rows = runProvider("failed-log-session", dirname(scriptPath));
  for (const row of rows.filter((row) => row.wardian_mirror)) receipt(row, "failed-log-session", false);
  assert.equal(rows[3].content, "Mock response completed successfully.");
});
