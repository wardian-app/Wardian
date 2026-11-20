// @tier ci — Scoped TLS fixture forwarding and socket cleanup; no app or provider.
import test from "node:test";
import assert from "node:assert/strict";
import crypto from "node:crypto";
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import http from "node:http";
import https from "node:https";
import path from "node:path";
import tls from "node:tls";
import { fileURLToPath } from "node:url";
import { once } from "node:events";
import os from "node:os";

import {
  countMockTranscriptNonceEvents,
  createRemoteClientAcceptanceFixture,
  mockTranscriptDelta,
  readMockTranscript,
  sha256Bytes,
  submitDuplicateRemoteClientRequests,
  waitForRemoteClientCondition,
} from "../lib/remoteClientAcceptanceFixture.mjs";
import { validateApprovedClientDevice, validatePendingClientPairing } from "../lib/nativeAcceptanceFixture.mjs";

import { startRemoteClientTlsFixture } from "../lib/remoteClientTlsFixture.mjs";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");

function sourceIdentity() {
  const sourceCommit = execFileSync("git", ["rev-parse", "HEAD"], {
    cwd: repoRoot,
    encoding: "utf8",
  }).trim();
  const diff = execFileSync("git", ["diff", "--binary", "HEAD"], {
    cwd: repoRoot,
    encoding: "buffer",
  });
  const untracked = execFileSync("git", [
    "ls-files",
    "--others",
    "--exclude-standard",
    "--",
    "e2e-native/lib/remoteClientTlsFixture.mjs",
    "e2e-native/tests/remote-client-conformance.test.mjs",
    "e2e-native/tests/remote-client-tls-fixture.test.mjs",
    "e2e-native/lib/remoteClientAcceptanceFixture.mjs",
    "e2e-native/lib/nativeAcceptanceFixture.mjs",
    "e2e-native/lib/remoteClientTlsFixture.mjs",
    "src-tauri/src/remote/client_messages.rs",
  ], {
    cwd: repoRoot,
    encoding: "utf8",
  }).split(/\r?\n/).filter(Boolean).sort();
  const digest = crypto.createHash("sha256").update(diff);
  for (const relativePath of untracked) {
    digest.update(relativePath).update("\0");
    digest.update(fs.readFileSync(path.join(repoRoot, relativePath)));
  }
  return { sourceCommit, sourceDeltaDigest: digest.digest("hex") };
}

function deferred() {
  let resolve;
  let reject;
  const promise = new Promise((complete, fail) => { resolve = complete; reject = fail; });
  return { promise, resolve, reject };
}

function fakeClock(start = 0) {
  let current = start;
  let nextId = 0;
  const timers = new Map();
  const runDue = () => {
    const due = [...timers.entries()]
      .filter(([, timer]) => timer.at <= current)
      .sort((left, right) => left[1].at - right[1].at || left[0] - right[0]);
    for (const [id, timer] of due) {
      if (!timers.delete(id)) continue;
      timer.callback();
    }
  };
  return {
    now: () => current,
    schedule(callback, delay) {
      const id = ++nextId;
      timers.set(id, { at: current + delay, callback });
      return id;
    },
    cancel(id) { timers.delete(id); },
    advance(delay) { current += delay; runDue(); },
    set(value) { current = value; },
    get pendingTimers() { return timers.size; },
  };
}

function readyEvidence(overrides = {}) {
  return {
    backend_health: true,
    authorized_roster: true,
    chat_ready: true,
    gateway_url: "https://127.0.0.1:48123",
    certificate_sha256: "a".repeat(64),
    certificate_spki_sha256_base64: "A".repeat(43) + "=",
    source_commit: "994a3b7f2899415a335122ab8c9cc5111224200f",
    source_delta_sha256: "b".repeat(64),
    app_sha256: "c".repeat(64),
    cli_sha256: "d".repeat(64),
    test_agent_ids: ["agent-roster", "agent-chat"],
    ownership: { run_id: "owned-run", tauri_driver_pid: 42 },
    bearer: "must-not-be-copied-to-ready-record",
    private_key: "must-not-be-copied-to-ready-record",
    ...overrides,
  };
}

test("remote Chat readiness uses one absolute deadline for requests and sleeps", async () => {
  const clock = fakeClock();
  const never = deferred();
  let requestCount = 0;
  let requestSignal;
  const pending = waitForRemoteClientCondition({
    timeoutMs: 10,
    now: clock.now,
    schedule: clock.schedule,
    cancel: clock.cancel,
    request: ({ signal, timeoutMs }) => {
      requestCount += 1;
      requestSignal = signal;
      assert.equal(timeoutMs, 10);
      return never.promise;
    },
    inspect: () => ({ ready: true, value: "late" }),
  });
  await Promise.resolve();
  clock.advance(10);
  await assert.rejects(pending, (error) => error.name === "TimeoutError");
  assert.equal(requestCount, 1);
  assert.equal(requestSignal.aborted, true);
  never.resolve({ status: 200 });
  assert.equal(clock.pendingTimers, 0);
});

test("remote Chat readiness rejects a late-ready response without inspecting it", async () => {
  const clock = fakeClock();
  const response = deferred();
  let inspectCount = 0;
  const pending = waitForRemoteClientCondition({
    timeoutMs: 20,
    now: clock.now,
    schedule: clock.schedule,
    cancel: clock.cancel,
    request: () => response.promise,
    inspect: () => {
      inspectCount += 1;
      return { ready: true, value: "late-ready" };
    },
  });
  await Promise.resolve();
  clock.advance(20);
  await assert.rejects(pending, (error) => error.name === "TimeoutError");
  response.resolve({ status: 200 });
  await Promise.resolve();
  assert.equal(inspectCount, 0);
});

test("remote Chat readiness rejects a request that resolves after the deadline", async () => {
  const clock = fakeClock();
  let inspectCount = 0;
  await assert.rejects(waitForRemoteClientCondition({
    timeoutMs: 25,
    now: clock.now,
    schedule: clock.schedule,
    cancel: clock.cancel,
    request: async () => {
      clock.set(26);
      return { status: 200 };
    },
    inspect: () => {
      inspectCount += 1;
      return { ready: true, value: "overrun" };
    },
  }), (error) => error.name === "TimeoutError");
  assert.equal(inspectCount, 0);
  assert.equal(clock.pendingTimers, 0);
});

test("remote Chat readiness cancellation aborts the in-flight request and prevents another poll", async () => {
  const clock = fakeClock();
  const cancellation = new AbortController();
  const pendingRequest = deferred();
  let requestCount = 0;
  let requestWasAborted = false;
  const pending = waitForRemoteClientCondition({
    timeoutMs: 100,
    signal: cancellation.signal,
    now: clock.now,
    schedule: clock.schedule,
    cancel: clock.cancel,
    request: ({ signal }) => {
      requestCount += 1;
      signal.addEventListener("abort", () => { requestWasAborted = true; }, { once: true });
      return pendingRequest.promise;
    },
    inspect: () => ({ ready: false }),
  });
  await Promise.resolve();
  cancellation.abort(new Error("caller cancelled"));
  await assert.rejects(pending, /caller cancelled/u);
  assert.equal(requestCount, 1);
  assert.equal(requestWasAborted, true);
  assert.equal(clock.pendingTimers, 0);
});

test("remote Chat readiness returns an observed ready value before its deadline", async () => {
  const clock = fakeClock();
  let requestTimeout;
  const result = await waitForRemoteClientCondition({
    timeoutMs: 30_000,
    now: clock.now,
    schedule: clock.schedule,
    cancel: clock.cancel,
    request: async ({ timeoutMs }) => {
      requestTimeout = timeoutMs;
      return { status: 200, body: { progress: "ready" } };
    },
    inspect: (response) => ({ ready: response.body.progress === "ready", value: response.body }),
  });
  assert.deepEqual(result, { progress: "ready" });
  assert.equal(requestTimeout, 15_000);
  assert.equal(clock.pendingTimers, 0);
});

test("a poll sleep is clipped to the absolute deadline and starts no later request", async () => {
  const clock = fakeClock();
  let requestCount = 0;
  const pending = waitForRemoteClientCondition({
    timeoutMs: 10,
    intervalMs: 50,
    now: clock.now,
    schedule: clock.schedule,
    cancel: clock.cancel,
    request: async () => {
      requestCount += 1;
      return { status: 200 };
    },
    inspect: () => ({ ready: false }),
  });
  await Promise.resolve();
  await Promise.resolve();
  clock.advance(10);
  await assert.rejects(pending, (error) => error.name === "TimeoutError");
  assert.equal(requestCount, 1);
});

test("acceptance fixture gates readiness, emits only public pins, and closes owned setup once", async () => {
  const clock = fakeClock();
  const callbackOrder = [];
  let readyRecord;
  const fixture = createRemoteClientAcceptanceFixture({
    hudOrigin: "https://hud.example",
    holdTimeoutMs: 20,
    now: clock.now,
    schedule: clock.schedule,
    cancel: clock.cancel,
    onReady: (record) => { readyRecord = record; callbackOrder.push("ready"); },
    close: async ({ owned }) => {
      callbackOrder.push("close");
      assert.equal(owned.session_id, "owned-session");
    },
  });
  await fixture.setup(async ({ hudOrigin, owned }) => {
    callbackOrder.push("setup");
    assert.equal(hudOrigin, "https://hud.example");
    owned.session_id = "owned-session";
  });
  assert.throws(() => fixture.ready(readyEvidence({ chat_ready: false })), /cannot become ready/u);
  const record = fixture.ready(readyEvidence());
  assert.deepEqual(callbackOrder, ["setup", "ready"]);
  assert.equal(record.hud_origin, "https://hud.example");
  assert.equal(Object.hasOwn(record, "bearer"), false);
  assert.equal(Object.hasOwn(record, "private_key"), false);
  assert.equal(Object.hasOwn(record.ownership, "tauri_driver_pid"), false);
  assert.equal(record.gateway_host, "127.0.0.1:48123");
  assert.deepEqual(record.test_agent_ids, ["agent-roster", "agent-chat"]);
  await fixture.holdUntilReleased(async ({ ready, signal }) => {
    assert.equal(ready, readyRecord);
    assert.equal(signal.aborted, false);
  });
  await fixture.close();
  await fixture.close();
  assert.equal(fixture.state, "closed");
  assert.deepEqual(callbackOrder, ["setup", "ready", "close"]);
  assert.equal(clock.pendingTimers, 0);
});

test("acceptance fixture bounds the hold until its release callback completes", async () => {
  const clock = fakeClock();
  const fixture = createRemoteClientAcceptanceFixture({
    hudOrigin: "http://127.0.0.1:4173",
    close: async () => {},
    holdTimeoutMs: 12,
    now: clock.now,
    schedule: clock.schedule,
    cancel: clock.cancel,
  });
  await fixture.setup(async () => {});
  fixture.ready(readyEvidence());
  const pending = fixture.holdUntilReleased(() => new Promise(() => {}));
  await Promise.resolve();
  clock.advance(12);
  await assert.rejects(pending, (error) => error.name === "TimeoutError");
  assert.equal(fixture.state, "hold_timed_out");
  await fixture.close();
});

test("acceptance fixture owns the caller-origin pairing and approval handoff", async () => {
  const fixture = createRemoteClientAcceptanceFixture({
    hudOrigin: "https://hud.example",
    close: async () => {},
  });
  await fixture.setup(async () => {});
  const calls = [];
  const suppliedKeys = crypto.generateKeyPairSync("ec", { namedCurve: "P-256" });
  const pairing = await fixture.pairClient({
    clientOrigin: "https://consumer.example",
    keyPair: suppliedKeys,
    deviceLabel: "Owned consumer",
    requestedScopes: ["chat:read"],
    createOffer: async ({ clientOrigin }) => {
      calls.push(["offer", clientOrigin]);
      return { client_origin: clientOrigin, pairing_offer_id: "offer-1", nonce: "nonce-1" };
    },
    submitPairing: async (request) => {
      calls.push(["submit", request.clientOrigin]);
      assert.equal(Buffer.from(request.publicKeySpkiDerBase64, "base64").length > 0, true);
      return { status: "pending", pairing_request_id: "request-1", device_id: "device-1" };
    },
    approvePairing: async ({ pending }) => calls.push(["approve", pending.pairing_request_id]),
  });
  assert.equal(pairing.keyPair, suppliedKeys);
  assert.equal(fixture.owned.pairing, pairing);
  assert.deepEqual(calls, [
    ["offer", "https://consumer.example"],
    ["submit", "https://consumer.example"],
    ["approve", "request-1"],
  ]);
  await fixture.close();
});

test("closing acceptance fixture aborts and settles an external hold permanently", async () => {
  const clock = fakeClock();
  const release = deferred();
  let holdSignal;
  let closeCount = 0;
  const fixture = createRemoteClientAcceptanceFixture({
    hudOrigin: "https://hud.example",
    close: async () => { closeCount += 1; },
    holdTimeoutMs: 50,
    now: clock.now,
    schedule: clock.schedule,
    cancel: clock.cancel,
  });
  await fixture.setup(async () => {});
  fixture.ready(readyEvidence());
  const hold = fixture.waitForExternalRelease(({ signal }) => {
    holdSignal = signal;
    return release.promise;
  });
  await Promise.resolve();
  await fixture.close();
  await assert.rejects(hold, (error) => error.name === "AbortError");
  assert.equal(holdSignal.aborted, true);
  assert.equal(fixture.state, "closed");
  assert.equal(clock.pendingTimers, 0);
  release.resolve();
  await Promise.resolve();
  assert.equal(fixture.state, "closed");
  assert.equal(closeCount, 1);
});

test("closing an external hold absorbs a late rejection without leaking timers or changing terminal state", async () => {
  const clock = fakeClock();
  const release = deferred();
  let closeCount = 0;
  let unhandled;
  const onUnhandled = (reason) => { unhandled = reason; };
  const fixture = createRemoteClientAcceptanceFixture({
    hudOrigin: "https://hud.example",
    close: async () => { closeCount += 1; },
    holdTimeoutMs: 50,
    now: clock.now,
    schedule: clock.schedule,
    cancel: clock.cancel,
  });
  await fixture.setup(async () => {});
  fixture.ready(readyEvidence());
  const hold = fixture.waitForExternalRelease(() => release.promise);
  await Promise.resolve();
  process.on("unhandledRejection", onUnhandled);
  await fixture.close();
  await assert.rejects(hold, (error) => error.name === "AbortError");
  release.reject(new Error("late release failure"));
  await new Promise((resolve) => setImmediate(resolve));
  process.removeListener("unhandledRejection", onUnhandled);
  assert.equal(unhandled, undefined);
  assert.equal(fixture.state, "closed");
  assert.equal(clock.pendingTimers, 0);
  assert.equal(closeCount, 1);
});

test("closing joins a pending pairing before cleanup and prevents later submit, approval, or publication", async () => {
  const offer = deferred();
  const calls = [];
  let closeCount = 0;
  const fixture = createRemoteClientAcceptanceFixture({
    hudOrigin: "https://hud.example",
    close: async () => { closeCount += 1; },
  });
  await fixture.setup(async () => {});
  const pairing = fixture.pairClient({
    deviceLabel: "Deferred consumer",
    requestedScopes: ["chat:read"],
    createOffer: () => offer.promise,
    submitPairing: async () => { calls.push("submit"); return { status: "pending", pairing_request_id: "r1", device_id: "d1" }; },
    approvePairing: async () => { calls.push("approve"); },
  });
  const pairingOutcome = pairing.then(
    () => ({ error: undefined }),
    (error) => ({ error }),
  );
  await Promise.resolve();
  await fixture.close();
  offer.resolve({ client_origin: "https://hud.example", pairing_offer_id: "o1" });
  const outcome = await pairingOutcome;
  assert.equal(outcome.error?.name, "AbortError");
  assert.deepEqual(calls, []);
  assert.equal(fixture.owned.pairing, undefined);
  assert.equal(fixture.state, "closed");
  assert.equal(closeCount, 1);
});

test("native approval validation binds the pending request to this session, origin, identity, and exact grants", () => {
  const pending = {
    request_id: "request-owned",
    public_key_fingerprint: "sha256:browser-key",
    canonical_origin: "https://gateway.fixture.example",
    client_origin: "https://hud.fixture.example",
    credential_kind: "client",
    audience: "wardian_remote_client_v1",
    read_grants: ["agents:read", "chat:read"],
    send_grants: ["chat:send"],
  };
  const expected = {
    pendingRequests: [pending],
    requestId: pending.request_id,
    publicKeyFingerprint: pending.public_key_fingerprint,
    gatewayOrigin: pending.canonical_origin,
    hudOrigin: pending.client_origin,
    readGrants: pending.read_grants,
    sendGrants: pending.send_grants,
  };
  assert.equal(validatePendingClientPairing(expected), pending);
  assert.throws(
    () => validatePendingClientPairing({ ...expected, requestId: "request-from-another-session" }),
    /owning native session/u,
  );
  assert.throws(
    () => validatePendingClientPairing({ ...expected, hudOrigin: "https://other-hud.example" }),
    /HUD origin/u,
  );
  assert.throws(
    () => validatePendingClientPairing({ ...expected, gatewayOrigin: "https://other-gateway.example" }),
    /gateway origin/u,
  );
  assert.throws(
    () => validatePendingClientPairing({ ...expected, publicKeyFingerprint: "sha256:other-key" }),
    /fingerprint/u,
  );
  assert.throws(
    () => validatePendingClientPairing({ ...expected, readGrants: ["chat:read"] }),
    /read grants/u,
  );
  assert.throws(
    () => validatePendingClientPairing({ ...expected, sendGrants: [] }),
    /send grants/u,
  );
  assert.throws(
    () => validatePendingClientPairing({ ...expected, pendingRequests: [{ ...pending, credential_kind: "pwa" }] }),
    /credential kind/u,
  );
  assert.throws(
    () => validatePendingClientPairing({ ...expected, pendingRequests: [{ ...pending, audience: "wardian_remote_pwa" }] }),
    /audience/u,
  );
});

test("native approval selects one nonrevoked DeviceRecord from the public pending DTO without device_id", () => {
  const pending = {
    request_id: "request-owned",
    device_label: "Owned browser",
    public_key_fingerprint: "sha256:browser-key",
    canonical_origin: "https://gateway.fixture.example",
    client_origin: "https://hud.fixture.example",
    credential_kind: "client",
    audience: "wardian_remote_client_v1",
    read_grants: ["agents:read", "chat:read"],
    send_grants: ["chat:send"],
    submitted_at: "2026-11-01T12:00:00Z",
    expires_at: "2026-11-01T12:05:00Z",
  };
  const validated = validatePendingClientPairing({
    pendingRequests: [pending],
    requestId: pending.request_id,
    publicKeyFingerprint: pending.public_key_fingerprint,
    gatewayOrigin: pending.canonical_origin,
    hudOrigin: pending.client_origin,
    readGrants: pending.read_grants,
    sendGrants: pending.send_grants,
  });
  const device = {
    device_id: "device-owned",
    label: pending.device_label,
    public_key_spki_der_base64: crypto.generateKeyPairSync("ec", { namedCurve: "P-256" })
      .publicKey.export({ type: "spki", format: "der" }).toString("base64"),
    public_key_fingerprint: pending.public_key_fingerprint,
    credential_kind: pending.credential_kind,
    audience: pending.audience,
    client_origin: pending.client_origin,
    read_grants: [...pending.read_grants].reverse(),
    send_grants: [...pending.send_grants],
    created_at: "2026-11-01T12:01:00Z",
    last_used_at: null,
    revoked_at: null,
  };
  assert.equal(Object.hasOwn(validated, "device_id"), false);
  const unrelated = { ...device, device_id: "device-other", public_key_fingerprint: "sha256:other-key" };
  assert.equal(validateApprovedClientDevice([
    unrelated,
    { ...device, device_id: "other-read-grants", read_grants: ["chat:read"] },
    { ...device, device_id: "other-send-grants", send_grants: [] },
    device,
  ], validated), device);

  const rejectedResults = [
    ["missing", []],
    ["multiple", [device, { ...device, device_id: "device-duplicate" }]],
    ["duplicate record", [device, device]],
    ...[
      ["fingerprint", { public_key_fingerprint: "sha256:other-key" }],
      ["origin", { client_origin: "https://other-hud.example" }],
      ["kind", { credential_kind: "pwa" }],
      ["audience", { audience: "wardian_remote_pwa" }],
      ["read grants", { read_grants: ["chat:read"] }],
      ["extra read grant", { read_grants: [...device.read_grants, "other:read"] }],
      ["duplicate read grant", { read_grants: ["chat:read", "chat:read"] }],
      ["send grants", { send_grants: [] }],
      ["extra send grant", { send_grants: [...device.send_grants, "other:send"] }],
      ["missing read grants", { read_grants: undefined }],
      ["missing send grants", { send_grants: undefined }],
      ["revoked", { revoked_at: "2026-11-01T12:02:00Z" }],
      ["missing revocation state", { revoked_at: undefined }],
      ["missing device ID", { device_id: undefined }],
      ["empty device ID", { device_id: "" }],
      ["blank device ID", { device_id: "   " }],
      ["invalid device ID", { device_id: 123 }],
    ].map(([label, changes]) => [label, [{ ...device, ...changes }]]),
    ["invalid result list", null],
  ];
  for (const [label, devices] of rejectedResults) {
    assert.throws(() => validateApprovedClientDevice(devices, validated), undefined, label);
  }
});

test("mock transcript oracle requires valid raw input and one assistant echo", () => {
  const nonce = "WARDIAN_M2_SINK_test_nonce";
  const bytes = Buffer.from([
    JSON.stringify({ type: "user", content: nonce }),
    JSON.stringify({ type: "model", content: `Echo: ${nonce}` }),
  ].join("\n") + "\n");
  assert.deepEqual(countMockTranscriptNonceEvents(bytes, nonce), {
    user_input_count: 1,
    assistant_echo_count: 1,
  });
  assert.throws(() => countMockTranscriptNonceEvents(Buffer.alloc(0), nonce), /empty/u);
  assert.throws(() => countMockTranscriptNonceEvents(Buffer.from("{broken\n"), nonce), /malformed/u);
  assert.throws(
    () => readMockTranscript(path.join(os.tmpdir(), `missing-${crypto.randomUUID()}.jsonl`)),
    (error) => error.code === "ENOENT",
  );
});

test("mock transcript restart delta distinguishes unchanged, appended, and replaced logs", () => {
  const nonce = "WARDIAN_M2_SINK_delta";
  const before = Buffer.from(`${JSON.stringify({ type: "user", content: nonce })}\n`);
  const appended = Buffer.concat([before, Buffer.from(`${JSON.stringify({ type: "model", content: `Echo: ${nonce}` })}\n`)]);
  const unchangedDelta = mockTranscriptDelta(before, before);
  assert.deepEqual(countMockTranscriptNonceEvents(unchangedDelta, nonce, { allowEmpty: true }), {
    user_input_count: 0,
    assistant_echo_count: 0,
  });
  assert.deepEqual(countMockTranscriptNonceEvents(mockTranscriptDelta(before, appended), nonce), {
    user_input_count: 0,
    assistant_echo_count: 1,
  });
  const replacement = Buffer.from(`${JSON.stringify({ type: "init", session_id: "new-session" })}\n`);
  assert.deepEqual(countMockTranscriptNonceEvents(mockTranscriptDelta(before, replacement), nonce), {
    user_input_count: 0,
    assistant_echo_count: 0,
  });
  assert.equal(sha256Bytes(before).length, 64);
});

function createSocketReader(socket) {
  let buffered = Buffer.alloc(0);
  let failure;
  let closed = false;
  const waiters = new Set();

  const rejectWaiters = (error) => {
    for (const waiter of waiters) {
      clearTimeout(waiter.timer);
      waiter.reject(error);
    }
    waiters.clear();
  };
  const flushWaiters = () => {
    for (const waiter of waiters) {
      const requiredLength = waiter.requiredLength(buffered);
      if (requiredLength === null || buffered.length < requiredLength) continue;
      waiters.delete(waiter);
      clearTimeout(waiter.timer);
      const bytes = buffered.subarray(0, requiredLength);
      buffered = buffered.subarray(requiredLength);
      waiter.resolve(bytes);
    }
  };

  socket.on("data", (chunk) => {
    buffered = Buffer.concat([buffered, chunk]);
    flushWaiters();
  });
  socket.on("error", (error) => {
    failure = error;
    rejectWaiters(error);
  });
  socket.on("close", () => {
    closed = true;
    rejectWaiters(new Error("socket closed before the expected bytes arrived"));
  });

  function waitFor(requiredLength, label) {
    return new Promise((resolve, reject) => {
      if (failure) {
        reject(failure);
        return;
      }
      if (closed) {
        reject(new Error(`socket closed before ${label}`));
        return;
      }
      const waiter = {
        requiredLength,
        resolve,
        reject,
        timer: setTimeout(() => {
          waiters.delete(waiter);
          reject(new Error(`timed out waiting for ${label}`));
        }, 5_000),
      };
      waiters.add(waiter);
      flushWaiters();
    });
  }

  return {
    readBytes(length) {
      return waitFor(() => length, `${length} socket bytes`);
    },
    readUpgradeResponseWithHead(headLength) {
      return waitFor((bytes) => {
        const headerEnd = bytes.indexOf(Buffer.from("\r\n\r\n"));
        return headerEnd < 0 ? null : headerEnd + 4 + headLength;
      }, "the WebSocket response head and opaque head bytes");
    },
  };
}

async function startUpgradeUpstream(onUpgrade) {
  const sockets = new Set();
  const server = http.createServer();
  server.on("connection", (socket) => {
    sockets.add(socket);
    socket.once("close", () => sockets.delete(socket));
  });
  server.on("upgrade", (request, socket, head) => {
    socket.once("end", () => socket.destroy());
    onUpgrade(request, socket, head);
  });
  server.listen(0, "127.0.0.1");
  await once(server, "listening");
  const address = server.address();
  assert.ok(address && typeof address !== "string");

  return {
    server,
    sockets,
    address,
    async close() {
      for (const socket of sockets) socket.destroy();
      if (!server.listening) return;
      await new Promise((resolve, reject) => {
        server.close((error) => (error ? reject(error) : resolve()));
      });
    },
  };
}

function requestUpgrade(fixture, requestPath, extraHeaders = {}) {
  return new Promise((resolve, reject) => {
    const request = https.request({
      hostname: fixture.ready.address.host,
      servername: "localhost",
      port: fixture.ready.address.port,
      path: requestPath,
      method: "GET",
      headers: {
        connection: "keep-alive, Upgrade",
        upgrade: "websocket",
        ...extraHeaders,
      },
      ca: fixture.certificatePem,
      rejectUnauthorized: true,
      timeout: 5_000,
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
    request.once("upgrade", (_response, socket) => {
      socket.destroy();
      reject(new Error("the fixture must not synthesize a WebSocket 101 response"));
    });
    request.once("timeout", () => request.destroy(new Error("TLS fixture request timed out")));
    request.once("error", reject);
    request.end();
  });
}

function assertPublishedCertificatePins(fixture) {
  const certificate = new crypto.X509Certificate(fixture.certificatePem);
  assert.equal(
    fixture.ready.certificate_sha256,
    crypto.createHash("sha256").update(certificate.raw).digest("hex"),
  );
  assert.equal(
    fixture.ready.certificate_spki_sha256_base64,
    crypto.createHash("sha256")
      .update(certificate.publicKey.export({ type: "spki", format: "der" }))
      .digest("base64"),
  );
  assert.match(fixture.ready.certificate_sha256, /^[0-9a-f]{64}$/);
  assert.match(fixture.ready.certificate_spki_sha256_base64, /^[A-Za-z0-9+/]{43}=$/);
}

const negotiationHeaders = {
  host: "gateway.fixture.example:443",
  origin: "https://client.fixture.example",
  cookie: "wardian_session=fixture-cookie",
  authorization: "Bearer fixture-token",
  "sec-websocket-key": "dGhlIHNhbXBsZSBub25jZQ==",
  "sec-websocket-version": "13",
  "sec-websocket-protocol": "wardian-v2, fixture-v1",
  "sec-websocket-extensions": "permessage-deflate; client_max_window_bits",
};

test("TLS fixture denies upgrades by default without contacting an upstream", { timeout: 10_000 }, async (t) => {
  let upstreamHits = 0;
  const upstream = await startUpgradeUpstream((_request, socket) => {
    upstreamHits += 1;
    socket.end("HTTP/1.1 426 Upgrade Required\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
  });
  const fixture = await startRemoteClientTlsFixture({ ...sourceIdentity() });
  fixture.setUpstream({ host: "127.0.0.1", port: upstream.address.port });
  t.after(async () => {
    await fixture.close();
    await upstream.close();
  });
  assertPublishedCertificatePins(fixture);

  await assert.rejects(requestUpgrade(fixture, "/remote/api/status-stream"));
  assert.equal(upstreamHits, 0);
});

test("independent Node clients cross pinned HTTPS through one shared GO barrier", { timeout: 10_000 }, async (t) => {
  const requests = [];
  const bothRequestsArrived = deferred();
  const upstream = http.createServer((request, response) => {
    const chunks = [];
    request.on("data", (chunk) => chunks.push(chunk));
    request.once("end", () => {
      requests.push({
        method: request.method,
        url: request.url,
        authorization: request.headers.authorization,
        origin: request.headers.origin,
        idempotencyKey: request.headers["idempotency-key"],
        body: Buffer.concat(chunks).toString("utf8"),
      });
      if (requests.length === 2) bothRequestsArrived.resolve();
      void bothRequestsArrived.promise.then(() => {
        response.writeHead(202, { "content-type": "application/json", connection: "close" });
        response.end(JSON.stringify({ accepted: true }));
      });
    });
  });
  upstream.listen(0, "127.0.0.1");
  await once(upstream, "listening");
  const upstreamAddress = upstream.address();
  assert.ok(upstreamAddress && typeof upstreamAddress !== "string");

  const fixture = await startRemoteClientTlsFixture({ ...sourceIdentity() });
  fixture.setUpstream({ host: "127.0.0.1", port: upstreamAddress.port });
  t.after(async () => {
    const results = await Promise.allSettled([
      fixture.close(),
      new Promise((resolve, reject) => upstream.close((error) => error ? reject(error) : resolve())),
    ]);
    const failed = results.find((result) => result.status === "rejected");
    if (failed) throw failed.reason;
  });

  const response = await submitDuplicateRemoteClientRequests({
    timeoutMs: 4_000,
    request: {
      url: `${fixture.ready.gateway_origin}/remote/client/v1/agents/fixture/messages`,
      headers: {
        authorization: "Bearer fixture-token",
        origin: "https://client.fixture.example",
        "content-type": "application/json",
        "idempotency-key": "fixture-namespace.fixture-request",
      },
      ca: fixture.certificatePem,
      body: { text: "shared barrier payload", input_mode: "message" },
    },
  });

  assert.equal(response.length, 2);
  for (const result of response) {
    assert.equal(result.status, 202, result.body);
    assert.deepEqual(JSON.parse(result.body), { accepted: true });
  }
  assert.deepEqual(requests, [0, 1].map(() => ({
    method: "POST",
    url: "/remote/client/v1/agents/fixture/messages",
    authorization: "Bearer fixture-token",
    origin: "https://client.fixture.example",
    idempotencyKey: "fixture-namespace.fixture-request",
    body: JSON.stringify({ text: "shared barrier payload", input_mode: "message" }),
  })));
});

test("opt-in TLS fixture forwards only PWA stream handshakes and preserves upstream errors", { timeout: 10_000 }, async (t) => {
  const received = [];
  const upstream = await startUpgradeUpstream((request, socket) => {
    received.push({ url: request.url, headers: request.headers });
    const body = Buffer.from("upstream declined upgrade");
    socket.end([
      "HTTP/1.1 426 Upgrade Required",
      "content-type: text/plain",
      `content-length: ${body.length}`,
      "x-fixture-upstream-decision: refused",
      "connection: close",
      "",
      body.toString("utf8"),
    ].join("\r\n"));
  });
  const fixture = await startRemoteClientTlsFixture({
    ...sourceIdentity(),
    allowPwaWebSocketUpgrades: true,
  });
  fixture.setUpstream({ host: "127.0.0.1", port: upstream.address.port });
  t.after(async () => {
    await fixture.close();
    await upstream.close();
  });

  const statusResponse = await requestUpgrade(
    fixture,
    "/remote/api/status-stream?fixture=1",
    negotiationHeaders,
  );
  assert.equal(statusResponse.status, 426);
  assert.equal(statusResponse.headers["x-fixture-upstream-decision"], "refused");
  assert.equal(statusResponse.body, "upstream declined upgrade");

  const terminalResponse = await requestUpgrade(
    fixture,
    "/remote/api/agents/session-123/terminal-stream",
    negotiationHeaders,
  );
  assert.equal(terminalResponse.status, 426);
  assert.equal(received.length, 2);
  assert.deepEqual(received.map(({ url }) => url), [
    "/remote/api/status-stream?fixture=1",
    "/remote/api/agents/session-123/terminal-stream",
  ]);
  for (const { headers } of received) {
    for (const name of Object.keys(negotiationHeaders)) {
      assert.equal(headers[name], negotiationHeaders[name], `${name} was not preserved`);
    }
    assert.equal(headers.connection, "Upgrade");
    assert.equal(headers.upgrade, "websocket");
  }

  await assert.rejects(requestUpgrade(fixture, "/remote/api/other-stream", negotiationHeaders));
  assert.equal(received.length, 2, "non-PWA stream paths must not reach the upstream");
});

// Transport-only: this controlled upstream completes RFC 6455, without gateway auth or PWA admission.
test("controlled upstream 101 tunnels opaque bytes and both head buffers, then closes open sockets", { timeout: 10_000 }, async (t) => {
  const upstreamUpgrade = deferred();
  const upstreamHeadBytes = Buffer.from([0xf1, 0x00, 0x7e, 0x80, 0x41]);
  const clientHeadBytes = Buffer.from([0x00, 0xff, 0x33, 0x0d, 0x0a]);
  const upstream = await startUpgradeUpstream((request, socket, requestHead) => {
    const key = request.headers["sec-websocket-key"];
    const accept = crypto.createHash("sha1")
      .update(`${key}258EAFA5-E914-47DA-95CA-C5AB0DC85B11`)
      .digest("base64");
    const responseHead = Buffer.from([
      "HTTP/1.1 101 Switching Protocols",
      "Upgrade: websocket",
      "Connection: Upgrade",
      `Sec-WebSocket-Accept: ${accept}`,
      "Sec-WebSocket-Protocol: wardian-v2",
      "",
      "",
    ].join("\r\n"));
    upstreamUpgrade.resolve({
      request,
      requestHead,
      socket,
      reader: createSocketReader(socket),
      closed: once(socket, "close"),
      accept,
    });
    socket.write(Buffer.concat([responseHead, upstreamHeadBytes]));
  });
  const fixture = await startRemoteClientTlsFixture({
    ...sourceIdentity(),
    allowPwaWebSocketUpgrades: true,
  });
  fixture.setUpstream({ host: "127.0.0.1", port: upstream.address.port });
  t.after(async () => {
    const results = await Promise.allSettled([fixture.close(), upstream.close()]);
    const failed = results.find((result) => result.status === "rejected");
    if (failed) throw failed.reason;
  });

  const clientSocket = tls.connect({
    host: fixture.ready.address.host,
    port: fixture.ready.address.port,
    ca: fixture.certificatePem,
    rejectUnauthorized: true,
  });
  await once(clientSocket, "secureConnect");
  const clientReader = createSocketReader(clientSocket);
  const clientClosed = once(clientSocket, "close");
  const clientKey = crypto.randomBytes(16).toString("base64");
  const clientHandshake = Buffer.from([
    "GET /remote/api/status-stream HTTP/1.1",
    `Host: ${fixture.ready.address.host}:${fixture.ready.address.port}`,
    "Origin: https://client.fixture.example",
    "Connection: keep-alive, Upgrade",
    "Upgrade: websocket",
    `Sec-WebSocket-Key: ${clientKey}`,
    "Sec-WebSocket-Version: 13",
    "Sec-WebSocket-Protocol: wardian-v2",
    "",
    "",
  ].join("\r\n"));
  clientSocket.write(Buffer.concat([clientHandshake, clientHeadBytes]));

  const accepted = await upstreamUpgrade.promise;
  assert.equal(accepted.requestHead.length, 0);
  assert.equal(accepted.request.url, "/remote/api/status-stream");
  assert.equal(accepted.request.headers.host, `${fixture.ready.address.host}:${fixture.ready.address.port}`);
  assert.equal(accepted.request.headers.origin, "https://client.fixture.example");
  assert.equal(accepted.request.headers.connection, "Upgrade");
  assert.equal(accepted.request.headers.upgrade, "websocket");
  assert.equal(accepted.request.headers["sec-websocket-key"], clientKey);
  assert.equal(accepted.request.headers["sec-websocket-protocol"], "wardian-v2");

  const response = await clientReader.readUpgradeResponseWithHead(upstreamHeadBytes.length);
  const responseSeparator = response.indexOf(Buffer.from("\r\n\r\n"));
  assert.ok(responseSeparator >= 0, "upstream response must contain an HTTP header terminator");
  const responseHead = response.subarray(0, responseSeparator + 4).toString("latin1");
  assert.match(responseHead, /^HTTP\/1\.1 101 Switching Protocols\r\n/);
  assert.ok(responseHead.includes("Connection: Upgrade\r\n"));
  assert.ok(responseHead.includes("Upgrade: websocket\r\n"));
  assert.ok(responseHead.includes(`Sec-WebSocket-Accept: ${accepted.accept}\r\n`));
  assert.ok(responseHead.includes("Sec-WebSocket-Protocol: wardian-v2\r\n"));
  assert.deepEqual(response.subarray(responseSeparator + 4), upstreamHeadBytes);

  const forwardedClientHead = await accepted.reader.readBytes(clientHeadBytes.length);
  assert.deepEqual(forwardedClientHead, clientHeadBytes);
  const clientOpaqueBytes = Buffer.from([0x00, 0x81, 0xff, 0x4c, 0x10, 0x00]);
  clientSocket.write(clientOpaqueBytes);
  assert.deepEqual(await accepted.reader.readBytes(clientOpaqueBytes.length), clientOpaqueBytes);

  const upstreamOpaqueBytes = Buffer.from([0x91, 0x00, 0xfe, 0x22, 0x80, 0x01]);
  accepted.socket.write(upstreamOpaqueBytes);
  assert.deepEqual(await clientReader.readBytes(upstreamOpaqueBytes.length), upstreamOpaqueBytes);

  await fixture.close();
  await Promise.all([clientClosed, accepted.closed]);
  assert.equal(clientSocket.destroyed, true);
  assert.equal(accepted.socket.destroyed, true);
});

test("TLS fixture close is idempotent and drains a pending upstream upgrade", { timeout: 10_000 }, async (t) => {
  const upgradeArrived = deferred();
  let upstreamSocket;
  let upstreamSocketClosed;
  const upstream = await startUpgradeUpstream((_request, socket) => {
    upstreamSocket = socket;
    upstreamSocketClosed = once(socket, "close");
    upgradeArrived.resolve();
  });
  const fixture = await startRemoteClientTlsFixture({
    ...sourceIdentity(),
    allowPwaWebSocketUpgrades: true,
  });
  fixture.setUpstream({ host: "127.0.0.1", port: upstream.address.port });
  t.after(async () => {
    await fixture.close();
    await upstream.close();
  });

  const clientResult = requestUpgrade(fixture, "/remote/api/status-stream")
    .then((response) => ({ response }), (error) => ({ error }));
  await upgradeArrived.promise;
  const firstClose = fixture.close();
  const secondClose = fixture.close();
  assert.equal(firstClose, secondClose);
  await firstClose;
  const result = await clientResult;
  assert.ok(result.error, "closing the fixture must terminate the pending client handshake");
  await upstreamSocketClosed;
  assert.equal(upstreamSocket.destroyed, true);
});

test("TLS fixture response observations retain boundary headers and redact cookie values and bodies", async (t) => {
  const observed = [];
  const upstream = http.createServer((_request, response) => {
    response.writeHead(200, {
      "cache-control": "no-store",
      "access-control-allow-origin": "https://hud.fixture.example",
      vary: "Origin",
      "set-cookie": "fixture_secret=do-not-capture",
    });
    response.end("response body is not diagnostic metadata");
  });
  upstream.listen(0, "127.0.0.1");
  await once(upstream, "listening");
  const upstreamAddress = upstream.address();
  assert.ok(upstreamAddress && typeof upstreamAddress !== "string");
  const fixture = await startRemoteClientTlsFixture({
    ...sourceIdentity(),
    onResponse: (observation) => observed.push(observation),
  });
  fixture.setUpstream({ host: "127.0.0.1", port: upstreamAddress.port });
  t.after(async () => {
    await fixture.close();
    await new Promise((resolve, reject) => upstream.close((error) => (error ? reject(error) : resolve())));
  });

  const response = await new Promise((resolve, reject) => {
    const request = https.get({
      hostname: fixture.ready.address.host,
      port: fixture.ready.address.port,
      path: "/response-observation",
      ca: fixture.certificatePem,
      rejectUnauthorized: true,
    }, (incoming) => {
      const chunks = [];
      incoming.on("data", (chunk) => chunks.push(chunk));
      incoming.once("error", reject);
      incoming.once("end", () => resolve({
        status: incoming.statusCode,
        body: Buffer.concat(chunks).toString("utf8"),
      }));
    });
    request.once("error", reject);
  });
  assert.equal(response.status, 200);
  assert.equal(response.body, "response body is not diagnostic metadata");
  assert.equal(observed.length, 1);
  assert.deepEqual(observed[0], {
    method: "GET",
    path: "/response-observation",
    status: 200,
    headers: {
      "cache-control": "no-store",
      "access-control-allow-origin": "https://hud.fixture.example",
      "access-control-allow-credentials": undefined,
      vary: "Origin",
      "set-cookie": true,
    },
  });
  assert.equal(JSON.stringify(observed).includes("do-not-capture"), false);
  assert.equal(JSON.stringify(observed).includes("response body"), false);
});
