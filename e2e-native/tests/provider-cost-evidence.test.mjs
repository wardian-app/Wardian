// @tier nightly — deterministic sanitized SQLite/JSONL evidence; no provider or WebDriver.
import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { DatabaseSync } from "node:sqlite";
import { assertCostParity, readNativeCosts, readRetainedCostEvidence } from "../lib/provider-cost-evidence.mjs";

const retained = JSON.parse(await fs.readFile(new URL("./fixtures/provider-cost-retained.json", import.meta.url), "utf8"));
const sessionBinding = JSON.parse(await fs.readFile(new URL("./fixtures/provider-cost-session-binding-retained.json", import.meta.url), "utf8"));

async function fixture(t) {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), "wardian-cost-evidence-"));
  t.after(() => fs.rm(root, { recursive: true, force: true })); // Exact directory created above.
  const nativePath = path.join(root, "opencode.db");
  const statePath = path.join(root, "state.db");
  const db = new DatabaseSync(nativePath);
  db.exec("CREATE TABLE session(id TEXT); CREATE TABLE message(id TEXT, session_id TEXT, data TEXT, time_created INTEGER)");
  db.prepare("INSERT INTO session VALUES (?)").run("ses_observed");
  for (const row of retained.native_messages) db.prepare("INSERT INTO message VALUES (?,?,?,?)")
    .run(row.id, "ses_observed", JSON.stringify(row.data), row.time_created);
  db.close();
  const state = new DatabaseSync(statePath);
  state.exec("CREATE TABLE telemetry_sources(source_key TEXT, source_path TEXT, provider_session_id TEXT, session_id TEXT, provider TEXT); CREATE TABLE telemetry_turns(turn_id TEXT, cost_usd REAL, source_key TEXT, session_id TEXT, provider TEXT)");
  state.prepare("INSERT INTO telemetry_sources VALUES (?,?,?,?,?)").run("source", nativePath, "ses_observed", "agent", "opencode");
  for (const row of retained.ingested) state.prepare("INSERT INTO telemetry_turns VALUES (?,?,?,?,?)")
    .run(row.turn_id, row.cost_usd, "source", "agent", "opencode");
  state.close();
  return { provider: "opencode", nativePath, statePath, providerSessionId: "ses_observed", wardianSessionId: "agent" };
}

test("retained OpenCode zero-cost snapshot exposes missing resumed turn, without changing either database", async (t) => {
  const options = await fixture(t);
  const before = await Promise.all([fs.readFile(options.nativePath), fs.readFile(options.statePath)]);
  const native = await readNativeCosts(options);
  assert.equal(native.length, 4);
  assert.equal(retained.ingested.length, 3);
  assert.ok(native.every((row) => row.cost_usd === 0));
  assert.equal(assertCostParity(native.slice(0, 3), retained.ingested).reported_turns, 3,
    "The genuine initial three-turn cohort has matching explicit zero costs");
  await assert.rejects(readRetainedCostEvidence(options), /turn sets differ/);
  assert.deepEqual(await Promise.all([fs.readFile(options.nativePath), fs.readFile(options.statePath)]), before);
  // Synthetic completion of the missing ingest row tests the positive branch;
  // it is not claimed as a second genuine retained observation.
  const db = new DatabaseSync(options.statePath);
  db.prepare("INSERT INTO telemetry_turns VALUES (?,?,?,?,?)").run(native[3].turn_id, 0, "source", "agent", "opencode");
  db.close();
  const first = await readRetainedCostEvidence(options);
  assert.equal(first.parity.reported_turns, 4);
  assert.equal(first.parity.cost_usd, 0);
  assert.deepEqual((await readRetainedCostEvidence(options)).parity, first.parity);
});

test("cost reader requires exact agent, provider session and source binding", async (t) => {
  const options = await fixture(t);
  await assert.rejects(readRetainedCostEvidence({ ...options, wardianSessionId: "another" }), /No telemetry source/);
  await assert.rejects(readNativeCosts({ ...options, providerSessionId: "another" }), /Native session missing/);
  const db = new DatabaseSync(options.statePath);
  db.exec("UPDATE telemetry_sources SET provider_session_id='another'"); db.close();
  await assert.rejects(readRetainedCostEvidence(options), /turn sets differ/,
    "OpenCode source metadata is not a per-turn session boundary; the missing native turn still fails");
  const otherPath = path.join(path.dirname(options.nativePath), "other.db");
  await fs.copyFile(options.nativePath, otherPath);
  await assert.rejects(readRetainedCostEvidence({ ...options, nativePath: otherPath }), /No telemetry source/);
});

async function bindingFixture(t, provider) {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), "wardian-cost-session-binding-"));
  t.after(() => fs.rm(root, { recursive: true, force: true })); // Only this exact created directory.
  const capture = sessionBinding[provider];
  const nativePath = path.join(root, provider === "pi" ? "pi.jsonl" : "opencode.db");
  if (provider === "pi") await fs.writeFile(nativePath, [capture.header, ...capture.records].map(JSON.stringify).join("\n"));
  else {
    const native = new DatabaseSync(nativePath);
    native.exec("CREATE TABLE session(id TEXT); CREATE TABLE message(id TEXT, session_id TEXT, data TEXT, time_created INTEGER)");
    for (const id of capture.sessions) native.prepare("INSERT INTO session VALUES (?)").run(id);
    for (const row of capture.native_messages) native.prepare("INSERT INTO message VALUES (?,?,?,?)")
      .run(row.id, row.session_id, JSON.stringify(row.data), row.time_created);
    native.close();
  }
  const statePath = path.join(root, "state.db");
  const state = new DatabaseSync(statePath);
  state.exec("CREATE TABLE telemetry_sources(source_key TEXT, source_path TEXT, provider_session_id TEXT, session_id TEXT, provider TEXT); CREATE TABLE telemetry_turns(turn_id TEXT, cost_usd REAL, source_key TEXT, session_id TEXT, provider TEXT)");
  state.prepare("INSERT INTO telemetry_sources VALUES (?,?,?,?,?)").run("source", nativePath, capture.source_provider_session_id, "agent", provider);
  for (const row of capture.ingested) state.prepare("INSERT INTO telemetry_turns VALUES (?,?,?,?,?)")
    .run(row.turn_id, row.cost_usd, "source", "agent", provider);
  state.close();
  return { provider, nativePath, statePath, providerSessionId: provider === "pi" ? capture.header.id : "ses_original", wardianSessionId: "agent" };
}

test("retained shared OpenCode source selects proven original ownership and exposes missing fresh turns", async (t) => {
  const options = await bindingFixture(t, "opencode");
  const before = await Promise.all([fs.readFile(options.nativePath), fs.readFile(options.statePath)]);
  const original = await readRetainedCostEvidence(options);
  assert.equal(sessionBinding.opencode.source_provider_session_id, "ses_fresh");
  assert.equal(original.parity.turns, 3);
  assert.equal(original.parity.cost_usd, 0);
  assert.deepEqual(original.ingested.map((row) => row.turn_id), sessionBinding.opencode.ingested.slice(0, 3).map((row) => row.turn_id));
  const fresh = { ...options, providerSessionId: "ses_fresh" };
  assert.equal((await readNativeCosts(fresh)).length, 4);
  await assert.rejects(readRetainedCostEvidence(fresh), /turn sets differ/,
    "Two proven fresh telemetry turns must not intersect away two missing completed native messages");
  assert.deepEqual(await Promise.all([fs.readFile(options.nativePath), fs.readFile(options.statePath)]), before);
});

test("OpenCode rejects unknown and ambiguous native ownership even for rows outside the requested session", async (t) => {
  // Deliberate corruption of a genuine fixture tests failure branches, not new observations.
  for (const mutation of ["unknown-turn", "ambiguous-turn", "unknown-session", "ambiguous-session", "not-completed"]) {
    const options = await bindingFixture(t, "opencode");
    const other = sessionBinding.opencode.native_messages.find((row) => row.session_id === "ses_fresh");
    if (mutation === "unknown-turn") {
      const state = new DatabaseSync(options.statePath);
      state.prepare("INSERT INTO telemetry_turns VALUES (?,?,?,?,?)").run("unknown", 0, "source", "agent", "opencode"); state.close();
    } else {
      const native = new DatabaseSync(options.nativePath);
      if (mutation === "ambiguous-turn") native.prepare("INSERT INTO message VALUES (?,?,?,?)")
        .run(other.id, "ses_original", JSON.stringify(other.data), other.time_created);
      if (mutation === "unknown-session") native.exec("DELETE FROM session WHERE id='ses_fresh'");
      if (mutation === "ambiguous-session") native.exec("INSERT INTO session VALUES ('ses_fresh')");
      if (mutation === "not-completed") native.prepare("UPDATE message SET data=? WHERE id=?")
        .run(JSON.stringify({ role: "assistant", time: { created: other.time_created } }), other.id);
      native.close();
    }
    await assert.rejects(readRetainedCostEvidence(options), /Unknown or ambiguous|not a completed/, mutation);
  }
});

test("OpenCode ownership filtering cannot hide missing or duplicated requested-session ingestion", async (t) => {
  for (const mutation of ["missing", "duplicate"]) {
    const options = await bindingFixture(t, "opencode");
    const state = new DatabaseSync(options.statePath);
    const first = sessionBinding.opencode.ingested[0];
    if (mutation === "missing") state.prepare("DELETE FROM telemetry_turns WHERE turn_id=?").run(first.turn_id);
    else state.prepare("INSERT INTO telemetry_turns VALUES (?,?,?,?,?)").run(first.turn_id, first.cost_usd, "source", "agent", "opencode");
    state.close();
    await assert.rejects(readRetainedCostEvidence(options), /turn sets differ|duplicate native turn ID/, mutation);
  }
});

test("retained Pi null metadata uses its exact header and still reports the missing resumed accounting record", async (t) => {
  const options = await bindingFixture(t, "pi");
  const before = await Promise.all([fs.readFile(options.nativePath), fs.readFile(options.statePath)]);
  const native = await readNativeCosts(options);
  assert.equal(native.length, 3);
  assert.equal(sessionBinding.pi.source_provider_session_id, null);
  assert.equal(assertCostParity(native.slice(0, 2), sessionBinding.pi.ingested).reported_turns, 2);
  assert.ok(native.every((row) => row.cost_usd > 0));
  await assert.rejects(readRetainedCostEvidence(options), /turn sets differ/,
    "The header binds the file, but full native accounting must not be intersected with ingestion");
  assert.deepEqual(await Promise.all([fs.readFile(options.nativePath), fs.readFile(options.statePath)]), before);
  // Synthetic completion of ingestion proves positive null-metadata binding separately.
  const state = new DatabaseSync(options.statePath);
  state.prepare("INSERT INTO telemetry_turns VALUES (?,?,?,?,?)").run(native[2].turn_id, native[2].cost_usd, "source", "agent", "pi"); state.close();
  const complete = await readRetainedCostEvidence(options);
  assert.equal(complete.parity.turns, 3);
  assert.equal(complete.parity.cost_usd, native.reduce((sum, row) => sum + row.cost_usd, 0));
});

test("Pi header fallback never overrides contradictory metadata or wrong agent, provider, or realpath", async (t) => {
  const options = await bindingFixture(t, "pi");
  await assert.rejects(readRetainedCostEvidence({ ...options, providerSessionId: "wrong" }), /another session/);
  await assert.rejects(readRetainedCostEvidence({ ...options, wardianSessionId: "wrong" }), /No telemetry source/);
  const otherPath = path.join(path.dirname(options.nativePath), "other.jsonl");
  await fs.copyFile(options.nativePath, otherPath);
  await assert.rejects(readRetainedCostEvidence({ ...options, nativePath: otherPath }), /No telemetry source/);
  const state = new DatabaseSync(options.statePath);
  state.exec("UPDATE telemetry_sources SET provider='opencode'");
  await assert.rejects(readRetainedCostEvidence(options), /No telemetry source/);
  state.exec("UPDATE telemetry_sources SET provider='pi', provider_session_id='conflicting'"); state.close();
  await assert.rejects(readRetainedCostEvidence(options), /conflicts with the native session header/);
});

test("cost parity rejects incomplete, duplicated, malformed, and zero-vs-unavailable accounting", () => {
  const known = [{ turn_id: "a", cost_usd: 0 }, { turn_id: "b", cost_usd: 0.25 }];
  assert.equal(assertCostParity(known, known).cost_usd, 0.25);
  for (const actual of [[], known.slice(0, 1), [...known, known[0]], [{ turn_id: "a", cost_usd: null }, known[1]],
    [known[0], { turn_id: "b", cost_usd: 0.26 }]]) assert.throws(() => assertCostParity(known, actual));
  for (const value of [-1, NaN, Infinity, "0"]) assert.throws(() => assertCostParity([{ turn_id: "a", cost_usd: value }], []), /Invalid/);
  assert.throws(() => assertCostParity([], []), /vacuously/);
  const unavailable = [{ turn_id: "a", cost_usd: null }];
  assert.equal(assertCostParity(unavailable, unavailable).cost_usd, null);
  assert.throws(() => assertCostParity(unavailable, [{ turn_id: "a", cost_usd: 0 }]), /availability/);
});

test("Pi reader binds header and response identity; absent native cost stays unavailable", async (t) => {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), "wardian-pi-cost-"));
  t.after(() => fs.rm(root, { recursive: true, force: true }));
  const nativePath = path.join(root, "session.jsonl");
  // Synthetic schema tests, not genuine provider acceptance evidence.
  await fs.writeFile(nativePath, [
    { type: "session", id: "pi-session" },
    { type: "message", id: "record", timestamp: "2026-09-07T12:00:00Z", message: { role: "assistant", responseId: "response", usage: { cost: { total: 0.125 } } } },
    { type: "message", id: "missing-cost", timestamp: "2026-09-07T12:01:00Z", message: { role: "assistant", usage: {} } },
    { type: "message", id: "no-accounting", timestamp: "2026-09-07T12:01:01Z", message: { role: "assistant" } },
  ].map(JSON.stringify).join("\n"));
  const options = { provider: "pi", nativePath, providerSessionId: "pi-session" };
  assert.deepEqual(await readNativeCosts(options), [{ turn_id: "response", cost_usd: 0.125 }, { turn_id: "missing-cost", cost_usd: null }]);
  await assert.rejects(readNativeCosts({ ...options, providerSessionId: "wrong" }), /another session/);
  for (const provider of ["claude", "codex"]) {
    assert.deepEqual(await readNativeCosts({ provider, providerSessionId: "bound" }),
      { availability: "unavailable", reason: "No supported native cost field", turns: null });
  }
  for (const provider of ["antigravity", "gemini"]) assert.equal((await readNativeCosts({ provider, providerSessionId: "bound" })).availability, "excluded");
});
