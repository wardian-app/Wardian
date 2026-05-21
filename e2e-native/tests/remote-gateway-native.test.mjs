import test from "node:test";
import assert from "node:assert/strict";
import crypto from "node:crypto";
import fs from "node:fs";
import http from "node:http";
import net from "node:net";
import path from "node:path";
import WebSocket from "ws";

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

function authSignatureMessage(challenge) {
  return Buffer.from(
    `wardian.remote.auth.v1\norigin:${challenge.origin}\ndevice:${challenge.device_id}\nchallenge:${challenge.challenge_id}\nnonce:${challenge.nonce}`,
  );
}

function requestWithForgedHost({ port, path: requestPath, method, headers, body }) {
  return new Promise((resolve, reject) => {
    const req = http.request(
      {
        hostname: "127.0.0.1",
        port,
        path: requestPath,
        method,
        headers,
      },
      (res) => {
        res.resume();
        res.on("end", () => resolve(res.statusCode));
      },
    );
    req.on("error", reject);
    if (body) req.write(body);
    req.end();
  });
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

async function assertStatus(response, expectedStatus, label) {
  if (response.status === expectedStatus) return;
  throw new Error(`${label} returned ${response.status}: ${await response.text()}`);
}

async function assertStatusStreamClosesWithoutTicket(baseUrl, canonicalOrigin, port) {
  await new Promise((resolve, reject) => {
    const socket = new WebSocket(`${baseUrl.replace("http:", "ws:")}/remote/api/status-stream`, {
      headers: {
        Host: `127.0.0.1:${port}`,
        Origin: canonicalOrigin,
      },
    });
    const timer = setTimeout(() => {
      socket.terminate();
      reject(new Error("status stream stayed open without an authentication ticket"));
    }, 6500);
    socket.once("close", () => {
      clearTimeout(timer);
      resolve();
    });
    socket.once("error", (error) => {
      clearTimeout(timer);
      reject(error);
    });
  });
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
  const canonicalOrigin = `https://127.0.0.1:${gatewayPort}`;

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
  await assertStatusStreamClosesWithoutTicket(baseUrl, canonicalOrigin, gatewayPort);

  const shell = await fetch(`${baseUrl}/remote`);
  assert.equal(shell.status, 200);
  assert.match(await shell.text(), /<div id="root">/);

  const manifest = await fetch(`${baseUrl}/manifest.webmanifest`);
  assert.equal(manifest.status, 200);
  assert.equal((await manifest.json()).start_url, "/remote");

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

  const keys = crypto.generateKeyPairSync("ec", { namedCurve: "P-256" });
  const publicKeySpkiDer = keys.publicKey.export({ type: "spki", format: "der" });

  const pairing = await session.driver.executeAsyncScript((done) => {
    window.__TAURI_INTERNALS__.invoke("create_remote_pairing_offer").then(
      (offer) => done({ ok: true, offer }),
      (error) => done({ ok: false, error: String(error) }),
    );
  });
  assert.equal(pairing.ok, true, `create_remote_pairing_offer failed: ${pairing.error}`);

  const pairingSubmit = await fetch(`${baseUrl}/remote/api/pairing/submit`, {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      Origin: canonicalOrigin,
    },
    body: JSON.stringify({
      pairing_offer_id: pairing.offer.pairing_offer_id,
      nonce: pairing.offer.nonce,
      device_label: `E2E phone ${RUN_ID}`,
      public_key_spki_der_base64: publicKeySpkiDer.toString("base64"),
    }),
  });
  await assertStatus(pairingSubmit, 200, "pairing submit");
  const pairedDevice = await pairingSubmit.json();
  assert.ok(pairedDevice.device_id);

  const challengeResponse = await fetch(`${baseUrl}/remote/api/auth/challenge`, {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      Origin: canonicalOrigin,
    },
    body: JSON.stringify({ device_id: pairedDevice.device_id }),
  });
  await assertStatus(challengeResponse, 200, "auth challenge");
  const challenge = await challengeResponse.json();

  const signature = crypto.createSign("SHA256").update(authSignatureMessage(challenge)).sign(keys.privateKey);
  const sessionResponse = await fetch(`${baseUrl}/remote/api/auth/session`, {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      Origin: canonicalOrigin,
    },
    body: JSON.stringify({
      challenge_id: challenge.challenge_id,
      device_id: pairedDevice.device_id,
      signature_der_base64: signature.toString("base64"),
    }),
  });
  await assertStatus(sessionResponse, 200, "auth session");
  const cookie = sessionResponse.headers.get("set-cookie");
  assert.ok(cookie?.includes(`${REMOTE_SESSION_COOKIE_NAME}=`));
  assert.ok(cookie.includes("Secure"));
  assert.ok(cookie.includes("HttpOnly"));
  assert.ok(cookie.includes("SameSite=Strict"));
  assert.ok(!cookie.includes("Domain="));

  const sessionBody = await sessionResponse.json();
  assert.ok(sessionBody.csrf_nonce);

  const unauthorized = await fetch(`${baseUrl}/remote/api/agents`);
  assert.equal(unauthorized.status, 401);

  const badOrigin = await fetch(`${baseUrl}/remote/api/agents`, {
    headers: { Origin: "https://wrong.tailnet.ts.net" },
  });
  assert.equal(badOrigin.status, 403);

  const response = await fetch(`${baseUrl}/remote/api/agents`, {
    headers: {
      Cookie: cookie,
    },
  });
  assert.equal(response.status, 200);
  const body = await response.json();
  assert.equal(
    body.agents.some((entry) => entry.session_id === sessionId),
    true,
  );

  const missingOriginMutation = await fetch(`${baseUrl}/remote/api/agents/action`, {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      Cookie: cookie,
      "x-wardian-csrf": sessionBody.csrf_nonce,
    },
    body: JSON.stringify({ action: "pause", target: sessionId }),
  });
  assert.equal(missingOriginMutation.status, 403);

  const forgedHostMutationStatus = await requestWithForgedHost({
    port: gatewayPort,
    path: "/remote/api/agents/action",
    method: "POST",
    headers: {
      Host: "forged.tailnet.ts.net",
      Origin: canonicalOrigin,
      Cookie: cookie,
      "Content-Type": "application/json",
      "x-wardian-csrf": sessionBody.csrf_nonce,
    },
    body: JSON.stringify({ action: "pause", target: sessionId }),
  });
  assert.equal(forgedHostMutationStatus, 403);
});
