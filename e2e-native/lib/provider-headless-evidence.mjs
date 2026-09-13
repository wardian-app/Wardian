/** Read-only, source-bound evidence for the disposable context harness.
 * Automation output is answer data, never a provider packet/authorship oracle.
 */
import fs from "node:fs/promises";
import path from "node:path";
import os from "node:os";
import { fileURLToPath } from "node:url";
import { createHash } from "node:crypto";

const LIMIT = 32 * 1024 * 1024;
const sha = (value) => createHash("sha256").update(value).digest("hex");

export class EvidenceBlocked extends Error {
  constructor(code, message, options) {
    super(message, options);
    this.name = "EvidenceBlocked";
    this.code = code;
  }
}
const block = (code, message) => { throw new EvidenceBlocked(code, message); };
const required = (ok, code, message) => { if (!ok) block(code, message); };

/** pause_agent acknowledges runtime termination; list_agents.is_off is its
 * lifecycle projection. Log-derived telemetry can subsequently report Idle
 * (retained Agy VT0Q9X). It must not authorize a live or headless owner.
 */
export function assessPausedHeadlessOwner({ agentId, agents, metrics, pauseAcknowledged }) {
  const configs = Array.isArray(agents) ? agents.filter((row) => row.session_id === agentId) : [];
  const observations = Array.isArray(metrics) ? metrics.filter((row) => row.session_id === agentId) : [];
  const config = configs.length === 1 ? configs[0] : null;
  const metric = observations.length === 1 ? observations[0] : null;
  const telemetryStatus = typeof metric?.current_status === "string" ? metric.current_status.toLowerCase() : null;
  const evidence = { pause_acknowledged: pauseAcknowledged === true, is_off: config?.is_off ?? null,
    telemetry_status: metric?.current_status ?? null,
    telemetry_differs_from_lifecycle: config?.is_off === true && telemetryStatus === "idle" };
  if (pauseAcknowledged !== true || !config || !metric || !telemetryStatus) {
    return { status: "blocked", classification: "paused_owner_unobservable", evidence };
  }
  if (config.is_off !== true || !["off", "idle"].includes(telemetryStatus)) {
    return { status: "blocked", classification: "provider_owner_not_quiescent", evidence };
  }
  return { status: "pass", classification: "acknowledged_paused_owner", evidence };
}

/** Only OpenCode's observed single JSON-string envelope is compatible. */
export function matchNativePrompt(provider, actual, expected) {
  if (actual === expected) return { matched: true, transport: "literal" };
  if (provider === "opencode" && typeof actual === "string" && actual.startsWith('"') && actual.endsWith('"')) {
    try {
      const decoded = JSON.parse(actual);
      if (typeof decoded === "string" && decoded === expected) {
        return { matched: true, transport: "opencode_single_json_string" };
      }
    } catch { /* Not the observed transport shape. */ }
  }
  return { matched: false, transport: null };
}

async function canonicalPath(value) {
  if (typeof value !== "string" || !value) return null;
  let local = value;
  if (local.startsWith("file:")) {
    try { local = fileURLToPath(local); } catch { return null; }
  }
  if (!path.isAbsolute(local)) return null;
  // realpath resolves the owned workspace junction, without enumerating targets.
  try { local = await fs.realpath(local); } catch { local = path.resolve(local); }
  local = local.replaceAll("\\", "/").replace(/\/+$/, "");
  return process.platform === "win32" ? local.toLowerCase() : local;
}
async function owned(value, directories) {
  const candidate = await canonicalPath(value);
  return candidate !== null && directories.includes(candidate);
}
async function entries(directory) {
  try { return await fs.readdir(directory, { withFileTypes: true }); }
  catch (error) { if (error.code === "ENOENT") return []; throw error; }
}
async function jsonLines(file) {
  const stat = await fs.stat(file);
  required(stat.size <= LIMIT, "source_too_large", "Native evidence exceeds bounded reader size");
  const text = await fs.readFile(file, "utf8");
  try { return text.split(/\r?\n/).filter(Boolean).map((line) => JSON.parse(line)); }
  catch (cause) { throw new EvidenceBlocked("incomplete_jsonl", "Native JSONL is incomplete or malformed", { cause }); }
}
async function header(file) {
  const handle = await fs.open(file, "r");
  try {
    const buffer = Buffer.alloc(64 * 1024);
    const { bytesRead } = await handle.read(buffer, 0, buffer.length, 0);
    const line = buffer.subarray(0, bytesRead).toString("utf8").split(/\r?\n/)[0];
    try { return JSON.parse(line); } catch { return null; }
  } finally { await handle.close(); }
}
async function walkJsonl(root, depth = 0) {
  required(depth <= 5, "source_depth", "Native log tree exceeds known directory layout");
  const files = [];
  for (const entry of await entries(root)) {
    if (entry.isFile() && entry.name.endsWith(".jsonl")) files.push(path.join(root, entry.name));
    else if (entry.isDirectory()) files.push(...await walkJsonl(path.join(root, entry.name), depth + 1));
    // Do not recursively follow arbitrary nested symlinks into global histories.
  }
  return files;
}

/** Bounded protobuf wire reader: only types present in retained Agy records. */
function fields(input) {
  const bytes = Buffer.from(input);
  let offset = 0;
  const result = new Map();
  const varint = () => {
    let value = 0n;
    for (let shift = 0n; shift < 70n; shift += 7n) {
      required(offset < bytes.length, "protobuf_truncated", "Truncated native protobuf");
      const byte = bytes[offset++];
      value |= BigInt(byte & 127) << shift;
      if (byte < 128) {
        required(value <= BigInt(Number.MAX_SAFE_INTEGER), "protobuf_integer", "Unsafe native protobuf integer");
        return Number(value);
      }
    }
    block("protobuf_varint", "Invalid native protobuf varint");
  };
  while (offset < bytes.length) {
    const key = varint(); const number = Math.floor(key / 8); const wire = key % 8;
    required(number > 0, "protobuf_field", "Invalid native protobuf field");
    let value;
    if (wire === 0) value = varint();
    else if ([1, 2, 5].includes(wire)) {
      const size = wire === 2 ? varint() : wire === 1 ? 8 : 4;
      required(offset + size <= bytes.length, "protobuf_truncated", "Truncated native protobuf field");
      value = bytes.subarray(offset, offset + size); offset += size;
    } else block("protobuf_wire", "Unknown native protobuf wire type");
    result.set(number, [...(result.get(number) ?? []), value]);
  }
  return result;
}
function field(bytes, ...keys) {
  let value = bytes;
  for (const key of keys) {
    if (!Buffer.isBuffer(value) && !(value instanceof Uint8Array)) return undefined;
    const values = fields(value).get(key);
    if (!values) return undefined;
    required(values.length === 1, "ambiguous_field", "Repeated scalar native field");
    value = values[0];
  }
  return value;
}
function nativeText(bytes, ...keys) {
  const value = field(bytes, ...keys);
  return Buffer.isBuffer(value) ? new TextDecoder("utf-8", { fatal: true }).decode(value) : undefined;
}
function agyWorkspaceUris(bytes) {
  // Retained trajectory metadata uses repeated workspace entries at 1, URI at 1.1.
  return (fields(bytes).get(1) ?? []).map((value) => nativeText(value, 1)).filter(Boolean);
}
function textBlocks(content, type = "text") {
  return Array.isArray(content) ? content.filter((part) => part.type === type).map((part) => part.text ?? "").join("") : "";
}

async function sqlite(file, inspect) {
  const { DatabaseSync } = await import("node:sqlite");
  const db = new DatabaseSync(file, { readOnly: true });
  try {
    db.exec("BEGIN");
    return await inspect(db);
  } finally { db.close(); }
}
function schema(db, table, columns) {
  const names = db.prepare(`PRAGMA table_info(${table})`).all().map((row) => row.name);
  required(columns.every((column) => names.includes(column)), "unsupported_schema", `Native ${table} schema lacks required columns`);
}
function rowJson(text) {
  try { return JSON.parse(text); }
  catch (cause) { throw new EvidenceBlocked("invalid_native_json", "Malformed native record JSON", { cause }); }
}

async function openCodeSnapshot(file, directories, current, exclude = []) {
  return sqlite(file, async (db) => {
    schema(db, "session", ["id", "directory"]);
    schema(db, "message", ["id", "session_id", "data", "time_created"]);
    schema(db, "part", ["id", "message_id", "session_id", "data", "time_created"]);
    const catalog = db.prepare("SELECT id, directory FROM session").all();
    const sessions = [];
    for (const session of catalog) {
      if ((current && session.id !== current) || (!current && exclude.includes(session.id))) continue;
      if (!await owned(session.directory, directories)) continue;
      const records = db.prepare("SELECT id, data, time_created FROM message WHERE session_id = ? ORDER BY time_created, id").all(session.id).map((message, index) => {
        const data = rowJson(message.data);
        const parts = db.prepare("SELECT id, data FROM part WHERE session_id = ? AND message_id = ? ORDER BY time_created, id").all(session.id, message.id);
        return { id: message.id, parent: data.parentID, role: data.role,
          text: parts.map((part) => rowJson(part.data)).filter((part) => part.type === "text").map((part) => part.text ?? "").join(""),
          complete: data.role === "assistant" && Number.isFinite(data.time?.completed) && data.finish === "stop",
          ordinal: index, part_ids: parts.map((part) => part.id) };
      });
      sessions.push({ id: session.id, records, source: "opencode_db", source_locator_sha256: sha(path.resolve(file)), causal: "native_parent_id" });
    }
    return { inventory: catalog.map((row) => row.id), sessions };
  });
}

/** Both agent-specific aliases are required. Shared/common aliases prove nothing. */
export function assessAntigravityAliases(candidate, original, expected) {
  if (!Array.isArray(expected) || expected.length !== 2 || !expected.every((uri) => typeof uri === "string") || new Set(expected).size !== 2) return false;
  const prefix = expected[0]?.slice(0, expected[0].lastIndexOf("/include/") + 9);
  if (!prefix?.includes("/wardian-antigravity/") || !expected.every((uri) => uri.startsWith(prefix))) return false;
  if ([...candidate, ...original].some((uri) => uri?.includes("/wardian-antigravity/") && !uri.startsWith(prefix))) return false;
  return expected.every((alias) => original.filter((uri) => uri === alias).length === 1 &&
    candidate.filter((uri) => uri === alias).length === 1);
}

async function antigravityAliasAuthority({ home, workspace, agentId, originalSession, before, launch, requireReceipt = true }) {
  const config = launch?.agentConfig;
  required(config?.session_id === agentId && config.provider === "antigravity" && config.resume_session === originalSession &&
    await canonicalPath(config.folder) === await canonicalPath(workspace), "agy_launch_config", "Fresh alias proof requires the exact owned original launch configuration");
  const original = before?.sessions?.filter((row) => row.id === originalSession && row.ownership_basis === "workspace_uri");
  required(original?.length === 1, "agy_original_binding", "Alias proof requires an original workspace-bound native session");
  const agentRoot = path.join(home, "agents", agentId);
  const habitat = path.join(agentRoot, "habitat");
  const directories = [...(config.system_include_directories ?? [])];
  // The existing spawn path adds the agent habitat to system includes.
  if (!directories.some((dir) => path.resolve(dir) === path.resolve(habitat))) directories.push(habitat);
  for (const dir of config.include_directories ?? []) if (!directories.includes(dir)) directories.push(dir);
  const canonical = await Promise.all(directories.map(canonicalPath));
  const expected = [];
  for (const source of [agentRoot, habitat]) {
    const normalized = await canonicalPath(source);
    const index = canonical.indexOf(normalized);
    required(normalized && index >= 0 && canonical.lastIndexOf(normalized) === index, "agy_include_config", "Agent-specific include source is missing or ambiguous");
    expected.push(await canonicalPath(path.join(os.tmpdir(), "wardian-antigravity", agentId.toLowerCase(), "include",
      `${String(index).padStart(2, "0")}-${path.basename(source).toLowerCase()}`)));
  }
  const originalUris = await Promise.all((original[0].workspace_uris ?? []).map(canonicalPath));
  required(assessAntigravityAliases(originalUris, originalUris, expected), "agy_original_aliases", "Original native metadata does not corroborate both configured agent-specific aliases");
  if (!requireReceipt) return { status: "pass", basis: "original_workspace_and_configured_agent_aliases" };
  const executionId = launch.executionId;
  required(typeof executionId === "string" && /^automation-bg-[a-zA-Z0-9-]+-provider-turn$/.test(executionId), "agy_execution_identity", "Owned automation execution identity is missing");
  const cwd = path.join(home, "agents", executionId, "habitat", "workspace");
  required(await canonicalPath(cwd) === await canonicalPath(workspace), "agy_execution_workspace", "Fresh execution workspace does not resolve to the owned scratch workspace");
  const logFile = path.join(home, "wardian_debug.log");
  required((await fs.stat(logFile)).size <= LIMIT, "source_too_large", "Owned launch log exceeds bounded evidence size");
  const log = await fs.readFile(logFile, "utf8");
  const lines = log.split(/\r?\n/);
  const starts = lines.flatMap((line, index) => line.startsWith(`[Wardian] run_headless: provider=antigravity, session_id=${executionId}, cwd=`) ? [index] : []);
  required(starts.length === 1, "agy_execution_receipt", "Expected exactly one owned fresh launch receipt");
  const index = starts[0];
  const loggedCwd = lines[index].split(", cwd=")[1]?.split(", prompt_len=")[0];
  required(await canonicalPath(loggedCwd) === await canonicalPath(cwd) &&
    lines[index - 1] === `[automation] node provider-turn: running assigned agent ${agentId} as a fresh background conversation` &&
    lines[index + 1]?.startsWith("[Wardian] run_headless launch:") && lines[index + 1].endsWith(", resume=false"),
  "agy_execution_receipt", "Fresh launch receipt does not corroborate original assignment, workspace and no resume");
  return { expected, originalUris, executionId, launch_sha256: sha(JSON.stringify({ config, receipt: lines.slice(index - 1, index + 2) })) };
}

async function antigravitySnapshot(root, directories, current, exclude = [], aliasAuthority = null) {
  const directory = path.join(root, "conversations");
  const inventory = (await entries(directory)).filter((item) => item.isFile() && item.name.endsWith(".db")).map((item) => item.name.slice(0, -3));
  const sessions = [];
  for (const id of inventory) {
    if ((current && id !== current) || (!current && exclude.includes(id))) continue;
    const candidate = await sqlite(path.join(directory, `${id}.db`), async (db) => {
      schema(db, "trajectory_meta", ["cascade_id"]);
      schema(db, "trajectory_metadata_blob", ["id", "data"]);
      const meta = db.prepare("SELECT data FROM trajectory_metadata_blob WHERE id = 'main'").get();
      if (!meta) return null;
      const workspaceUris = agyWorkspaceUris(meta.data);
      const matches = await Promise.all(workspaceUris.map((uri) => owned(uri, directories)));
      const direct = matches.some(Boolean);
      if (!direct && (!aliasAuthority || !assessAntigravityAliases(await Promise.all(workspaceUris.map(canonicalPath)), aliasAuthority.originalUris, aliasAuthority.expected))) return null;
      const identities = db.prepare("SELECT DISTINCT cascade_id FROM trajectory_meta").all();
      required(identities.length === 1 && identities[0].cascade_id === id, "cascade_identity", "Native cascade filename and metadata disagree");
      schema(db, "steps", ["idx", "step_type", "status", "step_payload"]);
      const records = db.prepare("SELECT idx, step_type, status, step_payload FROM steps ORDER BY idx").all().map((step) => {
        const source = field(step.step_payload, 5, 3);
        const status = field(step.step_payload, 4);
        const isUser = step.step_type === 14 && source === 4;
        const isAssistant = step.step_type === 15 && source === 2;
        return { id: String(step.idx), ordinal: step.idx, role: isUser ? "user" : isAssistant ? "assistant" : "other",
          text: isUser ? nativeText(step.step_payload, 19, 2) ?? "" : isAssistant ? nativeText(step.step_payload, 20, 1) ?? "" : "",
          complete: isAssistant && step.status === 3 && status === 3,
          native_type: step.step_type, native_source: source, native_status: step.status };
      });
      return { id, records, workspace_uris: workspaceUris,
        ownership_basis: direct ? "workspace_uri" : "corroborated_agent_specific_include_aliases",
        ownership_sha256: direct ? null : aliasAuthority.launch_sha256,
        source_locator_sha256: sha(path.join(directory, `${id}.db`)), source: "antigravity_conversation_database", causal: "ordered_native_step_interval" };
    });
    if (candidate) sessions.push(candidate);
  }
  return { inventory, sessions };
}

function ancestryReaches(records, from, target) {
  const byId = new Map(records.map((row) => [row.id, row]));
  const seen = new Set();
  let row = from;
  while (row?.parent && !seen.has(row.parent)) {
    if (row.parent === target) return true;
    seen.add(row.parent);
    row = byId.get(row.parent);
    if (row?.role === "user") return false;
  }
  return false;
}

function parseJsonl(provider, rows, id) {
  if (provider === "claude") {
    const records = rows.filter((row) => row.uuid).map((row, ordinal) => {
      required((!row.sessionId || row.sessionId === id) && (!row.session_id || row.session_id === id), "session_mismatch", "Claude row has a foreign native session");
      const message = row.message ?? {};
      const isUser = row.type === "user" && message.role === "user" && !row.isMeta && !row.isSidechain && (row.origin?.kind === "human" || (!row.origin && typeof message.content === "string"));
      const isAssistant = row.type === "assistant" && message.role === "assistant" && !row.isMeta && !row.isSidechain;
      return { id: row.uuid, parent: row.parentUuid, ordinal, role: isUser ? "user" : isAssistant ? "assistant" : "other",
        text: isUser && typeof message.content === "string" ? message.content : textBlocks(message.content),
        complete: isAssistant && message.stop_reason === "end_turn", group: message.id };
    });
    return { id, records, source: "claude_jsonl", causal: "native_uuid_ancestry" };
  }
  if (provider === "pi") {
    const records = rows.filter((row) => row.id && row.type !== "session").map((row, ordinal) => ({
      id: row.id, parent: row.parentId, ordinal,
      role: row.type === "message" && ["user", "assistant"].includes(row.message?.role) ? row.message.role : "other",
      text: textBlocks(row.message?.content), complete: row.type === "message" && row.message?.role === "assistant" && row.message.stopReason === "stop",
    }));
    return { id, records, source: "pi_jsonl", causal: "native_parent_id" };
  }
  // Codex's retained event_msg turn envelope supplies completion; response_item
  // duplicates and arbitrary item text are never a second answer oracle.
  const records = [];
  let turn = null;
  for (const [ordinal, row] of rows.entries()) {
    const value = row.payload ?? {};
    if (row.type === "event_msg" && value.type === "task_started") turn = value.turn_id;
    if (!turn || row.type !== "event_msg") continue;
    if (value.type === "user_message") records.push({ id: `${turn}:user:${ordinal}`, turn, ordinal, role: "user", text: value.message, complete: false });
    if (value.type === "agent_message") records.push({ id: `${turn}:assistant:${ordinal}`, turn, ordinal, role: "assistant", text: value.message, complete: false });
    if (value.type === "task_complete") {
      required(value.turn_id === turn, "turn_mismatch", "Codex completion has a foreign turn ID");
      const matching = records.filter((record) => record.turn === turn && record.role === "assistant" && record.text === value.last_agent_message);
      if (matching.length === 1) matching[0].complete = true;
      turn = null;
    }
  }
  return { id, records, source: "codex_jsonl", causal: "native_turn_envelope" };
}

async function jsonlSnapshot(provider, roots, directories, current, exclude = []) {
  const inventory = []; const sessions = [];
  for (const root of roots) {
    for (const file of await walkJsonl(root)) {
      const first = await header(file);
      if (!first) continue;
      const id = provider === "claude" ? first.sessionId : provider === "pi" && first.type === "session" ? first.id : provider === "codex" && first.type === "session_meta" ? first.payload?.id : null;
      // Claude logs can begin with queue metadata without cwd/sessionId. Resolve
      // the filename only inside a project directory owned by this workspace.
      const candidateId = id ?? (provider === "claude" ? path.basename(file, ".jsonl") : null);
      if (!candidateId) continue;
      inventory.push(candidateId);
      if ((current && candidateId !== current) || (!current && exclude.includes(candidateId))) continue;
      if (provider !== "claude" && !await owned(provider === "pi" ? first.cwd : first.payload?.cwd, directories)) continue;
      const rows = await jsonLines(file);
      if (provider === "claude") {
        const bound = rows.filter((row) => row.type === "user" || row.type === "assistant");
        if (!bound.length || !(await Promise.all(bound.map((row) => owned(row.cwd, directories)))).every(Boolean)) continue;
        if (bound.some((row) => row.sessionId !== candidateId)) continue;
      } else {
        const headers = rows.filter((row) => row.type === (provider === "pi" ? "session" : "session_meta"));
        for (const row of headers) {
          const meta = provider === "pi" ? row : row.payload;
          required(meta?.id === candidateId && (!meta.session_id || meta.session_id === candidateId), "session_mismatch", "Native log headers disagree about session identity");
          required(await owned(meta.cwd, directories), "workspace_mismatch", "Native log header has a foreign workspace");
        }
      }
      required(!sessions.some((session) => session.id === candidateId), "duplicate_source", "Native session is present in multiple evidence files");
      sessions.push({ ...parseJsonl(provider, rows, candidateId), source_locator_sha256: sha(path.resolve(file)) });
    }
  }
  return { inventory, sessions };
}

/** Preflight capability/storage before any paid prompt. Session binding is
 * checked again with snapshot() after pause and before each headless launch.
 */
export async function createHeadlessEvidenceReader({ provider, isolatedHome, workspace, env = process.env }) {
  required(["opencode", "antigravity", "claude", "codex", "pi"].includes(provider), "unsupported_provider", "No grounded native reader for provider");
  const home = path.resolve(isolatedHome);
  const paths = {
    opencode: env.WARDIAN_E2E_CONTEXT_OPENCODE_DB || path.join(os.homedir(), ".local", "share", "opencode", "opencode.db"),
    antigravity: env.WARDIAN_E2E_CONTEXT_ANTIGRAVITY_HOME || path.join(os.homedir(), ".gemini", "antigravity-cli"),
    claude: env.WARDIAN_E2E_CONTEXT_CLAUDE_PROJECTS || path.join(os.homedir(), ".claude", "projects"),
  };
  try {
    if (provider === "opencode") await sqlite(paths.opencode, (db) => {
      schema(db, "session", ["id", "directory"]); schema(db, "message", ["id", "session_id", "data", "time_created"]); schema(db, "part", ["id", "session_id", "message_id", "data", "time_created"]);
    });
    if (provider === "antigravity") {
      await import("node:sqlite");
      required((await fs.stat(path.join(paths.antigravity, "conversations"))).isDirectory(), "missing_store", "Antigravity native conversation store is unavailable");
    }
    if (provider === "claude") required((await fs.stat(paths.claude)).isDirectory(), "missing_store", "Claude native project store is unavailable");
    required((await fs.stat(home)).isDirectory() && (await fs.stat(workspace)).isDirectory(), "missing_workspace", "Owned test home/workspace must exist");
  } catch (cause) {
    if (cause instanceof EvidenceBlocked) throw cause;
    throw new EvidenceBlocked("source_preflight", "Native evidence storage/runtime preflight failed", { cause });
  }
  return {
    provider,
    async preflightFreshAliases({ agentId, originalSession, before, agentConfig }) {
      required(provider === "antigravity", "unsupported_alias_provider", "Only Antigravity has this observed alias contract");
      return antigravityAliasAuthority({ home, workspace, agentId, originalSession, before, launch: { agentConfig }, requireReceipt: false });
    },
    async snapshot({ agentId, originalSession, mode = "current", before, launch }) {
      required(typeof agentId === "string" && /^[a-zA-Z0-9-]+$/.test(agentId), "agent_identity", "Invalid owned agent ID");
      required(typeof originalSession === "string" && originalSession.length > 0, "native_identity", "Original native session is missing");
      const habitat = path.join(home, "agents", agentId, "habitat");
      const directories = [...new Set(await Promise.all([workspace, habitat, path.join(habitat, "workspace")].map(canonicalPath)))].filter(Boolean);
      const current = mode === "current" ? originalSession : null;
      const exclude = mode === "fresh" ? before?.inventory ?? [] : [];
      let snapshot;
      try {
        if (provider === "opencode") snapshot = await openCodeSnapshot(paths.opencode, directories, current, exclude);
        else if (provider === "antigravity") {
          const authority = mode === "fresh" && launch
            ? await antigravityAliasAuthority({ home, workspace, agentId, originalSession, before, launch }) : null;
          snapshot = await antigravitySnapshot(paths.antigravity, directories, current, exclude, authority);
        }
        else {
          const roots = [];
          if (provider === "claude") {
            // Claude's project encoding matches the retained native selector.
            for (const directory of [workspace, habitat, path.join(habitat, "workspace")]) {
              roots.push(path.join(paths.claude, path.resolve(directory).replace(/[^a-zA-Z0-9]/g, "-")));
            }
          } else {
            for (const agent of await entries(path.join(home, "agents"))) {
              if (agent.isDirectory()) roots.push(path.join(home, "agents", agent.name, ...(provider === "pi" ? ["pi", "sessions"] : ["habitat", ".codex", "sessions"])));
            }
          }
          snapshot = await jsonlSnapshot(provider, [...new Set(roots)], directories, current, exclude);
        }
      } catch (cause) {
        if (cause instanceof EvidenceBlocked) throw cause;
        throw new EvidenceBlocked("source_read", "Owned native source could not be read", { cause });
      }
      if (mode === "current") required(snapshot.sessions.length === 1 && snapshot.sessions[0].id === originalSession, "owned_session_missing", "No unique native session bound to this test workspace");
      return { provider, ...snapshot };
    },
  };
}

/** Assess only current native request/answer records. Missing support is blocked;
 * a proven assistant giving the wrong answer is a real assertion failure.
 */
export function assessHeadlessEvidence({ provider, before, after, originalSession, mode, prompt, output, secret, marker }) {
  const checks = {};
  const result = (status, classification, evidence = {}) => ({ status, classification, checks, evidence });
  if (before?.provider !== provider || after?.provider !== provider || !["current", "fresh"].includes(mode)) {
    return result("blocked", "snapshot_provider_or_mode");
  }
  if (!Array.isArray(before.inventory) || !before.inventory.includes(originalSession) ||
    new Set(before.inventory).size !== before.inventory.length) return result("blocked", "baseline_inventory_invalid");
  const candidates = [];
  for (const session of after.sessions) {
    if (mode === "current" ? session.id !== originalSession : before.inventory.includes(session.id) || session.id === originalSession) continue;
    const oldIds = new Set(before.sessions.find((old) => old.id === session.id)?.records.map((row) => row.id) ?? []);
    for (const request of session.records.filter((row) => row.role === "user" && !oldIds.has(row.id))) {
      const match = matchNativePrompt(provider, request.text, prompt);
      if (match.matched) candidates.push({ session, request, match, oldIds });
    }
  }
  if (candidates.length !== 1) return result("blocked", candidates.length ? "ambiguous_native_request" : "missing_native_request");
  const { session, request, match, oldIds } = candidates[0];
  checks.native_session_identity = true; checks.current_native_request = true;
  if (mode === "fresh") checks.native_session_absent_from_full_baseline = true;
  const evidence = { native_session: session.id, request_id: request.id, source: session.source,
    ownership_basis: session.ownership_basis ?? "workspace_uri", ownership_sha256: session.ownership_sha256 ?? null,
    baseline_inventory_sha256: sha(JSON.stringify([...before.inventory].sort())),
    source_locator_sha256: session.source_locator_sha256,
    native_snapshot_sha256: sha(JSON.stringify(session.records)),
    baseline_record_ids_sha256: sha(JSON.stringify([...oldIds])),
    causal_binding: session.causal, prompt_transport: match.transport };
  let answers = session.records.filter((row) => row.role === "assistant" && row.complete && row.text && !oldIds.has(row.id) && row.ordinal > request.ordinal);
  if (provider === "opencode") answers = answers.filter((row) => row.parent === request.id);
  else if (["claude", "pi"].includes(provider)) answers = answers.filter((row) => ancestryReaches(session.records, row, request.id));
  else if (provider === "codex") answers = answers.filter((row) => row.turn === request.turn && session.records.filter((item) => item.role === "user" && item.turn === row.turn).length === 1);
  else {
    const next = session.records.find((row) => row.role === "user" && row.ordinal > request.ordinal);
    answers = answers.filter((row) => !next || row.ordinal < next.ordinal);
  }
  // Claude writes one row per content block; group only the provider message ID.
  if (provider === "claude") {
    const groups = new Map();
    for (const answer of answers) {
      const key = answer.group || answer.id;
      const prior = groups.get(key);
      groups.set(key, prior ? { ...answer, text: prior.text + answer.text } : answer);
    }
    answers = [...groups.values()];
  }
  if (answers.length !== 1) return result("blocked", answers.length ? "ambiguous_native_answer" : "missing_completed_native_answer", evidence);
  const answer = answers[0];
  checks.assistant_authorship = true;
  evidence.answer_id = answer.id; evidence.answer_sha256 = sha(answer.text);
  if (mode === "fresh") checks.secret_absent_from_observed_prior_text = !session.records.filter((row) => row.ordinal < request.ordinal).some((row) => row.text?.includes(secret));
  const text = output && !Array.isArray(output) && typeof output.text === "string" ? output.text : null;
  if (text === null) return result("blocked", "normalized_output_shape", evidence);
  checks.output_matches_native_answer = text.trim() === answer.text.trim();
  checks.expected_answer = answer.text.trim() === (mode === "fresh" ? `UNKNOWN|${marker}` : `${secret}|${marker}`);
  if (checks.secret_absent_from_observed_prior_text === false) return result("fail", "fresh_context_leak", evidence);
  return Object.values(checks).every(Boolean) ? result("pass", "native_answer_verified", evidence) : result("fail", checks.expected_answer ? "output_native_mismatch" : "answer_mismatch", evidence);
}

/** Poll persistence only, never rerun a provider prompt. */
export async function observeHeadlessEvidence(reader, request, { timeoutMs = 5000, pollMs = 100 } = {}) {
  const deadline = Date.now() + timeoutMs;
  let last;
  do {
    try {
      const after = await reader.snapshot({ ...request, mode: request.mode, before: request.before });
      last = assessHeadlessEvidence({ ...request, provider: reader.provider, after });
    } catch (error) {
      if (!(error instanceof EvidenceBlocked)) throw error;
      last = { status: "blocked", classification: error.code, checks: {}, evidence: {} };
    }
    if (last.status !== "blocked") return last;
    if (Date.now() < deadline) await new Promise((resolve) => setTimeout(resolve, pollMs));
  } while (Date.now() < deadline);
  return last;
}

/** A settled recall failure must not suppress an independent fresh probe.
 * The caller proves the original owner is off and the preceding run terminal.
 */
export async function runIndependentHeadlessCases({ current, fresh, isSafe, record = async () => {} }) {
  const results = {};
  for (const [name, run] of [["current", current], ["fresh", fresh]]) {
    let safe = false;
    let unsafeReason = "provider_owner_not_quiescent";
    try { safe = await isSafe(); }
    catch { unsafeReason = "provider_owner_unobservable"; }
    if (!safe) {
      results[name] = { status: "blocked", classification: unsafeReason, checks: {}, evidence: {} };
    } else {
      try { results[name] = await run(); }
      catch (error) {
        results[name] = { status: error instanceof EvidenceBlocked ? "blocked" : "fail",
          classification: error instanceof EvidenceBlocked ? error.code : "automation_run_failed", checks: {}, evidence: {} };
      }
    }
    await record(name, results[name]);
  }
  return results;
}
