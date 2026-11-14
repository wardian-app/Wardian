import https from "node:https";
import fs from "node:fs";
import net from "node:net";
import path from "node:path";
import { performance } from "node:perf_hooks";

import {
  createNativeHarness,
  ensureNativeAppBuilt,
  invokeTauri,
  prepareIsolatedHome,
  startNativeSession,
  waitForAppShell,
} from "./harness.mjs";
import { createRemoteClientAcceptanceFixture, readMockTranscript } from "./remoteClientAcceptanceFixture.mjs";
import { startRemoteClientTlsFixture } from "./remoteClientTlsFixture.mjs";

const DEFAULT_HOLD_TIMEOUT_MS = 180_000;
const DEFAULT_STARTUP_TIMEOUT_MS = 20_000;
const CLIENT_READ_GRANTS = Object.freeze(["agents:read", "chat:read"]);

function exactOrigin(value) {
  if (typeof value !== "string") throw new TypeError("hudOrigin must be an exact HTTP(S) origin");
  let parsed;
  try {
    parsed = new URL(value);
  } catch {
    throw new TypeError("hudOrigin must be an exact HTTP(S) origin");
  }
  if (!["http:", "https:"].includes(parsed.protocol)
    || parsed.origin !== value
    || parsed.username
    || parsed.password
    || parsed.pathname !== "/"
    || parsed.search
    || parsed.hash) {
    throw new TypeError("hudOrigin must contain only the exact HTTP(S) origin");
  }
  return parsed.origin;
}

function equalStrings(actual, expected) {
  const left = [...actual].sort();
  const right = [...expected].sort();
  return left.length === right.length && left.every((value, index) => value === right[index]);
}

function sameStrings(actual, expected, label) {
  if (!Array.isArray(actual) || !Array.isArray(expected)) {
    throw new TypeError(`${label} must be an array`);
  }
  if (!equalStrings(actual, expected)) {
    throw new Error(`${label} did not match the requested grants`);
  }
}

/** Validate an approval against the pending DTOs read from this native session. */
export function validatePendingClientPairing({
  pendingRequests,
  requestId,
  publicKeyFingerprint,
  gatewayOrigin,
  hudOrigin,
  readGrants,
  sendGrants,
}) {
  if (!Array.isArray(pendingRequests)) throw new TypeError("pendingRequests must be the native-session DTO list");
  if (typeof requestId !== "string" || requestId.length === 0) throw new TypeError("requestId is required");
  const matches = pendingRequests.filter((request) => request?.request_id === requestId);
  if (matches.length !== 1) throw new Error("pairing request is not pending in the owning native session");
  const request = matches[0];
  if (request.canonical_origin !== gatewayOrigin) throw new Error("pending pairing gateway origin did not match the owned fixture");
  if (request.client_origin !== hudOrigin) throw new Error("pending pairing HUD origin did not match the owned fixture");
  if (request.credential_kind !== "client") throw new Error("pending pairing credential kind was not paired-client");
  if (request.audience !== "wardian_remote_client_v1") throw new Error("pending pairing audience was not paired-client v1");
  if (request.public_key_fingerprint !== publicKeyFingerprint) {
    throw new Error("pending pairing fingerprint did not match the browser submission");
  }
  sameStrings(request.read_grants, readGrants, "pending read grants");
  sameStrings(request.send_grants, sendGrants, "pending send grants");
  return request;
}

/**
 * Bind approval to the validated public pending DTO, which intentionally omits device_id.
 * The browser independently checks the selected ID against its pairing-submit response.
 */
export function validateApprovedClientDevice(devices, pending) {
  if (!Array.isArray(devices)) throw new TypeError("devices must be the native approval result list");
  const matches = devices.filter((device) => device
    && device.public_key_fingerprint === pending.public_key_fingerprint
    && device.client_origin === pending.client_origin
    && device.credential_kind === pending.credential_kind
    && device.audience === pending.audience
    && ["read_grants", "send_grants"].every((field) => Array.isArray(device[field])
      && equalStrings(device[field], pending[field])));
  if (matches.length !== 1) {
    throw new Error("native approval result did not uniquely match the validated pending request");
  }
  const device = matches[0];
  if (typeof device.device_id !== "string" || device.device_id.trim().length === 0
    || device.revoked_at !== null) {
    throw new Error("native approval result must contain a nonrevoked device with a nonempty device_id");
  }
  sameStrings(device.read_grants, pending.read_grants, "approved read grants");
  sameStrings(device.send_grants, pending.send_grants, "approved send grants");
  return device;
}

function getFreePort() {
  return new Promise((resolve, reject) => {
    const server = net.createServer();
    server.once("error", reject);
    server.listen(0, "127.0.0.1", () => {
      const address = server.address();
      server.close((error) => {
        if (error) return reject(error);
        if (!address || typeof address === "string") return reject(new Error("unable to allocate a loopback gateway port"));
        resolve(address.port);
      });
    });
  });
}

function writeGatewayConfig(harness, port, gatewayOrigin, hudOrigin) {
  const remoteAccessDir = path.join(harness.isolatedHome, "remote-access");
  fs.mkdirSync(remoteAccessDir, { recursive: true });
  fs.writeFileSync(path.join(remoteAccessDir, "config.json"), JSON.stringify({
    schema_version: 1,
    enabled: true,
    canonical_origin: gatewayOrigin,
    client_origin_allowlist: [hudOrigin],
    loopback_host: "127.0.0.1",
    loopback_port: port,
    gateway_identity_public_key: "e2e-public-key",
    gateway_identity_fingerprint: "e2e-fingerprint",
  }, null, 2));
}

async function waitForGateway(url, timeoutMs) {
  const deadlineAt = performance.now() + timeoutMs;
  let lastError = "gateway has not responded";
  while (performance.now() < deadlineAt) {
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(new Error("gateway health request reached its deadline")), deadlineAt - performance.now());
    try {
      const response = await fetch(`${url}/remote/api/health`, { signal: controller.signal });
      if (performance.now() >= deadlineAt) break;
      if (response.ok) return response;
      lastError = `gateway health returned ${response.status}`;
    } catch (error) {
      lastError = String(error);
    } finally {
      clearTimeout(timer);
    }
    const sleepMs = Math.min(100, deadlineAt - performance.now());
    if (sleepMs <= 0) break;
    await new Promise((resolve) => setTimeout(resolve, sleepMs));
  }
  throw new Error(`Timed out waiting for the owned native gateway: ${lastError}`);
}

function requestTls(tlsFixture, requestPath, {
  method = "GET",
  headers = {},
  body,
  signal,
  timeoutMs = 15_000,
  tlsOptions = {},
} = {}) {
  const requestTimeoutMs = Math.max(1, Math.min(15_000, Math.ceil(timeoutMs)));
  return new Promise((resolve, reject) => {
    const request = https.request({
      hostname: tlsFixture.ready.address.host,
      port: tlsFixture.ready.address.port,
      path: requestPath,
      method,
      headers,
      ca: tlsFixture.certificatePem,
      rejectUnauthorized: true,
      ...tlsOptions,
      timeout: requestTimeoutMs,
      signal,
    }, (response) => {
      const chunks = [];
      response.on("data", (chunk) => chunks.push(chunk));
      response.once("error", reject);
      response.once("end", () => resolve({
        status: response.statusCode,
        headers: response.headers,
        body: Buffer.concat(chunks).toString("utf8"),
      }));
    });
    request.once("timeout", () => request.destroy(new Error("TLS acceptance request timed out")));
    request.once("error", reject);
    request.end(body);
  });
}

function safeWorkspaceName(value) {
  if (typeof value !== "string" || !/^[A-Za-z0-9][A-Za-z0-9_-]{0,63}$/u.test(value)) {
    throw new TypeError("workspace name must be a short filesystem-safe fixture label");
  }
  return value;
}

function assertOpen(state) {
  if (["closing", "closed", "setup_failed"].includes(state())) {
    throw new Error(`native acceptance fixture is ${state()}`);
  }
}

/**
 * Start an isolated native gateway and its pinned TLS boundary for a caller-owned HUD origin.
 * Browser code owns its non-extractable key and submits the offer returned by `controls.createOffer`.
 */
export async function createNativeAcceptanceFixture({
  hudOrigin,
  sourceCommit,
  sourceDeltaDigest,
  onReady,
  holdTimeoutMs = DEFAULT_HOLD_TIMEOUT_MS,
  startupTimeoutMs = DEFAULT_STARTUP_TIMEOUT_MS,
  enableTestSupport = false,
}) {
  const clientOrigin = exactOrigin(hudOrigin);
  if (typeof sourceCommit !== "string" || !sourceCommit) throw new TypeError("sourceCommit is required");
  if (typeof sourceDeltaDigest !== "string" || !/^[0-9a-f]{64}$/u.test(sourceDeltaDigest)) {
    throw new TypeError("sourceDeltaDigest must be a lowercase SHA-256 digest");
  }
  if (!Number.isFinite(startupTimeoutMs) || startupTimeoutMs <= 0) {
    throw new TypeError("startupTimeoutMs must be a positive finite duration");
  }

  const owned = Object.create(null);
  const activeOperations = new Set();
  const responseObservations = [];
  const lifecycle = createRemoteClientAcceptanceFixture({
    hudOrigin: clientOrigin,
    holdTimeoutMs,
    onReady,
    close: async () => {
      const operations = [...activeOperations];
      for (const operation of operations) operation.controller.abort(new Error("native acceptance fixture is closing"));
      await Promise.allSettled(operations.map((operation) => operation.promise));
      const session = owned.session;
      owned.session = undefined;
      const tlsFixture = owned.tlsFixture;
      owned.tlsFixture = undefined;
      const results = await Promise.allSettled([
        session?.close(),
        tlsFixture?.close(),
      ].filter(Boolean));
      const failed = results.find((result) => result.status === "rejected");
      if (failed) throw failed.reason;
    },
  });

  await lifecycle.setup(async () => {
    const tlsFixture = await startRemoteClientTlsFixture({
      sourceCommit,
      sourceDeltaDigest,
      onResponse: (observation) => responseObservations.push(Object.freeze({
        sequence: responseObservations.length + 1,
        ...observation,
      })),
    });
    owned.tlsFixture = tlsFixture;
    const harness = await createNativeHarness();
    owned.harness = harness;
    prepareIsolatedHome(harness);
    const loopbackPort = await getFreePort();
    const loopbackOrigin = `http://127.0.0.1:${loopbackPort}`;
    writeGatewayConfig(harness, loopbackPort, tlsFixture.ready.gateway_origin, clientOrigin);
    if (process.env.WARDIAN_NATIVE_SKIP_BUILD !== "1") ensureNativeAppBuilt(harness);
    const session = await startNativeSession(harness);
    owned.session = session;
    await waitForAppShell(session.driver, startupTimeoutMs);
    const health = await waitForGateway(loopbackOrigin, startupTimeoutMs);
    tlsFixture.setUpstream({ host: "127.0.0.1", port: loopbackPort });
    owned.loopbackOrigin = loopbackOrigin;
    owned.backendHealth = health.status === 200;
    return { harness, session, tlsFixture, loopbackOrigin, backendHealth: health.status === 200 };
  });

  const context = Object.freeze({
    hud_origin: clientOrigin,
    tls: owned.tlsFixture.ready,
    gateway_origin: owned.tlsFixture.ready.gateway_origin,
    certificate_sha256: owned.tlsFixture.ready.certificate_sha256,
    certificate_spki_sha256_base64: owned.tlsFixture.ready.certificate_spki_sha256_base64,
    source_commit: owned.tlsFixture.ready.source_commit,
    source_delta_sha256: owned.tlsFixture.ready.source_delta_sha256,
    app_sha256: owned.harness.frozenArtifacts.app.sha256,
    cli_sha256: owned.harness.frozenArtifacts.cli.sha256,
  });

  let releaseResolver;
  let releaseRequested = false;
  let holdPromise;
  let approvedDevice;
  const lifecycleState = () => lifecycle.state;
  const runSessionOperation = (operation) => {
    assertOpen(lifecycleState);
    const entry = { controller: new AbortController(), promise: undefined };
    activeOperations.add(entry);
    const pending = Promise.resolve().then(() => operation(entry.controller.signal));
    entry.promise = pending.finally(() => activeOperations.delete(entry));
    return entry.promise;
  };
  const requireSession = () => {
    assertOpen(lifecycleState);
    if (!owned.session) throw new Error("owned native session is unavailable");
    return owned.session;
  };
  const requestTlsFromFixture = (requestPath, options) => requestTls(owned.tlsFixture, requestPath, options);
  const createOffer = () => runSessionOperation(async (signal) => {
    const session = requireSession();
    const offer = await invokeTauri(session.driver, "create_remote_client_pairing_offer", { clientOrigin });
    if (signal.aborted || lifecycle.state === "closing" || lifecycle.state === "closed") {
      throw Object.assign(new Error("fixture closed while creating a pairing offer"), { name: "AbortError" });
    }
    if (offer?.gateway_origin !== context.gateway_origin
      || offer?.client_origin !== clientOrigin
      || offer?.credential_kind !== "client") {
      throw new Error("native pairing offer did not match the owned client fixture");
    }
    sameStrings(offer.read_grants, CLIENT_READ_GRANTS, "pairing offer read grants");
    return Object.freeze(offer);
  });
  const approvePending = (input) => runSessionOperation(async (signal) => {
    const session = requireSession();
    const { requestId, publicKeyFingerprint, readGrants, sendGrants } = input ?? {};
    const requests = await invokeTauri(session.driver, "list_pending_remote_pairing_requests");
    if (signal.aborted || lifecycle.state === "closing" || lifecycle.state === "closed") {
      throw Object.assign(new Error("fixture closed before pairing validation"), { name: "AbortError" });
    }
    const pending = validatePendingClientPairing({
      pendingRequests: requests,
      requestId,
      publicKeyFingerprint,
      gatewayOrigin: context.gateway_origin,
      hudOrigin: clientOrigin,
      readGrants,
      sendGrants,
    });
    if (signal.aborted || lifecycle.state === "closing" || lifecycle.state === "closed") {
      throw Object.assign(new Error("fixture closed before pairing approval"), { name: "AbortError" });
    }
    const devices = await invokeTauri(session.driver, "approve_remote_pairing_request", { requestId });
    if (signal.aborted || lifecycle.state === "closing" || lifecycle.state === "closed") {
      throw Object.assign(new Error("fixture closed while pairing approval was in flight"), { name: "AbortError" });
    }
    const device = validateApprovedClientDevice(devices, pending);
    sameStrings(device.read_grants, readGrants, "approved read grants");
    sameStrings(device.send_grants, sendGrants, "approved send grants");
    approvedDevice = Object.freeze({
      device_id: device.device_id,
      public_key_fingerprint: device.public_key_fingerprint,
      credential_kind: device.credential_kind,
      audience: device.audience,
      client_origin: device.client_origin,
      read_grants: Object.freeze([...device.read_grants]),
      send_grants: Object.freeze([...device.send_grants]),
    });
    return approvedDevice;
  });
  const revoke = () => runSessionOperation(async (signal) => {
    const session = requireSession();
    if (!approvedDevice) throw new Error("this fixture has no approved client device to revoke");
    const devices = await invokeTauri(session.driver, "list_remote_devices");
    if (signal.aborted || lifecycle.state === "closing" || lifecycle.state === "closed") {
      throw Object.assign(new Error("fixture closed before owned-device revocation"), { name: "AbortError" });
    }
    const device = devices.find((item) => item?.device_id === approvedDevice.device_id);
    if (!device
      || device.public_key_fingerprint !== approvedDevice.public_key_fingerprint
      || device.credential_kind !== "client"
      || device.audience !== "wardian_remote_client_v1"
      || device.client_origin !== clientOrigin) {
      throw new Error("owned paired-client record changed before revocation");
    }
    sameStrings(device.read_grants, approvedDevice.read_grants, "revoked device read grants");
    sameStrings(device.send_grants, approvedDevice.send_grants, "revoked device send grants");
    const updatedDevices = await invokeTauri(session.driver, "revoke_remote_device", { deviceId: approvedDevice.device_id });
    const revoked = updatedDevices.find((item) => item?.device_id === approvedDevice.device_id);
    if (!revoked?.revoked_at) throw new Error("native revocation result did not mark the owned device revoked");
    approvedDevice = undefined;
  });
  const ready = (evidence) => lifecycle.ready({
    backend_health: owned.backendHealth === true,
    authorized_roster: evidence?.authorized_roster === true,
    chat_ready: evidence?.chat_ready === true,
    gateway_url: context.gateway_origin,
    certificate_sha256: context.certificate_sha256,
    certificate_spki_sha256_base64: context.certificate_spki_sha256_base64,
    source_commit: context.source_commit,
    source_delta_sha256: context.source_delta_sha256,
    app_sha256: context.app_sha256,
    cli_sha256: context.cli_sha256,
    test_agent_ids: evidence?.test_agent_ids,
    ownership: { run_id: owned.harness.runId },
  });
  const holdUntilReleased = () => {
    if (holdPromise) return holdPromise;
    holdPromise = lifecycle.waitForExternalRelease(({ signal }) => new Promise((resolve, reject) => {
      let settled = false;
      const finish = (callback, value) => {
        if (settled) return;
        settled = true;
        signal.removeEventListener("abort", onAbort);
        releaseResolver = undefined;
        callback(value);
      };
      const onAbort = () => finish(reject, signal.reason);
      if (signal.aborted) return onAbort();
      signal.addEventListener("abort", onAbort, { once: true });
      releaseResolver = () => finish(resolve);
      if (releaseRequested) releaseResolver();
    }));
    return holdPromise;
  };
  const release = () => {
    if (lifecycle.state === "closed" || lifecycle.state === "closing") return false;
    releaseRequested = true;
    releaseResolver?.();
    return true;
  };
  const restart = () => runSessionOperation(async (signal) => {
    const previous = owned.session;
    if (previous) {
      owned.session = undefined;
      await previous.close();
      if (signal.aborted) throw Object.assign(new Error("fixture closed during native restart"), { name: "AbortError" });
      if (previous.tauriDriver.exitCode === null && previous.tauriDriver.signalCode === null) {
        throw new Error("owned native session was not joined before restart");
      }
    }
    const session = await startNativeSession(owned.harness);
    owned.session = session;
    await waitForAppShell(session.driver, startupTimeoutMs);
    if (signal.aborted) throw Object.assign(new Error("fixture closed during native restart"), { name: "AbortError" });
    const health = await waitForGateway(owned.loopbackOrigin, startupTimeoutMs);
    if (health.status !== 200) throw new Error(`restarted native gateway returned ${health.status}`);
    return Object.freeze({ previous_session_joined: Boolean(previous), backend_health: true });
  });
  const workspacePath = (name) => {
    const safeName = safeWorkspaceName(name);
    const folder = path.join(owned.harness.isolatedHome, "workspaces", safeName);
    fs.mkdirSync(folder, { recursive: true });
    return folder.split(path.sep).join("/");
  };
  const spawnAgent = ({ sessionName, workspaceName = sessionName, isOff = false, scenario, delayMs = 0 }) => runSessionOperation(async (signal) => {
    const session = requireSession();
    const folder = workspacePath(workspaceName);
    const configOverride = { provider: "mock" };
    if (scenario) configOverride.provider_config = { type: "mock", scenario, delay_ms: delayMs };
    const agent = await invokeTauri(session.driver, "spawn_agent", {
      req: {
        sessionName,
        agentClass: "Coder",
        folder,
        resumeSession: null,
        isOff,
        configOverride,
      },
    });
    if (signal.aborted) throw Object.assign(new Error("fixture closed while spawning a test agent"), { name: "AbortError" });
    return agent;
  });
  const transcript = (sessionId) => {
    if (typeof sessionId !== "string" || !/^[A-Za-z0-9_-]{1,128}$/u.test(sessionId)) {
      throw new TypeError("sessionId must be an owned fixture agent identifier");
    }
    return readMockTranscript(path.join(owned.harness.isolatedHome, "agents", sessionId, "mock-transcript.jsonl"));
  };

  const controls = Object.freeze({
    createOffer,
    approvePending,
    revoke,
    release,
    close: () => lifecycle.close(),
  });
  const testSupport = enableTestSupport ? Object.freeze({
    requestTls: requestTlsFromFixture,
    spawnAgent,
    workspacePath,
    readMockTranscript: transcript,
    restart,
    responseCursor: () => responseObservations.length,
    responsesSince: (cursor) => {
      if (!Number.isSafeInteger(cursor) || cursor < 0 || cursor > responseObservations.length) {
        throw new RangeError("response cursor is outside this fixture's observations");
      }
      return Object.freeze(responseObservations.filter((observation) => observation.sequence > cursor));
    },
  }) : undefined;

  return Object.freeze({
    context,
    get state() { return lifecycle.state; },
    ready,
    holdUntilReleased,
    controls,
    ...(testSupport ? { testSupport } : {}),
  });
}
