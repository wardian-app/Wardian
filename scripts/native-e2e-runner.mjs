import { spawn, spawnSync } from "node:child_process";
import fs from "node:fs";
import { createHash } from "node:crypto";
import path from "node:path";
import { fileURLToPath } from "node:url";

import {
  NATIVE_E2E_HOME_ENV,
  NATIVE_E2E_RUN_ID_ENV,
  acquireHomeLock,
  releaseHomeLock,
  resolveRunNativeHome,
  readHomeLock,
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

function runChild(plan, env, receipt) {
  return new Promise((resolve) => {
    const spawnPlan = process.platform === "win32" ? createWindowsSupervisorPlan(plan, "win32", receipt) : plan;
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
export function createWindowsSupervisorPlan(plan, platform = process.platform, receipt) {
  if (platform !== "win32") {
    return plan;
  }
  const produced = {
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
  if (receipt) {
    if (!path.isAbsolute(receipt.path) || typeof receipt.runId !== "string" || !receipt.runId
      || receipt.runId.length > 256 || /[\x00-\x1f]/.test(receipt.runId)
      || !Number.isSafeInteger(receipt.targetIndex) || receipt.targetIndex < 0 || receipt.targetIndex > 2147483647) {
      throw new Error("Invalid inner-supervisor receipt binding");
    }
    const targetSha256 = createHash("sha256").update(plan.command).update("\0").update(JSON.stringify(plan.args)).digest("hex");
    produced.args.push("-ReceiptPath", receipt.path, "-ReceiptRunId", receipt.runId,
      "-ReceiptTargetIndex", String(receipt.targetIndex), "-ReceiptTargetSha256", targetSha256);
  }
  return produced;
}

/** Optional diagnostics use a caller-owned fresh output directory outside the
 * resettable home. The existing harness resets home; lock release removes its lock. */
export function createWindowsSupervisorReceipts({ directory, home, runId, plans, platform = process.platform }) {
  if (!directory || platform !== "win32") return plans.map(() => undefined);
  const root = path.resolve(home);
  const output = path.resolve(directory);
  const relative = path.relative(root, output);
  if (!path.isAbsolute(directory) || !relative || (!path.isAbsolute(relative) && !relative.split(path.sep).includes(".."))
    || readHomeLock(root)?.runId !== runId || path.resolve(fs.realpathSync(root)) !== root) {
    throw new Error("Inner-supervisor receipts require the current owned run and fresh output outside its resettable home");
  }
  // The grant supplies the owned output parent. Create only its new final
  // directory, exclusively, and refuse any link on that exact output route.
  let current = path.dirname(output);
  while (true) {
    if (!fs.lstatSync(current).isDirectory() || fs.lstatSync(current).isSymbolicLink()) {
      throw new Error("Inner-supervisor receipt directory traverses a link");
    }
    const parent = path.dirname(current);
    if (parent === current) break;
    current = parent;
  }
  fs.mkdirSync(output);
  return plans.map((_, targetIndex) => ({ path: path.join(output, `target-${targetIndex}.json`), runId, targetIndex }));
}

/** Necessary inner evidence from a caller-proven fresh file only. The matching
 * outer phase/root/EOF/Job receipt is still required; this never grants PASS. */
export function innerSupervisorReceiptIsClean(receipt, expected) {
  return Boolean(receipt && expected && receipt.schema === 1 && receipt.kind === "native-e2e-inner-supervisor"
    && typeof receipt.run_id === "string" && receipt.run_id.length > 0 && receipt.run_id.length <= 256
    && !/[\x00-\x1f]/.test(receipt.run_id)
    && receipt.run_id === expected.runId && Number.isSafeInteger(receipt.target_index) && receipt.target_index >= 0
    && receipt.target_index === expected.targetIndex
    && receipt.target_sha256 === expected.targetSha256 && /^[a-f0-9]{64}$/.test(receipt.target_sha256)
    && receipt.writer_finished === true && receipt.observations_complete === true && receipt.run_returned === true
    && receipt.error_type === null && receipt.job_assigned === true && receipt.root_resumed === true
    && Number.isSafeInteger(receipt.root_pid) && receipt.root_pid > 0
    && typeof receipt.root_birth_filetime === "string" && /^[1-9][0-9]{0,19}$/.test(receipt.root_birth_filetime)
    && typeof receipt.root_birth_utc === "string" && /^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d{7}Z$/.test(receipt.root_birth_utc)
    && receipt.birth_error === null && receipt.wait_result === 0 && receipt.wait_error === null
    && receipt.joined === true && receipt.root_exit_code === 0 && receipt.root_terminate_before_join === false
    && receipt.job_query_error === null && receipt.active_processes_before_close === 0
    && receipt.kill_on_close_residue_requested === false && receipt.job_close_called === true
    && receipt.job_close_succeeded === true && receipt.stdio_eof === "not_observed_inherited");
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
    const receipts = createWindowsSupervisorReceipts({ directory: env.WARDIAN_NATIVE_INNER_RECEIPT_DIRECTORY,
      home, runId, plans });
    for (const [index, plan] of plans.entries()) {
      const { result, pid } = await runChild(plan, runEnv, receipts[index]);
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
