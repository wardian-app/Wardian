// @tier nightly — native-source fixtures only; never launches a provider or WebDriver.
import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { pathToFileURL } from "node:url";
import { DatabaseSync } from "node:sqlite";
import {
  createHeadlessEvidenceReader, assessHeadlessEvidence, matchNativePrompt,
  observeHeadlessEvidence, runIndependentHeadlessCases, EvidenceBlocked,
  assessPausedHeadlessOwner,
  assessAntigravityAliases,
} from "../lib/provider-headless-evidence.mjs";

const retained = JSON.parse(await fs.readFile(new URL("./fixtures/provider-headless-evidence.json", import.meta.url), "utf8"));
const AGENT = "fixture-agent";
const copy = (value) => structuredClone(value);

test("Agy inherited aliases require both exact agent-specific paths, never class/common or foreign aliases", () => {
  // CcdDfP retained metadata: original has workspace + four projected include
  // URIs; fresh has only includes00-common,01-testclass,02-agent,03-habitat.
  const prefix = "/temp/wardian-antigravity/owned-agent/include/";
  const expected = [`${prefix}02-owned-agent`, `${prefix}03-habitat`];
  const shared = [`${prefix}00-common`, `${prefix}01-testclass`];
  const original = ["/isolated/workspace", ...shared, ...expected];
  assert.equal(assessAntigravityAliases([...shared, ...expected], original, expected), true);
  for (const candidate of [
    shared, [expected[0]], [expected[1]], [...expected, expected[0]],
    expected.map((uri) => uri.replaceAll("owned-agent", "foreign-agent")),
    [...expected, "/temp/wardian-antigravity/foreign-agent/include/02-foreign-agent"],
  ]) assert.equal(assessAntigravityAliases(candidate, original, expected), false);
  assert.equal(assessAntigravityAliases(expected, shared, expected), false);
  assert.equal(assessAntigravityAliases(expected, original, [expected[0], expected[0]]), false);
});

test("acknowledged pause uses lifecycle is_off despite retained Agy Off-to-Idle telemetry", () => {
  // VT0Q9X settings/state.json retained is_off=true; state.db status_change
  // records Off then Idle at 18:02:29. Metrics uses current_status (not is_off).
  const agentId = "a6d4a26b-de26-457a-aa94-abe81fd3f069";
  const input = { agentId, pauseAcknowledged: true,
    agents: [{ session_id: agentId, is_off: true }],
    metrics: [{ session_id: agentId, current_status: "Idle" }] };
  const result = assessPausedHeadlessOwner(input);
  assert.equal(result.status, "pass");
  assert.equal(result.evidence.telemetry_differs_from_lifecycle, true);
  assert.equal(assessPausedHeadlessOwner({ ...input, metrics: [{ session_id: agentId, current_status: "Off" }] }).status, "pass");
  for (const override of [
    { pauseAcknowledged: false }, { agents: [] }, { metrics: [] },
    { agents: [...input.agents, ...input.agents] }, { metrics: [...input.metrics, ...input.metrics] },
    { agents: [{ session_id: agentId, is_off: false }] },
    { agents: [{ session_id: agentId, is_off: "true" }] },
    { metrics: [{ session_id: agentId, status: "Off" }] },
    { metrics: [{ session_id: "foreign-agent", current_status: "Off" }] },
    ...["Headless", "Processing...", "Action Needed", "Error"].map((current_status) => ({ metrics: [{ session_id: agentId, current_status }] })),
  ]) assert.equal(assessPausedHeadlessOwner({ ...input, ...override }).status, "blocked");
});
const varint = (value) => {
  const bytes = [];
  do { let byte = value & 127; value = Math.floor(value / 128); if (value) byte |= 128; bytes.push(byte); } while (value);
  return Buffer.from(bytes);
};
const number = (key, value) => Buffer.concat([varint(key * 8), varint(value)]);
const message = (key, value) => { const bytes = Buffer.from(value); return Buffer.concat([varint(key * 8 + 2), varint(bytes.length), bytes]); };
function wire(step) {
  const content = step.step_type === 14 ? message(19, message(2, step.text)) : step.step_type === 15 ? message(20, message(1, step.text)) : Buffer.alloc(0);
  return Buffer.concat([number(1, step.step_type), number(4, step.status), message(5, number(3, step.source)), content]);
}
async function isolated(t) {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), "wardian-headless-evidence-test-"));
  t.after(() => fs.rm(root, { recursive: true, force: true }));
  const workspace = path.join(root, "workspace"); await fs.mkdir(workspace);
  return { root, workspace };
}
function writeOpenCode(file, fixture, workspace) {
  const db = new DatabaseSync(file);
  db.exec("CREATE TABLE session(id TEXT PRIMARY KEY,directory TEXT); CREATE TABLE message(id TEXT PRIMARY KEY,session_id TEXT,data TEXT,time_created INTEGER,time_updated INTEGER); CREATE TABLE part(id TEXT PRIMARY KEY,message_id TEXT,session_id TEXT,data TEXT,time_created INTEGER,time_updated INTEGER)");
  db.prepare("INSERT INTO session VALUES(?,?)").run(fixture.session.id, workspace);
  for (const row of fixture.messages) db.prepare("INSERT INTO message VALUES(?,?,?,?,?)").run(row.id, row.session_id, JSON.stringify(row.data), row.time_created, row.time_updated);
  for (const row of fixture.parts) db.prepare("INSERT INTO part VALUES(?,?,?,?,?,?)").run(row.id, row.message_id, row.session_id, JSON.stringify(row.data), row.time_created, row.time_updated);
  db.close();
}
function writeAgy(file, fixture, workspace) {
  const db = new DatabaseSync(file);
  db.exec("CREATE TABLE trajectory_meta(cascade_id TEXT); CREATE TABLE trajectory_metadata_blob(id TEXT,data BLOB); CREATE TABLE steps(idx INTEGER,step_type INTEGER,status INTEGER,step_payload BLOB)");
  db.prepare("INSERT INTO trajectory_meta VALUES(?)").run(fixture.session);
  db.prepare("INSERT INTO trajectory_metadata_blob VALUES('main',?)").run(message(1, message(1, pathToFileURL(workspace).href)));
  for (const step of fixture.steps) db.prepare("INSERT INTO steps VALUES(?,?,?,?)").run(step.idx, step.step_type, step.status, wire(step));
  db.close();
}
function agyMetadata(file, uris) {
  const db = new DatabaseSync(file);
  db.prepare("UPDATE trajectory_metadata_blob SET data=? WHERE id='main'").run(
    Buffer.concat(uris.map((uri) => message(1, message(1, uri)))));
  db.close();
}
test("retained fresh Agy steps require corroborated config/launch aliases and full inventory novelty", async (t) => {
  const { root, workspace } = await isolated(t);
  const agentId = path.basename(root);
  const agentRoot = path.join(root, "agents", agentId);
  await fs.mkdir(path.join(agentRoot, "habitat"), { recursive: true });
  const nativeRoot = path.join(root, "native");
  await fs.mkdir(path.join(nativeRoot, "conversations"), { recursive: true });
  const originalFile = path.join(nativeRoot, "conversations", `${retained.antigravity.session}.db`);
  const freshFile = path.join(nativeRoot, "conversations", `${retained.antigravity_fresh.session}.db`);
  writeAgy(originalFile, retained.antigravity, workspace);
  const aliasRoot = path.join(os.tmpdir(), "wardian-antigravity", agentId.toLowerCase(), "include");
  const aliases = [path.join(aliasRoot, `02-${agentId.toLowerCase()}`), path.join(aliasRoot, "03-habitat")].map((dir) => pathToFileURL(dir).href);
  agyMetadata(originalFile, [pathToFileURL(workspace).href, ...aliases]);
  const reader = await createHeadlessEvidenceReader({ provider: "antigravity", isolatedHome: root, workspace,
    env: { WARDIAN_E2E_CONTEXT_ANTIGRAVITY_HOME: nativeRoot } });
  const originalSession = retained.antigravity.session;
  const before = await reader.snapshot({ agentId, originalSession });
  const config = { session_id: agentId, provider: "antigravity", folder: workspace, resume_session: originalSession,
    system_include_directories: [path.join(root, "common"), path.join(root, "class"), agentRoot] };
  assert.equal((await reader.preflightFreshAliases({ agentId, originalSession, before, agentConfig: config })).status, "pass");
  const executionId = "automation-bg-fixture-123-provider-turn";
  const cwd = path.join(root, "agents", executionId, "habitat", "workspace");
  await fs.mkdir(path.dirname(cwd), { recursive: true });
  await fs.symlink(workspace, cwd, process.platform === "win32" ? "junction" : "dir");
  const log = `[automation] node provider-turn: running assigned agent ${agentId} as a fresh background conversation\n` +
    `[Wardian] run_headless: provider=antigravity, session_id=${executionId}, cwd=${cwd}, prompt_len=335, output_format=json\n` +
    "[Wardian] run_headless launch: exe=fixture, arg_count=13, resume=false\n";
  await fs.writeFile(path.join(root, "wardian_debug.log"), log);
  writeAgy(freshFile, retained.antigravity_fresh, workspace);
  agyMetadata(freshFile, aliases);
  const request = { agentId, originalSession, mode: "fresh", before, launch: { agentConfig: config, executionId } };
  const after = await reader.snapshot(request);
  assert.equal(after.sessions.length, 1);
  assert.equal(after.sessions[0].ownership_basis, "corroborated_agent_specific_include_aliases");
  const prompt = retained.antigravity_fresh.steps[0].text;
  const text = retained.antigravity_fresh.steps[1].text;
  const marker = text.slice("UNKNOWN|".length);
  const input = { provider: "antigravity", before, after, originalSession, mode: "fresh", prompt, marker, secret: "absent-secret", output: { text } };
  assert.equal(assessHeadlessEvidence(input).status, "pass");
  const old = { ...before, inventory: [...before.inventory, retained.antigravity_fresh.session] };
  assert.equal((await reader.snapshot({ ...request, before: old })).sessions.length, 0);
  assert.equal(assessHeadlessEvidence({ ...input, before: old }).status, "blocked");
  for (const candidate of [[aliases[0]], aliases.map((uri) => uri.replaceAll(agentId.toLowerCase(), "foreign-agent")), [...aliases, aliases[0]]]) {
    agyMetadata(freshFile, candidate);
    assert.equal((await reader.snapshot(request)).sessions.length, 0);
  }
  agyMetadata(freshFile, aliases);
  await assert.rejects(reader.snapshot({ ...request, launch: { ...request.launch, agentConfig: { ...config, session_id: "foreign" } } }), EvidenceBlocked);
  await fs.writeFile(path.join(root, "wardian_debug.log"), log.replace("resume=false", "resume=true"));
  await assert.rejects(reader.snapshot(request), EvidenceBlocked);
  await fs.writeFile(path.join(root, "wardian_debug.log"), log + log);
  await assert.rejects(reader.snapshot(request), EvidenceBlocked);
});
async function databaseCase(t, provider) {
  const { root, workspace } = await isolated(t);
  const fixture = copy(retained[provider]); const env = {};
  let file;
  if (provider === "opencode") {
    file = path.join(root, "opencode.db"); writeOpenCode(file, fixture, workspace);
    env.WARDIAN_E2E_CONTEXT_OPENCODE_DB = file;
  } else {
    await fs.mkdir(path.join(root, "conversations"));
    file = path.join(root, "conversations", `${fixture.session}.db`); writeAgy(file, fixture, workspace);
    env.WARDIAN_E2E_CONTEXT_ANTIGRAVITY_HOME = root;
  }
  const originalSession = provider === "opencode" ? fixture.session.id : fixture.session;
  const reader = await createHeadlessEvidenceReader({ provider, isolatedHome: root, workspace, env });
  const after = await reader.snapshot({ agentId: AGENT, originalSession });
  const before = copy(after);
  before.sessions[0].records = before.sessions[0].records.filter((row) => provider === "opencode" ? fixture.baseline_message_ids.includes(row.id) : row.ordinal <= fixture.baseline_last_step);
  const session = after.sessions[0];
  const request = session.records.findLast((row) => row.role === "user");
  const answer = session.records.findLast((row) => row.role === "assistant");
  const prompt = provider === "opencode" ? JSON.parse(request.text) : request.text;
  const marker = prompt.match(/WARDIAN_CONTEXT_HEADLESS_RESUME_[\w-]+/)[0];
  const secret = session.records.find((row) => row.role === "user").text.match(/WARDIAN_CONTEXT_EPHEMERAL_RESUME_SECRET_[\w-]+/)[0];
  const input = { provider, before, after, originalSession, mode: "current", prompt, marker, secret, output: { text: answer.text } };
  return { input, reader, file, root, workspace, fixture };
}

test("retained OpenCode new parent-bound ACK proves authorship but fails recall", async (t) => {
  const { input, reader, file } = await databaseCase(t, "opencode");
  const beforeBytes = await fs.readFile(file);
  const result = assessHeadlessEvidence(input);
  assert.equal(result.status, "fail"); assert.equal(result.classification, "answer_mismatch");
  assert.equal(result.checks.native_session_identity, true);
  assert.equal(result.checks.assistant_authorship, true);
  assert.equal(result.checks.output_matches_native_answer, true);
  assert.equal(result.checks.expected_answer, false);
  assert.equal(result.evidence.prompt_transport, "opencode_single_json_string");
  assert.equal(result.evidence.answer_id, "msg_07cd39ab1001GHj2m6uXh9STIv");
  assert.equal((await observeHeadlessEvidence(reader, { ...input, agentId: AGENT }, { timeoutMs: 0 })).status, "fail");
  assert.deepEqual(await fs.readFile(file), beforeBytes, "read-only DB remains unchanged");
});

test("retained Agy cascade/source2 completion with system step between request and answer passes", async (t) => {
  const { input, file } = await databaseCase(t, "antigravity");
  const bytes = await fs.readFile(file);
  const result = assessHeadlessEvidence(input);
  assert.equal(result.status, "pass"); assert.equal(result.evidence.request_id, "8"); assert.equal(result.evidence.answer_id, "10");
  assert.equal(result.evidence.causal_binding, "ordered_native_step_interval");
  assert.deepEqual(await fs.readFile(file), bytes);
});

test("full prompt comparison accepts exactly one explicit OpenCode JSON string", () => {
  const prompt = 'one "quoted" prompt\nline';
  assert.deepEqual(matchNativePrompt("opencode", prompt, prompt), { matched: true, transport: "literal" });
  assert.equal(matchNativePrompt("opencode", JSON.stringify(prompt), prompt).transport, "opencode_single_json_string");
  for (const actual of [JSON.stringify(JSON.stringify(prompt)), `prefix${prompt}`, `${prompt}\n`, `[${JSON.stringify(prompt)}]`, ` ${JSON.stringify(prompt)}`, `${JSON.stringify(prompt)}\n`]) {
    assert.equal(matchNativePrompt("opencode", actual, prompt).matched, false);
  }
  for (const provider of ["claude", "codex", "pi", "antigravity"]) assert.equal(matchNativePrompt(provider, JSON.stringify(prompt), prompt).matched, false);
});

test("stale, echoed, tool, incomplete, wrong-parent and foreign native evidence never pass", async (t) => {
  const { input } = await databaseCase(t, "opencode");
  const expected = `${input.secret}|${input.marker}`;
  for (const mutate of [
    (value) => { value.after.sessions[0].records.findLast((row) => row.role === "assistant").role = "user"; },
    (value) => { value.after.sessions[0].records.findLast((row) => row.role === "assistant").role = "tool"; },
    (value) => { value.after.sessions[0].records.findLast((row) => row.role === "assistant").complete = false; },
    (value) => { value.after.sessions[0].records.findLast((row) => row.role === "assistant").parent = "different-user"; },
    (value) => { value.after.sessions[0].id = "foreign-session"; },
    (value) => { value.before = copy(value.after); },
    (value) => { value.after.sessions.push(copy(value.after.sessions[0])); },
  ]) {
    const value = copy(input); value.output.text = expected;
    value.after.sessions[0].records.findLast((row) => row.role === "assistant").text = expected;
    mutate(value);
    assert.equal(assessHeadlessEvidence(value).status, "blocked");
  }
  const value = copy(input); value.output.text = `${expected}\n${JSON.stringify(value.after)}`;
  assert.equal(assessHeadlessEvidence(value).status, "fail", "raw history containing both strings is not an answer");
});

test("packet-looking node output cannot become Claude or any provider authorship", async (t) => {
  const { input } = await databaseCase(t, "antigravity");
  const packets = [{ type: "system", subtype: "init", session_id: input.originalSession },
    { type: "assistant", message: { role: "assistant", content: [{ type: "text", text: input.output.text }] } }];
  for (const output of [packets, { raw: JSON.stringify(packets) }, { response: input.output.text }, { text: JSON.stringify(packets) }]) {
    const result = assessHeadlessEvidence({ ...input, output });
    assert.notEqual(result.status, "pass");
  }
  const absent = assessHeadlessEvidence({ ...input, after: { ...input.after, sessions: [] }, output: packets });
  assert.equal(absent.status, "blocked");
});

test("fresh native identity at unchanged DB path is independent of recall mismatch", async (t) => {
  const { input, reader, file, workspace, fixture } = await databaseCase(t, "opencode");
  const id = "ses_fresh_fixture"; const marker = "NEW_FRESH_MARKER"; const prompt = `Return UNKNOWN|${marker}`;
  const db = new DatabaseSync(file);
  db.prepare("INSERT INTO session VALUES(?,?)").run(id, workspace);
  db.prepare("INSERT INTO message VALUES(?,?,?,?,?)").run("fresh-user", id, JSON.stringify({ role: "user" }), 100, 100);
  db.prepare("INSERT INTO message VALUES(?,?,?,?,?)").run("fresh-assistant", id, JSON.stringify({ role: "assistant", parentID: "fresh-user", time: { completed: 102 }, finish: "stop" }), 101, 102);
  db.prepare("INSERT INTO part VALUES(?,?,?,?,?,?)").run("fresh-user-text", "fresh-user", id, JSON.stringify({ type: "text", text: prompt }), 100, 100);
  db.prepare("INSERT INTO part VALUES(?,?,?,?,?,?)").run("fresh-answer-text", "fresh-assistant", id, JSON.stringify({ type: "text", text: `UNKNOWN|${marker}` }), 101, 102);
  db.close();
  const after = await reader.snapshot({ agentId: AGENT, originalSession: fixture.session.id, mode: "fresh", before: input.before });
  const result = assessHeadlessEvidence({ ...input, before: input.before, after, mode: "fresh", prompt, marker, output: { text: `UNKNOWN|${marker}` } });
  assert.equal(result.status, "pass"); assert.equal(result.evidence.native_session, id);
  const reused = copy(after); reused.sessions[0].id = input.originalSession;
  assert.equal(assessHeadlessEvidence({ ...input, after: reused, mode: "fresh", prompt, marker }).status, "blocked");
  const leaked = copy(after); leaked.sessions[0].records.unshift({ id: "prior", ordinal: -1, role: "other", text: input.secret });
  assert.equal(assessHeadlessEvidence({ ...input, after: leaked, mode: "fresh", prompt, marker, output: { text: `UNKNOWN|${marker}` } }).status, "fail");
});

test("SQLite ownership/schema/identity preflight fails closed before launch", async (t) => {
  const { input, reader, file, root } = await databaseCase(t, "antigravity");
  const db = new DatabaseSync(file); db.prepare("UPDATE trajectory_meta SET cascade_id = ?").run("wrong"); db.close();
  await assert.rejects(reader.snapshot({ agentId: AGENT, originalSession: input.originalSession }), (error) => error.code === "cascade_identity");
  await assert.rejects(createHeadlessEvidenceReader({ provider: "opencode", isolatedHome: root, workspace: root, env: { WARDIAN_E2E_CONTEXT_OPENCODE_DB: file } }), (error) => error.code === "unsupported_schema");
  const missing = path.join(root, "missing.db");
  await assert.rejects(createHeadlessEvidenceReader({ provider: "opencode", isolatedHome: root, workspace: root, env: { WARDIAN_E2E_CONTEXT_OPENCODE_DB: missing } }), EvidenceBlocked);
  await assert.rejects(fs.stat(missing), { code: "ENOENT" });
});

async function jsonlCase(t, provider) {
  const { root, workspace } = await isolated(t);
  const fixture = copy(retained[provider]); const env = {};
  const rows = fixture.rows;
  for (const row of rows) {
    if (row.cwd === "<owned-workspace>") row.cwd = workspace;
    if (row.payload?.cwd === "<owned-workspace>") row.payload.cwd = workspace;
  }
  const session = provider === "claude" ? fixture.session : provider === "pi" ? rows[0].id : rows[0].payload.id;
  let directory;
  if (provider === "claude") {
    env.WARDIAN_E2E_CONTEXT_CLAUDE_PROJECTS = path.join(root, "projects");
    directory = path.join(env.WARDIAN_E2E_CONTEXT_CLAUDE_PROJECTS, workspace.replace(/[^a-zA-Z0-9]/g, "-"));
  } else directory = path.join(root, "agents", AGENT, ...(provider === "pi" ? ["pi", "sessions"] : ["habitat", ".codex", "sessions", "2026", "09", "07"]));
  await fs.mkdir(directory, { recursive: true });
  const file = path.join(directory, `${session}.jsonl`); await fs.writeFile(file, rows.map((row) => JSON.stringify(row)).join("\n") + "\n");
  const reader = await createHeadlessEvidenceReader({ provider, isolatedHome: root, workspace, env });
  const after = await reader.snapshot({ agentId: AGENT, originalSession: session });
  const request = after.sessions[0].records.findLast((row) => row.role === "user");
  const before = copy(after); before.sessions[0].records = before.sessions[0].records.filter((row) => row.ordinal < request.ordinal);
  return { reader, after, before, originalSession: session, request, file, rows, root, workspace };
}

for (const provider of ["claude", "codex", "pi"]) {
  test(`retained ${provider} native identity/request/completion is read from owned logs`, async (t) => {
    const value = await jsonlCase(t, provider);
    const answer = value.after.sessions[0].records.findLast((row) => row.role === "assistant" && row.text);
    const result = assessHeadlessEvidence({ ...value, provider, mode: "current", prompt: value.request.text, output: { text: answer.text }, secret: "fixture-secret", marker: "fixture-marker" });
    assert.equal(result.checks.assistant_authorship, true);
    assert.equal(result.checks.output_matches_native_answer, true);
    assert.equal(result.status, "fail", "fixture checks do not relabel unrelated retained answers as recall passes");
    assert.equal(assessHeadlessEvidence({ ...value, provider, mode: "current", prompt: value.request.text, output: [{ type: "assistant", text: "fixture-secret|fixture-marker" }], secret: "fixture-secret", marker: "fixture-marker" }).status, "blocked");
    // Owned filesystem placement never overrides a foreign native header/cwd.
    const foreign = copy(value.rows);
    if (provider === "claude") for (const row of foreign) row.cwd = path.join(value.root, "foreign");
    else if (provider === "pi") foreign[0].cwd = path.join(value.root, "foreign");
    else foreign[0].payload.cwd = path.join(value.root, "foreign");
    await fs.writeFile(value.file, foreign.map((row) => JSON.stringify(row)).join("\n") + "\n");
    await assert.rejects(value.reader.snapshot({ agentId: AGENT, originalSession: value.originalSession }), (error) => error.code === "owned_session_missing");
  });
}

test("independent fresh probe runs after settled failure, and never with an active owner", async () => {
  const calls = [];
  const current = async () => { calls.push("current"); return { status: "fail", classification: "answer_mismatch" }; };
  const fresh = async () => { calls.push("fresh"); return { status: "pass" }; };
  const result = await runIndependentHeadlessCases({ current, fresh, isSafe: async () => true });
  assert.deepEqual(calls, ["current", "fresh"]); assert.equal(result.current.status, "fail"); assert.equal(result.fresh.status, "pass");
  calls.length = 0; let checks = 0;
  const unsafe = await runIndependentHeadlessCases({ current, fresh, isSafe: async () => ++checks === 1 });
  assert.deepEqual(calls, ["current"]); assert.equal(unsafe.fresh.classification, "provider_owner_not_quiescent");
  const blocked = await runIndependentHeadlessCases({ current: async () => { throw new EvidenceBlocked("missing_source", "missing"); }, fresh, isSafe: async () => true });
  assert.equal(blocked.current.status, "blocked"); assert.equal(blocked.fresh.status, "pass");
  calls.length = 0;
  const unavailable = await runIndependentHeadlessCases({ current, fresh, isSafe: async () => { throw new Error("IPC unavailable"); } });
  assert.deepEqual(calls, []); assert.equal(unavailable.fresh.classification, "provider_owner_unobservable");
});

test("Agy fresh cascade discovery rejects stale cache-like identity and foreign workspace", async (t) => {
  const { input, reader, root, workspace, fixture } = await databaseCase(t, "antigravity");
  const fresh = { session: "fresh-cascade", steps: [
    { idx: 0, step_type: 14, source: 4, status: 3, text: "Return UNKNOWN|FRESH" },
    { idx: 1, step_type: 15, source: 2, status: 3, text: "UNKNOWN|FRESH" },
  ] };
  writeAgy(path.join(root, "conversations", `${fresh.session}.db`), fresh, workspace);
  const foreign = { ...fresh, session: "foreign-cascade" };
  writeAgy(path.join(root, "conversations", `${foreign.session}.db`), foreign, path.join(root, "not-owned"));
  const after = await reader.snapshot({ agentId: AGENT, originalSession: fixture.session, mode: "fresh", before: input.before });
  assert.deepEqual(after.sessions.map((row) => row.id), [fresh.session]);
  assert.equal(assessHeadlessEvidence({ ...input, after, mode: "fresh", prompt: "Return UNKNOWN|FRESH", marker: "FRESH", output: { text: "UNKNOWN|FRESH" } }).status, "pass");
  const copyFixture = { ...fresh, session: "second-owned-cascade" };
  writeAgy(path.join(root, "conversations", `${copyFixture.session}.db`), copyFixture, workspace);
  const ambiguous = await reader.snapshot({ agentId: AGENT, originalSession: fixture.session, mode: "fresh", before: input.before });
  assert.equal(assessHeadlessEvidence({ ...input, after: ambiguous, mode: "fresh", prompt: "Return UNKNOWN|FRESH", marker: "FRESH", output: { text: "UNKNOWN|FRESH" } }).classification, "ambiguous_native_request");
});

for (const provider of ["claude", "codex", "pi"]) {
  test(`${provider} fresh owned header/row identity is required, not path rotation`, async (t) => {
    const value = await jsonlCase(t, provider);
    const rows = copy(value.rows); const freshSession = "fresh-native-session";
    if (provider === "claude") for (const row of rows) { row.sessionId = freshSession; if (row.session_id) row.session_id = freshSession; }
    else if (provider === "pi") rows[0].id = freshSession;
    else { rows[0].payload.id = freshSession; rows[0].payload.session_id = freshSession; }
    const file = path.join(path.dirname(value.file), `${freshSession}.jsonl`);
    await fs.writeFile(file, rows.map((row) => JSON.stringify(row)).join("\n") + "\n");
    const after = await value.reader.snapshot({ agentId: AGENT, originalSession: value.originalSession, mode: "fresh", before: value.before });
    assert.deepEqual(after.sessions.map((row) => row.id), [freshSession]);
    const request = after.sessions[0].records.findLast((row) => row.role === "user");
    const answer = after.sessions[0].records.findLast((row) => row.role === "assistant" && row.text);
    const result = assessHeadlessEvidence({ ...value, provider, after, mode: "fresh", prompt: request.text, secret: "unused-secret", marker: "fresh", output: { text: answer.text } });
    assert.equal(result.checks.native_session_identity, true);
    assert.equal(result.checks.assistant_authorship, true);
    assert.equal(result.status, "fail", "copying old answers is not fresh-answer acceptance");
  });
}

test("persistence polling is bounded and distinguishes missing evidence from wrong answers", async () => {
  let reads = 0;
  const reader = { provider: "opencode", snapshot: async () => { reads++; throw new EvidenceBlocked("missing_source", "not present"); } };
  const result = await observeHeadlessEvidence(reader, {}, { timeoutMs: 0 });
  assert.equal(reads, 1); assert.equal(result.status, "blocked"); assert.equal(result.classification, "missing_source");
});

for (const provider of ["claude", "codex", "pi"]) {
  test(`${provider} conflicting native identities cannot authorize an answer`, async (t) => {
    const value = await jsonlCase(t, provider);
    const rows = copy(value.rows);
    if (provider === "claude") rows.find((row) => row.type === "assistant").session_id = "foreign-session";
    else if (provider === "codex") rows[0].payload.session_id = "foreign-session";
    else rows.push({ ...rows[0], id: "foreign-session" });
    await fs.writeFile(value.file, rows.map((row) => JSON.stringify(row)).join("\n") + "\n");
    await assert.rejects(value.reader.snapshot({ agentId: AGENT, originalSession: value.originalSession }), (error) => error.code === "session_mismatch");
  });
}
