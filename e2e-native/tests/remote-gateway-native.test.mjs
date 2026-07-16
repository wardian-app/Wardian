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
import {
  closeWorkbenchSurface,
  openWorkbenchSurface,
  waitForWorkbenchReady,
} from "../lib/workbench.mjs";

const skipNativeBuild = process.env.WARDIAN_NATIVE_SKIP_BUILD === "1";
const RUN_ID = `${process.pid}-${Date.now()}`;
const REMOTE_SESSION_COOKIE_NAME = "__Host-wardian_remote_session";

async function invokeTauri(driver, command, args = {}) {
  const result = await driver.executeAsyncScript((cmd, payload, done) => {
    window.__TAURI_INTERNALS__.invoke(cmd, payload).then(
      (value) => done({ ok: true, value }),
      (error) => done({ ok: false, error: String(error) }),
    );
  }, command, args);

  assert.equal(result.ok, true, `${command} failed: ${result.error}`);
  return result.value;
}

async function waitFor(label, timeoutMs, probe) {
  const startedAt = Date.now();
  let last = null;
  while (Date.now() - startedAt < timeoutMs) {
    try {
      last = await probe();
      if (last?.ok) return last;
    } catch (error) {
      last = { ok: false, error: String(error) };
    }
    await new Promise((resolve) => setTimeout(resolve, 200));
  }
  throw new Error(`Timed out waiting for ${label}: ${JSON.stringify(last)}`);
}

function writePersistentRemoteMockScript(harness, sessionId) {
  const scriptPath = path.join(harness.isolatedHome, `remote-terminal-${RUN_ID}.cjs`);
  fs.writeFileSync(
    scriptPath,
    `
"use strict";
const readline = require("node:readline");
let tick = 0;
process.stdout.write(JSON.stringify({
  type: "init",
  session_id: ${JSON.stringify(sessionId)},
  timestamp: new Date().toISOString(),
}) + "\\n");
process.stdout.write("REMOTE_BROKER_READY_${RUN_ID}\\r\\n");
setInterval(() => {
  tick += 1;
  process.stdout.write("remote-broker-tick:" + tick + "\\r\\n");
}, 100);
const lines = readline.createInterface({ input: process.stdin, crlfDelay: Infinity });
lines.on("line", (line) => process.stdout.write("remote-echo:" + line + "\\r\\n"));
process.stdin.resume();
`,
    "utf8",
  );
  return scriptPath;
}

function createJsonSocketInbox(socket) {
  const queued = [];
  const seen = [];
  const waiters = [];
  let closed = false;

  const flush = () => {
    for (let waiterIndex = 0; waiterIndex < waiters.length; waiterIndex += 1) {
      const waiter = waiters[waiterIndex];
      const messageIndex = queued.findIndex(waiter.predicate);
      if (messageIndex < 0) continue;
      const [message] = queued.splice(messageIndex, 1);
      waiters.splice(waiterIndex, 1);
      clearTimeout(waiter.timer);
      waiter.resolve(message);
      waiterIndex -= 1;
    }
  };

  socket.on("message", (data) => {
    const message = JSON.parse(data.toString());
    queued.push(message);
    seen.push(message);
    flush();
  });
  socket.on("close", () => {
    closed = true;
    for (const waiter of waiters.splice(0)) {
      clearTimeout(waiter.timer);
      waiter.reject(new Error(`terminal socket closed while waiting; seen=${JSON.stringify(seen)}`));
    }
  });
  socket.on("error", (error) => {
    for (const waiter of waiters.splice(0)) {
      clearTimeout(waiter.timer);
      waiter.reject(error);
    }
  });

  return {
    seen,
    next(predicate, timeoutMs = 15000) {
      const messageIndex = queued.findIndex(predicate);
      if (messageIndex >= 0) {
        const [message] = queued.splice(messageIndex, 1);
        return Promise.resolve(message);
      }
      if (closed) {
        return Promise.reject(new Error(`terminal socket is closed; seen=${JSON.stringify(seen)}`));
      }
      return new Promise((resolve, reject) => {
        const waiter = { predicate, resolve, reject, timer: null };
        waiter.timer = setTimeout(() => {
          const index = waiters.indexOf(waiter);
          if (index >= 0) waiters.splice(index, 1);
          reject(new Error(`Timed out waiting for terminal socket message; seen=${JSON.stringify(seen)}`));
        }, timeoutMs);
        waiters.push(waiter);
        flush();
      });
    },
  };
}

async function openJsonSocket(url, options) {
  const socket = new WebSocket(url, options);
  await new Promise((resolve, reject) => {
    socket.once("open", resolve);
    socket.once("error", reject);
  });
  return socket;
}

async function waitForSocketClose(socket, timeoutMs = 15000) {
  if (socket.readyState === WebSocket.CLOSED) return;
  await new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error("Timed out waiting for terminal socket close")), timeoutMs);
    socket.once("close", () => {
      clearTimeout(timer);
      resolve();
    });
  });
}

async function readDesktopBrokerState(driver, sessionId, presentationId, runtimeGeneration) {
  return await invokeTauri(driver, "update_terminal_presentation", {
    request: {
      presentation_id: presentationId,
      session_id: sessionId,
      runtime_generation: runtimeGeneration,
      desired_geometry: { cols: 96, rows: 28 },
      visibility: "visible",
      render_state: "mounted",
      requested_interaction: "interactive",
      observed_lease_epoch: 0,
    },
  });
}

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

function readRemoteAuditRecords(harness) {
  const auditPath = path.join(harness.isolatedHome, "remote-access", "audit.jsonl");
  if (!fs.existsSync(auditPath)) return [];
  return fs
    .readFileSync(auditPath, "utf8")
    .split(/\r?\n/)
    .filter(Boolean)
    .map((line) => JSON.parse(line));
}

async function assertPreAuthRateLimitIsAudited(baseUrl, canonicalOrigin, deviceId, harness) {
  let rateLimited = false;
  for (let attempt = 0; attempt < 6; attempt += 1) {
    const response = await fetch(`${baseUrl}/remote/api/auth/challenge`, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        Origin: canonicalOrigin,
      },
      body: JSON.stringify({ device_id: deviceId }),
    });
    if (response.status === 429) {
      rateLimited = true;
      break;
    }
    await assertStatus(response, 200, `extra auth challenge ${attempt}`);
  }
  assert.equal(rateLimited, true, "auth challenge rate limit was not reached");
  const records = readRemoteAuditRecords(harness);
  assert.equal(
    records.some(
      (record) =>
        record.origin === canonicalOrigin &&
        record.event_type === "authentication" &&
        record.action === "challenge" &&
        record.outcome === "rejected" &&
        record.error_code === "rate_limited",
    ),
    true,
    "auth challenge rate-limit rejection was not written to the remote audit log",
  );
}

test("remote gateway authenticates broker ownership transitions across desktop and remote", { timeout: 240000 }, async (t) => {
  const harness = await createNativeHarness();
  prepareIsolatedHome(harness);
  const gatewayPort = await getFreePort();
  writeRemoteGatewayConfig(harness, gatewayPort);
  const baseUrl = `http://127.0.0.1:${gatewayPort}`;
  const canonicalOrigin = `https://127.0.0.1:${gatewayPort}`;
  const workspacePath = path.join(harness.repoRoot, "e2e-native");
  const sessionId = `e2e-remote-gateway-${RUN_ID}`;
  const sessionName = `E2E-REMOTE-GATEWAY-${RUN_ID}`;
  const mockScript = writePersistentRemoteMockScript(harness, sessionId);
  const previousMockScript = process.env.WARDIAN_MOCK_SCRIPT;
  process.env.WARDIAN_MOCK_SCRIPT = mockScript;

  let session = null;
  try {
    if (!skipNativeBuild) ensureNativeAppBuilt(harness);
    assert.ok(harness.appPath, "Expected a native Wardian application path");
    session = await startNativeSession(harness);
  } finally {
    if (previousMockScript === undefined) delete process.env.WARDIAN_MOCK_SCRIPT;
    else process.env.WARDIAN_MOCK_SCRIPT = previousMockScript;
  }

  t.after(async () => {
    await session?.close();
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

  const result = await session.driver.executeAsyncScript(
    (sessionId, sessionName, folder, done) => {
      window.__TAURI_INTERNALS__.invoke("spawn_agent", {
        req: {
          sessionName,
          agentClass: "TestClass",
          folder,
          resumeSession: sessionId,
          isOff: false,
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

  await waitForWorkbenchReady(session.driver);
  // Agents auto-owns an otherwise unowned runtime for click-free first paint.
  // This test establishes its own explicit Agent Session desktop owner before
  // handing the lease to the authenticated remote client.
  await closeWorkbenchSurface(session.driver, "agents-overview");
  await openWorkbenchSurface(session.driver, "agent-session", sessionId);
  const desktopSurface = await waitFor("desktop agent presentation", 30000, async () => {
    const tabs = await session.driver.executeScript((resourceKey) => {
      return Array.from(
        document.querySelectorAll(
          '[role="tab"][data-surface-type="agent-session"][data-resource-key]',
        ),
      )
        .filter((tab) => tab.getAttribute("data-resource-key") === resourceKey)
        .map((tab) => ({ surfaceId: tab.getAttribute("data-surface-id") }));
    }, sessionId);
    return { ok: tabs.length === 1 && Boolean(tabs[0].surfaceId), tabs };
  });
  const desktopPresentationId = `${desktopSurface.tabs[0].surfaceId}:agent:${sessionId}`;

  const terminalSnapshot = await invokeTauri(session.driver, "request_terminal_snapshot", {
    request: { session_id: sessionId },
  });
  const desktopRegistered = await waitFor("registered desktop terminal presentation", 30000, async () => {
    const value = await invokeTauri(session.driver, "update_terminal_presentation", {
      request: {
        presentation_id: desktopPresentationId,
        session_id: sessionId,
        runtime_generation: terminalSnapshot.runtime_generation,
        desired_geometry: { cols: 96, rows: 28 },
        visibility: "visible",
        render_state: "mounted",
        requested_interaction: "interactive",
        observed_lease_epoch: 0,
      },
    });
    return { ok: value.presentation.presentation_id === desktopPresentationId, value };
  });
  assert.equal(desktopRegistered.value.broker_state.owner_presentation_id, null);
  assert.equal(desktopRegistered.value.broker_state.pending_activation, null);

  const desktopBegin = await invokeTauri(session.driver, "begin_terminal_activation", {
    request: {
      session_id: sessionId,
      presentation_id: desktopPresentationId,
      runtime_generation: terminalSnapshot.runtime_generation,
      observed_lease_epoch: desktopRegistered.value.broker_state.lease_epoch,
    },
  });
  assert.equal(desktopBegin.decision.status, "accepted");
  assert.ok(desktopBegin.activation_id);
  assert.ok(desktopBegin.snapshot);
  const desktopAck = await invokeTauri(session.driver, "ack_terminal_activation", {
    request: {
      session_id: sessionId,
      presentation_id: desktopPresentationId,
      runtime_generation: desktopBegin.decision.runtime_generation,
      lease_epoch: desktopBegin.decision.lease_epoch,
      activation_id: desktopBegin.activation_id,
    },
  });
  assert.equal(desktopAck.decision.status, "accepted");
  assert.equal(desktopAck.broker_state.owner_presentation_id, desktopPresentationId);

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
  await assertStatus(pairingSubmit, 202, "pairing submit");
  const pendingPairing = await pairingSubmit.json();
  assert.equal(pendingPairing.status, "pending");
  assert.ok(pendingPairing.pairing_request_id);

  const preApprovalChallenge = await fetch(`${baseUrl}/remote/api/auth/challenge`, {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      Origin: canonicalOrigin,
    },
    body: JSON.stringify({ device_id: pendingPairing.device_id }),
  });
  assert.equal(preApprovalChallenge.status, 404);

  const approval = await session.driver.executeAsyncScript((requestId, done) => {
    window.__TAURI_INTERNALS__.invoke("approve_remote_pairing_request", { requestId }).then(
      (devices) => done({ ok: true, devices }),
      (error) => done({ ok: false, error: String(error) }),
    );
  }, pendingPairing.pairing_request_id);
  assert.equal(approval.ok, true, `approve_remote_pairing_request failed: ${approval.error}`);
  const pairingAuditRecords = readRemoteAuditRecords(harness);
  assert.equal(
    pairingAuditRecords.some(
      (record) =>
        record.origin === canonicalOrigin &&
        record.event_type === "pairing" &&
        record.action === "create" &&
        record.target_type === "pairing_offer" &&
        record.target_id === pairing.offer.pairing_offer_id &&
        record.outcome === "accepted",
    ),
    true,
    "pairing offer creation was not written to the remote audit log",
  );
  assert.equal(
    pairingAuditRecords.some(
      (record) =>
        record.origin === canonicalOrigin &&
        record.event_type === "pairing" &&
        record.action === "approve" &&
        record.device_id === pendingPairing.device_id &&
        record.target_type === "device" &&
        record.target_id === pendingPairing.device_id &&
        record.outcome === "accepted",
    ),
    true,
    "pairing approval was not written to the remote audit log",
  );

  const pairingStatus = await fetch(`${baseUrl}/remote/api/pairing/${pendingPairing.pairing_request_id}`, {
    headers: {
      Origin: canonicalOrigin,
    },
  });
  await assertStatus(pairingStatus, 200, "pairing status");
  const pairedDevice = await pairingStatus.json();
  assert.equal(pairedDevice.status, "approved");
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

  await assertPreAuthRateLimitIsAudited(baseUrl, canonicalOrigin, pairedDevice.device_id, harness);

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

  const sessionCookie = cookie.split(";", 1)[0];
  const ticketResponse = await fetch(`${baseUrl}/remote/api/ws-ticket`, {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      Cookie: sessionCookie,
      Origin: canonicalOrigin,
      "x-wardian-csrf": sessionBody.csrf_nonce,
    },
    body: JSON.stringify({ stream: "terminal_attach" }),
  });
  await assertStatus(ticketResponse, 200, "terminal websocket ticket");
  const terminalTicket = await ticketResponse.json();
  assert.ok(terminalTicket.ticket);

  const desktopBeforeRemote = await readDesktopBrokerState(
    session.driver,
    sessionId,
    desktopPresentationId,
    terminalSnapshot.runtime_generation,
  );
  assert.equal(desktopBeforeRemote.broker_state.owner_presentation_id, desktopPresentationId);

  const terminalSocket = await openJsonSocket(
    `${baseUrl.replace("http:", "ws:")}/remote/api/agents/${encodeURIComponent(sessionId)}/terminal-stream`,
    {
      headers: {
        Cookie: sessionCookie,
        Host: `127.0.0.1:${gatewayPort}`,
        Origin: canonicalOrigin,
      },
    },
  );
  t.after(() => {
    if (terminalSocket.readyState === WebSocket.OPEN) terminalSocket.terminate();
  });
  const inbox = createJsonSocketInbox(terminalSocket);
  terminalSocket.send(JSON.stringify({
    protocol_version: 2,
    ticket: terminalTicket.ticket,
    cols: 120,
    rows: 40,
  }));

  const registered = await inbox.next((message) => message.type === "registered");
  assert.equal(registered.protocol_version, 2);
  assert.equal(registered.presentation.client_kind, "remote");
  assert.equal(registered.broker_state.owner_presentation_id, desktopPresentationId);
  const remotePresentationId = registered.presentation.presentation_id;

  terminalSocket.send(JSON.stringify({
    type: "report_viewport",
    runtime_generation: registered.broker_state.runtime_generation,
    cols: 220,
    rows: 70,
  }));
  const remoteViewport = await inbox.next(
    (message) => message.type === "presentation_state"
      && message.presentation?.presentation_id === remotePresentationId,
  );
  assert.deepEqual(remoteViewport.presentation.desired_geometry, { cols: 220, rows: 70 });
  const desktopAfterRemoteViewport = await readDesktopBrokerState(
    session.driver,
    sessionId,
    desktopPresentationId,
    terminalSnapshot.runtime_generation,
  );

  const staleLeaseEpoch = Math.max(0, registered.broker_state.lease_epoch - 1);
  terminalSocket.send(JSON.stringify({
    type: "input",
    runtime_generation: registered.broker_state.runtime_generation,
    lease_epoch: staleLeaseEpoch,
    data: `STALE_REMOTE_INPUT_${RUN_ID}\r`,
  }));
  const staleInput = await inbox.next(
    (message) => message.type === "input_result" && message.decision?.reason === "lease_epoch_changed",
  );
  assert.equal(staleInput.decision.status, "rejected");

  terminalSocket.send(JSON.stringify({
    type: "resize",
    runtime_generation: registered.broker_state.runtime_generation,
    lease_epoch: staleLeaseEpoch,
    geometry_sequence: 1,
    cols: 210,
    rows: 65,
  }));
  const staleResize = await inbox.next(
    (message) => message.type === "resize_result"
      && message.result?.decision?.reason === "lease_epoch_changed",
  );
  assert.equal(staleResize.result.decision.status, "rejected");

  terminalSocket.send(JSON.stringify({
    type: "resize",
    runtime_generation: registered.broker_state.runtime_generation,
    lease_epoch: registered.broker_state.lease_epoch,
    geometry_sequence: 2,
    cols: 210,
    rows: 65,
  }));
  const mirrorResize = await inbox.next(
    (message) => message.type === "resize_result"
      && message.result?.decision?.reason === "not_owner",
  );
  assert.equal(
    mirrorResize.result.decision.status,
    "rejected",
    "a remote mirror must not resize the canonical PTY",
  );

  terminalSocket.send(JSON.stringify({
    type: "input",
    runtime_generation: registered.broker_state.runtime_generation,
    lease_epoch: registered.broker_state.lease_epoch,
    data: `MIRROR_REMOTE_INPUT_${RUN_ID}\r`,
  }));
  const mirrorInput = await inbox.next(
    (message) => message.type === "input_result" && message.decision?.reason === "not_owner",
  );
  assert.equal(mirrorInput.decision.status, "rejected");

  terminalSocket.send(JSON.stringify({ type: "request_snapshot" }));
  const liveAfterStaleMessages = await inbox.next((message) => message.type === "snapshot");
  assert.equal(liveAfterStaleMessages.snapshot.runtime_generation, terminalSnapshot.runtime_generation);
  assert.equal(terminalSocket.readyState, WebSocket.OPEN);

  terminalSocket.send(JSON.stringify({
    type: "begin_activation",
    runtime_generation: registered.broker_state.runtime_generation,
    observed_lease_epoch: desktopAfterRemoteViewport.broker_state.lease_epoch,
  }));
  const remoteBegin = await inbox.next((message) => message.type === "activation_begin");
  assert.equal(remoteBegin.result.decision.status, "accepted");
  assert.ok(remoteBegin.result.activation_id);
  assert.ok(remoteBegin.result.snapshot);
  terminalSocket.send(JSON.stringify({
    type: "ack_activation",
    runtime_generation: remoteBegin.result.decision.runtime_generation,
    lease_epoch: remoteBegin.result.decision.lease_epoch,
    activation_id: remoteBegin.result.activation_id,
  }));
  const remoteAck = await inbox.next((message) => message.type === "activation_ack");
  assert.equal(remoteAck.result.decision.status, "accepted");
  assert.equal(remoteAck.result.broker_state.owner_presentation_id, remotePresentationId);
  assert.deepEqual(remoteAck.result.broker_state.geometry, { cols: 220, rows: 70 });

  terminalSocket.send(JSON.stringify({
    type: "input",
    runtime_generation: remoteAck.result.broker_state.runtime_generation,
    lease_epoch: remoteAck.result.broker_state.lease_epoch,
    data: `REMOTE_OWNER_INPUT_${RUN_ID}\r`,
  }));
  const remoteOwnerInput = await inbox.next(
    (message) => message.type === "input_result" && message.decision?.status === "accepted",
  );
  assert.equal(remoteOwnerInput.decision.owner_presentation_id, remotePresentationId);

  await readDesktopBrokerState(
    session.driver,
    sessionId,
    desktopPresentationId,
    terminalSnapshot.runtime_generation,
  );
  const desktopTakeoverBegin = await invokeTauri(session.driver, "begin_terminal_activation", {
    request: {
      session_id: sessionId,
      presentation_id: desktopPresentationId,
      runtime_generation: remoteAck.result.broker_state.runtime_generation,
      observed_lease_epoch: remoteAck.result.broker_state.lease_epoch,
    },
  });
  assert.equal(desktopTakeoverBegin.decision.status, "accepted");
  const desktopTakeoverAck = await invokeTauri(session.driver, "ack_terminal_activation", {
    request: {
      session_id: sessionId,
      presentation_id: desktopPresentationId,
      runtime_generation: desktopTakeoverBegin.decision.runtime_generation,
      lease_epoch: desktopTakeoverBegin.decision.lease_epoch,
      activation_id: desktopTakeoverBegin.activation_id,
    },
  });
  assert.equal(desktopTakeoverAck.decision.status, "accepted");
  assert.equal(desktopTakeoverAck.broker_state.owner_presentation_id, desktopPresentationId);

  terminalSocket.send(JSON.stringify({
    type: "input",
    runtime_generation: remoteAck.result.broker_state.runtime_generation,
    lease_epoch: remoteAck.result.broker_state.lease_epoch,
    data: `STALE_AFTER_DESKTOP_TAKEOVER_${RUN_ID}\r`,
  }));
  const staleAfterDesktopTakeover = await inbox.next(
    (message) => message.type === "input_result" && message.decision?.status === "rejected",
  );
  assert.equal(staleAfterDesktopTakeover.decision.reason, "lease_epoch_changed");
  terminalSocket.send(JSON.stringify({ type: "request_snapshot" }));
  await inbox.next((message) => message.type === "snapshot");
  assert.equal(terminalSocket.readyState, WebSocket.OPEN);

  await readDesktopBrokerState(
    session.driver,
    sessionId,
    desktopPresentationId,
    terminalSnapshot.runtime_generation,
  );
  terminalSocket.send(JSON.stringify({
    type: "begin_activation",
    runtime_generation: desktopTakeoverAck.broker_state.runtime_generation,
    observed_lease_epoch: desktopTakeoverAck.broker_state.lease_epoch,
  }));
  const remoteFallbackBegin = await inbox.next((message) => message.type === "activation_begin");
  terminalSocket.send(JSON.stringify({
    type: "ack_activation",
    runtime_generation: remoteFallbackBegin.result.decision.runtime_generation,
    lease_epoch: remoteFallbackBegin.result.decision.lease_epoch,
    activation_id: remoteFallbackBegin.result.activation_id,
  }));
  const remoteFallbackAck = await inbox.next((message) => message.type === "activation_ack");
  assert.equal(remoteFallbackAck.result.broker_state.owner_presentation_id, remotePresentationId);

  await invokeTauri(session.driver, "debug_remove_agent_input_sender", {
    sessionId,
  });
  const terminated = await inbox.next(
    (message) => message.type === "error" && message.code === "terminal_runtime_terminated",
  );
  assert.equal(terminated.fatal, false);
  assert.equal(terminalSocket.readyState, WebSocket.OPEN);

  await invokeTauri(session.driver, "resume_agent", { sessionId });
  const replacementRegistered = await inbox.next(
    (message) => message.type === "registered"
      && message.broker_state?.runtime_generation > terminalSnapshot.runtime_generation,
    30000,
  );
  const replacementGeneration = replacementRegistered.broker_state.runtime_generation;
  assert.equal(
    replacementGeneration,
    terminalSnapshot.runtime_generation + 1,
    "terminate/remove followed by same-session recreation must advance the generation",
  );
  assert.equal(replacementRegistered.presentation.presentation_id, remotePresentationId);
  assert.equal(replacementRegistered.initial_snapshot.runtime_generation, replacementGeneration);
  assert.equal(terminalSocket.readyState, WebSocket.OPEN);

  const replacementDesktop = await waitFor(
    "mounted desktop presentation to rebind to replacement runtime",
    30000,
    async () => {
      const value = await readDesktopBrokerState(
        session.driver,
        sessionId,
        desktopPresentationId,
        replacementGeneration,
      );
      return {
        ok: value.presentation.presentation_id === desktopPresentationId
          && value.broker_state.runtime_generation === replacementGeneration,
        value,
      };
    },
  );
  assert.equal(
    replacementDesktop.value.broker_state.owner_presentation_id,
    desktopPresentationId,
    "the mounted desktop presentation must reclaim input ownership after runtime replacement",
  );

  const replacementEvents = await inbox.next(
    (message) => message.type === "events"
      && message.batch?.runtime_generation === replacementGeneration
      && message.batch.events?.some((event) => event.type === "output"),
    30000,
  );
  assert.ok(
    replacementEvents.batch.events.some((event) => event.runtime_generation === replacementGeneration),
    "the connected remote consumer must drain replacement-generation output",
  );

  terminalSocket.send(JSON.stringify({
    type: "begin_activation",
    runtime_generation: replacementGeneration,
    observed_lease_epoch: replacementDesktop.value.broker_state.lease_epoch,
  }));
  const replacementRemoteBegin = await inbox.next(
    (message) => message.type === "activation_begin"
      && message.result?.decision?.runtime_generation === replacementGeneration,
  );
  terminalSocket.send(JSON.stringify({
    type: "ack_activation",
    runtime_generation: replacementGeneration,
    lease_epoch: replacementRemoteBegin.result.decision.lease_epoch,
    activation_id: replacementRemoteBegin.result.activation_id,
  }));
  const replacementRemoteAck = await inbox.next(
    (message) => message.type === "activation_ack"
      && message.result?.broker_state?.runtime_generation === replacementGeneration,
  );
  assert.equal(
    replacementRemoteAck.result.broker_state.owner_presentation_id,
    remotePresentationId,
  );

  terminalSocket.terminate();
  await waitForSocketClose(terminalSocket);
  const fallback = await waitFor("desktop fallback after remote disconnect", 30000, async () => {
    const value = await readDesktopBrokerState(
      session.driver,
      sessionId,
      desktopPresentationId,
      replacementGeneration,
    );
    return {
      ok: value.broker_state.owner_presentation_id === desktopPresentationId
        && value.broker_state.pending_activation === null,
      value,
    };
  });
  const fallbackGeometry = fallback.value.broker_state.geometry;
  assert.ok(
    Number.isInteger(fallbackGeometry.cols)
      && fallbackGeometry.cols >= 20
      && fallbackGeometry.cols <= 500,
    `fallback columns are outside the desktop broker range: ${JSON.stringify(fallbackGeometry)}`,
  );
  assert.ok(
    Number.isInteger(fallbackGeometry.rows)
      && fallbackGeometry.rows >= 8
      && fallbackGeometry.rows <= 200,
    `fallback rows are outside the desktop broker range: ${JSON.stringify(fallbackGeometry)}`,
  );
  const fallbackConsistency = await waitFor(
    "desktop fallback broker and snapshot geometry consistency",
    30000,
    async () => {
      const broker = await readDesktopBrokerState(
        session.driver,
        sessionId,
        desktopPresentationId,
        replacementGeneration,
      );
      const snapshot = await invokeTauri(session.driver, "request_terminal_snapshot", {
        request: { session_id: sessionId },
      });
      return {
        ok: broker.broker_state.owner_presentation_id === desktopPresentationId
          && broker.broker_state.runtime_generation === snapshot.runtime_generation
          && broker.broker_state.geometry.cols === snapshot.geometry.cols
          && broker.broker_state.geometry.rows === snapshot.geometry.rows,
        broker,
        snapshot,
      };
    },
  );
  assert.deepEqual(
    fallbackConsistency.snapshot.geometry,
    fallbackConsistency.broker.broker_state.geometry,
  );
});
