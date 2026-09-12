import fs from "node:fs";
import path from "node:path";
import { HOME_LOCK_DIRECTORY, readHomeLock, releaseHomeLock } from "./sessionHome.mjs";

/** WebDriver deletion must succeed and the owned driver must have actually exited. */
export async function closeConformanceSession(session) {
  await session.close();
  const child = session.tauriDriver;
  if (!child || !(Number.isInteger(child.exitCode)
    || (typeof child.signalCode === "string" && child.signalCode.length > 0))) {
    throw new Error("Owned native driver exit is unconfirmed; retain the home lock for supervised cleanup");
  }
}

/**
 * Final cleanup for isolated provider conformance suites. Run real suites through the native runner:
 * its process-tree supervisor remains the boundary for failed/uncertain startup.
 * A report error cannot prevent pause/close; uncertain cleanup never releases a lock.
 */
export async function cleanupConformanceSession({
  harness, session, startupAttempted, pause, save,
}) {
  const errors = [];
  let shutdownConfirmed = !startupAttempted && !session;
  let pauseConfirmed = true;
  if (session) {
    try { await pause(); }
    catch (error) { errors.push(error); pauseConfirmed = false; }
    try {
      await closeConformanceSession(session);
      shutdownConfirmed = pauseConfirmed;
    } catch (error) { errors.push(error); }
  } else if (startupAttempted) {
    errors.push(new Error("Native startup was attempted without a returned session; retain the lock for the supervisor"));
  }

  let homeLockReleased = false;
  if (shutdownConfirmed) {
    try {
      const lockDirectory = path.join(harness.isolatedHome, HOME_LOCK_DIRECTORY);
      const lock = readHomeLock(harness.isolatedHome);
      if (fs.existsSync(lockDirectory)) {
        if (!lock || lock.runId !== harness.runId || lock.pid !== process.pid) {
          throw new Error("Suite home lock ownership changed or is unreadable; refusing release");
        }
        releaseHomeLock({ home: harness.isolatedHome, runId: harness.runId });
        if (fs.existsSync(lockDirectory)) throw new Error("Suite home lock release was not observed");
      }
      homeLockReleased = true;
    } catch (error) { errors.push(error); }
  }
  const cleanup = { shutdown_confirmed: shutdownConfirmed, home_lock_released: homeLockReleased };
  // Save last, so even a rejected write cannot bypass any shutdown operation.
  try { await save(cleanup); }
  catch (error) { errors.push(error); }
  if (errors.length) {
    const error = new AggregateError(errors, "Conformance cleanup or report persistence failed");
    error.cleanupConfirmed = shutdownConfirmed && homeLockReleased;
    throw error;
  }
  return cleanup;
}

/** Pause every agent in this suite's isolated app, including off/native owners.
 * Attempt all captured IDs even if one rejects; never interpret a partial roster
 * or a failed pause as proof that provider work stopped. `invoke` is injectable
 * for deterministic tests and must reject IPC errors (not return error envelopes).
 */
export async function pauseConformanceAgents(invoke) {
  const agents = await invoke("list_agents");
  if (!Array.isArray(agents) || agents.some((agent) =>
    typeof agent?.session_id !== "string" || !agent.session_id)) {
    throw new Error("Owned agent roster unavailable; shutdown remains unconfirmed");
  }
  const results = await Promise.allSettled(agents.map((agent) =>
    invoke("pause_agent", { sessionId: agent.session_id })));
  const errors = results.filter((result) => result.status === "rejected").map((result) => result.reason);
  if (errors.length) throw new AggregateError(errors, "Owned agents did not all confirm pause");
}
