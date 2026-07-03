import test from "node:test";
import assert from "node:assert/strict";
import path from "node:path";
import { existsSync } from "node:fs";
import { spawnSync } from "node:child_process";

import {
  createNativeHarness,
  ensureNativeAppBuilt,
  prepareIsolatedHome,
  startNativeSession,
  waitForAppShell,
  watchStep,
} from "../lib/harness.mjs";

const skipNativeBuild = process.env.WARDIAN_NATIVE_SKIP_BUILD === "1";
const RUN_ID = `${process.pid}-${Date.now()}`;
const ALPHA_SESSION_ID = `e2e-topology-alpha-${RUN_ID}`;
const ALPHA_SESSION_NAME = `E2E-TOPOLOGY-ALPHA-${RUN_ID}`;
const BETA_SESSION_ID = `e2e-topology-beta-${RUN_ID}`;
const BETA_SESSION_NAME = `E2E-TOPOLOGY-BETA-${RUN_ID}`;
const GAMMA_SESSION_ID = `e2e-topology-gamma-${RUN_ID}`;
const GAMMA_SESSION_NAME = `E2E-TOPOLOGY-GAMMA-${RUN_ID}`;

function commandName(name) {
  return process.platform === "win32" ? `${name}.exe` : name;
}

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

  const localCandidate = path.join(harness.repoRoot, "target", "debug", commandName("wardian-cli"));
  if (existsSync(localCandidate)) {
    return localCandidate;
  }

  const metadata = spawnSync("cargo", ["metadata", "--no-deps", "--format-version", "1"], {
    cwd: harness.repoRoot,
    encoding: "utf8",
  });
  assert.equal(
    metadata.status,
    0,
    `cargo metadata failed\nstdout:\n${metadata.stdout}\nstderr:\n${metadata.stderr}`,
  );
  const targetDirectory = JSON.parse(metadata.stdout).target_directory;
  const metadataCandidate = path.join(targetDirectory, "debug", commandName("wardian-cli"));
  assert.equal(
    existsSync(metadataCandidate),
    true,
    `wardian-cli binary was not found at ${metadataCandidate}`,
  );
  return metadataCandidate;
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
  assert.ok(harness.appPath);

  try {
    if (!skipNativeBuild) {
      ensureNativeAppBuilt(harness);
    }
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
    sessionId: ALPHA_SESSION_ID,
    sessionName: ALPHA_SESSION_NAME,
    isOff: false,
  });
  assert.equal(alphaAgent.session_id, ALPHA_SESSION_ID);

  const betaAgent = await createMockAgent(session.driver, workspacePath, {
    sessionId: BETA_SESSION_ID,
    sessionName: BETA_SESSION_NAME,
    isOff: false,
  });
  assert.equal(betaAgent.session_id, BETA_SESSION_ID);

  const gammaAgent = await createMockAgent(session.driver, workspacePath, {
    sessionId: GAMMA_SESSION_ID,
    sessionName: GAMMA_SESSION_NAME,
    isOff: false,
  });
  assert.equal(gammaAgent.session_id, GAMMA_SESSION_ID);

  await watchStep(harness, `Created three agents: alpha, beta, gamma`);

  // Add a topology edge between alpha and beta
  await addTopologyEdge(session.driver, ALPHA_SESSION_ID, BETA_SESSION_ID);
  await watchStep(harness, "Added topology edge between alpha and beta");

  // Run CLI as alpha (using WARDIAN_SESSION_ID) with default scope.
  // Should see alpha + beta (neighbors), but not gamma. Uses --verbose to
  // prove the spec's "verbose output adds the visibility reason" contract;
  // the post-removal listing covers the explicit --fields path instead.
  const listAsAlpha = runCliOkAsAgent(cliPath, harness, ALPHA_SESSION_ID, [
    "agent",
    "list",
    "--verbose",
  ]);
  const parsedAsAlpha = JSON.parse(listAsAlpha.stdout);
  const agentsAsAlpha = parsedAsAlpha.agents;

  // Verify alpha is in the list
  const alphaInList = agentsAsAlpha.find((a) => a.uuid === ALPHA_SESSION_ID);
  assert.ok(alphaInList, "Alpha should be visible to itself");

  // Verify beta is in the list with visibility reason
  const betaInList = agentsAsAlpha.find((a) => a.uuid === BETA_SESSION_ID);
  assert.ok(betaInList, "Beta should be visible to alpha via topology edge");
  assert.equal(betaInList.visibility, "manual", "Beta should have 'manual' visibility reason");

  // Verify gamma is NOT in the list
  const gammaInList = agentsAsAlpha.find((a) => a.uuid === GAMMA_SESSION_ID);
  assert.equal(gammaInList, undefined, "Gamma should NOT be visible to alpha");

  await watchStep(harness, "Verified alpha sees beta (via manual edge) but not gamma");

  // Run CLI with --scope all to verify all three appear
  const listAllScopes = runCliOkAsAgent(cliPath, harness, ALPHA_SESSION_ID, [
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
    agentsAllScopes.find((a) => a.uuid === ALPHA_SESSION_ID),
    "Alpha should be in --scope all",
  );
  assert.ok(
    agentsAllScopes.find((a) => a.uuid === BETA_SESSION_ID),
    "Beta should be in --scope all",
  );
  assert.ok(
    agentsAllScopes.find((a) => a.uuid === GAMMA_SESSION_ID),
    "Gamma should be in --scope all",
  );

  await watchStep(harness, "Verified --scope all shows all three agents");

  // Remove the topology edge
  await removeTopologyEdge(session.driver, ALPHA_SESSION_ID, BETA_SESSION_ID);
  await watchStep(harness, "Removed topology edge between alpha and beta");

  // Now alpha has no edges and (assuming) no teams, so workspace-fallback applies
  // Since all three agents share the same workspace (e2e-native), alpha should see all three
  const listAfterRemove = runCliOkAsAgent(cliPath, harness, ALPHA_SESSION_ID, [
    "agent",
    "list",
    "--fields",
    "name,uuid,visibility",
  ]);
  const parsedAfterRemove = JSON.parse(listAfterRemove.stdout);
  const agentsAfterRemove = parsedAfterRemove.agents;

  // Alpha should see itself
  const alphaAfterRemove = agentsAfterRemove.find((a) => a.uuid === ALPHA_SESSION_ID);
  assert.ok(alphaAfterRemove, "Alpha should see itself");

  // Beta and gamma should be visible via workspace-fallback
  const betaAfterRemove = agentsAfterRemove.find((a) => a.uuid === BETA_SESSION_ID);
  assert.ok(betaAfterRemove, "Beta should be visible via workspace-fallback");
  assert.equal(
    betaAfterRemove.visibility,
    "rule:workspace-fallback",
    "Beta should have workspace-fallback reason",
  );

  const gammaAfterRemove = agentsAfterRemove.find((a) => a.uuid === GAMMA_SESSION_ID);
  assert.ok(gammaAfterRemove, "Gamma should be visible via workspace-fallback");
  assert.equal(
    gammaAfterRemove.visibility,
    "rule:workspace-fallback",
    "Gamma should have workspace-fallback reason",
  );

  await watchStep(harness, "Verified workspace-fallback applies after edge removal");
});
