import fs from "node:fs";
import os from "node:os";
import path from "node:path";

export const NATIVE_E2E_HOME_ENV = "WARDIAN_E2E_NATIVE_HOME";
export const NATIVE_E2E_RUN_ID_ENV = "WARDIAN_E2E_RUN_ID";
export const NATIVE_E2E_HOME_PREFIX = "wardian-e2e-native-";
export const HOME_LOCK_DIRECTORY = ".native-e2e-lock";
export const HOME_LOCK_FILE = `${HOME_LOCK_DIRECTORY}/owner.json`;

/** Identifies one run so its home, ports and children are attributable. */
export function nativeRunId() {
  return `${process.pid.toString(36)}${Math.random().toString(36).slice(2, 8)}`;
}

export function defaultNativeE2EHome(runId) {
  return path.join(os.tmpdir(), `${NATIVE_E2E_HOME_PREFIX}${runId}`);
}

/**
 * The home this run owns.
 *
 * An explicit home is honored so a caller can inspect state afterwards.
 * Otherwise each run gets its own, which is what lets two runs coexist.
 */
export function resolveRunNativeHome(env = process.env) {
  // A run always has an id, including when the home was supplied explicitly.
  // The id, not the pid, is what identifies a run: the runner claims the home
  // and then spawns a child that must recognise the claim as its own.
  const runId = env[NATIVE_E2E_RUN_ID_ENV] || nativeRunId();
  if (env[NATIVE_E2E_HOME_ENV]) {
    return { home: env[NATIVE_E2E_HOME_ENV], generated: false, runId };
  }
  return { home: defaultNativeE2EHome(runId), generated: true, runId };
}

/**
 * A lock holder is only real while its process is still alive.
 *
 * This deliberately does not exempt the calling process. Whether a lock belongs
 * to the caller is decided by run id, not pid, because one run spans the runner
 * and the child it spawns. Exempting our own pid here would hide a live holder
 * from a genuinely different run in the same process.
 */
export function lockHolderAlive(pid) {
  if (!Number.isInteger(pid) || pid <= 0) {
    return false;
  }
  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    // EPERM means the process exists but is owned by someone else.
    return Boolean(error) && error.code === "EPERM";
  }
}

export function readHomeLock(homePath) {
  try {
    return JSON.parse(fs.readFileSync(path.join(homePath, HOME_LOCK_FILE), "utf8"));
  } catch {
    return null;
  }
}

function lockDirectoryPath(home) {
  return path.join(home, HOME_LOCK_DIRECTORY);
}

function staleLockPath(home, runId) {
  return path.join(home, `${HOME_LOCK_DIRECTORY}.stale-${process.pid}-${runId ?? "unknown"}`);
}

/**
 * Claim a home before anything destructive touches it.
 *
 * Ownership has to be established here, in the runner, rather than later when
 * the harness resets the directory. Cleanup runs before the test child starts,
 * so a refusal that only happens inside the harness arrives after the damage:
 * a second run pointed at the same explicit home would already have terminated
 * the first run's processes.
 *
 * Returns the lock this run now holds. Throws if a live foreign holder exists.
 */
export function acquireHomeLock({ home, runId, pid = process.pid }) {
  fs.mkdirSync(home, { recursive: true });
  const lockPath = lockDirectoryPath(home);

  // The directory creation is the ownership claim. Unlike read-then-write of
  // a JSON file, mkdir is one OS-level exclusive operation, so two runners
  // cannot both pass the check and then reset the same home.
  for (let attempt = 0; attempt < 5; attempt += 1) {
    try {
      fs.mkdirSync(lockPath);
      const lock = {
        runId: runId ?? null,
        pid,
        startedAt: new Date().toISOString(),
      };
      fs.writeFileSync(path.join(home, HOME_LOCK_FILE), `${JSON.stringify(lock)}\n`);
      return { lock, reclaimed: false, staleHolder: null };
    } catch (error) {
      if (error?.code !== "EEXIST") {
        throw error;
      }
    }

    const existing = readHomeLock(home);
    // A run spans more than one process: the runner claims the home, then
    // spawns a child that opens the session. Identity is therefore the run id.
    // A missing or malformed record means a claim is in progress; fail closed
    // rather than deleting a lock another runner may just have created.
    if (!existing || existing.runId == null) {
      throw new Error(`Refusing to use native E2E home ${home}: its exclusive lock is active but unreadable.`);
    }
    if (existing.runId === runId) {
      return { lock: existing, reclaimed: true, staleHolder: null };
    }
    if (lockHolderAlive(existing.pid)) {
      throw new Error(
        `Refusing to use native E2E home ${home}: run ${existing.runId ?? "unknown"} (pid ${existing.pid}, ` +
          `started ${existing.startedAt ?? "unknown"}) is still using it. Give each concurrent run its own ` +
          `${NATIVE_E2E_HOME_ENV}, or leave it unset to get an isolated home automatically.`,
      );
    }

    // Reclaim stale ownership by moving the lock directory first. Rename is
    // exclusive here: only one stale-run reclaimer can win, while a live run's
    // directory is never removed merely because another reader saw old JSON.
    const stalePath = staleLockPath(home, runId);
    try {
      fs.renameSync(lockPath, stalePath);
      fs.rmSync(stalePath, { recursive: true, force: true });
      const acquired = acquireHomeLock({ home, runId, pid });
      return { ...acquired, staleHolder: existing };
    } catch (reclaimError) {
      if (reclaimError?.code !== "ENOENT" && reclaimError?.code !== "EEXIST" && reclaimError?.code !== "EPERM") {
        throw reclaimError;
      }
    }
  }
  throw new Error(`Could not acquire exclusive native E2E home lock for ${home}; another run is claiming it.`);
}

/** Release this run's claim; never remove a lock another run owns. */
export function releaseHomeLock({ home, runId, pid = process.pid }) {
  const lock = readHomeLock(home);
  const ours = lock && (runId != null ? lock.runId === runId : lock.pid === pid);
  if (ours) {
    try {
      // The directory is the lock and the owner metadata lives inside it, so
      // removing the directory releases both atomically from a new claimant's
      // point of view.
      fs.rmSync(lockDirectoryPath(home), { recursive: true, force: true });
    } catch {
      // A home already removed by teardown needs no release.
    }
  }
}
