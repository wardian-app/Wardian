import { spawn, spawnSync } from "node:child_process";
import path from "node:path";
import { fileURLToPath } from "node:url";

import {
  NATIVE_E2E_HOME_ENV,
  NATIVE_E2E_RUN_ID_ENV,
  acquireHomeLock,
  releaseHomeLock,
  resolveRunNativeHome,
} from "../e2e-native/lib/sessionHome.mjs";

const NODE_TEST_ARGS = ["--test", "--test-concurrency=1"];
const WINDOWS_SUPERVISOR_SCRIPT = fileURLToPath(new URL("./native-e2e-windows-supervisor.ps1", import.meta.url));

export { resolveRunNativeHome };

export function createNativeE2eRunPlans({ requestedTargets, defaultTargets }) {
  const targets = requestedTargets.length > 0 ? requestedTargets : defaultTargets;
  return targets.map((target) => ({
    command: process.execPath,
    args: [...NODE_TEST_ARGS, target],
  }));
}

function runChild(plan, env) {
  return new Promise((resolve) => {
    const spawnPlan = process.platform === "win32" ? createWindowsSupervisorPlan(plan) : plan;
    const child = spawn(spawnPlan.command, spawnPlan.args, {
      stdio: "inherit",
      env,
      // A process group makes the whole tree addressable on POSIX, so cleanup
      // can end exactly what this runner started and nothing else.
      detached: process.platform !== "win32",
    });
    const pid = child.pid;

    child.on("exit", (code, signal) => {
      resolve({ result: { code: code ?? 1, signal }, pid });
    });
  });
}

/**
 * Start a Windows child suspended, assign it to a kill-on-close Job Object,
 * then resume it. The supervisor owns the job for the whole child lifetime;
 * closing it after the root exits terminates descendants that outlived their
 * parent instead of relying on a post-exit PID tree walk.
 */
export function createWindowsSupervisorPlan(plan, platform = process.platform) {
  if (platform !== "win32") {
    return plan;
  }
  return {
    command: "powershell.exe",
    args: [
      "-NoProfile",
      "-ExecutionPolicy",
      "Bypass",
      "-File",
      path.resolve(WINDOWS_SUPERVISOR_SCRIPT),
      "-Executable",
      plan.command,
      "-ArgumentsJson",
      JSON.stringify(plan.args),
    ],
  };
}

/**
 * Terminate one process tree this runner started.
 *
 * This replaces a sweep that enumerated every process whose command line
 * contained the home path and force-stopped it. That matched by coincidence,
 * not ownership: an unrelated process that merely mentions the path was killed,
 * and two runs sharing an explicit home killed each other. Only the tree rooted
 * at a pid this runner spawned is ours to end.
 */
export function createOwnedTreeTerminationPlan(pid, platform = process.platform) {
  if (!Number.isInteger(pid) || pid <= 0) {
    return null;
  }
  if (platform === "win32") {
    // Windows runs are supervised by a Job Object from process creation. A
    // post-exit taskkill/PID walk is intentionally unavailable because the
    // root may already be gone while descendants remain alive.
    return null;
  }
  return { command: "kill", args: ["-TERM", `-${pid}`] };
}

function terminateOwnedTree(pid, platform = process.platform) {
  if (platform === "win32") {
    // The Windows supervisor closes its kill-on-close Job Object when the
    // supervised root exits, which is the only reliable ownership boundary.
    return;
  }
  const plan = createOwnedTreeTerminationPlan(pid, platform);
  if (!plan) {
    return;
  }
  try {
    spawnSync(plan.command, plan.args, { stdio: "ignore" });
  } catch {
    // The tree is already gone; nothing else is ours to end.
  }
}

export async function runNativeE2eTargets({
  requestedTargets,
  defaultTargets,
  env = process.env,
}) {
  const plans = createNativeE2eRunPlans({ requestedTargets, defaultTargets });
  // Pin the home once, before any target runs, and hand the same value to every
  // child so the runner and the harness never disagree about which home is in
  // play. Isolation therefore applies to the ordinary npm path with no manual
  // port or home selection.
  const { home, runId } = resolveRunNativeHome(env);

  // Claim the home BEFORE anything destructive happens. Cleanup used to run
  // ahead of the test child, so a refusal raised later inside the harness
  // arrived after a second run had already terminated the first run's
  // processes. Refusing here is what makes the guarantee real.
  const { staleHolder } = acquireHomeLock({ home, runId });
  if (staleHolder) {
    console.warn(
      `[native-e2e] Reusing ${home}; it was left locked by run ${staleHolder.runId ?? "unknown"} ` +
        `(pid ${staleHolder.pid}), which is no longer running. Processes orphaned by that run are not ` +
        `terminated automatically, because they cannot be told apart from unrelated processes.`,
    );
  }

  // The run id always reaches the child, including for an explicit home. The
  // child has to recognise the runner's claim as its own, and identity is the
  // run id rather than a pid that differs between runner and child.
  const runEnv = { ...env, [NATIVE_E2E_HOME_ENV]: home, [NATIVE_E2E_RUN_ID_ENV]: runId };

  try {
    for (const plan of plans) {
      const { result, pid } = await runChild(plan, runEnv);
      // End only the tree we started, and only after it has exited, so a child
      // that leaked the app or driver cannot outlive the run.
      terminateOwnedTree(pid);
      if (result.signal) {
        process.kill(process.pid, result.signal);
        return 1;
      }
      if (result.code !== 0) {
        return result.code;
      }
    }
  } finally {
    releaseHomeLock({ home, runId });
  }

  return 0;
}
