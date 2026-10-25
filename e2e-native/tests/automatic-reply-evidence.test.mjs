// @tier nightly — Read-only observer, owned gate, and deadline checks; no app or providers.
import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { DatabaseSync } from "node:sqlite";
import {
  activeConversationLeaseConflicts, appendOwnedReplyToolApproval, assertPromptDeliveryAccepted,
  assertRequesterOriginMatchesProviderInput, automaticReplyDeadlines, completedAssistantRows,
  automaticReplyExecShell, automaticReplyTaskInstructions,
  exactCurrentCascade, persistAutomaticReplyFailure, providerInputIsReady,
  readCanonicalTaskReply, readConversationLeaseSnapshot, readProviderInputState,
  recordConversationLeaseObservation, writeOwnedReplyReport,
} from "../lib/automatic-reply-evidence.mjs";
import { acquireHomeLock, readHomeLock, releaseHomeLock } from "../lib/sessionHome.mjs";
import { waitForReplyRelease } from "./fixtures/automatic-reply-gate.mjs";

async function fixture(t) {
  const parent = path.resolve(os.tmpdir());
  const home = await fs.mkdtemp(path.join(parent, "wardian-automatic-reply-"));
  t.after(async () => {
    const relative = path.relative(parent, path.resolve(home));
    assert.ok(relative && !relative.startsWith("..") && !path.isAbsolute(relative));
    await fs.rm(home, { recursive: true, force: true });
  });
  return home;
}

async function ownedCodexFixture(t, configSuffix = "") {
  const wardianHome = await fixture(t);
  const agentId = "123e4567-e89b-42d3-a456-426614174000";
  const codexHome = path.join(wardianHome, "agents", agentId, "habitat", ".codex");
  const bin = path.join(wardianHome, "bin");
  await fs.mkdir(codexHome, { recursive: true });
  await fs.mkdir(bin, { recursive: true });
  const commandPath = path.join(bin, process.platform === "win32" ? "wardian-mcp.exe" : "wardian-cli");
  await fs.writeFile(commandPath, "fixture executable\n");
  const command = await fs.realpath(commandPath);
  const owner = { version: 1, agent_id: agentId, command, wardian_home: wardianHome };
  const config = `model = "gpt-6-luna"\napproval_policy = "never"\nsandbox_mode = "workspace-write"\n` +
    `[mcp_servers.other]\nlabel = "preserved"\n` +
    `[mcp_servers.wardian]\ncommand = ${JSON.stringify(command)}\nargs = ["mcp", "serve"]\n` +
    `tool_timeout_sec = 70\n` +
    `[mcp_servers.wardian.env]\nWARDIAN_HOME = ${JSON.stringify(wardianHome)}\n` +
    `WARDIAN_SESSION_ID = "${agentId}"\n` + configSuffix;
  await fs.writeFile(path.join(codexHome, ".wardian-messaging.json"), JSON.stringify(owner));
  await fs.writeFile(path.join(codexHome, "config.toml"), config);
  return { wardianHome, agentId, codexHome, commandPath, command, owner, config };
}

test("fixture reply-tool grant is scoped to the exact managed registration and preserves config", async (t) => {
  const owned = await ownedCodexFixture(t,
    "\n[mcp_servers.wardian.tools.review]\napproval_mode = \"prompt\"\n");
  const result = await appendOwnedReplyToolApproval({
    wardianHome: owned.wardianHome,
    agentId: owned.agentId,
    expectedCommand: owned.commandPath,
  });
  const after = await fs.readFile(path.join(owned.codexHome, "config.toml"), "utf8");
  assert.equal(after.slice(0, owned.config.length), owned.config,
    "The fixture must preserve every existing managed and unrelated config key");
  assert.equal(after.slice(owned.config.length),
    "\n[mcp_servers.wardian.tools.reply]\napproval_mode = \"approve\"\n");
  assert.equal(result.agent_id, owned.agentId);
  assert.equal(result.fixture_permission_delta,
    "[mcp_servers.wardian.tools.reply] approval_mode=approve");
  assert.equal(result.command_file, path.basename(owned.commandPath));
});

test("fixture reply-tool grant rejects conflicting or mismatched ownership before writing", async (t) => {
  for (const policy of [
    "\n[mcp_servers.wardian.tools.reply]\napproval_mode = \"deny\"\n",
    "\n[mcp_servers.wardian.tools]\nreply = { approval_mode = \"prompt\" }\n",
    "\n[mcp_servers.wardian.tools] # policy\nreply = { approval_mode = \"prompt\" }\n",
    "\n[\"mcp_servers\".\"wardian\".\"tools\"] # policy\nreply = { approval_mode = \"prompt\" }\n",
    "\n[mcp_servers.wardian.tools.\"reply\"]\napproval_mode = \"prompt\"\n",
  ]) {
    const conflict = await ownedCodexFixture(t, policy);
    const conflictPath = path.join(conflict.codexHome, "config.toml");
    const conflictBefore = await fs.readFile(conflictPath, "utf8");
    await assert.rejects(appendOwnedReplyToolApproval({
      wardianHome: conflict.wardianHome,
      agentId: conflict.agentId,
      expectedCommand: conflict.commandPath,
    }), /reply-tool policy must be reviewed/u);
    assert.equal(await fs.readFile(conflictPath, "utf8"), conflictBefore);
  }

  const invalidIds = await ownedCodexFixture(t);
  for (const agentId of [".", "..", "../other", "fixture-worker-1"]) {
    await assert.rejects(appendOwnedReplyToolApproval({
      wardianHome: invalidIds.wardianHome,
      agentId,
      expectedCommand: invalidIds.commandPath,
    }), /canonical UUID/u);
  }
  assert.equal(await fs.readFile(path.join(invalidIds.codexHome, "config.toml"), "utf8"), invalidIds.config);

  const wrongAgent = await ownedCodexFixture(t);
  const wrongOwner = { ...wrongAgent.owner, agent_id: "foreign-agent" };
  await fs.writeFile(path.join(wrongAgent.codexHome, ".wardian-messaging.json"), JSON.stringify(wrongOwner));
  const wrongAgentPath = path.join(wrongAgent.codexHome, "config.toml");
  const wrongAgentBefore = await fs.readFile(wrongAgentPath, "utf8");
  await assert.rejects(appendOwnedReplyToolApproval({
    wardianHome: wrongAgent.wardianHome,
    agentId: wrongAgent.agentId,
    expectedCommand: wrongAgent.commandPath,
  }), /different agent/u);
  assert.equal(await fs.readFile(wrongAgentPath, "utf8"), wrongAgentBefore);

  const wrongCommand = await ownedCodexFixture(t);
  const otherCommand = path.join(wrongCommand.wardianHome, "bin", "other-mcp");
  await fs.writeFile(otherCommand, "foreign executable\n");
  const wrongCommandPath = path.join(wrongCommand.codexHome, "config.toml");
  const wrongCommandBefore = await fs.readFile(wrongCommandPath, "utf8");
  await assert.rejects(appendOwnedReplyToolApproval({
    wardianHome: wrongCommand.wardianHome,
    agentId: wrongCommand.agentId,
    expectedCommand: otherCommand,
  }), /different executable/u);
  assert.equal(await fs.readFile(wrongCommandPath, "utf8"), wrongCommandBefore);

  const wrongArgs = await ownedCodexFixture(t);
  const wrongArgsPath = path.join(wrongArgs.codexHome, "config.toml");
  const wrongArgsBefore = wrongArgs.config.replace('args = ["mcp", "serve"]', 'args = ["mcp", "receive"]');
  await fs.writeFile(wrongArgsPath, wrongArgsBefore);
  await assert.rejects(appendOwnedReplyToolApproval({
    wardianHome: wrongArgs.wardianHome,
    agentId: wrongArgs.agentId,
    expectedCommand: wrongArgs.commandPath,
  }), /args differ/u);
  assert.equal(await fs.readFile(wrongArgsPath, "utf8"), wrongArgsBefore);

  const wrongEnvironment = await ownedCodexFixture(t);
  const wrongEnvironmentPath = path.join(wrongEnvironment.codexHome, "config.toml");
  const wrongEnvironmentBefore = wrongEnvironment.config.replace(
    `WARDIAN_HOME = ${JSON.stringify(wrongEnvironment.wardianHome)}`,
    'WARDIAN_HOME = "C:/foreign-home"',
  );
  await fs.writeFile(wrongEnvironmentPath, wrongEnvironmentBefore);
  await assert.rejects(appendOwnedReplyToolApproval({
    wardianHome: wrongEnvironment.wardianHome,
    agentId: wrongEnvironment.agentId,
    expectedCommand: wrongEnvironment.commandPath,
  }), /different Wardian home/u);
  assert.equal(await fs.readFile(wrongEnvironmentPath, "utf8"), wrongEnvironmentBefore);
});

test("fixture reply-tool grant refuses a linked managed Codex profile", async (t) => {
  const owned = await ownedCodexFixture(t);
  const movedProfile = owned.codexHome + "-owned-target";
  await fs.rename(owned.codexHome, movedProfile);
  try {
    await fs.symlink(movedProfile, owned.codexHome, process.platform === "win32" ? "junction" : "dir");
  } catch (error) {
    await fs.rename(movedProfile, owned.codexHome);
    if (["EPERM", "EACCES", "ENOTSUP"].includes(error.code)) {
      t.skip("This host does not permit creating a directory link for the ownership guard test");
      return;
    }
    throw error;
  }
  await assert.rejects(appendOwnedReplyToolApproval({
    wardianHome: owned.wardianHome,
    agentId: owned.agentId,
    expectedCommand: owned.commandPath,
  }), /must not be a link/u);
});

test("automatic reply uses an explicit System32 shell on Windows and the portable path elsewhere", () => {
  const gateCommand = "node .wardian-e2e-gate/reply-gate.mjs --deadline \"2030-01-01T00:00:00.000Z\"";
  const windowsShell = automaticReplyExecShell("win32", "C:\\Windows");
  assert.equal(windowsShell, "C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe");
  const windowsInstructions = automaticReplyTaskInstructions(gateCommand, windowsShell);
  assert.match(windowsInstructions, /exec_command/u);
  assert.match(windowsInstructions, /shell/u);
  assert.ok(windowsInstructions.includes(JSON.stringify(windowsShell)));
  assert.ok(!/WindowsApps/iu.test(windowsInstructions));

  assert.equal(automaticReplyExecShell("linux", "/usr"), null);
  assert.equal(automaticReplyExecShell("darwin", "/Applications"), null);
  const portableInstructions = automaticReplyTaskInstructions(gateCommand);
  assert.match(portableInstructions, /^Run node /u);
  assert.ok(!portableInstructions.includes("exec_command"));
  assert.throws(() => automaticReplyExecShell("win32", null), /SystemRoot must be absolute/u);
  assert.throws(() => automaticReplyExecShell("win32", "C:\\WindowsApps"), /must not resolve/u);
});

function database(home, configure = () => {}) {
  const db = new DatabaseSync(path.join(home, "state.db"));
  try {
    db.exec(`
      CREATE TABLE interactions (
        id TEXT PRIMARY KEY, kind TEXT, sender_session_id TEXT, target_session_ids TEXT,
        parent_interaction_id TEXT, body_ref TEXT, status TEXT
      );
      CREATE TABLE structured_replies (request_id TEXT, status TEXT);
      CREATE TABLE agent_message_delivery (
        interaction_id TEXT, operation TEXT, requester_provider TEXT,
        requester_conversation_id TEXT, requester_generation INTEGER
      );
      CREATE TABLE provider_input_state (session_id TEXT PRIMARY KEY, state TEXT, generation INTEGER);
    `);
    configure(db);
  } finally { db.close(); }
}

test("report persistence preserves unowned homes and uses the actual acquired run claim", async (t) => {
  const home = await fixture(t);
  const reportPath = path.join(home, "automatic-inbound-reply-report.json");
  await fs.writeFile(reportPath, "operator evidence\n", { flag: "wx" });
  assert.equal(await writeOwnedReplyReport({ isolatedHome: home }, {}), false);
  const runId = "report-owner-" + path.basename(home);
  const lock = acquireHomeLock({ home, runId }).lock;
  try {
    assert.equal(await writeOwnedReplyReport({ isolatedHome: home, runId: "other-run" }, {}), false);
    assert.equal(await writeOwnedReplyReport({ isolatedHome: home, runId: "other-run", homeLock: lock }, {}), false);
    assert.equal(await fs.readFile(reportPath, "utf8"), "operator evidence\n");
    assert.deepEqual(readHomeLock(home), lock);
    const report = { status: "fail", cleanup: { shutdown_confirmed: false } };
    assert.equal(await writeOwnedReplyReport({ isolatedHome: home, runId, homeLock: lock }, report), true);
    assert.deepEqual(JSON.parse(await fs.readFile(reportPath, "utf8")), report);
  } finally { releaseHomeLock({ home, runId }); }
});

function row(db, { id = "task-1", kind = "task", sender = "requester", targets = ["worker"],
  parent = null, body = "gate instruction", status = "pending" } = {}) {
  db.prepare("INSERT INTO interactions VALUES (?,?,?,?,?,?,?)").run(
    id, kind, sender, JSON.stringify(targets), parent,
    JSON.stringify({ storage: "inline", body }), status,
  );
}

function task(db, { id = "task-1", sender = "requester", targets = ["worker"],
  provider = "antigravity", conversationId = "conversation-1", requesterGeneration = 7,
  providerInputGeneration = requesterGeneration } = {}) {
  row(db, { id, sender, targets });
  db.prepare("INSERT INTO agent_message_delivery VALUES (?,?,?,?,?)").run(
    id, "followup_task", provider, conversationId, requesterGeneration,
  );
  if (providerInputGeneration !== null) {
    db.prepare("INSERT INTO provider_input_state (session_id,state,generation) VALUES (?,?,?) " +
      "ON CONFLICT(session_id) DO UPDATE SET state=excluded.state,generation=excluded.generation")
      .run(sender, "ready", providerInputGeneration);
  }
}

function reply(db, overrides = {}) {
  row(db, { id: "reply-1", kind: "reply", sender: "worker", targets: ["requester"],
    parent: "task-1", body: "WORKER_ONLY_unpredictable", status: "completed", ...overrides });
  db.prepare("INSERT INTO structured_replies VALUES (?,?)").run("task-1", "done");
}

function lease(overrides = {}) {
  return {
    agent_id: "agent-1",
    provider: "antigravity",
    resume_session: "resume-1",
    owner_kind: "agent_lifecycle",
    owner_id: "lifecycle-1",
    acquisition_id: "opaque-acquisition-token",
    mode: "lifecycle_transition",
    started_at: "2026-10-24T19:00:00Z",
    heartbeat_at: "2026-10-24T19:00:01Z",
    expires_at: "2026-10-24T20:00:00Z",
    ...overrides,
  };
}

async function writeLeaseFile(home, document) {
  const runtime = path.join(home, "runtime");
  await fs.mkdir(runtime, { recursive: true });
  const leasePath = path.join(runtime, "conversation-leases.json");
  await fs.writeFile(leasePath, typeof document === "string" ? document : JSON.stringify(document));
  return leasePath;
}

const LEASE_CHECK_TIME = Date.parse("2026-10-24T19:30:00Z");

test("observer waits for missing database or unadmitted task without creating state", async (t) => {
  const home = await fixture(t);
  assert.equal(await readCanonicalTaskReply(home, "requester", "worker"), null);
  assert.deepEqual(await fs.readdir(home), []);
  database(home);
  assert.equal(await readCanonicalTaskReply(home, "requester", "worker"), null);
});

test("provider readiness observer distinguishes Booting, Ready, and missing state", async (t) => {
  const home = await fixture(t);
  database(home);
  assert.equal(await readProviderInputState(home, "requester"), null);
  assert.equal(providerInputIsReady(null, "requester"), false);

  const db = new DatabaseSync(path.join(home, "state.db"));
  try {
    db.prepare("INSERT INTO provider_input_state VALUES (?,?,?)").run("requester", "booting", 1);
  } finally { db.close(); }
  const beforeBootingRead = await fs.readFile(path.join(home, "state.db"));
  const booting = await readProviderInputState(home, "requester");
  assert.deepEqual({ ...booting }, { session_id: "requester", state: "booting", generation: 1 });
  assert.equal(providerInputIsReady(booting, "requester"), false);
  assert.deepEqual(await fs.readFile(path.join(home, "state.db")), beforeBootingRead);

  const readyDb = new DatabaseSync(path.join(home, "state.db"));
  try {
    readyDb.prepare("UPDATE provider_input_state SET state='ready' WHERE session_id='requester'").run();
  } finally { readyDb.close(); }
  const beforeReadyRead = await fs.readFile(path.join(home, "state.db"));
  const ready = await readProviderInputState(home, "requester");
  assert.equal(providerInputIsReady(ready, "requester"), true);
  assert.equal(providerInputIsReady({ ...ready, generation: 0 }, "requester"), false);
  assert.equal(providerInputIsReady(ready, "another-requester"), false);
  assert.deepEqual(await fs.readFile(path.join(home, "state.db")), beforeReadyRead);
});

test("lease predicate matches exact requester identity, optional session, and ordinary expiry", () => {
  assert.equal(activeConversationLeaseConflicts(
    [lease({ agent_id: "requester", resume_session: "" })], "requester", "", LEASE_CHECK_TIME,
  ).length, 1, "The same native agent conflicts even when it has no resume id");
  assert.equal(activeConversationLeaseConflicts(
    [lease({ agent_id: "another-agent", resume_session: "another-session" })],
    "requester", "", LEASE_CHECK_TIME,
  ).length, 0, "An unrelated agent and absent session do not collide");
  assert.equal(activeConversationLeaseConflicts(
    [lease({ agent_id: "another-agent", resume_session: "requester-session" })],
    "requester", "requester-session", LEASE_CHECK_TIME,
  ).length, 1, "A nonempty exact resume id conflicts across agent ids");
  assert.equal(activeConversationLeaseConflicts(
    [lease({ agent_id: "requester", expires_at: "2026-10-24T19:29:59Z" })],
    "requester", "", LEASE_CHECK_TIME,
  ).length, 0, "Expired ordinary rows do not block");
  assert.equal(activeConversationLeaseConflicts(
    [lease({ agent_id: "another-agent", resume_session: "requester-session",
      expires_at: "2026-10-24T19:29:59Z" })],
    "requester", "requester-session", LEASE_CHECK_TIME,
  ).length, 0, "Expired ordinary rows do not block by session either");
  assert.throws(() => activeConversationLeaseConflicts([], "", "", LEASE_CHECK_TIME), /nonempty requester id/u);
});

test("uncertain previous-provider holds are indefinite only for an exact nonempty session", () => {
  const hold = lease({
    agent_id: "another-agent",
    resume_session: "held-session",
    owner_kind: "prior_provider_hold",
    owner_id: "hold-1",
    mode: "uncertain_previous_provider",
    expires_at: "2020-01-01T00:00:00Z",
  });
  assert.equal(activeConversationLeaseConflicts([hold], "requester", "held-session", LEASE_CHECK_TIME).length, 1);
  assert.equal(activeConversationLeaseConflicts([hold], "requester", "different-session", LEASE_CHECK_TIME).length, 0);
  assert.equal(activeConversationLeaseConflicts([hold], "another-agent", "", LEASE_CHECK_TIME).length, 0,
    "An uncertain hold is not an agent-wide TTL lease");
});

test("lease snapshot reads are missing-as-empty, read-only, hashed, and owner-bounded", async (t) => {
  const home = await fixture(t);
  await assert.rejects(readConversationLeaseSnapshot(home, "", "", LEASE_CHECK_TIME), /nonempty requester id/u);
  const missing = await readConversationLeaseSnapshot(home, "requester", "", LEASE_CHECK_TIME);
  assert.equal(missing.exists, false);
  assert.equal(missing.has_active_conflict, false);
  assert.equal(missing.lease_count, 0);
  assert.equal(missing.snapshot_sha256,
    "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
  assert.deepEqual(await fs.readdir(home), [], "A missing lease file remains missing after observation");

  const leasePath = await writeLeaseFile(home, { schema: 1, leases: [lease({
    agent_id: "requester", resume_session: "", owner_id: "startup-owner",
  })] });
  const before = await fs.readFile(leasePath);
  const snapshot = await readConversationLeaseSnapshot(home, "requester", "", LEASE_CHECK_TIME);
  assert.equal(snapshot.has_active_conflict, true);
  assert.equal(snapshot.active_conflict_count, 1);
  assert.equal(snapshot.active_conflicts[0].owner_kind, "agent_lifecycle");
  assert.equal(snapshot.active_conflicts[0].owner_id, "startup-owner");
  assert.equal(snapshot.active_conflicts[0].mode, "lifecycle_transition");
  assert.equal(snapshot.active_conflicts[0].agent_id, "requester");
  assert.equal(snapshot.active_conflicts[0].provider, "antigravity");
  assert.equal(snapshot.active_conflicts[0].resume_session, "");
  assert.equal(snapshot.active_conflicts[0].expires_at, "2026-10-24T20:00:00Z");
  assert.equal("acquisition_id" in snapshot.active_conflicts[0], false);
  assert.equal(snapshot.snapshot_sha256.length, 64);
  assert.deepEqual(await fs.readFile(leasePath), before, "Observation must not alter the authoritative file");

  const evidence = { first_blocking: null, last_blocking: null, final_snapshot: null };
  recordConversationLeaseObservation(evidence, snapshot);
  const clear = await readConversationLeaseSnapshot(home, "other-agent", "", LEASE_CHECK_TIME);
  recordConversationLeaseObservation(evidence, clear);
  await writeLeaseFile(home, { schema: 1, leases: [lease({
    agent_id: "requester", resume_session: "", owner_id: "later-startup-owner",
  })] });
  const laterBlocking = await readConversationLeaseSnapshot(
    home, "requester", "", LEASE_CHECK_TIME + 1_000,
  );
  recordConversationLeaseObservation(evidence, laterBlocking);
  assert.equal(evidence.first_blocking.observed_at, snapshot.observed_at);
  assert.equal(evidence.last_blocking.snapshot_sha256, laterBlocking.snapshot_sha256);
  assert.equal(evidence.last_blocking.active_conflicts[0].owner_id, "later-startup-owner");
  assert.equal(evidence.final_snapshot.has_active_conflict, true);
  assert.equal("poll_history" in evidence, false);

  await writeLeaseFile(home, { schema: 1, leases: Array.from({ length: 6 }, (_, index) => lease({
    agent_id: "requester", resume_session: "", owner_id: `owner-${index}`,
  })) });
  const capped = await readConversationLeaseSnapshot(home, "requester", "", LEASE_CHECK_TIME);
  assert.equal(capped.active_conflict_count, 6);
  assert.equal(capped.active_conflicts.length, 4);
  assert.equal(capped.omitted_active_conflicts, 2);
});

test("lease observer fails closed on malformed JSON, unknown schema, and invalid lease dates", async (t) => {
  const cases = [
    "{ malformed",
    { schema: 2, leases: [] },
    { schema: 1, leases: [lease({ expires_at: "not-a-date" })] },
    { schema: 1, leases: [lease({ started_at: "2026-02-30T00:00:00Z" })] },
  ];
  for (const input of cases) {
    const home = await fixture(t);
    const leasePath = await writeLeaseFile(home, input);
    const before = await fs.readFile(leasePath);
    await assert.rejects(readConversationLeaseSnapshot(home, "requester", "", LEASE_CHECK_TIME), (error) => {
      assert.ok(error.lease_snapshot);
      assert.equal(error.lease_snapshot.valid, false);
      assert.equal(error.lease_snapshot.snapshot_sha256.length, 64);
      return true;
    });
    assert.deepEqual(await fs.readFile(leasePath), before, "Fail-closed parsing must remain read-only");
  }
});

test("core failure reporting preserves the first error and does not mask it with report I/O", async () => {
  const firstError = new Error("conversation_leased");
  const report = { cases: { delayed_reply: { status: "running", initial_prompt_attempts: 1 } } };
  const returnedError = await persistAutomaticReplyFailure(report, "delayed_reply", firstError, async () => {
    throw new Error("report disk unavailable");
  });
  assert.equal(returnedError, firstError);
  assert.equal(report.cases.delayed_reply.status, "fail");
  assert.equal(report.cases.delayed_reply.error, "conversation_leased");
  assert.equal(report.error, "conversation_leased");
  assert.equal(report.report_persistence_error, "report disk unavailable");
});

test("initial prompt delivery accepts only an accepted receipt for the exact requester", () => {
  assert.doesNotThrow(() => assertPromptDeliveryAccepted(
    { uuid: "requester", provider: "antigravity", delivery_state: "provider_accepted" },
    "requester", "antigravity",
  ));
  assert.doesNotThrow(() => assertPromptDeliveryAccepted(
    { uuid: "requester", provider: "antigravity", delivery_state: "queued" },
    "requester", "antigravity",
  ));
  for (const receipt of [
    { uuid: "requester", provider: "antigravity", delivery_state: "provider_input_not_ready" },
    { uuid: "requester", provider: "antigravity", delivery_state: "failed", error: { code: "provider_input_not_ready" } },
    { uuid: "other", provider: "antigravity", delivery_state: "provider_accepted" },
    { uuid: "requester", provider: "codex", delivery_state: "provider_accepted" },
  ]) assert.throws(() => assertPromptDeliveryAccepted(receipt, "requester", "antigravity"), assert.AssertionError);
});

test("observer binds pending and completed replies to one exact request without writing", async (t) => {
  const home = await fixture(t);
  database(home, (db) => task(db));
  const pending = await readCanonicalTaskReply(home, "requester", "worker", "task-1");
  assert.equal(pending.reply_count, 0);
  assert.equal(pending.task_status, "pending");
  assertRequesterOriginMatchesProviderInput(pending, "antigravity", "conversation-1");
  assert.throws(() => assertRequesterOriginMatchesProviderInput(pending, "codex", "conversation-1"),
    /provider changed/u);
  assert.throws(() => assertRequesterOriginMatchesProviderInput(pending, "antigravity", "other-conversation"),
    /captured provider conversation/u);
  const db = new DatabaseSync(path.join(home, "state.db"));
  try {
    db.exec("UPDATE interactions SET status='completed' WHERE id='task-1'");
    reply(db);
  } finally { db.close(); }
  const before = await fs.readFile(path.join(home, "state.db"));
  const proof = await readCanonicalTaskReply(home, "requester", "worker", "task-1");
  assert.equal(proof.reply_count, 1);
  assert.equal(proof.reply.status, "done");
  assert.equal(proof.reply.sender, "worker");
  assert.equal(proof.reply.parent_interaction_id, "task-1");
  assert.equal(proof.requester_reply_count, 0);
  assert.equal(proof.reply_to_reply_count, 0);
  assert.deepEqual(await fs.readFile(path.join(home, "state.db")), before);
  assert.deepEqual(await fs.readdir(home), ["state.db"]);
  await assert.rejects(readCanonicalTaskReply(home, "requester", "worker", "other-task"), /owned request changed/u);
});

test("observer rejects wrong recipients, extra tasks, duplicate replies, and spoofed reply origin", async (t) => {
  const cases = [
    (db) => task(db, { targets: ["foreign-worker"] }),
    (db) => { task(db); task(db, { id: "task-2" }); },
    (db) => { task(db); reply(db); reply(db, { id: "reply-2" }); },
    (db) => { task(db); reply(db, { sender: "foreign-worker" }); },
    (db) => { task(db); reply(db, { targets: ["foreign-requester"] }); },
    (db) => { task(db); reply(db); db.exec("DELETE FROM structured_replies"); },
  ];
  for (const configure of cases) {
    const home = await fixture(t);
    database(home, configure);
    await assert.rejects(readCanonicalTaskReply(home, "requester", "worker"), assert.AssertionError);
  }
});

test("observer exposes requester obligations and reply-to-reply loops to the acceptance oracle", async (t) => {
  const home = await fixture(t);
  database(home, (db) => {
    task(db); reply(db);
    row(db, { id: "loop", kind: "reply", sender: "requester", targets: ["worker"], parent: "reply-1" });
  });
  const proof = await readCanonicalTaskReply(home, "requester", "worker");
  assert.equal(proof.requester_reply_count, 1);
  assert.equal(proof.reply_to_reply_count, 1);
});

test("observer fails closed for missing requester origin and checks provider-input generation", async (t) => {
  const missingHome = await fixture(t);
  database(missingHome, (db) => row(db));
  await assert.rejects(readCanonicalTaskReply(missingHome, "requester", "worker"), /requester-origin row/u);

  const readyBeforePrompt = { session_id: "requester", state: "ready", generation: 7 };
  assert.equal(providerInputIsReady(readyBeforePrompt, "requester"), true);
  const advancedHome = await fixture(t);
  database(advancedHome, (db) => task(db, { requesterGeneration: 8, providerInputGeneration: 8 }));
  const advanced = await readCanonicalTaskReply(advancedHome, "requester", "worker");
  assertRequesterOriginMatchesProviderInput(advanced, "antigravity", "conversation-1");

  const staleHome = await fixture(t);
  database(staleHome, (db) => task(db, { requesterGeneration: 6, providerInputGeneration: 7 }));
  const stale = await readCanonicalTaskReply(staleHome, "requester", "worker");
  assert.throws(() => assertRequesterOriginMatchesProviderInput(stale, "antigravity", "conversation-1"),
    /match the current persisted provider-input generation/u);

  const missingGenerationHome = await fixture(t);
  database(missingGenerationHome, (db) => task(db, { providerInputGeneration: null }));
  const missingGeneration = await readCanonicalTaskReply(missingGenerationHome, "requester", "worker");
  assert.throws(() => assertRequesterOriginMatchesProviderInput(
    missingGeneration, "antigravity", "conversation-1",
  ), /current provider-input generation/u);
});

test("deadlines preserve the original absolute limit and require a positive cleanup reserve", () => {
  assert.deepEqual(automaticReplyDeadlines("2030-01-01T00:00:10Z", 2_000, Date.parse("2030-01-01T00:00:00Z")), {
    deadlineAt: Date.parse("2030-01-01T00:00:10Z"),
    workDeadlineAt: Date.parse("2030-01-01T00:00:08Z"), cleanupReserveMs: 2_000,
  });
  for (const [date, reserve, now] of [
    [undefined, 1, 0], ["2030-01-01T00:00:10", 1, 0], ["invalidZ", 1, 0],
    ["2030-01-01T00:00:10Z", 0, 0], ["2030-01-01T00:00:10Z", 1.5, 0],
    ["2030-01-01T00:00:10Z", 2_000, Date.parse("2030-01-01T00:00:08Z")],
  ]) assert.throws(() => automaticReplyDeadlines(date, reserve, now), assert.AssertionError);
});

test("native evidence requires exact direct workspace ownership and both completion signals", () => {
  const cascade = { id: "owned", ownership_basis: "workspace_uri", records: [] };
  assert.equal(exactCurrentCascade({ provider: "antigravity", sessions: [cascade] }, "owned"), cascade);
  for (const snapshot of [
    { provider: "codex", sessions: [cascade] },
    { provider: "antigravity", sessions: [cascade, cascade] },
    { provider: "antigravity", sessions: [{ ...cascade, id: "foreign" }] },
    { provider: "antigravity", sessions: [{ ...cascade, ownership_basis: "workspace_alias" }] },
  ]) assert.throws(() => exactCurrentCascade(snapshot, "owned"), assert.AssertionError);
  const complete = { role: "assistant", complete: true, native_status: 3 };
  assert.deepEqual(completedAssistantRows([
    complete, { ...complete, complete: false }, { ...complete, native_status: 2 }, { ...complete, role: "user" },
  ]), [complete]);
});

test("worker gate discloses one unpredictable token only after operator release", async (t) => {
  const home = await fixture(t);
  const readyPath = path.join(home, "ready");
  const releasePath = path.join(home, "release");
  let settled = false;
  const pending = waitForReplyRelease({ readyPath, releasePath, deadlineAt: Date.now() + 3_000, pollMs: 5 });
  pending.then(() => { settled = true; }, () => { settled = true; });
  const readyDeadline = Date.now() + 2_000;
  while (Date.now() < readyDeadline) {
    try { await fs.access(readyPath); break; }
    catch (error) { if (error.code !== "ENOENT") throw error; }
    await new Promise((resolve) => setTimeout(resolve, 5));
  }
  assert.equal(await fs.readFile(readyPath, "utf8"), "ready\n");
  assert.equal(settled, false);
  await assert.rejects(waitForReplyRelease({ readyPath, releasePath, deadlineAt: Date.now() + 1_000 }), /EEXIST/u);
  await fs.writeFile(releasePath, "release\n", { flag: "wx" });
  assert.match(await pending, /^WORKER_ONLY_[0-9a-f-]{36}$/u);
  assert.equal(await fs.readFile(readyPath + ".consumed", "utf8"), "consumed\n");
  await assert.rejects(waitForReplyRelease({ readyPath, releasePath, deadlineAt: Date.now() + 1_000 }), /released before/u);
});

test("worker gate fails at the original deadline without release or token", async (t) => {
  const home = await fixture(t);
  const readyPath = path.join(home, "ready");
  const releasePath = path.join(home, "release");
  await assert.rejects(waitForReplyRelease({ readyPath, releasePath, deadlineAt: Date.now() + 30, pollMs: 5 }), /deadline expired/u);
  await assert.rejects(fs.access(readyPath + ".consumed"), /ENOENT/u);
  await assert.rejects(waitForReplyRelease({ readyPath, releasePath, deadlineAt: Date.now() - 1 }), /deadline expired/u);
});
