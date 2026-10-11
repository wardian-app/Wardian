import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { after, before, describe, test } from "node:test";
import { setTimeout as delay } from "node:timers/promises";
import { fileURLToPath } from "node:url";

import { createWindowsSupervisorPlan } from "./native-e2e-runner.mjs";

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
        return Int32.Parse(Environment.GetEnvironmentVariable("WARDIAN_SUPERVISOR_EXIT") ?? "0");
    }
}
`;

/** Execute the exact producer plan; only child fixture environment is supplied. */
function runSupervisor(plan, env) {
  const produced = createWindowsSupervisorPlan(plan);
  assert.equal(produced.command, "powershell.exe");
  assert.deepEqual(produced.args.slice(0, 4), ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"]);
  assert.equal(produced.args[4], supervisorScript);
  assert.equal(produced.args[produced.args.indexOf("-Executable") + 1], plan.command);
  assert.equal(produced.args[produced.args.indexOf("-ArgumentsJson") + 1], JSON.stringify(plan.args));
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
