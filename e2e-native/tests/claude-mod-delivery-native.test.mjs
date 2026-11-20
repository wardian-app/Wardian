// @tier manual — Paid real Claude turns; serialize with other provider probes.
// Requires a current prebuilt app with terminal-trace and frontend
// VITE_WARDIAN_TERMINAL_DEBUG=1. Never builds or substitutes a mock provider.
// WARDIAN_E2E_REAL_CLAUDE_MOD=1 WARDIAN_E2E_CLAUDE_MOD_MODEL=haiku
// WARDIAN_NATIVE_SKIP_BUILD=1 WARDIAN_NATIVE_APP=<absolute-isolated-artifact-path>
// node scripts/run-native-e2e-fast.mjs e2e-native/tests/claude-mod-delivery-native.test.mjs
import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
import os from "node:os";
import { randomBytes, createHash } from "node:crypto";
import { By, Key } from "selenium-webdriver";
import { createNativeHarness, invokeTauri, prepareIsolatedHome, startNativeSession, waitForAppShell } from "../lib/harness.mjs";
import { cleanupConformanceSession, pauseConformanceWork } from "../lib/conformance-cleanup.mjs";
import { messageCli } from "../lib/canonical-messaging.mjs";
import { assertTerminalDebugAvailable, resolveAgentTerminalPresentationId, readTerminalDebugSnapshot } from "../lib/terminal-debug.mjs";

const OPT_IN = "WARDIAN_E2E_REAL_CLAUDE_MOD";
const MODEL_ENV = "WARDIAN_E2E_CLAUDE_MOD_MODEL";

// Only transient absence/partial JSON is retryable. Permissions and other I/O
// errors remain failures, rather than looking like a mod which has not started.
async function readJson(file) {
  try { return JSON.parse(await fs.readFile(file, "utf8")); }
  catch (error) {
    if (error.code === "ENOENT" || error instanceof SyntaxError) return null;
    throw error;
  }
}

async function idle(driver, sessionId) {
  await driver.wait(async () => {
    const metrics = await invokeTauri(driver, "list_agent_metrics");
    return metrics.some((row) => row.session_id === sessionId
      && row.current_status?.toLowerCase() === "idle");
  }, 180_000, "Real Claude did not reach idle");
}

const inputLines = (trace) => trace.split(/\r?\n/).filter((line) => /\] IN len=/.test(line));

/** Inspect canonical replies without a retired message-receive CLI or delivery replay. */
function canonicalReply(DatabaseSync, home, requestId, sender, recipient, expected) {
  const db = new DatabaseSync(path.join(home, "state.db"), { readOnly: true });
  try {
    const rows = db.prepare(`SELECT id, sender_session_id, target_session_ids,
      parent_interaction_id, body_ref FROM interactions
      WHERE kind='reply' AND parent_interaction_id=?`).all(requestId);
    assert.ok(rows.length <= 1, "Duplicate canonical replies to the same task");
    if (!rows.length) return null;
    const row = rows[0];
    assert.equal(row.sender_session_id, sender);
    assert.deepEqual(JSON.parse(row.target_session_ids), [recipient]);
    const body = JSON.parse(row.body_ref);
    assert.equal(body.storage, "inline", "Small reply must have an inline canonical body");
    assert.equal(body.body, expected);
    const structured = db.prepare("SELECT status, body FROM structured_replies WHERE request_id=?").get(requestId);
    assert.equal(structured?.status, "done");
    assert.equal(structured.body, expected);
    assert.ok(row.id);
    return { interaction_id: row.id, parent_interaction_id: row.parent_interaction_id,
      sender, recipient, reply_status: structured.status, message: body.body };
  } finally { db.close(); }
}

for (const modAbsent of [false, true]) {
  test(`interactive Claude ${modAbsent ? "records mod_not_loaded and delivers through the terminal" : "delivers a peer task through its generation mod without disturbing a draft"}`,
    { timeout: 420_000 }, async (t) => {
      if (process.env[OPT_IN] !== "1") {
        t.skip(`Set ${OPT_IN}=1; real-provider acceptance was not run`);
        return;
      }
      const model = process.env[MODEL_ENV]?.trim();
      assert.ok(model, `Set ${MODEL_ENV} to an explicitly preflighted catalog model`);
      assert.ok(path.isAbsolute(process.env.WARDIAN_NATIVE_APP ?? ""), "Select a current prebuilt isolated app with WARDIAN_NATIVE_APP");
      assert.equal(process.env.WARDIAN_NATIVE_SKIP_BUILD, "1", "This probe must not rebuild the shared app");
      const harness = await createNativeHarness();
      harness.appPath = process.env.WARDIAN_NATIVE_APP;
      harness.watchMode = false;
      // Each case retains its own report and trace; preparing the same home
      // twice would erase the first case's evidence.
      harness.isolatedHome = await fs.mkdtemp(path.join(os.tmpdir(),
        `wardian-e2e-native-claude-mod-${modAbsent ? "fallback" : "draft"}-`));
      prepareIsolatedHome(harness);
      const report = {
        schema: 1, provider: "claude", model, mod_absent: modAbsent,
        status: "running", started_at: new Date().toISOString(),
        app_sha256: createHash("sha256").update(await fs.readFile(harness.appPath)).digest("hex"),
      };
      const reportPath = path.join(harness.isolatedHome, `claude-mod-${modAbsent ? "fallback" : "draft"}-report.json`);
      const save = () => fs.writeFile(reportPath, `${JSON.stringify(report, null, 2)}\n`);
      const owned = { session: undefined, agent: undefined, spawnAttempted: false, startupAttempted: false };
      t.after(() => cleanupConformanceSession({
        harness, session: owned.session, startupAttempted: owned.startupAttempted,
        pause: () => pauseConformanceWork((cmd, args) => invokeTauri(owned.session.driver, cmd, args), {
          spawnAttempted: owned.spawnAttempted, sessionId: owned.agent?.session_id,
        }),
        save: async (cleanup) => {
          report.cleanup = cleanup;
          report.completed_at = new Date().toISOString();
          await save();
          t.diagnostic(`Private ${path.basename(reportPath)} retained in the isolated run home`);
        },
      }));
      try {
        await save();
        const token = randomBytes(6).toString("hex");
        const workspace = path.join(harness.isolatedHome, `claude-mod-workspace-${token}`);
        await fs.mkdir(path.join(workspace, ".claude"), { recursive: true });
        if (modAbsent) {
          // A real documented Claude setting prevents mod registration. Never
          // fabricate ready/heartbeat/outcome records to force either route.
          await fs.writeFile(path.join(workspace, ".claude", "settings.local.json"),
            JSON.stringify({ disableAllHooks: true }));
        }
        owned.startupAttempted = true;
        owned.session = await startNativeSession(harness);
        const driver = owned.session.driver;
        await driver.manage().setTimeouts({ script: 180_000 });
        await waitForAppShell(driver, 30_000);
        await assertTerminalDebugAvailable(driver);
        const catalog = await invokeTauri(driver, "list_provider_model_catalog", { provider: "claude", forceRefresh: true });
        assert.equal(catalog.refresh_error, null, "Claude model catalog refresh failed");
        assert.ok(catalog.models.some((entry) => entry.id === model), "Explicit model absent from refreshed catalog");
        report.provider_version = catalog.version;
        assert.ok(report.provider_version, "Retain the real Claude version for early-access mod API acceptance");
        const sender = await invokeTauri(driver, "spawn_agent", { req: {
          sessionName: `Mod-Peer-${token}`, agentClass: "TestClass", folder: workspace,
          isOff: true, resumeSession: null, configOverride: { provider: "mock" },
        } });
        owned.spawnAttempted = true;
        owned.agent = await invokeTauri(driver, "spawn_agent", { req: {
          sessionName: `Claude-Mod-${token}`, agentClass: "TestClass", folder: workspace,
          isOff: false, resumeSession: null,
          configOverride: { provider: "claude", model, session_persistence: "resume", conversation_logging: "enabled" },
        } });
        const sid = owned.agent.session_id;
        assert.equal(owned.agent.provider, "claude");
        assert.equal(owned.agent.model, model);
        await idle(driver, sid);
        const presentation = await resolveAgentTerminalPresentationId(driver, sid);
        const deliveryRoot = path.join(harness.isolatedHome, "agents", sid, "claude", "delivery");
        const generations = (await fs.readdir(deliveryRoot)).filter((entry) => /^\d+$/.test(entry))
          .sort((a, b) => Number(a) - Number(b));
        assert.equal(generations.length, 1, "Fresh agent must own exactly one delivery generation");
        const generation = Number(generations[0]);
        const generationRoot = path.join(deliveryRoot, generations[0]);
        const launch = await readJson(path.join(generationRoot, "mod", "launch.json"));
        assert.equal(launch?.agent_id, sid);
        assert.equal(launch.runtime_generation, generation);
        const inbox = path.join(generationRoot, "inbox");
        assert.equal(path.resolve(launch.inbox), path.resolve(inbox));
        if (!modAbsent) {
          await driver.wait(async () => {
            const ready = await readJson(path.join(inbox, "ready.json"));
            const heartbeat = await readJson(path.join(inbox, "heartbeat.json"));
            return ready?.generation === generation && heartbeat?.generation === generation
              && ready.session_id === launch.expected_claude_session_id
              && heartbeat.session_id === ready.session_id
              && Number.isFinite(heartbeat.at) && Date.now() - heartbeat.at >= 0
              && Date.now() - heartbeat.at < 5_000;
          }, 60_000, "The actual Claude mod never became ready for this generation/session");
        }
        const tracePath = path.join(harness.isolatedHome, "debug", "terminal-traces", `${sid}.log`);
        // A calibration keystroke proves trace capture is active. Keep the draft
        // short enough to fit one row, then use End/backspace after the reply to
        // prove it is still editable input rather than an old scrollback echo.
        const draft = `DRAFT_${token}`;
        const textarea = await driver.findElement(By.css(`#agent-card-${sid} .xterm-helper-textarea`));
        const draftRow = async (text) => {
          const snapshot = await readTerminalDebugSnapshot(driver, presentation);
          return snapshot?.lines?.find((line) => line.includes(text)) ?? false;
        };
        // Only mod delivery promises draft preservation. Leave the fallback
        // prompt empty so terminal delivery cannot combine with a user's draft.
        if (!modAbsent) {
          await textarea.sendKeys(draft);
          await driver.wait(async () => inputLines(await fs.readFile(tracePath, "utf8")).length > 0,
            10_000, "Requires a terminal-trace build; no calibrated PTY input was recorded");
          await driver.wait(() => draftRow(draft), 10_000, "Typed draft was not rendered");
        }
        const traceBefore = inputLines(await fs.readFile(tracePath, "utf8"));
        const logPath = path.join(harness.isolatedHome, "wardian_debug.log");
        const logBefore = (await fs.readFile(logPath, "utf8")).length;
        const cli = path.join(harness.isolatedHome, "bin", process.platform === "win32" ? "wardian-cli.exe" : "wardian-cli");
        const expected = `MOD_REPLY_${token}`;
        const { DatabaseSync } = await import("node:sqlite");
        const task = await messageCli(cli, harness.isolatedHome, workspace, sender.session_id,
          ["followup", sid, `Reply to this Wardian task with status done and message exactly ${expected}. Use the Wardian reply tool if available; otherwise you may run the Wardian message reply CLI command for this task's canonical request_id, passing the message on stdin. You may inspect its schema/help if necessary. Do not edit files or run unrelated commands.`]);
        assert.ok(task.request_id);
        report.request_id = task.request_id;
        await save();
        let reply;
        await driver.wait(async () => {
          reply = canonicalReply(DatabaseSync, harness.isolatedHome, task.request_id,
            sid, sender.session_id, expected);
          return !!reply;
        }, 180_000, "Real Claude did not produce the correlated canonical peer reply; no replay");
        await idle(driver, sid);
        if (!modAbsent) {
          // A queued mod record proves admission only. Require Wardian's real
          // transcript scanner to confirm this exact interaction crossed into
          // a provider turn before calling the delivery successful.
          let lastPhase = null;
          try {
            report.native_delivery_phase = await driver.wait(() => {
              const db = new DatabaseSync(path.join(harness.isolatedHome, "state.db"), { readOnly: true });
              try {
                lastPhase = db.prepare("SELECT phase FROM native_deliveries WHERE interaction_id=?")
                  .get(task.request_id)?.phase ?? null;
                return ["turn_started", "completed"].includes(lastPhase) ? lastPhase : false;
              } finally { db.close(); }
            }, 30_000, "Transcript scanner did not confirm the peer task as turn_started or completed", 250);
          } finally {
            report.native_delivery_last_observed_phase = lastPhase;
          }
        }
        const traceAfter = inputLines(await fs.readFile(tracePath, "utf8"));
        if (modAbsent) {
          assert.equal(await readJson(path.join(inbox, "ready.json")), null, "Hooks-disabled Claude unexpectedly loaded the mod");
          assert.ok(traceAfter.length > traceBefore.length, "Fallback did not write terminal input");
          const log = (await fs.readFile(logPath, "utf8")).slice(logBefore);
          assert.ok(log.split(/\r?\n/).some((line) => line.includes(sid) && line.includes("mod_not_loaded")),
            "Fallback must record mod_not_loaded for this exact agent");
          report.fallback_reason = "mod_not_loaded";
        } else {
          const pendingNames = (await fs.readdir(path.join(inbox, "pending")))
            .filter((name) => name.endsWith(`-${task.request_id}.json`));
          assert.equal(pendingNames.length, 1, "Peer task was not published exactly once in the generation inbox");
          const pending = await readJson(path.join(inbox, "pending", pendingNames[0]));
          assert.equal(pending.interaction_id, task.request_id);
          assert.equal(pending.runtime_generation, generation);
          for (const kind of ["claimed", "queued"]) {
            const record = await readJson(path.join(inbox, kind, `${task.request_id}.json`));
            assert.equal(record?.interaction_id, task.request_id, `Missing ${kind} evidence`);
            assert.equal(record.generation, generation);
          }
          assert.deepEqual(traceAfter, traceBefore, "Mod delivery wrote PTY input while the user had a draft");
          assert.ok(await draftRow(draft), "Draft disappeared during peer delivery");
          await textarea.sendKeys(Key.END, Key.BACK_SPACE);
          await driver.wait(async () => (await draftRow(draft.slice(0, -1))) && !(await draftRow(draft)),
            10_000, "Surviving draft must still be editable at the Claude prompt");
          report.draft_preserved_and_editable = true;
          report.delivery_pty_writes = 0;
        }
        report.reply = reply;
        report.generation = generation;
        report.status = "pass";
        await save();
      } catch (error) {
        report.status = "fail";
        report.error = String(error);
        await save();
        throw error;
      }
    });
}
