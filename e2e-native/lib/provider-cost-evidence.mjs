import assert from "node:assert/strict";
import fs from "node:fs/promises";
import { DatabaseSync } from "node:sqlite";

function cost(value) {
  if (value === undefined || value === null) return null;
  assert.ok(typeof value === "number" && Number.isFinite(value) && value >= 0,
    "Invalid native/telemetry cost; unavailable is null, never invented zero");
  return value;
}

function unique(rows, label) {
  const map = new Map();
  for (const row of rows) {
    assert.ok(typeof row.turn_id === "string" && row.turn_id.length, `${label}: missing native turn ID`);
    assert.ok(!map.has(row.turn_id), `${label}: duplicate native turn ID`);
    map.set(row.turn_id, cost(row.cost_usd));
  }
  return map;
}

/** Exact native-turn parity, including unavailable costs. Not a UI or invoice oracle. */
export function assertCostParity(native, ingested) {
  const expected = unique(native, "native");
  const actual = unique(ingested, "telemetry");
  assert.ok(expected.size > 0, "No completed native turns; cannot pass vacuously");
  assert.deepEqual([...actual.keys()].sort(), [...expected.keys()].sort(),
    "Completed native/telemetry turn sets differ (missing, stale, or duplicated ingestion)");
  for (const [id, value] of expected) {
    const observed = actual.get(id);
    if (value === null || observed === null) assert.equal(observed, value, "Cost availability differs");
    else assert.ok(Math.abs(value - observed) <= 1e-10 * Math.max(1, value), "Native cost differs from ingestion");
  }
  const reported = [...expected.values()].filter((value) => value !== null);
  return { turns: expected.size, reported_turns: reported.length,
    unavailable_turns: expected.size - reported.length,
    cost_usd: reported.length ? reported.reduce((sum, value) => sum + value, 0) : null,
    scope: "native cost ingestion/availability; no cost display or invoice claim" };
}

async function readOnly(file, inspect) {
  const db = new DatabaseSync(file, { readOnly: true });
  try { db.exec("BEGIN"); return await inspect(db); }
  finally { db.close(); }
}

function openCodeCosts(db, providerSessionId) {
  assert.equal(db.prepare("SELECT id FROM session WHERE id = ?").all(providerSessionId).length, 1,
    "Native session missing or ambiguous");
  return db.prepare("SELECT id, data FROM message WHERE session_id = ? ORDER BY time_created, id")
    .all(providerSessionId).flatMap((row) => {
      const message = JSON.parse(row.data);
      return message.role === "assistant" && Number.isFinite(message.time?.completed)
        ? [{ turn_id: row.id, cost_usd: cost(message.cost) }] : [];
    });
}

/** A shared database source is not a session boundary. Prove every turn's owner
 * before excluding another session; unknown IDs must not disappear by intersection. */
function openCodeSessionTurns(db, rows, providerSessionId) {
  const ownerQuery = db.prepare("SELECT session_id, data FROM message WHERE id = ?");
  const sessionQuery = db.prepare("SELECT id FROM session WHERE id = ?");
  return rows.filter((row) => {
    const owners = ownerQuery.all(row.turn_id);
    assert.equal(owners.length, 1, "Unknown or ambiguous native owner for telemetry turn");
    const owner = owners[0];
    assert.ok(typeof owner.session_id === "string" && owner.session_id.length,
      "Unknown native session owner for telemetry turn");
    assert.equal(sessionQuery.all(owner.session_id).length, 1,
      "Unknown or ambiguous native session owner for telemetry turn");
    const message = JSON.parse(owner.data);
    assert.ok(message.role === "assistant" && Number.isFinite(message.time?.completed),
      "Telemetry turn is not a completed native assistant message");
    return owner.session_id === providerSessionId;
  });
}

/** Read an exact, already-established provider session. No discovery or provider execution. */
export async function readNativeCosts({ provider, nativePath, providerSessionId }) {
  assert.ok(providerSessionId?.trim(), "Exact provider session required");
  if (provider === "opencode") return readOnly(nativePath, (db) => openCodeCosts(db, providerSessionId));
  if (provider === "pi") {
    const records = (await fs.readFile(nativePath, "utf8")).split(/\r?\n/).filter(Boolean).map(JSON.parse);
    const headers = records.filter((row) => row.type === "session");
    assert.equal(headers.length, 1, "Pi transcript must have one session header");
    assert.equal(headers[0].id, providerSessionId, "Pi source belongs to another session");
    // An assistant record without usage is not an accounting record. A usage
    // block without cost, however, explicitly retains unavailable cost.
    return records.filter((row) => row.type === "message" && row.message?.role === "assistant" && row.timestamp && row.message.usage)
      .map((row) => ({ turn_id: row.message.responseId || row.id, cost_usd: cost(row.message.usage?.cost?.total) }));
  }
  // These adapters currently have no native cost field. Do not manufacture a
  // row or a successful ingestion claim merely because cost is unavailable.
  if (["claude", "codex"].includes(provider)) return { availability: "unavailable", reason: "No supported native cost field", turns: null };
  if (["antigravity", "gemini"].includes(provider)) return { availability: "excluded", reason: provider === "antigravity" ? "Intentional accounting exclusion" : "Retired provider" };
  throw new Error("Unsupported provider");
}

async function boundTelemetryTurns({ provider, nativePath, providerSessionId, statePath, wardianSessionId }) {
  assert.ok(wardianSessionId?.trim(), "Exact Wardian agent required");
  const realSource = await fs.realpath(nativePath);
  return readOnly(statePath, async (db) => {
    const sources = db.prepare("SELECT source_key, source_path, provider_session_id FROM telemetry_sources WHERE session_id = ? AND provider = ?")
      .all(wardianSessionId, provider);
    const keys = new Set();
    for (const source of sources) {
      if (await fs.realpath(source.source_path) !== realSource) continue;
      // Pi's exact per-file header was already verified by readNativeCosts.
      // Null ingestion metadata is absent evidence, not a competing identity.
      if (provider === "pi") assert.ok(source.provider_session_id === null || source.provider_session_id === providerSessionId,
        "Pi telemetry source conflicts with the native session header");
      // OpenCode metadata describes the latest session in this shared DB.
      // Individual message.session_id ownership is checked separately.
      keys.add(source.source_key);
    }
    assert.ok(keys.size > 0, "No telemetry source binds the exact provider session, agent, and native path");
    const rows = db.prepare("SELECT turn_id, cost_usd, source_key FROM telemetry_turns WHERE session_id = ? AND provider = ?")
      .all(wardianSessionId, provider);
    return rows.filter((row) => keys.has(row.source_key)).map(({ turn_id, cost_usd }) => ({ turn_id, cost_usd }));
  });
}

/** Match agent/provider/realpath, then prove provider-specific session ownership.
 * Native OpenCode costs and turn ownership use the same read-only DB snapshot. */
export async function readRetainedCostEvidence(options) {
  const { provider, nativePath, providerSessionId } = options;
  assert.ok(providerSessionId?.trim(), "Exact provider session required");
  if (provider === "opencode") return readOnly(nativePath, async (db) => {
    const native = openCodeCosts(db, providerSessionId);
    const ingested = openCodeSessionTurns(db, await boundTelemetryTurns(options), providerSessionId);
    return { native, ingested, parity: assertCostParity(native, ingested) };
  });
  const native = await readNativeCosts(options);
  if (!Array.isArray(native)) return native;
  const ingested = await boundTelemetryTurns(options);
  return { native, ingested, parity: assertCostParity(native, ingested) };
}
