// @tier manual — Requires explicit real-provider authorization and the native paired-artifact runner.
import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
import { createHash, randomUUID } from "node:crypto";
import { fileURLToPath } from "node:url";
import {
  createNativeHarness,
  prepareIsolatedHome,
  startNativeSession,
  waitForAppShell,
  invokeTauri,
} from "../lib/harness.mjs";
import { createHeadlessEvidenceReader } from "../lib/provider-headless-evidence.mjs";
import { messageCli } from "../lib/canonical-messaging.mjs";
import {
  appendOwnedReplyToolApproval,
  automaticReplyExecShell,
  automaticReplyTaskInstructions,
  assertRequesterOriginMatchesProviderInput,
  automaticReplyDeadlines,
  assertPromptDeliveryAccepted,
  completedAssistantRows,
  exactCurrentCascade,
  providerInputIsReady,
  readConversationLeaseSnapshot,
  readProviderInputState,
  readCanonicalTaskReply,
  recordConversationLeaseObservation,
  persistAutomaticReplyFailure,
  writeOwnedReplyReport,
} from "../lib/automatic-reply-evidence.mjs";
import { cleanupConformanceSession, pauseConformanceWork } from "../lib/conformance-cleanup.mjs";

const REQUESTER_MODEL = "gemini-3.8-flash-low";
const WORKER_MODEL = "gpt-6-luna";
const WORKER_EFFORT = "low";
const acceptanceEnabled = process.env.WARDIAN_E2E_REAL_AUTOMATIC_REPLY === "1";
const deadlines = acceptanceEnabled ? automaticReplyDeadlines(
  process.env.WARDIAN_E2E_AUTOMATIC_REPLY_DEADLINE_UTC,
  Number(process.env.WARDIAN_E2E_AUTOMATIC_REPLY_CLEANUP_MS),
) : null;
const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

async function exists(file) {
  try {
    await fs.access(file);
    return true;
  } catch (error) {
    if (error.code === "ENOENT") return false;
    throw error;
  }
}

async function pollUntil(read, deadline, message) {
  do {
    const value = await read();
    if (value) return value;
    if (Date.now() < deadline) await delay(200);
  } while (Date.now() < deadline);
  throw new Error(message);
}

async function requesterConfigAndStatus(driver, agentId, expectedSession = null) {
  const agents = await invokeTauri(driver, "list_agents");
  const config = agents.find((row) => row.session_id === agentId);
  assert.ok(config, "The test-owned requester must remain present");
  assert.equal(config.provider, "antigravity");
  assert.equal(config.model, REQUESTER_MODEL);
  if (expectedSession) assert.equal(config.resume_session, expectedSession, "The requester changed native conversation");
  const metrics = await invokeTauri(driver, "list_agent_metrics");
  const metric = metrics.find((row) => row.session_id === agentId);
  assert.ok(metric, "The test-owned requester must have observable metrics");
  return { config, metric };
}

async function sha256(file) {
  return createHash("sha256").update(await fs.readFile(file)).digest("hex");
}

async function verifySystemPowerShell(shell) {
  const metadata = await fs.lstat(shell);
  assert.equal(metadata.isFile(), true, "The explicit Windows shell must be a regular executable file");
  assert.equal(metadata.isSymbolicLink(), false, "The explicit Windows shell must not be a link");
  const realPath = await fs.realpath(shell);
  assert.equal(path.win32.resolve(realPath).toLowerCase(), path.win32.resolve(shell).toLowerCase(),
    "The explicit Windows shell must resolve to the selected System32 path");
  assert.ok(!/\\WindowsApps(?:\\|$)/iu.test(realPath), "The fixture must not select the WindowsApps alias");
  return sha256(realPath);
}

async function assertWorkerRemainsOff(driver, worker) {
  assert.equal(worker.is_off, true, "The newly bootstrapped Codex fixture worker must remain Off");
  const agents = await invokeTauri(driver, "list_agents");
  const current = agents.find((entry) => entry.session_id === worker.session_id);
  assert.ok(current, "The fixture worker must remain registered under its original identity");
  assert.equal(current.provider, "codex");
  assert.equal(current.is_off, true, "Fixture preparation must not wake the Codex worker");
  assert.equal(current.provider_config.sandbox_mode, "workspace-write");
  assert.equal(current.provider_config.approval_policy, "never");
}

test("real Antigravity requester receives one delayed Codex reply in its original conversation", {
  timeout: deadlines ? Math.max(1, deadlines.deadlineAt - Date.now()) : 1_000,
}, async (t) => {
  if (!acceptanceEnabled) {
    t.skip("Requires the explicit issue-1691 real-provider acceptance grant");
    return;
  }
  const workDeadline = () => {
    assert.ok(deadlines.workDeadlineAt > Date.now(), "The original work deadline expired; preserve cleanup reserve");
    return deadlines.workDeadlineAt;
  };

  assert.equal(process.env.WARDIAN_NATIVE_SKIP_BUILD, "1", "The acceptance case never builds");
  assert.equal(process.env.WARDIAN_NATIVE_SESSION_START_ATTEMPTS, "1", "Uncertain startup must not be retried");
  for (const name of ["WARDIAN_NATIVE_APP", "WARDIAN_NATIVE_CLI", "CODEX_HOME",
    "WARDIAN_E2E_CONTEXT_ANTIGRAVITY_HOME"]) {
    assert.ok(path.isAbsolute(process.env[name] ?? ""), name + " must be an explicit absolute input");
  }

  const harness = await createNativeHarness();
  harness.watchMode = false;
  assert.equal(harness.pairedCli, true, "The existing native runner must supply the matching CLI");
  assert.equal(path.resolve(harness.appPath), path.resolve(process.env.WARDIAN_NATIVE_APP));
  assert.equal(path.resolve(harness.sharedCliPath), path.resolve(process.env.WARDIAN_NATIVE_CLI));
  const home = harness.isolatedHome;
  const workerWorkspace = path.join(home, "workspaces", "automatic-reply-worker");
  const requesterWorkspace = path.join(home, "workspaces", "automatic-reply-requester");
  const controlDirectory = path.join(home, "control");
  const reportPath = path.join(home, "automatic-inbound-reply-report.json");
  const report = {
    schema: 1,
    issue: 1691,
    status: "running",
    run_id: harness.runId,
    started_at: new Date().toISOString(),
    deadlines,
    artifacts: {},
    requested_models: { antigravity: REQUESTER_MODEL, codex: WORKER_MODEL, codex_effort: WORKER_EFFORT },
    cases: { delayed_reply: { status: "not_run" }, new_peer_fyi: { status: "not_run" } },
    agents: [],
    cleanup: null,
  };
  const save = () => writeOwnedReplyReport(harness, report);
  let session;
  let requester;
  let worker;
  let initialOwner;
  let coreFailure;
  let startupAttempted = false;
  let requesterSpawnAttempted = false;
  let workerSpawnAttempted = false;

  try {
    prepareIsolatedHome(harness);
    assert.equal(harness.homeLock?.runId, harness.runId);
    assert.equal(path.resolve(harness.frozenArtifacts.app.source), path.resolve(process.env.WARDIAN_NATIVE_APP));
    assert.equal(path.resolve(harness.frozenArtifacts.cli.source), path.resolve(process.env.WARDIAN_NATIVE_CLI));
    await fs.mkdir(workerWorkspace, { recursive: true });
    await fs.mkdir(requesterWorkspace, { recursive: true });
    await fs.mkdir(controlDirectory, { recursive: true });
    const packagedCli = path.join(path.dirname(harness.appPath), "resources", "bin", path.basename(harness.cliPath));
    const [frozenCliHash, packagedCliHash] = await Promise.all([
      sha256(harness.cliPath), sha256(packagedCli),
    ]);
    assert.equal(packagedCliHash, frozenCliHash, "The frozen app must carry the same frozen CLI");
    report.artifacts = {
      app: harness.frozenArtifacts.app,
      cli: harness.frozenArtifacts.cli,
      packaged_cli_sha256: packagedCliHash,
    };
    await save();
    workDeadline();
    startupAttempted = true;
    session = await startNativeSession(harness);
    await session.driver.manage().setTimeouts({ script: Math.max(1, workDeadline() - Date.now()) });
    await waitForAppShell(session.driver, Math.max(1, workDeadline() - Date.now()));

    requesterSpawnAttempted = true;
    requester = await invokeTauri(session.driver, "spawn_agent", { req: {
      sessionName: "AutomaticReplyRequester-" + randomUUID().slice(0, 8),
      agentClass: "TestClass",
      folder: requesterWorkspace,
      isOff: false,
      resumeSession: null,
      configOverride: {
        provider: "antigravity",
        model: REQUESTER_MODEL,
        session_persistence: "resume",
      },
    } });
    report.agents.push({ role: "requester", session_id: requester.session_id, workspace: requesterWorkspace });
    assert.equal(requester.provider, "antigravity");
    assert.equal(requester.model, REQUESTER_MODEL);
    await save();

    workDeadline();
    workerSpawnAttempted = true;
    worker = await invokeTauri(session.driver, "spawn_agent", { req: {
      sessionName: "AutomaticReplyWorker-" + randomUUID().slice(0, 8),
      agentClass: "TestClass",
      folder: workerWorkspace,
      isOff: true,
      resumeSession: null,
      configOverride: {
        provider: "codex",
        model: WORKER_MODEL,
        codex_skip_git_repo_check: true,
        session_persistence: "resume",
        provider_config: {
          type: "codex",
          reasoning_effort: WORKER_EFFORT,
          sandbox_mode: "workspace-write",
          approval_policy: "never",
        },
      },
    } });
    report.agents.push({ role: "worker", session_id: worker.session_id, workspace: workerWorkspace });
    assert.equal(worker.provider, "codex");
    assert.equal(worker.model, WORKER_MODEL);
    assert.equal(worker.provider_config.reasoning_effort, WORKER_EFFORT);
    assert.equal(worker.provider_config.sandbox_mode, "workspace-write");
    assert.equal(worker.provider_config.approval_policy, "never");
    await assertWorkerRemainsOff(session.driver, worker);
    const execShell = automaticReplyExecShell();
    if (execShell !== null) {
      report.artifacts.windows_exec_shell_sha256 = await verifySystemPowerShell(execShell);
    }
    const registeredMcpCommand = path.join(home, "bin",
      process.platform === "win32" ? "wardian-mcp.exe" : "wardian-cli");
    report.worker_messaging_fixture = await appendOwnedReplyToolApproval({
      wardianHome: home,
      agentId: worker.session_id,
      expectedCommand: registeredMcpCommand,
    });
    await assertWorkerRemainsOff(session.driver, worker);
    await save();

    const nativeStore = process.env.WARDIAN_E2E_CONTEXT_ANTIGRAVITY_HOME;
    const reader = await createHeadlessEvidenceReader({
      provider: "antigravity",
      isolatedHome: home,
      workspace: requesterWorkspace,
      env: process.env,
    });
    let coreProof;
    let preReleaseRecords;
    let initialPrompt;
    let initialMarker;
    let leaseEvidence;
    try {
      await t.test("delayed reply resumes the original Antigravity cascade", async () => {
        try {
          const gateDirectory = path.join(workerWorkspace, ".wardian-e2e-gate");
          const gatePath = path.join(gateDirectory, "reply-gate.mjs");
          const readyPath = path.join(gateDirectory, "ready");
          const releasePath = path.join(controlDirectory, "reply-release");
          assert.equal(path.relative(workerWorkspace, releasePath).startsWith(".."), true,
            "The Codex workspace sandbox must not own the operator release file");
          const gateFixture = fileURLToPath(new URL("./fixtures/automatic-reply-gate.mjs", import.meta.url));
          await fs.mkdir(gateDirectory, { recursive: true });
          await fs.copyFile(gateFixture, gatePath);
          assert.equal(await exists(releasePath), false, "The release file must start absent");

          initialMarker = "AGY_INITIAL_DONE_" + randomUUID();
          const gateCommand = "node .wardian-e2e-gate/reply-gate.mjs --deadline " +
            JSON.stringify(new Date(workDeadline()).toISOString());
          const taskInstructions = automaticReplyTaskInstructions(
            gateCommand, execShell,
          );
          initialPrompt = [
            "Use followup_task exactly once for Codex agent " + worker.session_id + " with this task:",
            taskInstructions,
            "After admitting it, finish this initial turn now with exactly " + initialMarker + ". Do not receive, poll, wait for, or answer the task.",
            "When the canonical peer reply automatically continues this same conversation later, finish with exactly the returned WORKER_ONLY_ token.",
          ].join("\n");
          leaseEvidence = {
            first_blocking: null,
            last_blocking: null,
            pre_admission: null,
            on_rejection: null,
            final_snapshot: null,
          };
          report.cases.delayed_reply = {
            status: "running", initial_prompt_attempts: 0, lease_evidence: leaseEvidence,
          };
          await save();

          const admissionPreflight = await pollUntil(async () => {
            const state = await readProviderInputState(home, requester.session_id);
            const { config } = await requesterConfigAndStatus(session.driver, requester.session_id);
            const resumeSession = config.resume_session ?? "";
            assert.equal(typeof resumeSession, "string", "The current requester resume session must be a string when present");
            let leaseSnapshot;
            try {
              leaseSnapshot = await readConversationLeaseSnapshot(
                home, requester.session_id, resumeSession,
              );
            } catch (error) {
              if (error.lease_snapshot) recordConversationLeaseObservation(leaseEvidence, error.lease_snapshot);
              throw error;
            }
            const blocked = recordConversationLeaseObservation(leaseEvidence, leaseSnapshot);
            if (!providerInputIsReady(state, requester.session_id) || blocked) return null;
            leaseEvidence.pre_admission = leaseSnapshot;
            return { resumeSession };
          }, workDeadline(),
          "Antigravity requester did not reach positive provider-input Ready with no active conflicting lease before the sole initial prompt");

          report.cases.delayed_reply.initial_prompt_attempts = 1;
          let initialDelivery;
          try {
            workDeadline();
            initialDelivery = await invokeTauri(session.driver, "submit_prompt_to_agent", {
              sessionId: requester.session_id,
              prompt: initialPrompt,
              inputMode: "message",
            });
            assertPromptDeliveryAccepted(initialDelivery, requester.session_id, "antigravity");
          } catch (error) {
            try {
              const rejectedSnapshot = await readConversationLeaseSnapshot(
                home, requester.session_id, admissionPreflight.resumeSession,
              );
              recordConversationLeaseObservation(leaseEvidence, rejectedSnapshot);
              leaseEvidence.on_rejection = rejectedSnapshot;
            } catch (observationError) {
              if (observationError.lease_snapshot) {
                recordConversationLeaseObservation(leaseEvidence, observationError.lease_snapshot);
                leaseEvidence.on_rejection = observationError.lease_snapshot;
              }
              leaseEvidence.rejection_observation_error = observationError instanceof Error
                ? observationError.message : String(observationError);
            }
            throw error;
          }

          initialOwner = await pollUntil(async () => {
            const { config, metric } = await requesterConfigAndStatus(session.driver, requester.session_id);
            if (!config.resume_session || metric.current_status?.toLowerCase() !== "idle") return null;
            const snapshot = await reader.snapshot({
              agentId: requester.session_id,
              originalSession: config.resume_session,
              mode: "current",
            });
            return { provider_session: config.resume_session, cascade: exactCurrentCascade(snapshot, config.resume_session) };
          }, workDeadline(),
          "Antigravity requester did not expose one directly workspace-owned idle cascade after its initial prompt");
          report.antigravity_evidence_store = nativeStore;
          report.requester_provider_session = initialOwner.provider_session;
          report.initial_cascade = {
            id: initialOwner.cascade.id,
            ownership_basis: initialOwner.cascade.ownership_basis,
            source: initialOwner.cascade.source,
            source_locator_sha256: initialOwner.cascade.source_locator_sha256,
          };
          await save();

          const held = await pollUntil(async () => {
            const proof = await readCanonicalTaskReply(home, requester.session_id, worker.session_id);
            if (!proof) return null;
            assertRequesterOriginMatchesProviderInput(
              proof, "antigravity", initialOwner.provider_session,
            );
            assert.equal(proof.reply_count, 0, "The worker reply arrived before the explicit release");
            const { config, metric } = await requesterConfigAndStatus(session.driver, requester.session_id, initialOwner.provider_session);
            const snapshot = await reader.snapshot({
              agentId: requester.session_id,
              originalSession: initialOwner.provider_session,
              mode: "current",
            });
            const cascade = exactCurrentCascade(snapshot, initialOwner.cascade.id);
            const userPrompts = cascade.records.filter((row) => row.role === "user" && row.text === initialPrompt);
            assert.ok(userPrompts.length <= 1, "The original requester prompt must not be duplicated");
            const initialFinals = completedAssistantRows(cascade.records).filter((row) => row.text.trim() === initialMarker);
            assert.ok(initialFinals.length <= 1, "The initial assistant completion must be unique");
            const gateReady = await exists(readyPath);
            if (userPrompts.length === 1 && initialFinals.length === 1 &&
              metric.current_status?.toLowerCase() === "idle" && gateReady) {
              assert.ok(initialFinals[0].ordinal > userPrompts[0].ordinal,
                "The native assistant completion must follow the exact initial requester prompt");
              assert.equal(config.resume_session, initialOwner.provider_session);
              assert.ok(proof.task_body.includes(gateCommand), "The admitted task must carry the worker gate instruction");
              preReleaseRecords = cascade.records;
              return { proof, cascade, initialFinal: initialFinals[0] };
            }
            return null;
          }, workDeadline(),
          "No single admitted task reached the worker gate before the requester turn ended");

          assert.equal(await exists(releasePath), false, "The operator gate must remain closed until requester completion and Idle");
          coreProof = held.proof;
          report.cases.delayed_reply = {
            status: "running",
            initial_prompt_attempts: 1,
            lease_evidence: leaseEvidence,
            request_id: coreProof.request_id,
            task_status_before_release: coreProof.task_status,
            task_body: coreProof.task_body,
            requester_initial_turn_id: held.initialFinal.id,
            requester_cascade_id: held.cascade.id,
            release_attempts: 1,
          };
          await save();
          await fs.writeFile(releasePath, "release\n", { flag: "wx" });

          const completed = await pollUntil(async () => {
            const proof = await readCanonicalTaskReply(home, requester.session_id, worker.session_id, coreProof.request_id);
            if (!proof?.reply) return null;
            if (proof.task_status !== "completed" || proof.reply.status !== "done") return null;
            assert.equal(proof.task_status, "completed");
            assert.equal(proof.reply.status, "done");
            assert.equal(proof.reply.request_id, proof.request_id);
            assert.equal(proof.reply.parent_interaction_id, proof.request_id);
            assert.equal(proof.reply.sender, worker.session_id);
            assert.deepEqual(proof.reply.recipients, [requester.session_id]);
            assert.equal(proof.reply_count, 1);
            assert.equal(proof.requester_reply_count, 0);
            assert.equal(proof.reply_to_reply_count, 0);
            assert.match(proof.reply.body, /^WORKER_ONLY_[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/iu);
            assert.ok(!initialPrompt.includes(proof.reply.body), "The unpredictable worker token must be absent from the requester prompt");
            assert.ok(!proof.task_body.includes(proof.reply.body), "The requester-created task must not contain the worker token");
            assert.ok(!preReleaseRecords.some((row) => row.text?.includes(proof.reply.body)),
              "The worker token must be absent from all requester-native records before release");

            const { config, metric } = await requesterConfigAndStatus(session.driver, requester.session_id, initialOwner.provider_session);
            const snapshot = await reader.snapshot({
              agentId: requester.session_id,
              originalSession: initialOwner.provider_session,
              mode: "current",
            });
            const cascade = exactCurrentCascade(snapshot, initialOwner.cascade.id);
            const continuations = completedAssistantRows(cascade.records).filter((row) =>
              row.ordinal > held.initialFinal.ordinal && row.text.trim() === proof.reply.body);
            if (continuations.length > 1) assert.equal(continuations.length, 1, "Duplicate automatic assistant continuations");
            if (continuations.length === 1 && metric.current_status?.toLowerCase() === "idle" &&
              config.resume_session === initialOwner.provider_session) {
              return { proof, cascade, continuation: continuations[0] };
            }
            return null;
          }, workDeadline(),
          "The original Antigravity cascade did not complete an assistant-authored reply continuation");

          coreProof = completed.proof;
          report.cases.delayed_reply = {
            status: "pass",
            initial_prompt_attempts: 1,
            lease_evidence: leaseEvidence,
            request_id: completed.proof.request_id,
            task_status: completed.proof.task_status,
            task_body: completed.proof.task_body,
            reply: completed.proof.reply,
            requester_cascade_id: completed.cascade.id,
            requester_initial_turn_id: held.initialFinal.id,
            continuation_turn_id: completed.continuation.id,
            continuation_text: completed.continuation.text,
            submission_attempts: 1,
            release_attempts: 1,
            reply_count: completed.proof.reply_count,
            reply_to_reply_count: completed.proof.reply_to_reply_count,
          };
          await save();
        } catch (error) {
          coreFailure = error;
          throw error;
        }
      });
    } catch (error) {
      coreFailure ??= error;
    }
    if (coreFailure) {
      report.cases.new_peer_fyi = {
        status: "not_run", reason: "Delayed-reply acceptance failed before the FYI case began",
      };
      report.status = "fail";
      await persistAutomaticReplyFailure(report, "delayed_reply", coreFailure, save);
      return;
    }

    await t.test("new peer FYI wakes idle Antigravity without creating reply work", async () => {
      const beforeSnapshot = await reader.snapshot({
        agentId: requester.session_id,
        originalSession: initialOwner.provider_session,
        mode: "current",
      });
      const beforeCascade = exactCurrentCascade(beforeSnapshot, initialOwner.cascade.id);
      const beforeIds = new Set(beforeCascade.records.map((row) => row.id));
      const fyiBody = "Reference only. No task or response is required. FYI_" + randomUUID();
      report.cases.new_peer_fyi = { status: "running", submission_attempts: 1 };
      await save();
      workDeadline();
      const receipt = await messageCli(harness.cliPath, home, harness.repoRoot, worker.session_id,
        ["send", requester.session_id, fyiBody]);
      assert.equal(receipt.operation, "send_message");
      assert.ok(receipt.interaction_id);
      const awakened = await pollUntil(async () => {
        const { config, metric } = await requesterConfigAndStatus(session.driver, requester.session_id, initialOwner.provider_session);
        const snapshot = await reader.snapshot({
          agentId: requester.session_id,
          originalSession: initialOwner.provider_session,
          mode: "current",
        });
        const cascade = exactCurrentCascade(snapshot, initialOwner.cascade.id);
        const fyiInputs = cascade.records.filter((row) => row.role === "user" &&
          !beforeIds.has(row.id) && row.text.includes(fyiBody));
        const assistantTurns = completedAssistantRows(cascade.records).filter((row) =>
          fyiInputs.some((input) => row.ordinal > input.ordinal));
        if (fyiInputs.length > 1) assert.fail("The peer FYI was duplicated in the requester conversation");
        if (fyiInputs.length === 1 && assistantTurns.length > 0 && metric.current_status?.toLowerCase() === "idle") {
          assert.equal(config.resume_session, initialOwner.provider_session);
          return { cascade, fyiInput: fyiInputs[0], assistantTurns };
        }
        return null;
      }, workDeadline(),
      "The new peer FYI did not wake and complete a turn in the original Antigravity cascade");

      const afterProof = await readCanonicalTaskReply(home, requester.session_id, worker.session_id, coreProof.request_id);
      assert.ok(afterProof, "The completed canonical task must remain observable");
      assert.equal(afterProof.request_id, coreProof.request_id);
      assert.equal(afterProof.reply_count, 1);
      assert.equal(afterProof.requester_reply_count, 0, "An information message must not create a reply obligation");
      assert.equal(afterProof.reply_to_reply_count, 0, "A reply continuation must not create a reply loop");
      report.cases.new_peer_fyi = {
        status: "pass",
        interaction_id: receipt.interaction_id,
        fyi_input_id: awakened.fyiInput.id,
        requester_cascade_id: awakened.cascade.id,
        assistant_turn_ids: awakened.assistantTurns.map((row) => row.id),
        no_new_task_or_reply_obligation: true,
        no_reply_loop: true,
      };
      await save();
    });

    report.status = report.cases.delayed_reply.status === "pass" &&
      report.cases.new_peer_fyi.status === "pass" ? "pass" : "fail";
  } catch (error) {
    report.status = "fail";
    if (report.cases.delayed_reply.status === "running") report.cases.delayed_reply.status = "fail";
    if (report.cases.new_peer_fyi.status === "running") report.cases.new_peer_fyi.status = "fail";
    report.error = error.message;
    throw error;
  } finally {
    await cleanupConformanceSession({
      harness, session, startupAttempted,
      pause: async () => {
        const errors = [];
        for (const [spawnAttempted, agent] of [
          [requesterSpawnAttempted, requester], [workerSpawnAttempted, worker],
        ]) {
          try {
            await pauseConformanceWork((command, args) => invokeTauri(session.driver, command, args), {
              spawnAttempted, sessionId: agent?.session_id,
            });
          } catch (error) { errors.push(error); }
        }
        if (errors.length) throw new AggregateError(errors, "Owned spawn or pause remains uncertain");
      },
      save: async (cleanup) => {
        report.cleanup = cleanup;
        if (!cleanup.shutdown_confirmed) report.status = "fail";
        report.finished_at = new Date().toISOString();
        await save();
        t.diagnostic("Automatic inbound reply status=" + report.status + "; evidence=" + reportPath);
      },
    });
  }
});
