// @tier nightly — Runs on the nightly schedule; too slow or too broad for every pull request.
//
// Acceptance fixture for issue #1222. Not a normal assertion test: it opens one
// native session through the ordinary runner path, publishes what that run
// claimed, and holds the session open until told to stop. Two of these, started
// by two separate runner processes in two separate worktrees, are what prove
// independent runs.
//
// Driven by WARDIAN_ISOLATION_ACCEPTANCE_DIR and WARDIAN_ISOLATION_LABEL.
// Without those it skips, so ordinary suites are unaffected.
import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { setTimeout as delay } from "node:timers/promises";

import {
  assertNativePreflight,
  createNativeHarness,
  prepareIsolatedHome,
  startNativeSession,
  waitForAppShell,
} from "../lib/harness.mjs";
import { readHomeLock } from "../lib/sessionHome.mjs";

const acceptanceDir = process.env.WARDIAN_ISOLATION_ACCEPTANCE_DIR;
const label = process.env.WARDIAN_ISOLATION_LABEL;

test(
  "holds one native session open so an independent run can be observed",
  { skip: !acceptanceDir || !label ? "acceptance-only fixture" : false },
  async () => {
    const harness = await createNativeHarness();
    assertNativePreflight(harness);
    prepareIsolatedHome(harness);

    const session = await startNativeSession(harness);
    await waitForAppShell(session.driver, 60_000);

    const identity = {
      label,
      runnerPid: process.pid,
      runId: harness.runId,
      home: harness.isolatedHome,
      driverPort: harness.driverPort,
      nativeDriverPort: harness.nativeDriverPort,
      driverPid: session.tauriDriver.pid,
      portOwnership: harness.driverPortOwnership ?? null,
      appArtifact: harness.appArtifact,
      lock: readHomeLock(harness.isolatedHome),
      readyAt: new Date().toISOString(),
    };
    fs.mkdirSync(acceptanceDir, { recursive: true });
    fs.writeFileSync(path.join(acceptanceDir, `${label}.ready.json`), `${JSON.stringify(identity, null, 2)}\n`);

    // Hold the session until the coordinator asks for a liveness proof, or the
    // coordinator terminates this runner's tree, which is the point for the
    // session being sacrificed.
    const verifyFlag = path.join(acceptanceDir, `${label}.verify`);
    const deadline = Date.now() + 180_000;
    while (!fs.existsSync(verifyFlag) && Date.now() < deadline) {
      await delay(500);
    }
    assert.ok(fs.existsSync(verifyFlag), "coordinator should request verification before the deadline");

    // Still usable after the other run was terminated.
    const title = await session.driver.getTitle();
    const homeIntact = fs.existsSync(harness.isolatedHome);
    fs.writeFileSync(
      path.join(acceptanceDir, `${label}.verified.json`),
      `${JSON.stringify({ ...identity, titleAfter: title, homeIntact, verifiedAt: new Date().toISOString() }, null, 2)}\n`,
    );

    assert.equal(typeof title, "string");
    assert.equal(homeIntact, true);
    await session.close();
  },
);
