import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import fs from "node:fs/promises";
import path from "node:path";

const CONVERSATION_LEASE_MODES = new Set([
  "background_resume", "background_fresh", "lifecycle_transition", "uncertain_previous_provider",
]);
const MAX_REPORTED_LEASE_CONFLICTS = 4;

function samePath(left, right, platform = process.platform) {
  if (platform === "win32") {
    const normalizedLeft = path.win32.toNamespacedPath(path.win32.resolve(left)).toLowerCase();
    const normalizedRight = path.win32.toNamespacedPath(path.win32.resolve(right)).toLowerCase();
    return normalizedLeft === normalizedRight;
  }
  return path.resolve(left) === path.resolve(right);
}

function generatedTomlSection(config, name) {
  const lines = config.split(/\r?\n/u);
  const header = new RegExp(`^\\s*\\[${name.replaceAll(".", "\\.")}\\]\\s*(?:#.*)?$`, "u");
  const matches = lines.flatMap((line, index) => header.test(line) ? [index] : []);
  assert.equal(matches.length, 1, `Expected exactly one generated TOML section ${name}`);
  const [start] = matches;
  let end = lines.findIndex((line, index) => index > start && /^\s*\[/u.test(line));
  if (end === -1) end = lines.length;
  return lines.slice(start + 1, end).join("\n");
}

function generatedTomlValue(section, key) {
  const matches = section.split("\n").filter((entry) =>
    new RegExp(`^\\s*${key}\\s*=`, "u").test(entry));
  assert.equal(matches.length, 1, `Expected exactly one generated TOML field ${key}`);
  const [line] = matches;
  const value = line.slice(line.indexOf("=") + 1).trim();
  if (/^'[^'\r\n]*'$/u.test(value)) return value.slice(1, -1);
  return JSON.parse(value);
}

function hasReplyToolPolicy(config) {
  let section = "";
  for (const line of config.split(/\r?\n/u)) {
    const trimmed = line.trim();
    if (!trimmed || trimmed.startsWith("#")) continue;
    if (trimmed.startsWith("[")) {
      const header = trimmed.split("#", 1)[0].trim()
        .replace(/^\[+|\]+$/gu, "").replace(/[\s"']/gu, "");
      section = header;
      if (header === "mcp_servers.wardian.tools" ||
        header.startsWith("mcp_servers.wardian.tools.reply") ||
        (header.startsWith("mcp_servers.wardian.tools.") && header.slice("mcp_servers.wardian.tools.".length).includes("."))) {
        return true;
      }
      continue;
    }

    const normalized = trimmed.replace(/[\s"']/gu, "");
    const assignment = normalized.split("=", 1)[0];
    if ((section === "mcp_servers.wardian" && assignment.startsWith("tools")) ||
      normalized.startsWith("mcp_servers.wardian.tools.reply") ||
      normalized.startsWith("mcp_servers.wardian.tools=") ||
      (section === "mcp_servers" && assignment.startsWith("wardian.tools"))) {
      return true;
    }
  }
  return false;
}

function assertPlainPath(stat, kind, label) {
  assert.equal(stat.isSymbolicLink(), false, `${label} must not be a link`);
  assert.equal(kind === "directory" ? stat.isDirectory() : stat.isFile(), true,
    `${label} must be a regular ${kind}`);
}

async function lstatPlain(pathname, kind, label) {
  const stat = await fs.lstat(pathname);
  assertPlainPath(stat, kind, label);
  assert.ok(samePath(await fs.realpath(pathname), pathname), `${label} resolved through a link`);
  return stat;
}

function assertSameFileIdentity(expected, actual, label) {
  assert.equal(actual.isSymbolicLink(), false, `${label} became a link`);
  assert.equal(actual.dev, expected.dev, `${label} device changed during fixture preparation`);
  assert.equal(actual.ino, expected.ino, `${label} identity changed during fixture preparation`);
}

async function readHandleUtf8(handle) {
  const { size } = await handle.stat();
  const bytes = Buffer.alloc(size);
  let offset = 0;
  while (offset < bytes.length) {
    const { bytesRead } = await handle.read(bytes, offset, bytes.length - offset, offset);
    assert.ok(bytesRead > 0, "Managed Codex config changed while it was being read");
    offset += bytesRead;
  }
  return bytes.toString("utf8");
}

async function writeHandleUtf8(handle, text) {
  const bytes = Buffer.from(text, "utf8");
  let offset = 0;
  while (offset < bytes.length) {
    const { bytesWritten } = await handle.write(bytes, offset, bytes.length - offset, offset);
    assert.ok(bytesWritten > 0, "Managed Codex config write made no progress");
    offset += bytesWritten;
  }
  await handle.truncate(bytes.length);
  await handle.sync();
}

/** Resolve only the real system PowerShell location for Windows Codex shell calls. */
export function automaticReplyExecShell(platform = process.platform, systemRoot = process.env.SystemRoot) {
  if (platform !== "win32") return null;
  assert.ok(typeof systemRoot === "string" && path.win32.isAbsolute(systemRoot),
    "Windows SystemRoot must be absolute before selecting the fixture shell");
  const shell = path.win32.join(systemRoot, "System32", "WindowsPowerShell", "v1.0", "powershell.exe");
  assert.ok(!/\\WindowsApps(?:\\|$)/iu.test(shell), "The fixture shell must not resolve through WindowsApps");
  return shell;
}

/** Keep the Windows shell choice explicit in the sole initial worker task. */
export function automaticReplyTaskInstructions(gateCommand, shell = null) {
  assert.ok(typeof gateCommand === "string" && gateCommand.trim(), "The worker gate command is required");
  if (shell !== null) {
    assert.ok(typeof shell === "string" && path.win32.isAbsolute(shell)
      && path.win32.basename(shell).toLowerCase() === "powershell.exe"
      && !/\\WindowsApps(?:\\|$)/iu.test(shell), "Only the verified absolute Windows PowerShell shell is supported");
  }
  const execution = shell === null
    ? `Run ${gateCommand} in your assigned workspace.`
    : `Use exactly one exec_command request with cmd ${JSON.stringify(gateCommand)} and shell ${JSON.stringify(shell)}. Do not omit or replace the shell field.`;
  return `${execution} Wait for it to print one WORKER_ONLY_ UUID after the external test gate opens. ` +
    "Then call the Wardian reply tool once with status done and exactly that printed token. Do no other work.";
}

/**
 * Append the one fixture-owned reply-tool grant only to the exact, prepared
 * agent Codex home whose registration record and config still match Wardian.
 */
export async function appendOwnedReplyToolApproval({ wardianHome, agentId, expectedCommand }) {
  assert.ok(path.isAbsolute(wardianHome), "The isolated Wardian home must be absolute");
  assert.match(agentId ?? "", /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/iu,
    "A canonical UUID agent ID is required");
  assert.ok(path.isAbsolute(expectedCommand), "The registered MCP command must be absolute");

  const home = path.resolve(wardianHome);
  const agentRoot = path.join(home, "agents", agentId);
  const codexHome = path.join(agentRoot, "habitat", ".codex");
  const configPath = path.join(codexHome, "config.toml");
  const ownerPath = path.join(codexHome, ".wardian-messaging.json");
  const commandPath = path.resolve(expectedCommand);
  const directories = [home, path.join(home, "agents"), agentRoot,
    path.join(agentRoot, "habitat"), codexHome, path.dirname(commandPath)];
  const directoryStats = new Map();
  for (const directory of directories) {
    directoryStats.set(directory, await lstatPlain(directory, "directory", "Owned fixture directory"));
  }
  const homeReal = await fs.realpath(home);
  assert.ok(samePath(homeReal, home), "The isolated Wardian home must not be redirected");
  const codexReal = await fs.realpath(codexHome);
  assert.ok(samePath(codexReal, path.join(homeReal, "agents", agentId, "habitat", ".codex")),
    "The worker Codex profile must remain in its exact managed habitat");
  const commandReal = await fs.realpath(commandPath);
  const relativeCommand = path.relative(homeReal, commandReal);
  assert.ok(relativeCommand && !relativeCommand.startsWith("..") && !path.isAbsolute(relativeCommand),
    "The managed MCP command must remain inside the isolated Wardian home");
  const commandStat = await lstatPlain(commandReal, "file", "Managed MCP command");

  const ownerStat = await lstatPlain(ownerPath, "file", "Managed messaging ownership record");
  const configStat = await lstatPlain(configPath, "file", "Managed Codex config");
  const [ownerBytes, original] = await Promise.all([
    fs.readFile(ownerPath), fs.readFile(configPath, "utf8"),
  ]);
  const owner = JSON.parse(ownerBytes.toString("utf8"));
  assert.deepEqual(Object.keys(owner).sort(), ["agent_id", "command", "version", "wardian_home"],
    "The ownership record must retain Wardian's exact schema");
  assert.equal(owner.version, 1);
  assert.equal(owner.agent_id, agentId, "The messaging record belongs to a different agent");
  assert.ok(samePath(owner.wardian_home, home), "The messaging record belongs to a different Wardian home");
  assert.ok(samePath(owner.command, commandReal), "The messaging record names a different executable");

  const server = generatedTomlSection(original, "mcp_servers.wardian");
  const env = generatedTomlSection(original, "mcp_servers.wardian.env");
  assert.equal(generatedTomlValue(server, "command"), owner.command,
    "The Codex server command differs from its ownership record");
  assert.deepEqual(generatedTomlValue(server, "args"), ["mcp", "serve"],
    "The Codex server args differ from Wardian's managed registration");
  assert.ok(samePath(generatedTomlValue(env, "WARDIAN_HOME"), owner.wardian_home),
    "The Codex server environment names a different Wardian home");
  assert.equal(generatedTomlValue(env, "WARDIAN_SESSION_ID"), agentId,
    "The Codex server environment names a different agent");
  assert.equal(hasReplyToolPolicy(original), false,
    "A preexisting or ambiguous Wardian reply-tool policy must be reviewed, never overwritten");
  const newline = original.includes("\r\n") ? "\r\n" : "\n";
  const updated = original + (original.endsWith("\n") ? "" : newline)
    + newline + "[mcp_servers.wardian.tools.reply]" + newline + "approval_mode = \"approve\"" + newline;

  const handle = await fs.open(configPath, "r+");
  try {
    const openedStat = await handle.stat();
    assertSameFileIdentity(configStat, openedStat, "Managed Codex config");
    assert.equal(await readHandleUtf8(handle), original,
      "Managed Codex config changed between validation and open");
    for (const [directory, before] of directoryStats) {
      assertSameFileIdentity(before, await fs.lstat(directory), "Owned fixture directory");
    }
    assertSameFileIdentity(ownerStat, await fs.lstat(ownerPath), "Managed messaging ownership record");
    assert.deepEqual(await fs.readFile(ownerPath), ownerBytes,
      "Managed messaging ownership changed during fixture preparation");
    assertSameFileIdentity(commandStat, await fs.lstat(commandReal), "Managed MCP command");
    assertSameFileIdentity(openedStat, await fs.lstat(configPath), "Managed Codex config");
    assert.equal(await readHandleUtf8(handle), original,
      "Managed Codex config changed before the fixture grant was written");
    await writeHandleUtf8(handle, updated);
    assertSameFileIdentity(openedStat, await fs.lstat(configPath), "Managed Codex config");
    for (const [directory, before] of directoryStats) {
      assertSameFileIdentity(before, await fs.lstat(directory), "Owned fixture directory");
    }
    assert.equal(await readHandleUtf8(handle), updated, "The exact fixture grant was not persisted");
  } finally {
    await handle.close();
  }

  return {
    agent_id: agentId,
    config_before_sha256: createHash("sha256").update(original).digest("hex"),
    config_after_sha256: createHash("sha256").update(updated).digest("hex"),
    fixture_permission_delta: "[mcp_servers.wardian.tools.reply] approval_mode=approve",
    command_file: path.basename(commandReal),
  };
}

/** Persist evidence only after this run acquired its isolated home. */
export async function writeOwnedReplyReport(harness, report) {
  if (typeof harness.runId !== "string" || !harness.runId ||
    harness.homeLock?.runId !== harness.runId) return false;
  await fs.writeFile(path.join(harness.isolatedHome, "automatic-inbound-reply-report.json"),
    JSON.stringify(report, null, 2) + "\n");
  return true;
}

/** Record the first causal failure without allowing report I/O to replace it. */
export async function persistAutomaticReplyFailure(report, caseName, error, save) {
  const message = error instanceof Error ? error.message : String(error);
  report.cases[caseName] = { ...report.cases[caseName], status: "fail", error: message };
  report.error = message;
  try {
    await save();
  } catch (saveError) {
    report.report_persistence_error = saveError instanceof Error ? saveError.message : String(saveError);
  }
  return error;
}

/** Preserve the caller's single absolute run deadline, including cleanup. */
export function automaticReplyDeadlines(deadlineUtc, cleanupReserveMs, now = Date.now()) {
  assert.match(deadlineUtc ?? "", /(Z|[+-]\d{2}:\d{2})$/u, "An explicit granted UTC deadline is required");
  const deadlineAt = Date.parse(deadlineUtc);
  assert.ok(Number.isFinite(deadlineAt), "The granted deadline must be valid");
  assert.ok(Number.isSafeInteger(cleanupReserveMs) && cleanupReserveMs > 0, "Cleanup reserve must be positive");
  assert.ok(deadlineAt - now > cleanupReserveMs, "The original run budget must include cleanup");
  return { deadlineAt, workDeadlineAt: deadlineAt - cleanupReserveMs, cleanupReserveMs };
}

/** Select only the exact, directly owned native Antigravity conversation. */
export function exactCurrentCascade(snapshot, expectedSession) {
  assert.equal(snapshot.provider, "antigravity");
  assert.equal(snapshot.sessions.length, 1, "Current mode must resolve one owned native cascade");
  const cascade = snapshot.sessions[0];
  assert.equal(cascade.id, expectedSession);
  assert.equal(cascade.ownership_basis, "workspace_uri",
    "Current-mode evidence requires direct workspace ownership");
  assert.ok(Array.isArray(cascade.records));
  return cascade;
}

/** Read the persisted readiness for one exact requester without touching delivery state. */
export async function readProviderInputState(home, sessionId) {
  const databasePath = path.join(home, "state.db");
  try {
    await fs.access(databasePath);
  } catch (error) {
    if (error.code === "ENOENT") return null;
    throw error;
  }
  const { DatabaseSync } = await import("node:sqlite");
  const db = new DatabaseSync(databasePath, { readOnly: true });
  try {
    db.exec("BEGIN");
    return db.prepare(
      "SELECT session_id,state,generation FROM provider_input_state WHERE session_id=?",
    ).get(sessionId) ?? null;
  } finally {
    db.close();
  }
}

export function providerInputIsReady(state, expectedSessionId) {
  return state?.session_id === expectedSessionId && state.state === "ready"
    && Number.isSafeInteger(state.generation) && state.generation > 0;
}

function parseLeaseTimestamp(value) {
  if (typeof value !== "string") return null;
  const match = /^(\d{4})-(\d{2})-(\d{2})[Tt](\d{2}):(\d{2}):(\d{2})(?:\.(\d+))?([Zz]|([+-])(\d{2}):(\d{2}))$/u.exec(value);
  if (!match) return null;
  const [, yearText, monthText, dayText, hourText, minuteText, secondText, , zone, , offsetHourText, offsetMinuteText] = match;
  const year = Number(yearText);
  const month = Number(monthText);
  const day = Number(dayText);
  const hour = Number(hourText);
  const minute = Number(minuteText);
  const second = Number(secondText);
  const leapYear = year % 4 === 0 && (year % 100 !== 0 || year % 400 === 0);
  const daysInMonth = [31, leapYear ? 29 : 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31][month - 1];
  if (!daysInMonth || day < 1 || day > daysInMonth || hour > 23 || minute > 59 || second > 59) return null;
  if (zone !== "Z" && zone !== "z" && (Number(offsetHourText) > 23 || Number(offsetMinuteText) > 59)) return null;
  const calendar = new Date(0);
  calendar.setUTCFullYear(year, month - 1, day);
  calendar.setUTCHours(hour, minute, second, 0);
  const fractionMs = Number(((match[7] ?? "") + "000").slice(0, 3));
  let timestamp = calendar.getTime() + fractionMs;
  if (zone !== "Z" && zone !== "z") {
    const offset = (Number(offsetHourText) * 60 + Number(offsetMinuteText)) * 60_000;
    timestamp += match[9] === "+" ? -offset : offset;
  }
  return Number.isFinite(timestamp) ? timestamp : null;
}

function validateConversationLease(lease, index) {
  assert.ok(lease && typeof lease === "object" && !Array.isArray(lease),
    `conversation lease ${index} must be an object`);
  for (const field of ["agent_id", "provider", "resume_session", "owner_kind", "owner_id", "mode",
    "started_at", "heartbeat_at", "expires_at"]) {
    assert.equal(typeof lease[field], "string", `conversation lease ${index} has invalid ${field}`);
  }
  for (const field of ["agent_id", "provider", "owner_kind", "owner_id"]) {
    assert.ok(lease[field].trim(), `conversation lease ${index} has empty ${field}`);
  }
  assert.ok(CONVERSATION_LEASE_MODES.has(lease.mode), `conversation lease ${index} has unknown mode`);
  if (lease.mode === "background_resume") {
    assert.ok(lease.resume_session.trim(), `conversation lease ${index} has empty resume_session`);
  }
  if (lease.mode === "uncertain_previous_provider") {
    assert.equal(lease.owner_kind, "prior_provider_hold",
      `conversation lease ${index} has invalid previous provider hold owner`);
    assert.ok(lease.resume_session.trim(), `conversation lease ${index} has empty previous provider session`);
  }
  for (const field of ["started_at", "heartbeat_at", "expires_at"]) {
    assert.notEqual(parseLeaseTimestamp(lease[field]), null,
      `conversation lease ${index} has invalid ${field}`);
  }
  if (Object.hasOwn(lease, "acquisition_id")) {
    assert.equal(typeof lease.acquisition_id, "string", `conversation lease ${index} has invalid acquisition_id`);
  }
  if (Object.hasOwn(lease, "owner_node_id")) {
    assert.ok(lease.owner_node_id === null || typeof lease.owner_node_id === "string",
      `conversation lease ${index} has invalid owner_node_id`);
  }
}

function safeLeaseOwner(lease) {
  return {
    owner_kind: lease.owner_kind,
    owner_id: lease.owner_id,
    mode: lease.mode,
    agent_id: lease.agent_id,
    provider: lease.provider,
    resume_session: lease.resume_session,
    started_at: lease.started_at,
    heartbeat_at: lease.heartbeat_at,
    expires_at: lease.expires_at,
  };
}

/** Apply the same requester conflict and expiry rules as the canonical lease store. */
export function activeConversationLeaseConflicts(leases, requesterId, resumeSession = "", now = Date.now()) {
  assert.ok(typeof requesterId === "string" && requesterId.trim(), "A nonempty requester id is required");
  assert.ok(resumeSession === null || typeof resumeSession === "string", "resume_session must be a string or null");
  assert.ok(Array.isArray(leases), "Conversation leases must be an array");
  const nowMs = now instanceof Date ? now.getTime()
    : typeof now === "string" ? parseLeaseTimestamp(now) : Number(now);
  assert.ok(Number.isFinite(nowMs), "The lease observation time must be valid");
  const resume = resumeSession ?? "";
  return leases.flatMap((lease, index) => {
    validateConversationLease(lease, index);
    const matchesResume = resume.trim() !== "" && lease.resume_session === resume;
    const conflicts = lease.mode === "uncertain_previous_provider"
      ? matchesResume
      : parseLeaseTimestamp(lease.expires_at) > nowMs && (lease.agent_id === requesterId || matchesResume);
    return conflicts ? [safeLeaseOwner(lease)] : [];
  });
}

function observationTime(now) {
  const timestamp = now instanceof Date ? now.getTime()
    : typeof now === "string" ? parseLeaseTimestamp(now) : Number(now);
  assert.ok(Number.isFinite(timestamp), "The lease observation time must be valid");
  return { timestamp, observedAt: new Date(timestamp).toISOString() };
}

/** Read the owned runtime lease file without creating, changing, or releasing any lease. */
export async function readConversationLeaseSnapshot(home, requesterId, resumeSession = "", now = Date.now()) {
  assert.ok(typeof requesterId === "string" && requesterId.trim(), "A nonempty requester id is required");
  assert.ok(resumeSession === null || typeof resumeSession === "string", "resume_session must be a string or null");
  const { timestamp, observedAt } = observationTime(now);
  const exactResumeSession = resumeSession ?? "";
  const leasePath = path.join(home, "runtime", "conversation-leases.json");
  let bytes;
  let exists = true;
  try {
    bytes = await fs.readFile(leasePath);
  } catch (error) {
    if (error.code !== "ENOENT") {
      error.lease_snapshot = {
        observed_at: observedAt, exists: null, snapshot_sha256: null, valid: false,
        read_error: error instanceof Error ? error.message : String(error),
      };
      throw error;
    }
    bytes = Buffer.alloc(0);
    exists = false;
  }
  const base = {
    observed_at: observedAt,
    exists,
    snapshot_sha256: createHash("sha256").update(bytes).digest("hex"),
  };
  if (!exists) {
    return {
      ...base, valid: true, lease_count: 0, active_conflict_count: 0,
      has_active_conflict: false, active_conflicts: [], omitted_active_conflicts: 0,
    };
  }

  try {
    const document = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(bytes));
    assert.ok(document && typeof document === "object" && !Array.isArray(document),
      "conversation lease file must contain an object");
    const schema = Object.hasOwn(document, "schema") ? document.schema : 1;
    assert.equal(schema, 1, `unsupported conversation lease schema ${String(schema)}`);
    const leases = Object.hasOwn(document, "leases") ? document.leases : [];
    assert.ok(Array.isArray(leases), "conversation lease file leases must be an array");
    leases.forEach(validateConversationLease);
    const conflicts = activeConversationLeaseConflicts(leases, requesterId, exactResumeSession, timestamp);
    return {
      ...base,
      valid: true,
      lease_count: leases.length,
      active_conflict_count: conflicts.length,
      has_active_conflict: conflicts.length > 0,
      active_conflicts: conflicts.slice(0, MAX_REPORTED_LEASE_CONFLICTS),
      omitted_active_conflicts: Math.max(0, conflicts.length - MAX_REPORTED_LEASE_CONFLICTS),
    };
  } catch (error) {
    error.lease_snapshot = {
      ...base, valid: false,
      validation_error: error instanceof Error ? error.message : String(error),
    };
    throw error;
  }
}

/** Retain only first/last blockers and the latest bounded snapshot, never poll history. */
export function recordConversationLeaseObservation(evidence, snapshot) {
  evidence.final_snapshot = snapshot;
  if (snapshot.has_active_conflict) {
    evidence.first_blocking ??= snapshot;
    evidence.last_blocking = snapshot;
  }
  return snapshot.has_active_conflict;
}

/** A complete record requires both the reader's completion and native status. */
export function completedAssistantRows(records) {
  return records.filter((row) => row.role === "assistant" && row.complete === true && row.native_status === 3);
}

/** Verify the admitted requester origin against its live provider-input generation. */
export function assertRequesterOriginMatchesProviderInput(
  proof, expectedProvider, expectedConversationId,
) {
  const origin = proof?.requester_origin;
  assert.ok(origin, "The admitted task must have a persisted requester origin");
  assert.equal(origin.provider, expectedProvider, "The task requester provider changed");
  assert.equal(origin.conversation_id, expectedConversationId,
    "The task requester conversation differs from the captured provider conversation");
  assert.ok(Number.isSafeInteger(origin.generation) && origin.generation > 0,
    "The task requester origin must have a positive provider-input generation");
  assert.ok(Number.isSafeInteger(proof.provider_input_generation) && proof.provider_input_generation > 0,
    "The requester must expose its current provider-input generation");
  assert.equal(origin.generation, proof.provider_input_generation,
    "The task requester generation must match the current persisted provider-input generation, not terminal runtime generation");
}

export function assertPromptDeliveryAccepted(receipt, expectedSessionId, expectedProvider) {
  assert.ok(receipt && typeof receipt === "object", "The initial prompt must return one delivery detail");
  assert.equal(receipt.uuid, expectedSessionId, "The initial prompt receipt targeted a different agent");
  assert.equal(receipt.provider, expectedProvider, "The initial prompt receipt reported a different provider");
  assert.ok(["provider_accepted", "queued"].includes(receipt.delivery_state),
    `The initial prompt was not accepted or queued: ${JSON.stringify(receipt)}`);
}

function inlineBody(reference) {
  const value = JSON.parse(reference);
  assert.equal(value.storage, "inline", "The bounded acceptance text must remain inline");
  assert.equal(typeof value.body, "string");
  return value.body;
}

/**
 * Read the one task admitted by the requester and its canonical reply without
 * claiming, acknowledging, or polling the delivery mailbox.
 */
export async function readCanonicalTaskReply(home, sender, recipient, expectedRequestId = null) {
  const databasePath = path.join(home, "state.db");
  try {
    await fs.access(databasePath);
  } catch (error) {
    if (error.code === "ENOENT") return null;
    throw error;
  }
  const { DatabaseSync } = await import("node:sqlite");
  const db = new DatabaseSync(databasePath, { readOnly: true });
  try {
    db.exec("BEGIN");
    const tasks = db.prepare(
      "SELECT id,target_session_ids,body_ref,status FROM interactions WHERE kind='task' AND sender_session_id=?",
    ).all(sender);
    if (tasks.length === 0) return null;
    assert.equal(tasks.length, 1, "The requester must admit exactly one task");
    const task = tasks[0];
    if (expectedRequestId !== null) assert.equal(task.id, expectedRequestId, "The owned request changed");
    assert.deepEqual(JSON.parse(task.target_session_ids), [recipient]);

    const origins = db.prepare(
      "SELECT requester_provider,requester_conversation_id,requester_generation FROM agent_message_delivery " +
      "WHERE interaction_id=? AND operation='followup_task'",
    ).all(task.id);
    assert.equal(origins.length, 1, "The admitted task must have one canonical requester-origin row");
    const origin = origins[0];
    assert.ok(typeof origin.requester_provider === "string" && origin.requester_provider.trim(),
      "The admitted task requester provider is missing");
    assert.ok(typeof origin.requester_conversation_id === "string" && origin.requester_conversation_id.trim(),
      "The admitted task requester conversation is missing");
    assert.ok(Number.isSafeInteger(origin.requester_generation) && origin.requester_generation > 0,
      "The admitted task requester generation is missing or invalid");
    const providerInputState = db.prepare(
      "SELECT generation FROM provider_input_state WHERE session_id=?",
    ).get(sender);

    const replies = db.prepare(
      "SELECT id,sender_session_id,target_session_ids,parent_interaction_id,body_ref FROM interactions WHERE kind='reply' AND parent_interaction_id=?",
    ).all(task.id);
    assert.ok(replies.length <= 1, "The task received duplicate canonical replies");

    const requesterReplies = db.prepare(
      "SELECT id,parent_interaction_id FROM interactions WHERE kind='reply' AND sender_session_id=?",
    ).all(sender);
    let reply = null;
    let replyToReplyCount = 0;
    if (replies.length === 1) {
      const row = replies[0];
      assert.equal(row.sender_session_id, recipient);
      assert.deepEqual(JSON.parse(row.target_session_ids), [sender]);
      assert.equal(row.parent_interaction_id, task.id);
      const statuses = db.prepare(
        "SELECT status FROM structured_replies WHERE request_id=?",
      ).all(task.id);
      assert.equal(statuses.length, 1, "The canonical request must have one structured reply status");
      replyToReplyCount = db.prepare(
        "SELECT id FROM interactions WHERE kind='reply' AND parent_interaction_id=?",
      ).all(row.id).length;
      reply = {
        interaction_id: row.id,
        request_id: task.id,
        parent_interaction_id: row.parent_interaction_id,
        sender: row.sender_session_id,
        recipients: JSON.parse(row.target_session_ids),
        status: statuses[0].status,
        body: inlineBody(row.body_ref),
      };
    }

    return {
      request_id: task.id,
      task_status: task.status,
      task_body: inlineBody(task.body_ref),
      requester_origin: {
        provider: origin.requester_provider,
        conversation_id: origin.requester_conversation_id,
        generation: origin.requester_generation,
      },
      provider_input_generation: providerInputState?.generation ?? null,
      reply_count: replies.length,
      reply,
      requester_reply_count: requesterReplies.length,
      reply_to_reply_count: replyToReplyCount,
    };
  } finally {
    db.close();
  }
}
