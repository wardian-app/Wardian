import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import fs from "node:fs/promises";
import path from "node:path";

export const WINDOWS_CWD_LIMIT = 258;
const ALIAS_RECORD = ".wardian-habitat-alias.json";
const LAUNCH_JOURNAL = ".wardian-launch-config.json";

function comparablePath(value) {
  let normalized = path.resolve(String(value)).replace(/[\\/]+/gu, "\\");
  if (normalized.startsWith("\\\\?\\")) normalized = normalized.slice(4);
  return process.platform === "win32" ? normalized.toLowerCase() : normalized;
}

function assertSamePath(actual, expected, message) {
  assert.equal(comparablePath(actual), comparablePath(expected), message);
}

async function exists(target) {
  try {
    await fs.lstat(target);
    return true;
  } catch (error) {
    if (error?.code === "ENOENT") return false;
    throw error;
  }
}

async function requireJson(target, label) {
  try {
    return JSON.parse(await fs.readFile(target, "utf8"));
  } catch (error) {
    throw new Error(`could not read ${label}`, { cause: error });
  }
}

function hashValue(value) {
  return createHash("sha256").update(JSON.stringify(value)).digest("hex");
}

function safeRelativePath(home, target) {
  return path.relative(home, target).replace(/[\\/]+/gu, "/") || ".";
}

function utf16Units(value) {
  assert.equal(typeof value, "string", "path value must be a string");
  return value.length;
}

function assertIdentityShape(identity, label) {
  assert.ok(Array.isArray(identity) && identity.length === 2, `${label} must be a pair`);
  for (const component of identity) {
    assert.equal(typeof component, "number", `${label} must contain numeric components`);
    assert.ok(Number.isInteger(component) && component >= 0, `${label} contains an invalid integer`);
  }
}

async function readDirectoryNames(target) {
  return (await fs.readdir(target)).sort((left, right) => left.localeCompare(right));
}

async function assertAliasLink(record, home, agentId, workspacePath) {
  const agentHabitat = path.join(home, "agents", agentId, "habitat");
  const habitat = await fs.realpath(agentHabitat);
  const slot = path.dirname(record.target);
  const slotRecord = path.join(slot, ALIAS_RECORD);

  assert.equal(record.version, 1, "habitat alias record version changed");
  assert.equal(record.agent_id, agentId, "habitat alias record agent identity changed");
  assertSamePath(record.wardian_home, home, "habitat alias record home changed");
  assertSamePath(record.habitat, habitat, "habitat alias record source changed");
  assertSamePath(record.target, path.join(slot, "h"), "habitat alias target is not the slot h entry");
  assert.match(record.token, /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/iu,
    "habitat alias token is not a UUID v4");
  assertIdentityShape(record.habitat_identity, "habitat identity");
  assertIdentityShape(record.slot_identity, "slot identity");
  assert.equal(await fs.realpath(record.target), habitat,
    "habitat alias target does not resolve to the owning habitat");

  const aliasWorkspace = path.join(record.target, "workspace");
  const logicalHabitatCwd = path.join(agentHabitat, "workspace");
  const logicalHabitatCwdUnits = utf16Units(logicalHabitatCwd);
  const aliasWorkspaceUnits = utf16Units(aliasWorkspace);
  assert.ok(logicalHabitatCwdUnits > WINDOWS_CWD_LIMIT,
    "long-habitat gate requires an over-limit managed habitat cwd");
  assert.ok(aliasWorkspaceUnits <= WINDOWS_CWD_LIMIT,
    "owned habitat alias workspace still exceeds the observed Windows limit");
  assert.equal(await fs.realpath(logicalHabitatCwd), await fs.realpath(workspacePath),
    "managed habitat workspace changed the external project target");
  assert.equal(await fs.realpath(aliasWorkspace), await fs.realpath(workspacePath),
    "alias workspace changed the logical target");
  assert.deepEqual(await readDirectoryNames(slot), [ALIAS_RECORD, "h"].sort(),
    "habitat alias slot contains an unexpected entry");
  assert.deepEqual(await requireJson(slotRecord, "slot alias record"), record,
    "slot alias record does not match the agent record");

  return {
    agent_id: record.agent_id,
    token: record.token,
    wardian_home: record.wardian_home,
    habitat: record.habitat,
    target: record.target,
    habitat_identity: [...record.habitat_identity],
    slot_identity: [...record.slot_identity],
    slot,
    alias_workspace: aliasWorkspace,
    logical_habitat_cwd_utf16_units: logicalHabitatCwdUnits,
    alias_workspace_utf16_units: aliasWorkspaceUnits,
  };
}

async function captureAlias(home, agentId, workspacePath) {
  const recordPath = path.join(home, "agents", agentId, ALIAS_RECORD);
  assert.ok(await exists(recordPath), `habitat alias record is missing for ${agentId}`);
  const record = await requireJson(recordPath, "agent alias record");
  const evidence = await assertAliasLink(record, home, agentId, workspacePath);
  return { recordPath, record, evidence };
}

function tomlSection(text, sectionName = null) {
  const lines = text.split(/\r?\n/u);
  if (sectionName === null) {
    const firstHeader = lines.findIndex((line) => /^\s*\[[^\]]+\]\s*$/u.test(line));
    return lines.slice(0, firstHeader < 0 ? undefined : firstHeader).join("\n");
  }
  const header = `[${sectionName}]`;
  const start = lines.findIndex((line) => line.trim() === header);
  assert.ok(start >= 0, `missing generated TOML section ${header}`);
  const tail = lines.slice(start + 1);
  const end = tail.findIndex((line) => /^\s*\[[^\]]+\]\s*$/u.test(line));
  return tail.slice(0, end < 0 ? undefined : end).join("\n");
}

function generatedTomlValue(section, key) {
  const line = section.split("\n").find((candidate) => candidate.trimStart().startsWith(`${key} =`));
  assert.ok(line, `missing generated TOML field ${key}`);
  const value = line.slice(line.indexOf("=") + 1).trim();
  if (/^'[^'\r\n]*'$/u.test(value) || /^"[^"\r\n]*"$/u.test(value)) return value.slice(1, -1);
  try {
    return JSON.parse(value);
  } catch (error) {
    throw new Error(`unsupported generated TOML representation for ${key}`, { cause: error });
  }
}

function assertExpectedSemanticValues(expectedValues) {
  assert.ok(Array.isArray(expectedValues) && expectedValues.length > 0,
    "semantic Codex config assertions require explicit expected values");
  for (const expected of expectedValues) {
    assert.ok(expected && typeof expected === "object", "expected Codex value must be an object");
    assert.ok(typeof expected.key === "string" && expected.key.trim(),
      "expected Codex value requires a nonempty key");
    assert.ok(typeof expected.source === "string" && expected.source.trim(),
      "expected Codex value requires a source-derived label");
    assert.notEqual(expected.value, undefined, `expected Codex value is missing for ${expected.key}`);
    assert.notEqual(expected.value, null, `expected Codex value is null for ${expected.key}`);
    if (typeof expected.value === "string") {
      assert.ok(expected.value.trim(), `expected Codex value is empty for ${expected.key}`);
    }
  }
}

/** Assert the semantic launch values owned by the long-habitat acceptance gate. */
export async function assertCodexLaunchArtifacts({ home, agentId, expectedValues }) {
  assertExpectedSemanticValues(expectedValues);
  const habitat = path.join(home, "agents", agentId, "habitat");
  const configPath = path.join(habitat, ".codex", "config.toml");
  const journalPath = path.join(habitat, ".codex", LAUNCH_JOURNAL);
  const config = await fs.readFile(configPath, "utf8");
  for (const expected of expectedValues) {
    assert.deepEqual(
      generatedTomlValue(tomlSection(config, expected.section ?? null), expected.key),
      expected.value,
      `generated TOML value changed for ${expected.section ? `${expected.section}.` : ""}${expected.key}`,
    );
  }
  assert.equal(await exists(journalPath), false, "Codex launch journal remained after semantic restore");
  return {
    config_path_relative: safeRelativePath(home, configPath),
    journal_path_relative: safeRelativePath(home, journalPath),
    journal_present: false,
    expected_values: expectedValues.map((expected) => ({
      section: expected.section ?? null,
      key: expected.key,
      source: expected.source,
      value_type: typeof expected.value,
      value_sha256: hashValue(expected.value),
    })),
  };
}

/**
 * Pre-prompt gate. The caller supplies AgentConfig-shaped data from list_agents;
 * this deliberately reads `folder`, the persisted logical workspace field.
 */
export async function assertLongHabitatPrerequisites({
  home,
  agent,
  provider,
  workspacePath,
  expectedCodexValues,
}) {
  assert.ok(["claude", "codex"].includes(provider),
    `long-habitat gate supports Claude and Codex, received ${provider}`);
  assert.ok(agent?.session_id, "long-habitat gate requires the provider agent session ID");
  assertSamePath(agent.folder, workspacePath, "provider AgentConfig.folder changed before delivery");

  const alias = await captureAlias(home, agent.session_id, workspacePath);
  const config = provider === "codex"
    ? await assertCodexLaunchArtifacts({ home, agentId: agent.session_id, expectedValues: expectedCodexValues })
    : null;
  return { provider, agent_id: agent.session_id, alias, config };
}

async function currentAgent(invokeTauri, driver, sessionId) {
  const agents = await invokeTauri(driver, "list_agents");
  assert.ok(Array.isArray(agents), "list_agents did not return a roster");
  return agents.find((entry) => entry.session_id === sessionId) ?? null;
}

async function assertMarker(markerPath, expectedBytes) {
  assert.deepEqual(await fs.readFile(markerPath), expectedBytes, "external logical workspace marker changed");
}

async function assertRemoved(target, label) {
  assert.equal(await exists(target), false, `${label} remained after owned agent removal`);
}

function safeMaintainedTurnEvidence(report, provider) {
  const entries = Array.isArray(report?.transcript_user_evidence)
    ? report.transcript_user_evidence.filter((entry) => entry?.provider === provider)
    : [];
  return entries.slice(-4).map((entry) => ({
    provider: entry.provider,
    candidates: (Array.isArray(entry.candidates) ? entry.candidates : []).slice(-8).map((candidate) => ({
      id_sha256: candidate.id ? hashValue(candidate.id) : null,
      provider: candidate.provider,
      kind: candidate.kind,
      role: candidate.role,
      source: candidate.source,
      provider_log: candidate.provider_log,
      native_identity_present: Boolean(candidate.native_identity),
      timestamp_present: candidate.timestamp !== null && candidate.timestamp !== undefined,
      text_sha256: candidate.text_sha256,
      text_byte_count: candidate.text_byte_count,
    })),
  }));
}

function safeTranscriptEvent(event, provider, marker) {
  const metadata = event?.metadata && typeof event.metadata === "object" ? event.metadata : {};
  const source = typeof event?.source === "string" ? event.source.trim() : "";
  const knownSources = new Set([
    "conversation_archive", "gemini_log", "headless_process", "opencode_db",
    "provider_session", "response_item", "terminal_fallback",
  ]);
  const text = typeof event?.text === "string" ? event.text : "";
  const providerAuthored = event?.provider === provider
    && event?.kind === "message"
    && event?.role === "assistant"
    && metadata.provider_log === true
    && source.length > 0
    && text.includes(marker);
  return {
    id_sha256: typeof event?.id === "string" ? hashValue(event.id) : null,
    provider: event?.provider === provider ? provider : "other",
    kind: event?.kind === "message" ? "message" : "other",
    role: ["assistant", "user", "system", "tool"].includes(event?.role) ? event.role : "other",
    source: knownSources.has(source) ? source : "other",
    provider_log: typeof metadata.provider_log === "boolean" ? metadata.provider_log : "absent",
    marker: text.includes(marker),
    provider_authored_marker: providerAuthored,
    provider_session_id_sha256: typeof metadata.provider_session_id === "string"
      ? hashValue(metadata.provider_session_id)
      : null,
    turn_id_sha256: typeof metadata.turn_id === "string"
      ? hashValue(metadata.turn_id)
      : typeof event?.turn_id === "string" ? hashValue(event.turn_id) : null,
    text_sha256: hashValue(text),
    text_byte_count: Buffer.byteLength(text, "utf8"),
  };
}

async function captureBoundedTurnEvidence({
  invokeTauri, driver, provider, sessionId, marker, maintainedReport,
}) {
  assert.ok(typeof marker === "string" && marker.length > 0,
    "long-habitat evidence requires the maintained delivery marker");
  const events = await invokeTauri(driver, "load_agent_chat_transcript", { sessionId });
  assert.ok(Array.isArray(events), "load_agent_chat_transcript did not return an event array");
  const userMarkers = events.filter((event) => event?.role === "user" && (event.text ?? "").includes(marker));
  const providerAnswers = events.filter((event) => safeTranscriptEvent(event, provider, marker).provider_authored_marker);
  assert.equal(userMarkers.length, 1, "bounded evidence did not find exactly one maintained user marker");
  assert.equal(providerAnswers.length, 1, "bounded evidence did not find exactly one provider-authored marker");
  const relevant = events
    .filter((event) => (event?.text ?? "").includes(marker)
      || safeTranscriptEvent(event, provider, marker).provider_authored_marker)
    .slice(-12)
    .map((event) => safeTranscriptEvent(event, provider, marker));
  return {
    marker,
    total_events: events.length,
    captured_events: relevant.length,
    maintained_report_reuse: safeMaintainedTurnEvidence(maintainedReport, provider),
    bounded_transcript: relevant,
    provider_authored_marker_count: providerAnswers.length,
  };
}

async function captureBoundedArchiveEvidence({ invokeTauri, driver, provider, sessionId }) {
  const result = await invokeTauri(driver, "list_conversations", { agent: sessionId, scopeAll: false });
  const archive = Array.isArray(result?.conversations)
    ? result.conversations.find((entry) => entry?.agent_id === sessionId
      && entry?.provider === provider && Number(entry.record_count) > 0)
    : null;
  assert.ok(archive, "bounded evidence did not find a durable provider archive");
  return {
    conversation_id_sha256: typeof archive.conversation_id === "string"
      ? hashValue(archive.conversation_id)
      : null,
    agent_id: sessionId,
    provider,
    record_count: Number(archive.record_count),
  };
}

function safeAliasEvidence(alias, home) {
  const evidence = alias.evidence;
  return {
    agent_id: evidence.agent_id,
    token_sha256: hashValue(evidence.token),
    habitat_relative: safeRelativePath(home, evidence.habitat),
    target_entry: "h",
    slot_name: path.basename(evidence.slot),
    habitat_identity: evidence.habitat_identity,
    slot_identity: evidence.slot_identity,
    logical_habitat_cwd_utf16_units: evidence.logical_habitat_cwd_utf16_units,
    alias_workspace_utf16_units: evidence.alias_workspace_utf16_units,
  };
}

async function writeBoundedEvidence(evidenceRoot, provider, sessionId, evidence) {
  assert.ok(typeof evidenceRoot === "string" && path.isAbsolute(evidenceRoot),
    "long-habitat evidence requires an absolute private evidence root");
  await fs.mkdir(evidenceRoot, { recursive: true });
  const safeProvider = provider.replace(/[^a-z0-9_-]/giu, "_");
  const safeSession = sessionId.replace(/[^a-z0-9_-]/giu, "_");
  const target = path.join(evidenceRoot, `joint-${safeProvider}-${safeSession}.json`);
  await fs.writeFile(target, JSON.stringify(evidence, null, 2));
  return target;
}

/**
 * Continue a maintained provider case after its existing pause succeeded.
 * The helper retains only bounded hashes, counts, identities, and relative paths
 * before explicit deletion of the owned agent.
 */
export async function afterMaintainedProviderPause({
  driver,
  harness,
  cliPath,
  agent,
  provider,
  workspacePath,
  preflight,
  markerPath = path.join(workspacePath, "marker.txt"),
  deliveryMarker,
  maintainedReport,
  evidenceRoot,
  invokeTauri,
  pauseRealProviderAgent,
  waitForProviderInputReady,
  runCliOk,
}) {
  for (const [name, value] of Object.entries({
    invokeTauri,
    pauseRealProviderAgent,
    waitForProviderInputReady,
    runCliOk,
  })) {
    assert.equal(typeof value, "function", `long-habitat hook requires maintained helper ${name}`);
  }
  assert.ok(preflight?.alias?.recordPath, "long-habitat hook requires pre-prompt alias evidence");
  assert.ok(agent?.session_id, "long-habitat hook requires the returned provider agent");
  assert.equal(preflight.provider, provider, "long-habitat preflight provider changed");
  assertSamePath(agent.folder, workspacePath, "provider AgentConfig.folder changed before pause");

  const markerBefore = await fs.readFile(markerPath);
  const paused = await currentAgent(invokeTauri, driver, agent.session_id);
  assert.ok(paused, "paused provider agent was not retained");
  assert.equal(paused.is_off, true, "maintained pause did not leave the agent off");
  assertSamePath(paused.folder, workspacePath, "paused provider agent changed logical workspace");

  const aliasAfterPause = await captureAlias(harness.isolatedHome, agent.session_id, workspacePath);
  assert.deepEqual(aliasAfterPause.evidence, preflight.alias.evidence,
    "maintained pause changed the owned habitat alias");

  await invokeTauri(driver, "resume_agent", { sessionId: agent.session_id });
  await waitForProviderInputReady(driver, provider, agent.session_id);
  const resumed = await currentAgent(invokeTauri, driver, agent.session_id);
  assert.ok(resumed, "resumed provider agent disappeared from the roster");
  assert.equal(resumed.is_off, false, "resume_agent did not restore the provider runtime");
  assert.equal(resumed.session_id, paused.session_id, "pause/resume changed the Wardian agent identity");
  assert.equal(resumed.provider, paused.provider, "pause/resume changed the provider identity");
  assertSamePath(resumed.folder, workspacePath, "pause/resume changed the logical workspace");
  if (paused.resume_session && resumed.resume_session) {
    assert.equal(resumed.resume_session, paused.resume_session,
      "pause/resume changed the provider session identity");
  }

  const aliasAfterResume = await captureAlias(harness.isolatedHome, agent.session_id, workspacePath);
  assert.deepEqual(aliasAfterResume.evidence, preflight.alias.evidence,
    "pause/resume replaced the owned habitat alias");
  const turnEvidence = await captureBoundedTurnEvidence({
    invokeTauri,
    driver,
    provider,
    sessionId: agent.session_id,
    marker: deliveryMarker,
    maintainedReport,
  });
  const archiveEvidence = await captureBoundedArchiveEvidence({
    invokeTauri,
    driver,
    provider,
    sessionId: agent.session_id,
  });
  const evidencePath = await writeBoundedEvidence(evidenceRoot, provider, agent.session_id, {
    schema: 1,
    provider,
    agent_id: agent.session_id,
    cwd_inference: {
      logical_habitat_cwd_over_limit: true,
      direct_child_get_current_directory_observed: false,
      basis: "over-limit managed habitat cwd, verified owned short alias, and inspected spawn builder cwd wiring",
    },
    turn: turnEvidence,
    archive: archiveEvidence,
    alias_before_delete: safeAliasEvidence(preflight.alias, harness.isolatedHome),
    alias_after_pause: safeAliasEvidence(aliasAfterPause, harness.isolatedHome),
    alias_after_resume: safeAliasEvidence(aliasAfterResume, harness.isolatedHome),
    config: preflight.config,
    credentials_or_auth_material_copied: false,
    raw_config_or_transcript_copied: false,
  });

  await pauseRealProviderAgent(driver, agent.session_id);
  const pausedAgain = await currentAgent(invokeTauri, driver, agent.session_id);
  assert.ok(pausedAgain, "second pause did not retain the provider agent");
  assert.equal(pausedAgain.is_off, true, "second pause did not join the provider runtime");
  const currentName = pausedAgain.session_name;
  assert.ok(currentName, "owned agent name was unavailable for explicit deletion");
  await runCliOk(cliPath, harness, ["agent", "delete", currentName, "--confirm", currentName]);

  assert.equal(await currentAgent(invokeTauri, driver, agent.session_id), null,
    "explicit owned agent removal retained the agent");
  await assertRemoved(preflight.alias.recordPath, "habitat alias record");
  await assertRemoved(preflight.alias.evidence.target, "habitat alias junction");
  await assertRemoved(preflight.alias.evidence.slot, "habitat alias slot");
  await assertMarker(markerPath, markerBefore);

  return {
    provider,
    session_id: agent.session_id,
    maintained_pause_confirmed: true,
    pause_resume_identity_preserved: true,
    alias_reused_across_pause_resume: true,
    alias_removed_after_explicit_agent_delete: true,
    external_marker_retained: true,
    bounded_evidence_path: evidencePath,
    maintained_report_turn_evidence_reused: turnEvidence.maintained_report_reuse.length > 0,
    cwd_evidence: {
      logical_habitat_cwd_utf16_units: aliasAfterResume.evidence.logical_habitat_cwd_utf16_units,
      alias_workspace_utf16_units: aliasAfterResume.evidence.alias_workspace_utf16_units,
      inference: "over-limit managed habitat cwd plus owned short alias and inspected spawn builder cwd wiring",
      direct_child_get_current_directory_observed: false,
    },
    codex_launch_artifacts: preflight.config,
    removed_agent_local_habitat_or_archive_asserted: false,
  };
}
