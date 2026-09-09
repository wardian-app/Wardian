// @tier nightly — Runs on the nightly schedule; too slow or too broad for every pull request.
import test from "node:test";
import assert from "node:assert/strict";
import os from "node:os";
import path from "node:path";

import fs from "node:fs";
import { spawn } from "node:child_process";
import { setTimeout as delay } from "node:timers/promises";

import {
  createNativeE2eRunPlans,
  createOwnedTreeTerminationPlan,
  createWindowsSupervisorPlan,
  resolveRunNativeHome,
} from "../../scripts/native-e2e-runner.mjs";
import { HOME_LOCK_DIRECTORY, HOME_LOCK_FILE, acquireHomeLock, readHomeLock, releaseHomeLock } from "../lib/sessionHome.mjs";

test("each run without an explicit home gets its own, so concurrent runs cannot reset each other", () => {
  const first = resolveRunNativeHome({});
  const second = resolveRunNativeHome({});

  assert.notEqual(first.home, second.home);
  assert.equal(first.generated, true);
  for (const resolved of [first, second]) {
    assert.equal(path.dirname(resolved.home), path.resolve(os.tmpdir()));
    assert.ok(
      path.basename(resolved.home).startsWith("wardian-e2e-native-"),
      `${resolved.home} must keep the prefix the reset guard requires`,
    );
  }
});
test("an explicit home is honored verbatim and never silently replaced", () => {
  const explicit = path.join(os.tmpdir(), "wardian-e2e-native-explicit");
  const resolved = resolveRunNativeHome({ WARDIAN_E2E_NATIVE_HOME: explicit });

  assert.equal(resolved.home, explicit);
  assert.equal(resolved.generated, false);
});

// The command-line sweep these tests used to assert is deliberately gone. It
// enumerated every process whose command line contained the home path and
// force-stopped it, which matched by coincidence rather than ownership.
// Cleanup is now limited to the process tree the runner itself started.

test("cleanup addresses only a process tree this runner started", () => {
  assert.equal(createOwnedTreeTerminationPlan(4321, "win32"), null);
  assert.deepEqual(createOwnedTreeTerminationPlan(4321, "linux"), {
    command: "kill",
    args: ["-TERM", "-4321"],
  });
  // No pid means nothing is ours to end, so nothing is terminated.
  for (const invalid of [undefined, null, 0, -1, "1234"]) {
    assert.equal(createOwnedTreeTerminationPlan(invalid, "win32"), null);
  }
});

test("an unrelated process naming the home survives POSIX owned-tree cleanup", { skip: process.platform === "win32" }, async () => {
  const { home } = resolveRunNativeHome({});
  fs.mkdirSync(home, { recursive: true });

  // A bystander whose command line contains the home path. The old sweep
  // matched exactly this and killed it; owned-tree cleanup must not.
  const bystander = spawn(
    process.execPath,
    ["-e", `setTimeout(() => {}, 60000); process.title = ${JSON.stringify(home)};`],
    { stdio: "ignore" },
  );
  try {
    await delay(300);
    assert.equal(bystander.exitCode, null, "bystander should be running before cleanup");

    // Terminate a tree that is not the bystander, the way a run ends its own child.
    const owned = spawn(process.execPath, ["-e", "setTimeout(() => {}, 60000);"], { stdio: "ignore" });
    await delay(200);
    const plan = createOwnedTreeTerminationPlan(owned.pid);
    assert.ok(plan, "an owned pid must produce a termination plan");
    owned.kill();
    await delay(300);

    assert.equal(bystander.exitCode, null, "an unrelated process must survive owned-tree cleanup");
  } finally {
    bystander.kill();
    fs.rmSync(home, { recursive: true, force: true });
  }
});

test("a second run refuses a live explicit home instead of clearing it", () => {
  const home = path.join(os.tmpdir(), `wardian-e2e-native-lock-${process.pid}`);
  fs.rmSync(home, { recursive: true, force: true });
  try {
    acquireHomeLock({ home, runId: "run-a", pid: process.pid });
    const held = readHomeLock(home);
    assert.equal(held.runId, "run-a");

    // A different, live pid holding the same explicit home. Refusal has to
    // happen here, before any destructive step, or the second run terminates
    // the first run's processes and wipes its home first.
    fs.writeFileSync(
      path.join(home, HOME_LOCK_FILE),
      JSON.stringify({ runId: "run-a", pid: process.ppid, startedAt: new Date().toISOString() }),
    );
    assert.throws(
      () => acquireHomeLock({ home, runId: "run-b", pid: process.pid }),
      /Refusing to use native E2E home/,
      "the second run must refuse a home a live run still holds",
    );

    // The refusal must leave the first run's claim intact.
    assert.equal(readHomeLock(home).runId, "run-a");
  } finally {
    releaseHomeLock({ home, pid: process.ppid });
    fs.rmSync(home, { recursive: true, force: true });
  }
});

/**
 * A run spans the runner and the test child it spawns. The runner claims the
 * home first, so the child meets a lock held by a different, very much alive
 * pid. Keying ownership on the pid made every run refuse its own home; both
 * runners failed identically in the first two-runner acceptance.
 */
test("the child of a run reclaims the home its own runner already locked", () => {
  const home = path.join(os.tmpdir(), `wardian-e2e-native-reclaim-${process.pid}`);
  fs.rmSync(home, { recursive: true, force: true });
  try {
    // The runner claims it under the runner's pid.
    const runnerClaim = acquireHomeLock({ home, runId: "run-x", pid: process.ppid });
    assert.equal(runnerClaim.lock.runId, "run-x");

    // The child is a different live process, but the same run.
    const childClaim = acquireHomeLock({ home, runId: "run-x", pid: process.pid });
    assert.equal(childClaim.reclaimed, true, "same run id must reclaim, not refuse");
    assert.equal(childClaim.lock.runId, "run-x");
    assert.equal(
      childClaim.lock.startedAt,
      runnerClaim.lock.startedAt,
      "the run keeps its original start time across processes",
    );

    // A genuinely different run is still refused.
    assert.throws(
      () => acquireHomeLock({ home, runId: "run-y", pid: process.ppid }),
      /Refusing to use native E2E home/,
    );
  } finally {
    releaseHomeLock({ home, runId: "run-x" });
    fs.rmSync(home, { recursive: true, force: true });
  }
});

test("a stale lock from a dead run is reported, not treated as a live holder", () => {
  const home = path.join(os.tmpdir(), `wardian-e2e-native-stale-${process.pid}`);
  fs.rmSync(home, { recursive: true, force: true });
  fs.mkdirSync(home, { recursive: true });
  fs.mkdirSync(path.join(home, HOME_LOCK_DIRECTORY));
  try {
    // Pid 0 is never a live holder, so this stands in for a crashed run.
    fs.writeFileSync(
      path.join(home, HOME_LOCK_FILE),
      JSON.stringify({ runId: "crashed", pid: 0, startedAt: new Date().toISOString() }),
    );
    const { staleHolder } = acquireHomeLock({ home, runId: "run-c", pid: process.pid });
    assert.equal(staleHolder.runId, "crashed");
    assert.equal(readHomeLock(home).runId, "run-c");
  } finally {
    fs.rmSync(home, { recursive: true, force: true });
  }
});

test("native e2e runner isolates each test target in a separate node process", () => {
  const plans = createNativeE2eRunPlans({
    requestedTargets: [],
    defaultTargets: [
      "e2e-native/tests/alpha.test.mjs",
      "e2e-native/tests/beta.test.mjs",
    ],
  });

  assert.deepEqual(plans, [
    {
      command: process.execPath,
      args: ["--test", "--test-concurrency=1", "e2e-native/tests/alpha.test.mjs"],
    },
    {
      command: process.execPath,
      args: ["--test", "--test-concurrency=1", "e2e-native/tests/beta.test.mjs"],
    },
  ]);
});

test("native e2e runner preserves explicitly requested target ordering", () => {
  const plans = createNativeE2eRunPlans({
    requestedTargets: [
      "e2e-native/tests/worktree-cli-native.test.mjs",
      "e2e-native/tests/cli-shared-state-native.test.mjs",
    ],
    defaultTargets: ["e2e-native/tests/alpha.test.mjs"],
  });

  assert.deepEqual(
    plans.map((plan) => plan.args.at(-1)),
    [
      "e2e-native/tests/worktree-cli-native.test.mjs",
      "e2e-native/tests/cli-shared-state-native.test.mjs",
    ],
  );
});

test("Windows supervision assigns the root before it can create descendants", () => {
  const plan = createWindowsSupervisorPlan(
    { command: process.execPath, args: ["--version"] },
    "win32",
  );
  assert.equal(plan.command, "powershell.exe");
  assert.deepEqual(plan.args.slice(0, 5), [
    "-NoProfile",
    "-ExecutionPolicy",
    "Bypass",
    "-File",
    expectPath(plan.args[4]),
  ]);
  assert.equal(plan.args[5], "-Executable");
  assert.equal(plan.args[6], process.execPath);
  assert.equal(plan.args[7], "-ArgumentsJson");
  assert.equal(plan.args[8], JSON.stringify(["--version"]));
});

function expectPath(value) {
  assert.match(value, /native-e2e-windows-supervisor\.ps1$/i);
  return value;
}

test("simultaneous explicit-home starts have one atomic owner", { timeout: 15000 }, async () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "wardian-native-lock-race-"));
  const home = path.join(root, "home");
  const gate = path.join(root, "start");
  const release = path.join(root, "release");
  const moduleUrl = new URL("../lib/sessionHome.mjs", import.meta.url).href;
  const childSource = `
    import fs from "node:fs";
    import { acquireHomeLock, releaseHomeLock } from ${JSON.stringify(moduleUrl)};
    const [targetHome, startGate, readyPath, resultPath, runId, releaseGate] = process.argv.slice(1);
    fs.writeFileSync(readyPath, "ready");
    while (!fs.existsSync(startGate)) {}
    try {
      const claim = acquireHomeLock({ home: targetHome, runId, pid: process.pid });
      fs.writeFileSync(resultPath, JSON.stringify({ ok: true, runId, reclaimed: claim.reclaimed }));
      while (!fs.existsSync(releaseGate)) {}
      releaseHomeLock({ home: targetHome, runId, pid: process.pid });
    } catch (error) {
      fs.writeFileSync(resultPath, JSON.stringify({ ok: false, message: String(error.message) }));
    }
  `;
  const children = ["run-a", "run-b"].map((runId) => {
    const ready = path.join(root, `${runId}.ready`);
    const result = path.join(root, `${runId}.json`);
    const child = spawn(
      process.execPath,
      ["--input-type=module", "-e", childSource, home, gate, ready, result, runId, release],
      { stdio: "ignore" },
    );
    const exit = new Promise((resolve, reject) => {
      child.once("exit", (code) => (code === 0 ? resolve() : reject(new Error(`child exited ${code}`))));
      child.once("error", reject);
    });
    return { child, ready, result, exit };
  });
  const waitForFiles = async (files) => {
    const deadline = Date.now() + 5000;
    while (files.some((file) => !fs.existsSync(file))) {
      if (Date.now() >= deadline) throw new Error(`Timed out waiting for ${files.join(", ")}`);
      await delay(25);
    }
  };
  try {
    await waitForFiles(children.map(({ ready }) => ready));
    fs.writeFileSync(gate, "go");
    await waitForFiles(children.map(({ result }) => result));
    const outcomes = children.map(({ result }) => JSON.parse(fs.readFileSync(result, "utf8")));
    assert.equal(outcomes.filter(({ ok }) => ok).length, 1, JSON.stringify(outcomes));
    assert.equal(outcomes.filter(({ ok }) => !ok).length, 1, JSON.stringify(outcomes));
    assert.match(outcomes.find(({ ok }) => !ok).message, /Refusing to use native E2E home/);
    fs.writeFileSync(release, "done");
    await Promise.all(children.map(({ exit }) => exit));
  } finally {
    for (const { child } of children) child.kill();
    fs.rmSync(root, { recursive: true, force: true });
  }
});

test("Windows Job Object cleanup removes a grandchild after root exit", { skip: process.platform !== "win32", timeout: 15000 }, async () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "wardian-native-job-"));
  const marker = path.join(root, "grandchild.pid");
  const bystander = spawn(process.execPath, ["-e", "setTimeout(() => {}, 60000);"], { stdio: "ignore" });
  const childSource = `
    const fs = require("node:fs");
    const { spawn } = require("node:child_process");
    const grandchild = spawn(process.execPath, ["-e", "setTimeout(() => {}, 60000);"], { stdio: "ignore" });
    grandchild.unref();
    fs.writeFileSync(${JSON.stringify(marker)}, String(grandchild.pid));
  `;
  const supervisor = spawn(
    createWindowsSupervisorPlan({ command: process.execPath, args: ["-e", childSource] }, "win32").command,
    createWindowsSupervisorPlan({ command: process.execPath, args: ["-e", childSource] }, "win32").args,
    { stdio: "ignore" },
  );
  try {
    const deadline = Date.now() + 5000;
    while (!fs.existsSync(marker)) {
      if (Date.now() >= deadline) throw new Error("supervised root did not start");
      await delay(25);
    }
    const grandchildPid = Number(fs.readFileSync(marker, "utf8"));
    await new Promise((resolve, reject) => {
      supervisor.once("exit", resolve);
      supervisor.once("error", reject);
    });
    const goneBy = Date.now() + 5000;
    while (Date.now() < goneBy) {
      try {
        process.kill(grandchildPid, 0);
      } catch {
        break;
      }
      await delay(50);
    }
    assert.throws(() => process.kill(grandchildPid, 0));
    assert.equal(bystander.exitCode, null, "a foreign bystander must survive job cleanup");
  } finally {
    supervisor.kill();
    bystander.kill();
    fs.rmSync(root, { recursive: true, force: true });
  }
});

// Keep the file terminator explicit for cross-platform checkout normalization.
