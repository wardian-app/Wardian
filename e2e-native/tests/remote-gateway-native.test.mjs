import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import net from "node:net";
import path from "node:path";

import {
  createNativeHarness,
  ensureNativeAppBuilt,
  prepareIsolatedHome,
  startNativeSession,
  waitForAppShell,
} from "../lib/harness.mjs";

const skipNativeBuild = process.env.WARDIAN_NATIVE_SKIP_BUILD === "1";
const RUN_ID = `${process.pid}-${Date.now()}`;
const REMOTE_SESSION_COOKIE_NAME = "__Host-wardian_remote_session";

function getFreePort() {
  return new Promise((resolve, reject) => {
    const server = net.createServer();
    server.once("error", reject);
    server.listen(0, "127.0.0.1", () => {
      const address = server.address();
      server.close(() => {
        if (!address || typeof address === "string") {
          reject(new Error("unable to allocate a loopback port"));
          return;
        }
        resolve(address.port);
      });
    });
  });
}

function writeRemoteGatewayConfig(harness, port) {
  const remoteAccessDir = path.join(harness.isolatedHome, "remote-access");
  fs.mkdirSync(remoteAccessDir, { recursive: true });
  fs.writeFileSync(
    path.join(remoteAccessDir, "config.json"),
    JSON.stringify(
      {
        schema_version: 1,
        enabled: true,
        canonical_origin: `https://127.0.0.1:${port}`,
        loopback_host: "127.0.0.1",
        loopback_port: port,
        gateway_identity_public_key: "e2e-public-key",
        gateway_identity_fingerprint: "e2e-fingerprint",
      },
      null,
      2,
    ),
  );
}

async function waitForGateway(baseUrl, timeoutMs = 15000) {
  const startedAt = Date.now();
  let lastError;

  while (Date.now() - startedAt < timeoutMs) {
    try {
      const response = await fetch(`${baseUrl}/remote/api/health`);
      if (response.ok) return;
      lastError = new Error(`gateway health returned ${response.status}`);
    } catch (error) {
      lastError = error;
    }
    await new Promise((resolve) => setTimeout(resolve, 250));
  }

  throw new Error(`Timed out waiting for remote gateway: ${lastError}`);
}

test("remote gateway authenticates native app agent reads", { timeout: 180000 }, async (t) => {
  const harness = await createNativeHarness();
  try {
    if (!skipNativeBuild) {
      ensureNativeAppBuilt(harness);
    }
  } catch (error) {
    t.skip(String(error));
    return;
  }

  prepareIsolatedHome(harness);
  const gatewayPort = await getFreePort();
  writeRemoteGatewayConfig(harness, gatewayPort);
  const baseUrl = `http://127.0.0.1:${gatewayPort}`;

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
  await waitForGateway(baseUrl);

  const workspacePath = path.join(harness.repoRoot, "e2e-native");
  const sessionId = `e2e-remote-gateway-${RUN_ID}`;
  const sessionName = `E2E-REMOTE-GATEWAY-${RUN_ID}`;

  const result = await session.driver.executeAsyncScript(
    (sessionId, sessionName, folder, done) => {
      window.__TAURI_INTERNALS__.invoke("spawn_agent", {
        req: {
          sessionName,
          agentClass: "TestClass",
          folder,
          resumeSession: sessionId,
          isOff: true,
          configOverride: { provider: "mock" },
        },
      }).then(
        (agent) => done({ ok: true, agent }),
        (error) => done({ ok: false, error: String(error) }),
      );
    },
    sessionId,
    sessionName,
    workspacePath,
  );

  assert.equal(result.ok, true, `spawn_agent failed: ${result.error}`);
  assert.equal(result.agent.session_id, sessionId);
  assert.equal(result.agent.session_name, sessionName);

  const deviceId = `e2e-device-${RUN_ID}`;
  const remoteSessionId = `e2e-remote-session-${RUN_ID}`;
  const sessionResult = await session.driver.executeAsyncScript(
    (deviceId, sessionId, done) => {
      window.__TAURI_INTERNALS__.invoke("debug_create_remote_session", {
        deviceId,
        sessionId,
      }).then(
        (session) => done({ ok: true, session }),
        (error) => done({ ok: false, error: String(error) }),
      );
    },
    deviceId,
    remoteSessionId,
  );

  assert.equal(sessionResult.ok, true, `debug_create_remote_session failed: ${sessionResult.error}`);
  assert.ok(sessionResult.session.csrf_nonce);

  const unauthorized = await fetch(`${baseUrl}/remote/api/agents`);
  assert.equal(unauthorized.status, 401);

  const badOrigin = await fetch(`${baseUrl}/remote/api/agents`, {
    headers: { Origin: "https://wrong.tailnet.ts.net" },
  });
  assert.equal(badOrigin.status, 403);

  const response = await fetch(`${baseUrl}/remote/api/agents`, {
    headers: {
      Cookie: `${REMOTE_SESSION_COOKIE_NAME}=${remoteSessionId}`,
    },
  });
  assert.equal(response.status, 200);
  const body = await response.json();
  assert.equal(
    body.agents.some((entry) => entry.session_id === sessionId),
    true,
  );
});
