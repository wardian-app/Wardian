import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { after, before, describe, test } from "node:test";
import { setTimeout as delay } from "node:timers/promises";
import { fileURLToPath } from "node:url";
import { createHash } from "node:crypto";

import { createWindowsSupervisorPlan, createWindowsSupervisorReceipts, innerSupervisorReceiptIsClean } from "./native-e2e-runner.mjs";
import { acquireHomeLock } from "../e2e-native/lib/sessionHome.mjs";

const supervisorScript = fileURLToPath(new URL("./native-e2e-windows-supervisor.ps1", import.meta.url));
const identityBirth = "2026-10-09T12:34:56.1234567Z";

// A compiled argv recorder lets [] really mean zero child arguments. A Node
// script or -e fixture would prepend its own arguments and miss that boundary.
const recorderSource = `
using System;
using System.IO;
using System.Text;
using System.Web.Script.Serialization;

public static class ArgvFixture
{
    public static int Main(string[] arguments)
    {
        File.WriteAllText(Environment.GetEnvironmentVariable("WARDIAN_SUPERVISOR_REPORT"),
            new JavaScriptSerializer().Serialize(arguments), new UTF8Encoding(false));
        string identityReport = Environment.GetEnvironmentVariable("WARDIAN_SUPERVISOR_IDENTITY_REPORT");
        if (!String.IsNullOrEmpty(identityReport))
        {
            using (var self = System.Diagnostics.Process.GetCurrentProcess())
            {
                DateTime birth = self.StartTime.ToUniversalTime();
                File.WriteAllText(identityReport, new JavaScriptSerializer().Serialize(new string[] {
                    self.Id.ToString(System.Globalization.CultureInfo.InvariantCulture),
                    birth.ToFileTimeUtc().ToString(System.Globalization.CultureInfo.InvariantCulture),
                    birth.ToString("O", System.Globalization.CultureInfo.InvariantCulture)
                }), new UTF8Encoding(false));
            }
        }
        return Int32.Parse(Environment.GetEnvironmentVariable("WARDIAN_SUPERVISOR_EXIT") ?? "0");
    }
}
`;

/** Execute the exact producer plan; only child fixture environment is supplied. */
function runSupervisor(plan, env, receipt, script) {
  const produced = createWindowsSupervisorPlan(plan, "win32", receipt);
  assert.equal(produced.command, "powershell.exe");
  assert.deepEqual(produced.args.slice(0, 4), ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"]);
  assert.equal(produced.args[4], supervisorScript);
  assert.equal(produced.args[produced.args.indexOf("-Executable") + 1], plan.command);
  assert.equal(produced.args[produced.args.indexOf("-ArgumentsJson") + 1], JSON.stringify(plan.args));
  if (script) produced.args[4] = script;
  return spawnSync(produced.command, produced.args, {
    env,
    encoding: "utf8",
    windowsHide: true,
    timeout: 15000,
    maxBuffer: 1024 * 1024,
  });
}

function assertExit(result, expected) {
  assert.ifError(result.error);
  assert.equal(result.signal, null);
  assert.equal(result.status, expected, result.stderr || result.stdout);
}

const wireHash = (plan) => createHash("sha256").update(plan.command).update("\0").update(JSON.stringify(plan.args)).digest("hex");

function pureHome(t) {
  const base = fs.realpathSync(os.tmpdir());
  const outputParent = fs.mkdtempSync(path.join(base, "wardian-inner-receipt-"));
  t.after(() => {
    assert.equal(path.dirname(fs.realpathSync(outputParent)), base);
    assert.ok(path.basename(outputParent).startsWith("wardian-inner-receipt-"));
    fs.rmSync(outputParent, { recursive: true, force: true });
  });
  const root = path.join(outputParent, "native-home");
  const runId = "contract-run";
  acquireHomeLock({ home: root, runId });
  return { root, runId, outputParent };
}

test("optional inner receipts have unique per-target fresh paths bound to the owned run and exact wire", (t) => {
  const { root, runId, outputParent } = pureHome(t);
  const plans = [{ command: process.execPath, args: ["--test", "first-case"] },
    { command: process.execPath, args: ["--test", "second-case"] }];
  const directory = path.join(outputParent, "receipts");
  const receipts = createWindowsSupervisorReceipts({ directory, home: root, runId, plans, platform: "win32" });
  assert.equal(new Set(receipts.map((receipt) => receipt.path)).size, plans.length);
  for (const [index, receipt] of receipts.entries()) {
    const produced = createWindowsSupervisorPlan(plans[index], "win32", receipt);
    const arg = (key) => produced.args[produced.args.indexOf(key) + 1];
    assert.equal(arg("-ReceiptPath"), receipt.path); assert.equal(arg("-ReceiptRunId"), runId);
    assert.equal(arg("-ReceiptTargetIndex"), String(index)); assert.equal(arg("-ReceiptTargetSha256"), wireHash(plans[index]));
    assert.equal(arg("-Executable"), plans[index].command); assert.equal(arg("-ArgumentsJson"), JSON.stringify(plans[index].args));
    assert.equal(fs.existsSync(receipt.path), false, "only the actual owner may create the final receipt");
  }
  assert.throws(() => createWindowsSupervisorReceipts({ directory, home: root, runId, plans, platform: "win32" }), /EEXIST/);
  assert.throws(() => createWindowsSupervisorReceipts({ directory: path.join(root, "resettable-receipts"),
    home: root, runId, plans, platform: "win32" }), /outside its resettable home/);
  assert.throws(() => createWindowsSupervisorReceipts({ directory: path.join(outputParent, "foreign-run"),
    home: root, runId: "foreign", plans, platform: "win32" }), /owned/);
  assert.deepEqual(createWindowsSupervisorReceipts({ home: root, runId, plans, platform: "win32" }), [undefined, undefined]);
  assert.equal(createWindowsSupervisorPlan(plans[0], "win32").args.includes("-ReceiptPath"), false);
  assert.equal(createWindowsSupervisorPlan(plans[0], "linux", receipts[0]), plans[0]);
  const realOutputParent = path.join(outputParent, "real-output-parent");
  const linkedOutputParent = path.join(outputParent, "linked-output-parent");
  fs.mkdirSync(realOutputParent);
  fs.symlinkSync(realOutputParent, linkedOutputParent, process.platform === "win32" ? "junction" : "dir");
  assert.throws(() => createWindowsSupervisorReceipts({ directory: path.join(linkedOutputParent, "receipts"),
    home: root, runId, plans, platform: "win32" }), /link/);
});

test("missing, partial, foreign, signalled, wait-failed, unknown or forced inner receipts never meet the inner condition", () => {
  const expected = { runId: "contract-run", targetIndex: 0, targetSha256: "a".repeat(64) };
  // Synthetic field matrix tests the consumer predicate; it is not runtime evidence.
  const clean = { schema: 1, kind: "native-e2e-inner-supervisor", run_id: expected.runId, target_index: 0,
    target_sha256: expected.targetSha256, writer_finished: true, observations_complete: true, run_returned: true,
    error_type: null, job_assigned: true, root_resumed: true, root_pid: 123,
    root_birth_filetime: "134000000001234567", root_birth_utc: identityBirth, birth_error: null,
    wait_result: 0, wait_error: null, joined: true, root_exit_code: 0, root_terminate_before_join: false,
    job_query_error: null, active_processes_before_close: 0, kill_on_close_residue_requested: false,
    job_close_called: true, job_close_succeeded: true, stdio_eof: "not_observed_inherited" };
  assert.equal(innerSupervisorReceiptIsClean(clean, expected), true);
  for (const value of [undefined, null, {}, { schema: 1 }, ...[
    { run_id: "foreign" }, { target_index: 1 }, { target_sha256: "b".repeat(64) }, { root_birth_filetime: Number("134000000001234567") },
    { writer_finished: false }, { observations_complete: false }, { run_returned: false }, { error_type: "Exception" },
    { wait_result: 4294967295, wait_error: 6, joined: false }, { wait_result: 128 }, { wait_result: null },
    { root_exit_code: null }, { root_exit_code: 23 }, { root_terminate_before_join: true },
    { root_birth_filetime: null, birth_error: 6 },
    { active_processes_before_close: null, kill_on_close_residue_requested: null, job_query_error: 6 },
    { active_processes_before_close: 1, kill_on_close_residue_requested: true }, { job_close_succeeded: false },
    { stdio_eof: true },
  ].map((change) => ({ ...clean, ...change }))]) assert.equal(innerSupervisorReceiptIsClean(value, expected), false);
});

describe("Windows PowerShell 5.1 supervisor contract", { skip: process.platform !== "win32" }, () => {
  let fixtureRoot;
  let recorder;
  let sequence = 0;

  before(() => {
    const version = spawnSync("powershell.exe", [
      "-NoProfile", "-NonInteractive", "-Command", "$PSVersionTable.PSVersion.ToString()",
    ], { encoding: "utf8", windowsHide: true, timeout: 15000 });
    assertExit(version, 0);
    assert.match(version.stdout.trim(), /^5\.1\./, "exercise the canonical Windows PowerShell consumer");

    const fixtureBase = process.env.WARDIAN_SUPERVISOR_TEST_ROOT ?? os.tmpdir();
    // The override allows a private qualification to keep every artifact in
    // its assigned directory. CI uses a fresh OS temporary directory.
    fixtureRoot = fs.mkdtempSync(path.join(fixtureBase, "wardian-supervisor-contract-"));
    const sourceFile = path.join(fixtureRoot, "argv fixture.cs");
    recorder = path.join(fixtureRoot, "argv fixture.exe");
    fs.writeFileSync(sourceFile, recorderSource);
    const compile = `
      $ErrorActionPreference = 'Stop'
      Add-Type -Path $env:WARDIAN_SUPERVISOR_SOURCE -OutputAssembly $env:WARDIAN_SUPERVISOR_EXE -OutputType ConsoleApplication -ReferencedAssemblies System.Web.Extensions
    `;
    assertExit(spawnSync("powershell.exe", [
      "-NoProfile", "-NonInteractive", "-EncodedCommand", Buffer.from(compile, "utf16le").toString("base64"),
    ], {
      env: { ...process.env, WARDIAN_SUPERVISOR_SOURCE: sourceFile, WARDIAN_SUPERVISOR_EXE: recorder },
      encoding: "utf8",
      windowsHide: true,
      timeout: 20000,
    }), 0);
  });

  after(() => {
    if (!fixtureRoot) return;
    // Never delete a supplied base directory: only our fresh, resolved child.
    const fixtureBase = fs.realpathSync(process.env.WARDIAN_SUPERVISOR_TEST_ROOT ?? os.tmpdir());
    assert.equal(path.dirname(fs.realpathSync(fixtureRoot)), fixtureBase);
    assert.ok(path.basename(fixtureRoot).startsWith("wardian-supervisor-contract-"));
    fs.rmSync(fixtureRoot, { recursive: true, force: true });
  });

  function reportEnv(exitCode = 0) {
    const report = path.join(fixtureRoot, `argv-${sequence++}.json`);
    return {
      report,
      env: { ...process.env, WARDIAN_SUPERVISOR_REPORT: report, WARDIAN_SUPERVISOR_EXIT: String(exitCode) },
    };
  }

  function receiptCase(plan, exitCode = 0) {
    const home = fs.realpathSync(fs.mkdtempSync(path.join(fixtureRoot, "receipt-home-")));
    const runId = `supervisor-receipt-${sequence++}`;
    acquireHomeLock({ home, runId });
    const directory = path.join(fixtureRoot, `receipt-output-${sequence++}`);
    const [binding] = createWindowsSupervisorReceipts({ directory, home, runId, plans: [plan] });
    const { report, env } = reportEnv(exitCode);
    return { binding, report,
      expected: { runId, targetIndex: 0, targetSha256: wireHash(plan) },
      env: { ...env, WARDIAN_E2E_NATIVE_HOME: home, WARDIAN_E2E_RUN_ID: runId,
        WARDIAN_NATIVE_INNER_RECEIPT_DIRECTORY: directory },
      read: () => JSON.parse(fs.readFileSync(binding.path, "utf8")) };
  }

  const roundTrips = [
    ["zero arguments", []],
    ["one plain argument", ["plain"]],
    ["one empty string", [""]],
    ["one argument with spaces", ["two words"]],
    ["one argument with quotes", ['say "hello"']],
    ["one trailing backslash", ["relative\\"]],
    ["quoted trailing backslash", ["path with spaces\\"]],
    ["backslashes before a quote", ['two\\\\"quotes']],
    ["seven-fraction ISO identity string", [identityBirth]],
    ["multiple mixed arguments", ["", "plain", "two words", 'say "hello"', "path with spaces\\", "relative\\", identityBirth]],
  ];

  for (const [name, args] of roundTrips) {
    test(`producer -> JSON -> actual consumer preserves ${name}`, () => {
      const { report, env } = reportEnv();
      assertExit(runSupervisor({ command: recorder, args }, env), 0);
      assert.deepEqual(JSON.parse(fs.readFileSync(report, "utf8")), args);
    });
  }

  test("literal serialized identity survives both argv serialization and child JSON parsing", () => {
    const identity = { pid: 123, birth: identityBirth };
    const { report, env } = reportEnv();
    assertExit(runSupervisor({ command: recorder, args: [JSON.stringify(identity)] }, env), 0);
    const [serialized] = JSON.parse(fs.readFileSync(report, "utf8"));
    assert.equal(serialized, JSON.stringify(identity));
    const actual = JSON.parse(serialized);
    assert.equal(typeof actual.birth, "string");
    assert.equal(actual.birth, identityBirth);
  });

  test("supervisor returns the owned child's nonzero exit code", () => {
    const { report, env } = reportEnv(23);
    assertExit(runSupervisor({ command: recorder, args: ["exit-code"] }, env), 23);
    assert.deepEqual(JSON.parse(fs.readFileSync(report, "utf8")), ["exit-code"]);
  });

  test("malformed serialized arguments fail before starting the child", () => {
    const { report, env } = reportEnv();
    const produced = createWindowsSupervisorPlan({ command: recorder, args: ["must-not-start"] });
    // A focused negative changes only the producer's final wire value.
    produced.args[produced.args.indexOf("-ArgumentsJson") + 1] = "[invalid";
    const result = spawnSync(produced.command, produced.args, {
      env, encoding: "utf8", windowsHide: true, timeout: 15000,
    });
    assertExit(result, 1);
    assert.match(result.stderr, /ConvertFrom-Json|Invalid JSON/i);
    assert.equal(fs.existsSync(report), false);
  });

  test("missing child executable fails without a child report", () => {
    const { report, env } = reportEnv();
    const result = runSupervisor({ command: path.join(fixtureRoot, "missing.exe"), args: ["must-not-start"] }, env);
    assertExit(result, 1);
    assert.match(result.stderr, /CreateProcess failed/);
    assert.equal(fs.existsSync(report), false);
  });

  for (const exitCode of [0, 23]) {
    test(`inner receipt binds actual original root identity and natural exit ${exitCode}`, () => {
      const plan = { command: recorder, args: ["receipt-identity"] };
      const fixture = receiptCase(plan, exitCode);
      const identityReport = path.join(path.dirname(fixture.binding.path), "child-identity.json");
      assertExit(runSupervisor(plan, { ...fixture.env, WARDIAN_SUPERVISOR_IDENTITY_REPORT: identityReport }, fixture.binding), exitCode);
      const receipt = fixture.read();
      const [pid, filetime, utc] = JSON.parse(fs.readFileSync(identityReport, "utf8"));
      assert.equal(receipt.root_pid, Number(pid)); assert.equal(receipt.root_birth_filetime, filetime);
      assert.equal(receipt.root_birth_utc, utc); assert.equal(typeof receipt.root_birth_filetime, "string");
      assert.equal(receipt.wait_result, 0); assert.equal(receipt.joined, true); assert.equal(receipt.root_exit_code, exitCode);
      assert.equal(receipt.active_processes_before_close, 0); assert.equal(receipt.kill_on_close_residue_requested, false);
      assert.equal(receipt.observations_complete, true); assert.equal(receipt.writer_finished, true);
      assert.equal(receipt.stdio_eof, "not_observed_inherited", "inner owner cannot supply outer EOF proof");
      assert.equal(innerSupervisorReceiptIsClean(receipt, fixture.expected), exitCode === 0);
    });
  }

  test("root zero with positive observed owned residue records kill-on-close and remains unqualified", () => {
    const childSource = `
      const { spawn } = require("node:child_process");
      const child = spawn(process.execPath, ["-e", "setTimeout(() => {}, 60000)"], { stdio: "ignore", windowsHide: true });
      child.once("spawn", () => child.unref());
    `;
    const plan = { command: process.execPath, args: ["-e", childSource] };
    const fixture = receiptCase(plan);
    assertExit(runSupervisor(plan, fixture.env, fixture.binding), 0);
    const receipt = fixture.read();
    assert.equal(receipt.joined, true); assert.equal(receipt.root_exit_code, 0);
    assert.ok(receipt.active_processes_before_close > 0, "owned residue must be observed before inner Job close masks it");
    assert.equal(receipt.kill_on_close_residue_requested, true); assert.equal(receipt.job_close_succeeded, true);
    assert.equal(innerSupervisorReceiptIsClean(receipt, fixture.expected), false);
  });

  for (const [name, rawResult] of [["WAIT_FAILED", "0xffffffff"], ["unexpected wait result", "0x00000080"]]) {
    test(`${name} is retained and never becomes joined or root zero`, () => {
      // Controlled unit fault at the actual wait seam, never a production override.
      const source = fs.readFileSync(supervisorScript, "utf8");
      const needle = "uint waitResult = WaitForSingleObject(process.hProcess, INFINITE);";
      assert.equal(source.split(needle).length, 2);
      const script = path.join(fixtureRoot, `wait-fault-${sequence++}.ps1`);
      fs.writeFileSync(script, source.replace(needle, `uint waitResult = ${rawResult};`));
      const plan = { command: recorder, args: ["wait-fault"] };
      const fixture = receiptCase(plan);
      const result = runSupervisor(plan, fixture.env, fixture.binding, script);
      assertExit(result, 1);
      const receipt = fixture.read();
      assert.equal(receipt.wait_result, Number(rawResult)); assert.equal(receipt.joined, false);
      assert.equal(receipt.root_exit_code, null); assert.equal(receipt.root_terminate_before_join, true);
      assert.equal(receipt.observations_complete, false); assert.equal(receipt.run_returned, false);
      assert.equal(typeof receipt.error_type, "string");
      assert.equal(innerSupervisorReceiptIsClean(receipt, fixture.expected), false);
    });
  }

  test("child creation failure emits an incomplete receipt, and a stale path cannot overwrite or start a child", () => {
    const missing = { command: path.join(fixtureRoot, "missing-receipt-child.exe"), args: [] };
    const failed = receiptCase(missing);
    assertExit(runSupervisor(missing, failed.env, failed.binding), 1);
    const incomplete = failed.read();
    assert.equal(incomplete.root_pid, null); assert.equal(incomplete.joined, false);
    assert.equal(incomplete.observations_complete, false);
    assert.equal(innerSupervisorReceiptIsClean(incomplete, failed.expected), false);
    const plan = { command: recorder, args: ["must-not-overwrite"] };
    const stale = receiptCase(plan);
    fs.writeFileSync(stale.binding.path, "stale-sentinel", { flag: "wx" });
    assertExit(runSupervisor(plan, stale.env, stale.binding), 1);
    assert.equal(fs.readFileSync(stale.binding.path, "utf8"), "stale-sentinel");
    assert.equal(fs.existsSync(stale.report), false, "freshness refusal precedes CreateProcess");
  });

  test("an incorrect target digest is rejected before receipt creation or child start", () => {
    const plan = { command: recorder, args: ["must-not-start-wrong-scope"] };
    const fixture = receiptCase(plan);
    const produced = createWindowsSupervisorPlan(plan, "win32", fixture.binding);
    produced.args[produced.args.indexOf("-ReceiptTargetSha256") + 1] = "0".repeat(64);
    assertExit(spawnSync(produced.command, produced.args, { env: fixture.env, encoding: "utf8",
      windowsHide: true, timeout: 15000, maxBuffer: 1024 * 1024 }), 1);
    assert.equal(fs.existsSync(fixture.binding.path), false);
    assert.equal(fs.existsSync(fixture.report), false);
  });

  test("an interrupted inner supervisor cannot turn its empty reserved file into completed evidence", { timeout: 20000 }, async (t) => {
    const started = path.join(fixtureRoot, `interrupted-root-${sequence++}.ready`);
    const childSource = `
      require("node:fs").writeFileSync(${JSON.stringify(started)}, "ready");
      setTimeout(() => {}, 60000);
    `;
    const plan = { command: process.execPath, args: ["-e", childSource] };
    const fixture = receiptCase(plan);
    const produced = createWindowsSupervisorPlan(plan, "win32", fixture.binding);
    const supervisor = spawn(produced.command, produced.args, { env: fixture.env,
      stdio: ["ignore", "pipe", "pipe"], windowsHide: true });
    supervisor.stdout.resume(); supervisor.stderr.resume();
    const closed = new Promise((resolve, reject) => { supervisor.once("close", resolve); supervisor.once("error", reject); });
    t.after(async () => {
      if (supervisor.exitCode === null && supervisor.signalCode === null) supervisor.kill();
      await closed;
    });
    const deadline = Date.now() + 10000;
    // Reservation precedes CreateProcess. Interrupt only after the child runs,
    // proving Job assignment and ResumeThread completed; otherwise a suspended
    // root can retain the inherited pipe before the inner Job ever owned it.
    const childStarted = () => fs.existsSync(started) && fs.readFileSync(started, "utf8") === "ready";
    while (!fs.existsSync(fixture.binding.path) || !childStarted()) {
      assert.ok(Date.now() < deadline, "fresh receipt reservation and owned child startup were not observed");
      assert.equal(supervisor.exitCode, null);
      await delay(25);
    }
    supervisor.kill();
    await closed;
    assert.throws(() => fixture.read(), SyntaxError);
    assert.equal(innerSupervisorReceiptIsClean(null, fixture.expected), false);
  });

  test("Job close ends an owned descendant and preserves an owned bystander", { timeout: 20000 }, async (t) => {
    const marker = path.join(fixtureRoot, "descendant.pid");
    const bystander = spawn(process.execPath, ["-e", "setTimeout(() => {}, 60000)"], {
      stdio: "ignore", windowsHide: true,
    });
    const bystanderExit = new Promise((resolve, reject) => {
      bystander.once("exit", resolve);
      bystander.once("error", reject);
    });
    const childSource = `
      const fs = require("node:fs");
      const { spawn } = require("node:child_process");
      const descendant = spawn(process.execPath, ["-e", "setTimeout(() => {}, 60000)"], { stdio: "ignore", windowsHide: true });
      descendant.unref();
      fs.writeFileSync(${JSON.stringify(marker)}, String(descendant.pid));
    `;
    const produced = createWindowsSupervisorPlan({ command: process.execPath, args: ["-e", childSource] });
    const supervisor = spawn(produced.command, produced.args, { stdio: ["ignore", "pipe", "pipe"], windowsHide: true });
    let stderr = "";
    supervisor.stderr.setEncoding("utf8").on("data", (chunk) => { stderr += chunk; });
    supervisor.stdout.resume();
    const supervisorExit = new Promise((resolve, reject) => {
      supervisor.once("exit", (code, signal) => resolve({ code, signal }));
      supervisor.once("error", reject);
    });
    t.after(async () => {
      // Address only ChildProcess handles created by this fixture. Killing the
      // supervisor closes its Job even if an assertion failed before root exit.
      if (supervisor.exitCode === null && supervisor.signalCode === null) supervisor.kill();
      if (bystander.exitCode === null && bystander.signalCode === null) bystander.kill();
      await Promise.all([supervisorExit, bystanderExit]);
    });

    const deadline = Date.now() + 10000;
    while (!fs.existsSync(marker)) {
      assert.ok(Date.now() < deadline, `owned descendant marker missing: ${stderr}`);
      assert.equal(supervisor.exitCode, null, stderr);
      await delay(25);
    }
    const descendantPid = Number(fs.readFileSync(marker, "utf8"));
    assert.ok(Number.isInteger(descendantPid) && descendantPid > 0);
    assert.deepEqual(await supervisorExit, { code: 0, signal: null }, stderr);

    const goneBy = Date.now() + 5000;
    let gone = false;
    while (Date.now() < goneBy) {
      try {
        process.kill(descendantPid, 0);
      } catch (error) {
        assert.equal(error.code, "ESRCH");
        gone = true;
        break;
      }
      await delay(25);
    }
    assert.ok(gone, "Job close must end the descendant reported by our owned root");
    assert.equal(bystander.exitCode, null, "the separately spawned bystander must survive");
    assert.equal(bystander.signalCode, null);
  });
});
