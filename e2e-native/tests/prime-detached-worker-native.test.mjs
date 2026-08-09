/**
 * Proves the one Prime lifecycle claim no lower layer can reach: that tearing
 * down a Wardian-spawned Prime agent stops its resident daemon worker without
 * taking down the machine-wide supervisor.
 *
 * It cannot be a browser or unit test. Prime's `--print` and `--mode rpc`
 * clients request `client_owned` workers, which the supervisor hides from
 * every other client and reaps with the client, so they can never exercise
 * `prime-agent stop`. Only the interactive PTY spawn produces the `resident`
 * worker that outlives its client, and only the native harness can drive that
 * through Wardian's own teardown path.
 *
 * Safety: Prime's supervisor is machine-wide and shared with the developer's
 * own sessions. This test never calls `prime-agent shutdown`, never stops a
 * session it did not create, and identifies its own worker by the isolated
 * workspace it runs in.
 *
 * Writing it found two defects it now guards. Deserializing an AgentConfig
 * with `provider: "prime"` panicked inside `Deserialize`, which left the
 * command with no response at all and hung the caller instead of failing, so
 * no Prime agent could be created from the UI. And teardown could only address
 * a worker through an identity Prime does not publish until the agent's first
 * reply, so killing a fresh agent leaked its worker.
 */
import test from "node:test";
import assert from "node:assert/strict";
import path from "node:path";
import { existsSync, mkdirSync, writeFileSync } from "node:fs";
import { spawnSync } from "node:child_process";

import {
  createNativeHarness,
  ensureNativeAppBuilt,
  invokeTauri,
  prepareIsolatedHome,
  startNativeSession,
  waitForAppShell,
} from "../lib/harness.mjs";

const RUN_ID = `${process.pid}-${Date.now()}`;
const SESSION_NAME = `E2E-PRIME-DETACH-${RUN_ID}`;
const skipNativeBuild = process.env.WARDIAN_NATIVE_SKIP_BUILD === "1";
/**
 * Opt-in, following the WARDIAN_E2E_REAL_OPENCODE convention. This drives a
 * real provider against a daemon shared with whatever else is running on the
 * machine, so it does not belong in an unattended suite.
 */
const runRealPrime = process.env.WARDIAN_E2E_REAL_PRIME === "1";

function primeCli(args, options = {}) {
  return spawnSync(process.platform === "win32" ? "prime-agent.cmd" : "prime-agent", args, {
    encoding: "utf8",
    shell: process.platform === "win32",
    ...options,
  });
}

/** Sessions the supervisor is currently hosting. Never includes saved ones. */
function hostedSessions() {
  const result = primeCli(["list", "--json"]);
  if (result.status !== 0) return null;
  try {
    const parsed = JSON.parse(result.stdout);
    return Array.isArray(parsed.sessions) ? parsed.sessions : null;
  } catch {
    return null;
  }
}

function sessionsInWorkspace(sessions, workspace) {
  const target = path.resolve(workspace).toLowerCase();
  return sessions.filter(
    (session) => typeof session.cwd === "string" && path.resolve(session.cwd).toLowerCase() === target,
  );
}

function processAlive(pid) {
  if (!Number.isInteger(pid) || pid <= 0) return false;
  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    return error.code === "EPERM";
  }
}

async function waitFor(predicate, { timeoutMs = 30000, intervalMs = 500 } = {}) {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    const value = await predicate();
    if (value) return value;
    if (Date.now() > deadline) return null;
    await new Promise((resolve) => setTimeout(resolve, intervalMs));
  }
}

/**
 * Points the isolated run at an existing kernel environment.
 *
 * Prime cannot bootstrap its IPython kernel on Windows, so Wardian's readiness
 * gate refuses to launch without one. The isolated WARDIAN_HOME has no kernel
 * of its own, and building one here would make the test a package-install
 * test. Reuse a discoverable environment instead, or skip: a run that cannot
 * launch the provider proves nothing about lifecycle.
 */
function resolveKernelPython() {
  const configured = process.env.PRIME_AGENT_KERNEL_PYTHON;
  if (configured && existsSync(configured)) return configured;

  const home = process.env.HOME || process.env.USERPROFILE;
  if (!home) return null;
  const candidate = path.join(
    home,
    ".prime",
    "agent",
    "wardian-kernel-venv",
    ...(process.platform === "win32" ? ["Scripts", "python.exe"] : ["bin", "python"]),
  );

  return existsSync(candidate) ? candidate : null;
}

function primeIsUsable() {
  const version = primeCli(["--version"]);
  if (version.status !== 0) return false;
  return hostedSessions() !== null;
}

test("Wardian stops its own Prime worker and leaves the shared daemon running", async (t) => {
  if (!runRealPrime) {
    t.skip("Set WARDIAN_E2E_REAL_PRIME=1 to run real Prime Agent native E2E.");
    return;
  }
  if (!primeIsUsable()) {
    t.skip("prime-agent is not installed or its daemon is unreachable");
    return;
  }
  const kernelPython = resolveKernelPython();
  if (!kernelPython) {
    t.skip("no Prime kernel environment is available for the isolated home");
    return;
  }
  // Inherited by tauri-driver and therefore by the app under test.
  process.env.PRIME_AGENT_KERNEL_PYTHON = kernelPython;

  const harness = await createNativeHarness();
  if (!skipNativeBuild) {
    ensureNativeAppBuilt(harness);
  }
  prepareIsolatedHome(harness);

  // A workspace of its own, which is how this test identifies its worker
  // among the developer's live sessions.
  const workspace = path.join(harness.isolatedHome, "prime-detach-workspace");
  mkdirSync(workspace, { recursive: true });
  writeFileSync(path.join(workspace, "AGENTS.md"), "# Workspace\n", "utf8");

  const supervisorBefore = hostedSessions();
  assert.ok(supervisorBefore, "expected a readable daemon session list before spawning");
  assert.equal(
    sessionsInWorkspace(supervisorBefore, workspace).length,
    0,
    "the isolated workspace must start with no Prime sessions",
  );
  const foreignBefore = supervisorBefore
    .filter((session) => sessionsInWorkspace([session], workspace).length === 0)
    .map((session) => session.id)
    .sort();

  const { driver, close } = await startNativeSession(harness);
  let spawned;
  try {
    await waitForAppShell(driver);
    // Spawning Prime is slower than the default 30s async-script budget: the
    // client has to reach the supervisor, which may have to launch a worker
    // and boot an IPython kernel before the command returns.
    await driver.manage().setTimeouts({ script: 180000 });

    // SpawnAgentRequest is camelCase over IPC, unlike the snake_case
    // AgentConfig it carries.
    spawned = await invokeTauri(driver, "spawn_agent", {
      req: {
        sessionName: SESSION_NAME,
        agentClass: "",
        folder: workspace,
        resumeSession: null,
        isOff: false,
        configOverride: { provider: "prime" },
      },
    });
    assert.ok(spawned?.session_id, "spawn_agent must return the created agent");

    // The interactive client registers a resident worker with the supervisor;
    // that registration is what teardown then has to undo.
    const worker = await waitFor(() => {
      const sessions = hostedSessions();
      return sessions ? sessionsInWorkspace(sessions, workspace)[0] ?? null : null;
    });
    assert.ok(
      worker,
      "Prime should have registered a daemon session for the test workspace",
    );
    assert.equal(worker.rlmDepth ?? 0, 0, "the spawned session should be a root");

    await invokeTauri(driver, "kill_agent", { sessionId: spawned.session_id });
    spawned = null;

    const gone = await waitFor(() => {
      const sessions = hostedSessions();
      return sessions && sessionsInWorkspace(sessions, workspace).length === 0;
    });
    assert.ok(
      gone,
      "the worker must be stopped by teardown; closing the PTY alone only detaches it",
    );

    // The whole point of suppressing the process-tree kill: everything else on
    // the machine survives.
    const supervisorAfter = hostedSessions();
    assert.ok(
      supervisorAfter,
      "the supervisor must still answer after Wardian tore down its own agent",
    );
    const foreignAfter = supervisorAfter.map((session) => session.id).sort();
    for (const id of foreignBefore) {
      assert.ok(
        foreignAfter.includes(id),
        `session ${id} was not Wardian's and must survive its teardown`,
      );
    }
    for (const session of supervisorBefore) {
      if (Number.isInteger(session.workerPid)) {
        assert.ok(
          processAlive(session.workerPid),
          `worker ${session.workerPid} was not Wardian's and must still be running`,
        );
      }
    }
  } finally {
    // Never leave a worker behind, including when an assertion failed before
    // the teardown step ran.
    if (spawned?.session_id) {
      await invokeTauri(driver, "kill_agent", { sessionId: spawned.session_id }).catch(
        () => undefined,
      );
    }
    const sessions = hostedSessions();
    for (const leftover of sessions ? sessionsInWorkspace(sessions, workspace) : []) {
      primeCli(["stop", leftover.id, "--json"]);
    }
    await close();
  }
});

// @native-only
// Kernel provisioning writes a real virtualenv under the app's WARDIAN_HOME, so
// only the native layer can prove it. Left unwritten deliberately: this harness
// exports PRIME_AGENT_KERNEL_PYTHON before launching the app, which short-
// circuits the discovery path the provisioner exists to populate. Proving it
// needs a variant harness that withholds that variable and budgets for a real
// package install.
//
// Covered meanwhile by unit tests over the staging/verify/publish steps in
// `src-tauri/src/providers/prime_kernel.rs`, and by a manual run recorded in
// docs/specs/2026-08-05-prime-agent-provider.md.
test.skip("Wardian provisions Prime's IPython kernel into an isolated home", () => {});
