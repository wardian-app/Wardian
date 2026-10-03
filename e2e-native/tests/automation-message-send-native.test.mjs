// @tier nightly — Real host automation admission and canonical inbox, without provider turns.
import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { createHash } from "node:crypto";
import { fileURLToPath } from "node:url";
import { By, until } from "selenium-webdriver";

import {
  createNativeHarness,
  ensureNativeAppBuilt,
  prepareIsolatedHome,
  startNativeSession,
  waitForAppShell,
  invokeTauri,
} from "../lib/harness.mjs";
import { messageCli } from "../lib/canonical-messaging.mjs";

const body = '\uFEFFSynthetic transport fixture: café 日本語 🦀\r\n'
  + '\'apostrophes\' "quotes" `backticks` $(literal) $HOME & | ; < > %PATH%\r\n'
  + '{{run.id}} must remain literal body data.\nVerdict: revise\r\n';
const bytes = Buffer.from(body, "utf8");
const sha256 = (value) => createHash("sha256").update(value).digest("hex");

/** Every case has a private workspace and exactly one preflight-reserved run directory. */
function seedFixture(harness, caseId, { missing = false } = {}) {
  const workspace = path.join(harness.isolatedHome, "workspaces", caseId);
  const reviewRoot = path.join(workspace, ".wardian-review");
  fs.mkdirSync(reviewRoot, { recursive: true });
  // An older workspace artifact must never substitute for this run's artifact.
  fs.writeFileSync(path.join(reviewRoot, "review.md"), "STALE: must not be delivered\n");
  const scriptPath = path.join(workspace, "synthetic-artifact.cjs");
  fs.writeFileSync(scriptPath, `
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
assert.equal(process.env.WARDIAN_SESSION_ID, undefined, "artifact writer inherited an agent identity");
const root = path.join(process.cwd(), ".wardian-review");
const reserved = fs.readdirSync(root, { withFileTypes: true }).filter((entry) => entry.isDirectory());
assert.equal(reserved.length, 1, "host must reserve exactly this run's artifact directory");
const artifact = path.join(root, reserved[0].name, "review.md");
${missing ? "// Deliberately omit the current artifact after successful preflight." : `fs.writeFileSync(artifact, Buffer.from(${JSON.stringify(bytes.toString("base64"))}, "base64"));`}
process.stdout.write(JSON.stringify({ run_id: reserved[0].name, missing: ${missing} }) + "\\n");
`, "utf8");
  const blueprint = {
    schema: 2,
    id: `native-message-send-${caseId}`,
    name: `Synthetic message delivery ${caseId}`,
    nodes: [
      { id: "trigger-1", type: "manual_trigger", name: "Synthetic request", position: { x: 0, y: 0 } },
      { id: "fixture-1", type: "script", name: "Write synthetic artifact", position: { x: 360, y: 0 }, fields: { runtime: "node", path: scriptPath } },
      { id: "deliver-1", type: "message_send", name: "Host message delivery", position: { x: 720, y: 0 }, fields: {
        recipient: "{{trigger.output.requesting_agent}}",
        artifact_path: ".wardian-review/{{run.id}}/review.md",
      } },
      { id: "notify-1", type: "notify", name: "Admission confirmed", position: { x: 1080, y: 0 }, fields: {
        message: "Synthetic artifact admitted to {{nodes.deliver-1.output.recipient_id}}.",
      } },
    ],
    edges: [
      { from: "trigger-1", to: "fixture-1" },
      { from: "fixture-1", to: "deliver-1" },
      { from: "deliver-1", to: "notify-1" },
    ],
  };
  const library = path.join(harness.isolatedHome, "library", "automations");
  fs.mkdirSync(library, { recursive: true });
  const blueprintPath = path.join(library, `${blueprint.id}.md`);
  // JSON is a YAML subset; this avoids platform-specific path/string escaping.
  fs.writeFileSync(blueprintPath, `---\n${JSON.stringify(blueprint, null, 2)}\n---\n\nSynthetic transport fixture; no Reviewer or model task.\n`);
  return { workspace, blueprintPath, blueprintId: blueprint.id };
}

/** Read only the exact run returned by production automation_run, never the newest directory. */
async function waitForRun(harness, launch, timeoutMs = 20_000) {
  assert.equal(launch.ok, true, JSON.stringify(launch));
  assert.equal(launch.status, "started");
  assert.equal(typeof launch.run_id, "string");
  const runDir = path.join(harness.isolatedHome, "logs", "automations", launch.blueprint_id, launch.run_id);
  assert.equal(path.resolve(launch.run_dir), path.resolve(runDir));
  let lastError;
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    try {
      const state = JSON.parse(fs.readFileSync(path.join(runDir, "state.json"), "utf8"));
      const events = fs.readFileSync(path.join(runDir, "events.jsonl"), "utf8")
        .split(/\r?\n/u).filter(Boolean).map((line) => JSON.parse(line));
      lastError = null;
      const completed = state.status === "completed" && events.some((event) => event.kind === "run_completed");
      // NodeFailed directly makes RunState terminal; a separate RunFailed event is not required.
      const failed = state.status === "failed" && events.some((event) => event.kind === "run_failed" || event.kind === "node_failed");
      if (completed || failed) return { state, events, runDir };
    } catch (error) {
      // Checkpoint/event writes can briefly be incomplete while the engine is active.
      lastError = error;
    }
    await new Promise((resolve) => setTimeout(resolve, 200));
  }
  assert.fail(`Run ${launch.run_id} did not settle: ${lastError?.message ?? "no terminal event"}`);
}

function completedOutput(run, node) {
  const matches = run.events.filter((event) => event.kind === "node_completed" && event.node === node);
  assert.equal(matches.length, 1, `${node} must complete exactly once`);
  assert.deepEqual(matches[0].output, run.state.registry.nodes[node].output);
  return matches[0].output;
}

/** Inspect only this fixture home's canonical delivery row, without modifying the database. */
async function canonicalDelivery(harness, interactionId) {
  const { DatabaseSync } = await import("node:sqlite");
  const db = new DatabaseSync(path.join(harness.isolatedHome, "state.db"), { readOnly: true });
  try {
    const row = db.prepare(`SELECT d.interaction_id, d.sender, d.recipient, d.operation,
      d.idempotency_key, d.owner, h.run_id, h.node
      FROM agent_message_delivery d JOIN agent_message_host_tasks h USING(interaction_id)
      WHERE d.interaction_id = ?`).get(interactionId);
    assert.ok(row, "Recovered receipt has no canonical host delivery row");
    const count = db.prepare(`SELECT COUNT(*) AS count FROM agent_message_host_tasks h
      JOIN agent_message_delivery d USING(interaction_id) WHERE h.run_id = ? AND h.node = ?`)
      .get(row.run_id, row.node).count;
    return { ...row, run_node_count: count };
  } finally {
    db.close();
  }
}

/** Synthesize a lost event/checkpoint tail only after this isolated fixture run is terminal. */
function stageInterruptedDelivery(harness, run) {
  assert.equal(run.state.status, "completed");
  const relative = path.relative(fs.realpathSync(harness.isolatedHome), fs.realpathSync(run.runDir));
  assert.ok(relative && !path.isAbsolute(relative) && relative.split(path.sep)[0] !== "..",
    "Recovery fixture must stay inside the owned native home");
  const completion = run.events.findIndex((event) => event.kind === "node_completed" && event.node === "deliver-1");
  assert.ok(completion > 0);
  const interrupted = run.events.slice(0, completion);
  assert.ok(interrupted.some((event) => event.kind === "node_started" && event.node === "deliver-1"));
  assert.equal(interrupted.some((event) => event.kind === "notification"), false);
  // Keep the committed canonical interaction intact. Only the engine's fixture log loses its tail.
  fs.copyFileSync(path.join(run.runDir, "events.jsonl"), path.join(run.runDir, "native-fixture-original-events.jsonl"));
  fs.copyFileSync(path.join(run.runDir, "state.json"), path.join(run.runDir, "native-fixture-original-state.json"));
  fs.writeFileSync(path.join(run.runDir, "events.jsonl"), interrupted.map((event) => JSON.stringify(event)).join("\n") + "\n");
  fs.unlinkSync(path.join(run.runDir, "state.json"));
  return interrupted;
}

/** Capture the real monitor/inspector after verifying it renders this run's canonical receipt. */
async function captureDeliveryReceipt(harness, driver, launch, receipt) {
  const clickVisible = async (locator) => {
    const element = await driver.wait(until.elementLocated(locator), 15_000);
    await driver.wait(until.elementIsVisible(element), 15_000);
    await element.click();
  };
  await driver.manage().window().setRect({ width: 1600, height: 1050 });
  await clickVisible(By.css('[data-testid="sidebar-tab-automations"]'));
  await clickVisible(By.xpath('//aside//button[normalize-space()="Monitor"]'));
  await clickVisible(By.xpath('//*[@data-testid="automation-monitor"]//button[normalize-space()="History"]'));
  await clickVisible(By.css(`[aria-label="Open ${launch.blueprint_id} run ${launch.run_id}"]`));
  const observe = await driver.wait(until.elementLocated(By.css('[data-testid="automations-observe-mode"]')), 15_000);
  await driver.wait(until.elementIsVisible(observe), 15_000);
  await clickVisible(By.css('[data-testid="run-dag-node-deliver-1"]'));
  await driver.wait(async () => {
    const outputs = await observe.findElements(By.css("aside pre"));
    if (outputs.length !== 1) return false;
    const text = await outputs[0].getText();
    return text.includes(receipt.interaction_id) && text.includes(receipt.artifact_sha256);
  }, 15_000, "Native inspector did not render the canonical message_send receipt");
  const output = await observe.findElement(By.css("aside pre"));
  assert.deepEqual(JSON.parse(await output.getText()), receipt);
  // Give the graph more room through its normal UI, without altering rendered data or styles.
  await clickVisible(By.css('[data-testid="automations-observe-mode"] button[aria-label="Collapse events"]'));
  const visibleText = await observe.getText();
  assert.ok(visibleText.includes(launch.run_id) && visibleText.includes("Host message delivery"));
  assert.equal(visibleText.includes(harness.isolatedHome), false, "Screenshot would expose an absolute home path");
  assert.equal(visibleText.includes(harness.repoRoot), false, "Screenshot would expose an absolute worktree path");
  await driver.executeAsyncScript((done) => requestAnimationFrame(() => requestAnimationFrame(() => done())));
  const timestamp = new Date().toISOString().replace(/[:.]/gu, "-");
  const screenshotPath = path.join(harness.repoRoot, "e2e", "screenshots", "autoreview-delivery", timestamp, "native-message-send-receipt.png");
  fs.mkdirSync(path.dirname(screenshotPath), { recursive: true });
  // Element capture excludes the desktop/sidebar and contains only synthetic workflow content.
  fs.writeFileSync(screenshotPath, await observe.takeScreenshot(true), "base64");
  return { path: screenshotPath, sha256: sha256(fs.readFileSync(screenshotPath)), run_id: launch.run_id, interaction_id: receipt.interaction_id };
}

test("message_send uses host provenance, exact artifact admission and honest failure notification", { timeout: 180_000 }, async () => {
  const harness = await createNativeHarness();
  harness.watchMode = false;
  // Match both standard native runners; fast mode never starts a competing build.
  if (process.env.WARDIAN_NATIVE_SKIP_BUILD !== "1") await ensureNativeAppBuilt(harness);
  prepareIsolatedHome(harness);
  const evidencePath = path.join(harness.isolatedHome, "automation-message-send-report.json");
  const report = {
    status: "running", started_at: new Date().toISOString(),
    test_sha256: sha256(fs.readFileSync(fileURLToPath(import.meta.url))),
    app_sha256: sha256(fs.readFileSync(harness.appPath)),
    cases: [],
  };
  const save = () => fs.writeFileSync(evidencePath, `${JSON.stringify(report, null, 2)}\n`);
  let session;
  let failure;
  // The host must not inherit the real caller's managed identity or capability.
  const removedEnv = new Map();
  for (const key of Object.keys(process.env)) {
    if (/^WARDIAN_(SESSION_ID|MEMORY_CAPABILITY|MOCK_.*)$/iu.test(key)) {
      removedEnv.set(key, process.env[key]);
      delete process.env[key];
    }
  }
  try {
    save();
    session = await startNativeSession(harness);
    await waitForAppShell(session.driver, 30_000);
    report.ports = { driver: harness.driverPort, native: harness.nativeDriverPort };
    assert.ok(Number.isInteger(harness.driverPort) && harness.driverPort > 0);
    assert.ok(Number.isInteger(harness.nativeDriverPort) && harness.nativeDriverPort > 0);
    assert.notEqual(harness.driverPort, harness.nativeDriverPort);
    const cli = path.join(harness.isolatedHome, "bin", process.platform === "win32" ? "wardian-cli.exe" : "wardian-cli");
    report.cli_sha256 = sha256(fs.readFileSync(cli));
    assert.ok(harness.cliPath, "A candidate CLI must be frozen alongside the app");
    assert.equal(report.cli_sha256, sha256(fs.readFileSync(harness.cliPath)), "Host-installed and frozen candidate CLI differ");

    const agents = [];
    for (const name of ["Synthetic-Review-Recipient", "Synthetic-Other-Recipient"]) {
      const folder = path.join(harness.isolatedHome, "workspaces", name);
      fs.mkdirSync(folder, { recursive: true });
      agents.push(await invokeTauri(session.driver, "spawn_agent", { req: {
        sessionName: name, agentClass: "TestClass", folder, isOff: true,
        resumeSession: null, configOverride: { provider: "mock" },
      } }));
    }
    const [receiver, other] = agents;
    // Only canonical reads use these host-registered fixture identities. Sending has no agent identity.
    const receive = (agentId, cursor) => messageCli(cli, harness.isolatedHome, harness.repoRoot, agentId,
      ["receive", "--timeout-ms", "0", "--limit", "100", ...(cursor ? ["--cursor", cursor] : [])]);
    const initial = await receive(receiver.session_id);
    assert.deepEqual(initial.messages, []);
    const admittedIds = [];

    for (const scenario of [
      { id: "uuid", recipient: receiver.session_id },
      { id: "name", recipient: "Synthetic-Review-Recipient" },
      { id: "missing-artifact", recipient: receiver.session_id, missing: true },
    ]) {
      const fixture = seedFixture(harness, scenario.id, scenario);
      const parsed = await invokeTauri(session.driver, "automation_parse", { path: fixture.blueprintPath });
      assert.deepEqual(parsed.diagnostics, []);
      const validation = await invokeTauri(session.driver, "automation_validate", { blueprint: parsed.blueprint });
      assert.equal(validation.ok, true, `Candidate runtime must recognize message_send: ${JSON.stringify(validation)}`);
      const before = await receive(receiver.session_id, initial.next_cursor);
      const launch = await invokeTauri(session.driver, "automation_run", {
        path: fixture.blueprintPath, provider: "mock", workspace: fixture.workspace,
        input: { requesting_agent: scenario.recipient },
      });
      assert.equal(launch.blueprint_id, fixture.blueprintId);
      const run = await waitForRun(harness, launch);
      assert.equal(run.state.run_id, launch.run_id);
      const writer = completedOutput(run, "fixture-1");
      assert.equal(writer.exit_code, 0);
      assert.deepEqual(JSON.parse(writer.stdout), { run_id: launch.run_id, missing: Boolean(scenario.missing) });
      const artifactPath = `.wardian-review/${launch.run_id}/review.md`;
      const after = await receive(receiver.session_id, initial.next_cursor);
      const evidence = { case: scenario.id, run_id: launch.run_id, status: run.state.status, events: run.events, inbox: after };
      report.cases.push(evidence);
      save();

      if (scenario.missing) {
        assert.equal(run.state.status, "failed");
        assert.equal(fs.existsSync(path.join(fixture.workspace, artifactPath)), false);
        const failed = run.events.filter((event) => event.kind === "node_failed" && event.node === "deliver-1");
        assert.equal(failed.length, 1);
        assert.match(failed[0].error, /artifact/iu);
        assert.equal(run.events.some((event) => event.kind === "notification" || event.node === "notify-1" && ["node_started", "node_completed"].includes(event.kind)), false);
        assert.equal(run.events.some((event) => event.kind === "node_completed" && event.node === "deliver-1"), false);
        assert.deepEqual(after.messages, before.messages, "Failed artifact delivery admitted a message");
      } else {
        assert.equal(run.state.status, "completed");
        const receipt = completedOutput(run, "deliver-1");
        for (const field of ["interaction_id", "run_id", "node", "recipient_id", "artifact_path", "artifact_sha256", "idempotency_key", "delivery_state"]) {
          assert.equal(typeof receipt[field], "string", `Missing typed receipt field ${field}`);
          assert.ok(receipt[field].length > 0);
        }
        assert.equal(receipt.run_id, launch.run_id);
        assert.equal(receipt.node, "deliver-1");
        assert.equal(receipt.recipient_id, receiver.session_id);
        assert.equal(receipt.artifact_path, artifactPath);
        assert.equal(receipt.artifact_sha256, sha256(bytes));
        assert.equal(receipt.delivery_state, "stored");
        assert.equal(receipt.duplicate, false);
        assert.deepEqual(fs.readFileSync(path.join(fixture.workspace, artifactPath)), bytes);
        const added = after.messages.filter((message) => !before.messages.some((prior) => prior.interaction_id === message.interaction_id));
        assert.equal(added.length, 1, "One delivery must admit exactly one canonical message");
        assert.equal(added[0].interaction_id, receipt.interaction_id);
        assert.equal(added[0].kind, "message");
        assert.equal(added[0].sender, "", "Host delivery must not impersonate a managed agent");
        assert.deepEqual(added[0].host_automation, { run_id: launch.run_id, node: "deliver-1" });
        assert.deepEqual(Buffer.from(added[0].message, "utf8"), bytes);
        assert.equal(sha256(Buffer.from(added[0].message, "utf8")), receipt.artifact_sha256);
        admittedIds.push(receipt.interaction_id);
        const notices = run.events.filter((event) => event.kind === "notification");
        assert.equal(notices.length, 1);
        assert.equal(notices[0].node, "notify-1");
        assert.equal(notices[0].message, `Synthetic artifact admitted to ${receiver.session_id}.`);
        assert.ok(notices[0].seq > run.events.find((event) => event.kind === "node_completed" && event.node === "deliver-1").seq);
        evidence.receipt = receipt;

        if (scenario.id === "uuid") {
          evidence.screenshot = await captureDeliveryReceipt(harness, session.driver, launch, receipt);
          save();
        }

        if (scenario.id === "name") {
          // A reused display name must not rebind the preflight recipient on desktop resume.
          await invokeTauri(session.driver, "rename_agent", {
            sessionId: receiver.session_id, newName: "Synthetic-Original-Recipient-Renamed",
          });
          await invokeTauri(session.driver, "rename_agent", {
            sessionId: other.session_id, newName: "Synthetic-Review-Recipient",
          });
          evidence.interrupted_events = stageInterruptedDelivery(harness, run);
          save();
          const resumed = await invokeTauri(session.driver, "automation_resume", {
            blueprintId: launch.blueprint_id, runId: launch.run_id, blueprintPath: fixture.blueprintPath,
            // Provider and workspace must come from the persisted invocation.
          });
          assert.equal(resumed.ok, true);
          assert.equal(resumed.run_id, launch.run_id);
          const recovered = await waitForRun(harness, launch);
          evidence.resume = { status: recovered.state.status, events: recovered.events };
          save();
          assert.equal(recovered.state.status, "completed", JSON.stringify(recovered.state.failure));
          const recoveredReceipt = completedOutput(recovered, "deliver-1");
          // Inbox reads may advance delivery state. Compare it to the current canonical row.
          const canonical = await canonicalDelivery(harness, receipt.interaction_id);
          evidence.resume.canonical_delivery = canonical;
          assert.equal(canonical.sender, `host:automation:${launch.run_id}`);
          assert.equal(canonical.recipient, receiver.session_id);
          assert.equal(canonical.operation, "send_message");
          assert.equal(canonical.idempotency_key, receipt.idempotency_key);
          assert.equal(canonical.run_id, launch.run_id);
          assert.equal(canonical.node, "deliver-1");
          assert.equal(canonical.run_node_count, 1);
          assert.deepEqual(recoveredReceipt, { ...receipt, delivery_state: canonical.owner, duplicate: true },
            "Desktop resume must reconcile the original canonical recipient, body and interaction");
          assert.deepEqual(completedOutput(recovered, "fixture-1"), writer,
            "Recovery must preserve the completed synthetic review step");
          assert.equal(recovered.events.filter((event) => event.kind === "node_started" && event.node === "fixture-1").length, 1);
          const recoveredNotices = recovered.events.filter((event) => event.kind === "notification");
          assert.equal(recoveredNotices.length, 1);
          assert.equal(recoveredNotices[0].message, `Synthetic artifact admitted to ${receiver.session_id}.`);
          assert.ok(recoveredNotices[0].seq > recovered.events.find((event) => event.kind === "node_completed" && event.node === "deliver-1").seq);
          assert.deepEqual((await receive(receiver.session_id, initial.next_cursor)).messages, after.messages,
            "Desktop recovery must not create a second canonical admission");
          assert.deepEqual(fs.readFileSync(path.join(fixture.workspace, artifactPath)), bytes);
        }
      }
      assert.equal(fs.readFileSync(path.join(fixture.workspace, ".wardian-review", "review.md"), "utf8"), "STALE: must not be delivered\n");
      assert.deepEqual((await receive(other.session_id)).messages, [], "Delivery reached an unintended agent");
      save();
    }
    const replay = await receive(receiver.session_id, initial.next_cursor);
    assert.equal(replay.messages.length, 2);
    assert.deepEqual(replay.messages.map((message) => message.interaction_id).sort(), admittedIds.sort());
    assert.equal(new Set(admittedIds).size, 2, "Different runs need different canonical admissions");
    assert.deepEqual((await receive(receiver.session_id, replay.next_cursor)).messages, []);
    report.status = "passed";
  } catch (error) {
    report.status = "failed";
    report.error = error.stack ?? String(error);
    failure = error;
  }
  try {
    if (session) await session.close();
  } catch (error) {
    report.status = "failed";
    report.cleanup_error = error.stack ?? String(error);
    // Keep the original test failure primary while recording cleanup failure separately.
    failure ??= error;
  }
  for (const [key, value] of removedEnv) process.env[key] = value;
  report.finished_at = new Date().toISOString();
  try {
    save();
    process.stdout.write(`message_send native evidence: ${evidencePath}\n`);
  } catch (error) {
    process.stderr.write(`message_send evidence write failed: ${String(error)}\n`);
    failure ??= error;
  }
  if (failure) throw failure;
});
