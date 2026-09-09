// @tier nightly — Runs on the nightly schedule; too slow or too broad for every pull request.
import test from "node:test";
import assert from "node:assert/strict";
import path from "node:path";
import { existsSync, readFileSync } from "node:fs";
import { spawnSync } from "node:child_process";

import {
  createNativeHarness,
  ensureNativeAppBuilt,
  freezeBuiltCliForRun,
  prepareIsolatedHome,
  startNativeSession,
  waitForAppShell,
  watchStep,
} from "../lib/harness.mjs";

const skipNativeBuild = process.env.WARDIAN_NATIVE_SKIP_BUILD === "1";
const RUN_ID = `${process.pid}-${Date.now()}`;
const ALPHA_PROVIDER_SESSION_ID = `e2e-topology-alpha-${RUN_ID}`;
const ALPHA_SESSION_NAME = `E2E-TOPOLOGY-ALPHA-${RUN_ID}`;
const BETA_PROVIDER_SESSION_ID = `e2e-topology-beta-${RUN_ID}`;
const BETA_SESSION_NAME = `E2E-TOPOLOGY-BETA-${RUN_ID}`;
const GAMMA_PROVIDER_SESSION_ID = `e2e-topology-gamma-${RUN_ID}`;
const GAMMA_SESSION_NAME = `E2E-TOPOLOGY-GAMMA-${RUN_ID}`;

function buildCli(harness) {
  const result = spawnSync(
    "cargo",
    ["build", "-p", "wardian-cli", "--bin", "wardian-cli"],
    {
      cwd: harness.repoRoot,
      encoding: "utf8",
    },
  );

  assert.equal(
    result.status,
    0,
    `cargo build -p wardian-cli failed\nstdout:\n${result.stdout}\nstderr:\n${result.stderr}`,
  );

  return freezeBuiltCliForRun(harness);
}

function runCliWithEnv(cliPath, harness, args, extraEnv) {
  const env = {
    ...process.env,
    WARDIAN_HOME: harness.isolatedHome,
    ...extraEnv,
  };
  if (!extraEnv || !Object.hasOwn(extraEnv, "WARDIAN_SESSION_ID")) {
    delete env.WARDIAN_SESSION_ID;
  }
  const result = spawnSync(cliPath, args, {
    cwd: harness.repoRoot,
    env,
    encoding: "utf8",
  });

  return {
    status: result.status,
    stdout: result.stdout,
    stderr: result.stderr,
  };
}

function runCliOkAsAgent(cliPath, harness, sessionId, args) {
  const result = runCliWithEnv(cliPath, harness, args, { WARDIAN_SESSION_ID: sessionId });
  assert.equal(
    result.status,
    0,
    `wardian ${args.join(" ")} failed\nstdout:\n${result.stdout}\nstderr:\n${result.stderr}`,
  );
  return result;
}

async function createMockAgent(
  driver,
  workspacePath,
  { sessionId, sessionName, isOff, mockScenario = null, mockDelayMs = null },
) {
  const result = await driver.executeAsyncScript((sessionId, sessionName, folder, isOff, mockScenario, mockDelayMs, done) => {
    const providerConfig =
      mockScenario || mockDelayMs
        ? {
            type: "mock",
            scenario: mockScenario,
            delay_ms: mockDelayMs,
          }
        : undefined;
    window.__TAURI_INTERNALS__.invoke("spawn_agent", {
      req: {
        sessionName,
        agentClass: "TestClass",
        folder,
        resumeSession: sessionId,
        isOff,
        configOverride: providerConfig
          ? { provider: "mock", provider_config: providerConfig }
          : { provider: "mock" },
      },
    }).then(
      (agent) => done({ ok: true, agent }),
      (error) => done({ ok: false, error: String(error) }),
    );
  }, sessionId, sessionName, workspacePath, isOff, mockScenario, mockDelayMs);

  assert.equal(result.ok, true, `spawn_agent failed: ${result.error}`);
  return result.agent;
}

async function addTopologyEdge(driver, aUuid, bUuid) {
  const result = await driver.executeAsyncScript((aUuid, bUuid, done) => {
    window.__TAURI_INTERNALS__.invoke("add_topology_edge", {
      a: aUuid,
      b: bUuid,
    }).then(
      (ok) => done({ ok: true, result: ok }),
      (error) => done({ ok: false, error: String(error) }),
    );
  }, aUuid, bUuid);

  assert.equal(result.ok, true, `add_topology_edge failed: ${result.error}`);
  return result.result;
}

async function removeTopologyEdge(driver, aUuid, bUuid) {
  const result = await driver.executeAsyncScript((aUuid, bUuid, done) => {
    window.__TAURI_INTERNALS__.invoke("remove_topology_edge", {
      a: aUuid,
      b: bUuid,
    }).then(
      (ok) => done({ ok: true, result: ok }),
      (error) => done({ ok: false, error: String(error) }),
    );
  }, aUuid, bUuid);

  assert.equal(result.ok, true, `remove_topology_edge failed: ${result.error}`);
  return result.result;
}

test("native CLI neighbors scoping reads app-written topology", { timeout: 180000 }, async (t) => {
  const harness = await createNativeHarness();

  try {
    if (!skipNativeBuild) {
      ensureNativeAppBuilt(harness);
    }
    assert.ok(harness.appPath);
  } catch (error) {
    t.skip(String(error));
    return;
  }

  prepareIsolatedHome(harness);

  const cliPath = buildCli(harness);
  const workspacePath = path.join(harness.repoRoot, "e2e-native");

  let session;
  try {
    session = await startNativeSession(harness);
  } catch (error) {
    t.skip(String(error));
    return;
  }

  t.after(async () => {
    await session.close();
  });

  await waitForAppShell(session.driver, 20000);
  await watchStep(harness, "Wardian app shell is ready for topology CLI test");

  // Spawn three agents: alpha, beta, gamma
  const alphaAgent = await createMockAgent(session.driver, workspacePath, {
    sessionId: ALPHA_PROVIDER_SESSION_ID,
    sessionName: ALPHA_SESSION_NAME,
    isOff: false,
  });
  const alphaSessionId = alphaAgent.session_id;
  assert.notEqual(alphaSessionId, ALPHA_PROVIDER_SESSION_ID);

  const betaAgent = await createMockAgent(session.driver, workspacePath, {
    sessionId: BETA_PROVIDER_SESSION_ID,
    sessionName: BETA_SESSION_NAME,
    isOff: false,
  });
  const betaSessionId = betaAgent.session_id;
  assert.notEqual(betaSessionId, BETA_PROVIDER_SESSION_ID);

  const gammaAgent = await createMockAgent(session.driver, workspacePath, {
    sessionId: GAMMA_PROVIDER_SESSION_ID,
    sessionName: GAMMA_SESSION_NAME,
    isOff: false,
  });
  const gammaSessionId = gammaAgent.session_id;
  assert.notEqual(gammaSessionId, GAMMA_PROVIDER_SESSION_ID);

  await watchStep(harness, `Created three agents: alpha, beta, gamma`);

  // Add a topology edge between alpha and beta
  await addTopologyEdge(session.driver, alphaSessionId, betaSessionId);
  await watchStep(harness, "Added topology edge between alpha and beta");

  // Run CLI as alpha (using WARDIAN_SESSION_ID) with default scope.
  // Should see alpha + beta (neighbors), but not gamma. Uses --verbose to
  // prove the spec's "verbose output adds the visibility reason" contract;
  // the post-removal listing covers the explicit --fields path instead.
  const listAsAlpha = runCliOkAsAgent(cliPath, harness, alphaSessionId, [
    "agent",
    "list",
    "--verbose",
  ]);
  const parsedAsAlpha = JSON.parse(listAsAlpha.stdout);
  const agentsAsAlpha = parsedAsAlpha.agents;

  // Verify alpha is in the list
  const alphaInList = agentsAsAlpha.find((a) => a.uuid === alphaSessionId);
  assert.ok(alphaInList, "Alpha should be visible to itself");

  // Verify beta is in the list with visibility reason
  const betaInList = agentsAsAlpha.find((a) => a.uuid === betaSessionId);
  assert.ok(betaInList, "Beta should be visible to alpha via topology edge");
  assert.equal(betaInList.visibility, "manual", "Beta should have 'manual' visibility reason");

  // Verify gamma is NOT in the list
  const gammaInList = agentsAsAlpha.find((a) => a.uuid === gammaSessionId);
  assert.equal(gammaInList, undefined, "Gamma should NOT be visible to alpha");

  await watchStep(harness, "Verified alpha sees beta (via manual edge) but not gamma");

  // Run CLI with --scope all to verify all three appear
  const listAllScopes = runCliOkAsAgent(cliPath, harness, alphaSessionId, [
    "agent",
    "list",
    "--scope",
    "all",
    "--fields",
    "name,uuid",
  ]);
  const parsedAllScopes = JSON.parse(listAllScopes.stdout);
  const agentsAllScopes = parsedAllScopes.agents;

  assert.ok(
    agentsAllScopes.find((a) => a.uuid === alphaSessionId),
    "Alpha should be in --scope all",
  );
  assert.ok(
    agentsAllScopes.find((a) => a.uuid === betaSessionId),
    "Beta should be in --scope all",
  );
  assert.ok(
    agentsAllScopes.find((a) => a.uuid === gammaSessionId),
    "Gamma should be in --scope all",
  );

  await watchStep(harness, "Verified --scope all shows all three agents");

  // Remove the topology edge
  await removeTopologyEdge(session.driver, alphaSessionId, betaSessionId);
  await watchStep(harness, "Removed topology edge between alpha and beta");

  // Now alpha has no edges and (assuming) no teams, so workspace-fallback applies
  // Since all three agents share the same workspace (e2e-native), alpha should see all three
  const listAfterRemove = runCliOkAsAgent(cliPath, harness, alphaSessionId, [
    "agent",
    "list",
    "--fields",
    "name,uuid,visibility",
  ]);
  const parsedAfterRemove = JSON.parse(listAfterRemove.stdout);
  const agentsAfterRemove = parsedAfterRemove.agents;

  // Alpha should see itself
  const alphaAfterRemove = agentsAfterRemove.find((a) => a.uuid === alphaSessionId);
  assert.ok(alphaAfterRemove, "Alpha should see itself");

  // Beta and gamma should be visible via workspace-fallback
  const betaAfterRemove = agentsAfterRemove.find((a) => a.uuid === betaSessionId);
  assert.ok(betaAfterRemove, "Beta should be visible via workspace-fallback");
  assert.equal(
    betaAfterRemove.visibility,
    "rule:workspace-fallback",
    "Beta should have workspace-fallback reason",
  );

  const gammaAfterRemove = agentsAfterRemove.find((a) => a.uuid === gammaSessionId);
  assert.ok(gammaAfterRemove, "Gamma should be visible via workspace-fallback");
  assert.equal(
    gammaAfterRemove.visibility,
    "rule:workspace-fallback",
    "Gamma should have workspace-fallback reason",
  );

  await watchStep(harness, "Verified workspace-fallback applies after edge removal");
});

// Exercises `wardian graph link/unlink/ignore/unignore` as real CLI subprocess
// calls against a running native app: the actual ControlRequest wire format,
// the CLI's live socket transport, and the real control-plane dispatch arms
// in src-tauri/src/control.rs, none of which the in-process unit tests can
// reach (they call the shared dispatch function directly, bypassing the
// wire). Also reproduces #1032's exact repro shape: unlink an in-team pair
// via the CLI, add a third member to the same team (which reseeds the whole
// team clique), and confirm the CLI-deleted pair does not resurrect.
test(
  "native CLI topology mutations route through the control plane and survive a team reseed",
  { timeout: 180000 },
  async (t) => {
    const harness = await createNativeHarness();

    try {
      if (!skipNativeBuild) {
        ensureNativeAppBuilt(harness);
      }
      assert.ok(harness.appPath);
    } catch (error) {
      t.skip(String(error));
      return;
    }

    prepareIsolatedHome(harness);

    const cliPath = buildCli(harness);
    const workspacePath = path.join(harness.repoRoot, "e2e-native");

    let session;
    try {
      session = await startNativeSession(harness);
    } catch (error) {
      t.skip(String(error));
      return;
    }

    t.after(async () => {
      await session.close();
    });

    await waitForAppShell(session.driver, 20000);
    await watchStep(harness, "Wardian app shell is ready for topology mutation test");

    const deltaAgent = await createMockAgent(session.driver, workspacePath, {
      sessionId: `e2e-topology-delta-${RUN_ID}`,
      sessionName: `E2E-TOPOLOGY-DELTA-${RUN_ID}`,
      isOff: false,
    });
    const deltaId = deltaAgent.session_id;

    const epsilonAgent = await createMockAgent(session.driver, workspacePath, {
      sessionId: `e2e-topology-epsilon-${RUN_ID}`,
      sessionName: `E2E-TOPOLOGY-EPSILON-${RUN_ID}`,
      isOff: false,
    });
    const epsilonId = epsilonAgent.session_id;

    const zetaAgent = await createMockAgent(session.driver, workspacePath, {
      sessionId: `e2e-topology-zeta-${RUN_ID}`,
      sessionName: `E2E-TOPOLOGY-ZETA-${RUN_ID}`,
      isOff: false,
    });
    const zetaId = zetaAgent.session_id;

    await watchStep(harness, "Created delta, epsilon, zeta for the topology mutation test");

    // link, as delta, defaulting the second endpoint to epsilon: exercises
    // TopologyLink over the real socket, authorized by the real self-serve
    // check in dispatch_topology_mutation.
    const linkResult = runCliOkAsAgent(cliPath, harness, deltaId, ["graph", "link", epsilonId]);
    const linkBody = JSON.parse(linkResult.stdout);
    assert.equal(linkBody.action, "link");
    assert.equal(linkBody.changed, true);
    await watchStep(harness, "Linked delta<->epsilon via the CLI against the live app");

    // ignore/unignore round trip against a third pair, as a real-wire smoke
    // test for those two verbs.
    const ignoreResult = runCliOkAsAgent(cliPath, harness, deltaId, ["graph", "ignore", zetaId]);
    assert.equal(JSON.parse(ignoreResult.stdout).changed, true);
    const unignoreResult = runCliOkAsAgent(cliPath, harness, deltaId, ["graph", "unignore", zetaId]);
    assert.equal(JSON.parse(unignoreResult.stdout).changed, true);
    await watchStep(harness, "Ignore/unignore round trip completed via the CLI");

    // Self-serve denial over the real wire: delta asks to link epsilon<->zeta,
    // a pair that excludes delta. The control plane must deny it and the CLI
    // must map that denial to self_serve_required / exit 1 (this is the one
    // authorization-path error mapping no in-process test reaches, since it
    // requires a real denial to travel back over the live socket).
    const deniedResult = runCliWithEnv(
      cliPath,
      harness,
      ["graph", "link", epsilonId, zetaId],
      { WARDIAN_SESSION_ID: deltaId },
    );
    assert.equal(deniedResult.status, 1, `expected self-serve denial\nstdout:\n${deniedResult.stdout}\nstderr:\n${deniedResult.stderr}`);
    assert.match(deniedResult.stderr, /self_serve_required/);
    await watchStep(harness, "Verified a real self-serve denial maps to exit 1 over the live socket");

    // Create a team containing delta and epsilon (CLI-local state; no app
    // round trip), then unlink delta<->epsilon through the CLI/control plane.
    const teamName = `E2E-Topology-Team-${RUN_ID}`;
    const teamCreateResult = runCliWithEnv(
      cliPath,
      harness,
      ["team", "create", teamName, "--agent", deltaId, "--agent", epsilonId],
      {},
    );
    assert.equal(
      teamCreateResult.status,
      0,
      `wardian team create failed\nstdout:\n${teamCreateResult.stdout}\nstderr:\n${teamCreateResult.stderr}`,
    );
    const unlinkResult = runCliOkAsAgent(cliPath, harness, deltaId, ["graph", "unlink", epsilonId]);
    assert.equal(JSON.parse(unlinkResult.stdout).changed, true);
    await watchStep(harness, "Created team and unlinked delta<->epsilon via the CLI");

    // Add a third member to the same team: this reseeds the whole team
    // clique. Before the #1032 fix, the CLI's unlink above did not record a
    // seed-suppression tombstone, so this reseed would have resurrected
    // delta<->epsilon even though it was never touched by this add.
    const teamAddResult = runCliWithEnv(cliPath, harness, ["team", "add", teamName, zetaId], {});
    assert.equal(
      teamAddResult.status,
      0,
      `wardian team add failed\nstdout:\n${teamAddResult.stdout}\nstderr:\n${teamAddResult.stderr}`,
    );
    await watchStep(harness, "Added zeta to the team, triggering a full clique reseed");

    const showResult = runCliWithEnv(cliPath, harness, ["graph", "show"], {});
    assert.equal(
      showResult.status,
      0,
      `wardian graph show failed\nstdout:\n${showResult.stdout}\nstderr:\n${showResult.stderr}`,
    );
    const showBody = JSON.parse(showResult.stdout);
    const hasEdge = (a, b) =>
      showBody.edges.some(
        (edge) => (edge.a === a && edge.b === b) || (edge.a === b && edge.b === a),
      );
    assert.equal(
      hasEdge(deltaId, epsilonId),
      false,
      "delta<->epsilon must stay unlinked after the team reseed (#1032 regression)",
    );
    assert.ok(hasEdge(deltaId, zetaId), "delta<->zeta should be seeded by the team add");
    assert.ok(hasEdge(epsilonId, zetaId), "epsilon<->zeta should be seeded by the team add");
    await watchStep(harness, "Verified the #1032 regression stays fixed through the real CLI/app path");

    // The control plane is the sole writer, so every mutation above should
    // have appended one record to the audit log.
    const auditPath = path.join(harness.isolatedHome, "topology", "audit.jsonl");
    assert.ok(existsSync(auditPath), "topology audit log should exist after CLI mutations");
    const auditRecords = readFileSync(auditPath, "utf8")
      .split("\n")
      .filter((line) => line.trim().length > 0)
      .map((line) => JSON.parse(line));
    const linkRecord = auditRecords.find(
      (record) => record.operation === "link" && record.outcome === "applied" && record.a === deltaId,
    );
    assert.ok(linkRecord, "audit log should contain the delta-initiated link");
    assert.equal(linkRecord.caller, `agent:${deltaId}`);
    const unlinkRecord = auditRecords.find(
      (record) => record.operation === "unlink" && record.outcome === "applied",
    );
    assert.ok(unlinkRecord, "audit log should contain the unlink");
    await watchStep(harness, "Verified the topology audit log recorded these mutations");
  },
);
