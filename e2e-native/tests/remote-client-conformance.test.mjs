// @tier nightly — Paired-client auth over a scoped HTTPS fixture and the native gateway.
import test from "node:test";
import assert from "node:assert/strict";
import crypto from "node:crypto";
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import http from "node:http";
import os from "node:os";
import path from "node:path";
import tls from "node:tls";
import { fileURLToPath } from "node:url";
import { chromium } from "@playwright/test";

import { closeHttpFixture } from "../lib/httpFixture.mjs";
import { createNativeAcceptanceFixture } from "../lib/nativeAcceptanceFixture.mjs";
import {
  REMOTE_CHAT_READY_TIMEOUT_MS,
  countMockTranscriptNonceEvents,
  mockTranscriptDelta,
  sha256Bytes,
  waitForRemoteClientCondition,
} from "../lib/remoteClientAcceptanceFixture.mjs";

const RUN_ID = `${process.pid}-${Date.now()}`;
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

async function startClientOriginFixture() {
  const server = http.createServer((_request, response) => {
    response.writeHead(200, {
      "cache-control": "no-store",
      "content-type": "text/html; charset=utf-8",
    });
    response.end("<!doctype html><meta charset=\"utf-8\"><title>Paired client fixture</title>");
  });
  server.listen(0, "127.0.0.1");
  await new Promise((resolve, reject) => {
    server.once("listening", resolve);
    server.once("error", reject);
  });
  const address = server.address();
  if (!address || typeof address === "string") {
    await closeHttpFixture(server);
    throw new Error("unable to bind the paired-client browser origin");
  }
  return { server, origin: `http://127.0.0.1:${address.port}` };
}

function createBrowserProfileRoot() {
  return fs.mkdtempSync(path.join(os.tmpdir(), `wardian-remote-client-${RUN_ID}-`));
}

function createBrowserProfileDirectory(profileRoot, name) {
  const root = path.resolve(profileRoot);
  const profile = path.resolve(root, name);
  if (path.dirname(profile) !== root || !["pinned", "unpinned"].includes(name)) {
    throw new Error("browser profile path must remain inside its owned fixture root");
  }
  fs.mkdirSync(profile);
  return profile;
}

function removeOwnedBrowserProfileRoot(profileRoot) {
  const root = path.resolve(profileRoot);
  const tempRoot = path.resolve(os.tmpdir());
  const relative = path.relative(tempRoot, root);
  const prefix = `wardian-remote-client-${RUN_ID}-`;
  if (
    !relative
    || relative === ".."
    || relative.startsWith(`..${path.sep}`)
    || path.isAbsolute(relative)
    || !path.basename(root).startsWith(prefix)
  ) {
    throw new Error("refusing to remove a browser profile outside its owned temporary root");
  }
  fs.rmSync(root, { recursive: true, force: true });
}

async function browserTlsRequest(page, fixture, requestPath, {
  method = "GET",
  headers = {},
  body,
  signal,
  timeoutMs = 15_000,
} = {}) {
  signal?.throwIfAborted();
  const cursor = fixture.testSupport.responseCursor();
  const requestId = crypto.randomUUID();
  const cancelRequest = () => page.evaluate((id) => window.__wardianClientAcceptance.cancel(id), requestId)
    .catch(() => {});
  signal?.addEventListener("abort", cancelRequest, { once: true });
  try {
    const result = await page.evaluate((request) => window.__wardianClientAcceptance.request(request), {
      id: requestId,
      gatewayOrigin: fixture.ready.gateway_origin,
      requestPath,
      method,
      headers,
      body,
      timeoutMs,
    });
    const observation = fixture.testSupport.responsesSince(cursor).findLast((item) => (
      item.method === method && item.path === requestPath
    ));
    if (!observation) throw new Error(`TLS fixture did not capture the browser response for ${method} ${requestPath}`);
    return { status: result.status, headers: observation.headers, body: result.body };
  } finally {
    signal?.removeEventListener("abort", cancelRequest);
  }
}

function tlsRequest(fixture, requestPath, options = {}) {
  const authorization = options.headers?.authorization;
  if (typeof authorization === "string" && /^Bearer browser-token-\d+$/u.test(authorization)) {
    if (!fixture.browserRequest) throw new Error("browser-held bearer request requires the owned browser client");
    return fixture.browserRequest(requestPath, options);
  }
  return fixture.requestTls(requestPath, options);
}

async function assertStrictFixtureTlsControls(fixture) {
  await assert.rejects(
    tlsRequest(fixture, "/remote/api/health", {
      tlsOptions: {
        hostname: "wrong-fixture-host.invalid",
        servername: "wrong-fixture-host.invalid",
        lookup: (_hostname, options, callback) => {
          const address = fixture.ready.address.host;
          if (options?.all) {
            callback(null, [{ address, family: 4 }]);
            return;
          }
          callback(null, address, 4);
        },
      },
    }),
    (error) => error.code === "ERR_TLS_CERT_ALTNAME_INVALID",
  );

  assert.ok(tls.rootCertificates.length > 0, "Node must expose its bundled roots for the negative control");
  await assert.rejects(
    tlsRequest(fixture, "/remote/api/health", {
      tlsOptions: { ca: tls.rootCertificates[0] },
    }),
    (error) => [
      "DEPTH_ZERO_SELF_SIGNED_CERT",
      "SELF_SIGNED_CERT_IN_CHAIN",
      "UNABLE_TO_VERIFY_LEAF_SIGNATURE",
    ].includes(error.code),
  );
}

function assertClientBoundary(response, expectedOrigin) {
  assert.equal(response.headers["cache-control"], "no-store");
  assert.equal(response.headers["access-control-allow-origin"], expectedOrigin);
  assert.equal(response.headers["access-control-allow-credentials"], undefined);
  assert.equal(response.headers["set-cookie"], undefined);
  assert.ok(String(response.headers.vary).split(",").some((value) => value.trim().toLowerCase() === "origin"));
}

function encodeRemoteChatQueryComponent(value) {
  return Array.from(Buffer.from(value, "utf8"), (byte) => {
    if (
      (byte >= 0x30 && byte <= 0x39)
      || (byte >= 0x41 && byte <= 0x5a)
      || (byte >= 0x61 && byte <= 0x7a)
      || [0x2d, 0x5f, 0x2e, 0x7e].includes(byte)
    ) return String.fromCharCode(byte);
    return `%${byte.toString(16).padStart(2, "0").toUpperCase()}`;
  }).join("");
}

function clientBearerHeaders(token, origin) {
  return { authorization: `Bearer ${token}`, origin };
}

test("paired-client challenge and bearer contract crosses the TLS fixture into the native gateway", {
  // 60s native startup/browser setup + 120s absolute Chat readiness +
  // 30s delivery + 90s restart, re-authentication and remaining assertions.
  // The gateway's 120s Chat and 30s delivery bounds remain unchanged.
  timeout: 300_000,
}, async (t) => {
  const owned = {};
  let hudHoldOutcome;
  const browserContexts = [];
  t.after(async () => {
    owned.acceptanceFixture?.controls.release();
    if (hudHoldOutcome) {
      const holdError = await hudHoldOutcome;
      if (holdError) throw holdError;
    }
    const contexts = browserContexts.splice(0);
    const contextResults = await Promise.allSettled(contexts.map((context) => context.close()));
    if (owned.acceptanceFixture) await owned.acceptanceFixture.controls.close();
    if (owned.clientOriginFixture) await closeHttpFixture(owned.clientOriginFixture.server);
    if (owned.browserProfileRoot) removeOwnedBrowserProfileRoot(owned.browserProfileRoot);
    const contextFailure = contextResults.find((result) => result.status === "rejected");
    if (contextFailure) throw contextFailure.reason;
  });

  owned.clientOriginFixture = await startClientOriginFixture();
  const clientOrigin = owned.clientOriginFixture.origin;
  owned.acceptanceFixture = await createNativeAcceptanceFixture({
    hudOrigin: clientOrigin,
    ...sourceIdentity(),
    holdTimeoutMs: 180_000,
    enableTestSupport: true,
    onReady: (record) => process.stdout.write(`${JSON.stringify(record)}\n`),
  });
  const fixtureContext = owned.acceptanceFixture.context;
  const testSupport = owned.acceptanceFixture.testSupport;
  const tlsFixture = { ready: fixtureContext.tls, requestTls: testSupport.requestTls, testSupport };

  assert.match(tlsFixture.ready.certificate_sha256, /^[0-9a-f]{64}$/);
  assert.match(tlsFixture.ready.certificate_spki_sha256_base64, /^[A-Za-z0-9+/]{43}=$/);
  const nodeTlsPreflight = await tlsRequest(tlsFixture, "/remote/api/health");
  assert.equal(nodeTlsPreflight.status, 200);
  await assertStrictFixtureTlsControls(tlsFixture);

  owned.browserProfileRoot = createBrowserProfileRoot();
  const unpinnedContext = await chromium.launchPersistentContext(
    createBrowserProfileDirectory(owned.browserProfileRoot, "unpinned"),
    { headless: true },
  );
  browserContexts.push(unpinnedContext);
  const unpinnedPage = await unpinnedContext.newPage();
  await assert.rejects(
    unpinnedPage.goto(tlsFixture.ready.gateway_origin),
    /ERR_CERT_AUTHORITY_INVALID/,
  );
  await unpinnedContext.close();
  browserContexts.splice(browserContexts.indexOf(unpinnedContext), 1);

  const browserContext = await chromium.launchPersistentContext(
    createBrowserProfileDirectory(owned.browserProfileRoot, "pinned"),
    {
      headless: true,
      // This test-only SPKI exception bypasses Chromium hostname validation.
      // Strict CA and hostname behavior is checked independently above by Node.
      args: [`--ignore-certificate-errors-spki-list=${tlsFixture.ready.certificate_spki_sha256_base64}`],
    },
  );
  browserContexts.push(browserContext);
  const browserPage = await browserContext.newPage();
  tlsFixture.browserRequest = (requestPath, options) => browserTlsRequest(browserPage, tlsFixture, requestPath, options);
  await browserPage.goto(clientOrigin);
  const browserCorsResult = await browserPage.evaluate(async (gatewayOrigin) => {
    const response = await fetch(`${gatewayOrigin}/remote/client/v1/auth/challenge`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: "{",
      signal: AbortSignal.timeout(15_000),
    });
    return {
      status: response.status,
      cacheControl: response.headers.get("cache-control"),
      body: await response.json(),
    };
  }, tlsFixture.ready.gateway_origin);
  assert.equal(browserCorsResult.status, 400);
  assert.deepEqual(browserCorsResult.body, { error: "invalid_request" });
  assert.equal(browserCorsResult.cacheControl, "no-store");
  const rawCorsResponse = await tlsRequest(
    tlsFixture,
    "/remote/client/v1/auth/challenge",
    {
      method: "POST",
      headers: {
        "content-type": "application/json",
        origin: clientOrigin,
      },
      body: "{",
    },
  );
  assert.equal(rawCorsResponse.status, 400);
  assertClientBoundary(rawCorsResponse, clientOrigin);
  await browserPage.evaluate(() => {
    const toBase64 = (bytes) => btoa(String.fromCharCode(...new Uint8Array(bytes)));
    const fetchBounded = async (url, options) => {
      const controller = new AbortController();
      const timer = setTimeout(() => controller.abort(new Error("browser gateway request timed out")), 15_000);
      try {
        return await fetch(url, { ...options, signal: controller.signal });
      } finally {
        clearTimeout(timer);
      }
    };
    const rawP256ToDer = (signature) => {
      const raw = new Uint8Array(signature);
      const integer = (part) => {
        let start = 0;
        while (start < part.length - 1 && part[start] === 0) start += 1;
        const significant = part.subarray(start);
        const prefixed = significant[0] & 0x80 ? [0, ...significant] : [...significant];
        return [0x02, prefixed.length, ...prefixed];
      };
      const r = integer(raw.subarray(0, 32));
      const s = integer(raw.subarray(32, 64));
      const content = [...r, ...s];
      return new Uint8Array([0x30, content.length, ...content]);
    };
    let privateKey;
    let deviceId;
    let tokenSequence = 0;
    const browserTokens = new Map();
    const requestControllers = new Map();
    const retainToken = (token) => {
      const accessToken = `browser-token-${++tokenSequence}`;
      browserTokens.set(accessToken, token.access_token);
      return {
        token: { ...token, access_token: accessToken },
        tokenShapeValid: /^[A-Za-z0-9_-]{43}$/u.test(token.access_token),
      };
    };
    window.__wardianClientAcceptance = Object.freeze({
      async submitPairing({ gatewayOrigin, offer, deviceLabel, requestedScopes }) {
        const keyPair = await crypto.subtle.generateKey(
          { name: "ECDSA", namedCurve: "P-256" },
          false,
          ["sign", "verify"],
        );
        privateKey = keyPair.privateKey;
        const publicKeySpkiDerBase64 = toBase64(await crypto.subtle.exportKey("spki", keyPair.publicKey));
        const submitResponse = await fetchBounded(`${gatewayOrigin}/remote/client/v1/pairing/submit`, {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({
            pairing_offer_id: offer.pairing_offer_id,
            nonce: offer.nonce,
            device_label: deviceLabel,
            public_key_spki_der_base64: publicKeySpkiDerBase64,
            requested_scopes: requestedScopes,
          }),
        });
        const submission = await submitResponse.json();
        deviceId = submission.device_id;
        return {
          privateKeyExtractable: privateKey.extractable,
          publicKeySpkiDerBase64,
          submitStatus: submitResponse.status,
          submitCacheControl: submitResponse.headers.get("cache-control"),
          submission,
        };
      },
      async authenticate(gatewayOrigin) {
        const challengeResponse = await fetchBounded(`${gatewayOrigin}/remote/client/v1/auth/challenge`, {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({ device_id: deviceId }),
        });
        const challenge = await challengeResponse.json();
        const signature = await crypto.subtle.sign(
          { name: "ECDSA", hash: "SHA-256" },
          privateKey,
          new TextEncoder().encode(challenge.signing_payload),
        );
        const tokenRequestStartedAt = Date.now();
        const tokenResponse = await fetchBounded(`${gatewayOrigin}/remote/client/v1/auth/token`, {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({
            challenge_id: challenge.challenge_id,
            device_id: deviceId,
            signature_der_base64: toBase64(rawP256ToDer(signature)),
          }),
        });
        const token = await tokenResponse.json();
        const retained = retainToken(token);
        return {
          challengeStatus: challengeResponse.status,
          challengeCacheControl: challengeResponse.headers.get("cache-control"),
          challenge,
          tokenRequestStartedAt,
          tokenStatus: tokenResponse.status,
          tokenCacheControl: tokenResponse.headers.get("cache-control"),
          token: retained.token,
          tokenShapeValid: retained.tokenShapeValid,
        };
      },
      async refresh(gatewayOrigin) {
        const challengeResponse = await fetchBounded(`${gatewayOrigin}/remote/client/v1/auth/challenge`, {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({ device_id: deviceId }),
        });
        const challenge = await challengeResponse.json();
        const signature = await crypto.subtle.sign(
          { name: "ECDSA", hash: "SHA-256" },
          privateKey,
          new TextEncoder().encode(challenge.signing_payload),
        );
        const tokenRequestStartedAt = Date.now();
        const tokenResponse = await fetchBounded(`${gatewayOrigin}/remote/client/v1/auth/token`, {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({
            challenge_id: challenge.challenge_id,
            device_id: deviceId,
            signature_der_base64: toBase64(rawP256ToDer(signature)),
          }),
        });
        const token = await tokenResponse.json();
        const retained = retainToken(token);
        return {
          challengeStatus: challengeResponse.status,
          challenge,
          tokenRequestStartedAt,
          tokenStatus: tokenResponse.status,
          token: retained.token,
          tokenShapeValid: retained.tokenShapeValid,
        };
      },
      async request({ id, gatewayOrigin, requestPath, method, headers, body, timeoutMs }) {
        const requestHeaders = new Headers(headers);
        const authorization = requestHeaders.get("authorization");
        const tokenKey = authorization?.match(/^Bearer (browser-token-\d+)$/u)?.[1];
        const accessToken = tokenKey && browserTokens.get(tokenKey);
        if (!accessToken) throw new Error("request did not reference a browser-owned token");
        requestHeaders.delete("origin");
        requestHeaders.set("authorization", `Bearer ${accessToken}`);
        const controller = new AbortController();
        requestControllers.set(id, controller);
        const timer = setTimeout(() => controller.abort(new Error("browser gateway request timed out")), timeoutMs);
        try {
          const response = await fetch(`${gatewayOrigin}${requestPath}`, {
            method,
            headers: requestHeaders,
            body,
            signal: controller.signal,
            credentials: "omit",
          });
          return { status: response.status, body: await response.text() };
        } finally {
          clearTimeout(timer);
          requestControllers.delete(id);
        }
      },
      cancel(id) {
        requestControllers.get(id)?.abort(new DOMException("request cancelled by test deadline", "AbortError"));
      },
      async duplicateRequests({ gatewayOrigin, requestPath, tokenKey, headers, body }) {
        const accessToken = browserTokens.get(tokenKey);
        if (!accessToken) throw new Error("duplicate clients did not reference a browser-owned token");
        const source = `
          let config;
          self.onmessage = async ({ data }) => {
            if (data.type === "init") {
              config = data.config;
              self.postMessage({ type: "ready" });
              return;
            }
            if (data.type !== "GO") return;
            try {
              const response = await fetch(config.url, {
                method: config.method,
                headers: config.headers,
                body: config.body,
                credentials: "omit",
                signal: AbortSignal.timeout(15000),
              });
              self.postMessage({ type: "result", value: { status: response.status, body: await response.text() } });
            } catch (error) {
              self.postMessage({ type: "error", error: String(error) });
            }
          };
        `;
        const url = URL.createObjectURL(new Blob([source], { type: "text/javascript" }));
        const workers = [new Worker(url), new Worker(url)];
        try {
          const ready = workers.map((worker) => new Promise((resolve, reject) => {
            worker.onmessage = ({ data }) => data.type === "ready" ? resolve() : undefined;
            worker.onerror = reject;
          }));
          const results = workers.map((worker) => new Promise((resolve, reject) => {
            const receive = ({ data }) => {
              if (data.type === "result") { worker.removeEventListener("message", receive); resolve(data.value); }
              if (data.type === "error") { worker.removeEventListener("message", receive); reject(new Error(data.error)); }
            };
            worker.addEventListener("message", receive);
          }));
          const requestHeaders = { ...headers, authorization: `Bearer ${accessToken}` };
          const config = {
            url: `${gatewayOrigin}${requestPath}`,
            method: "POST",
            headers: requestHeaders,
            body: JSON.stringify(body),
          };
          workers.forEach((worker) => worker.postMessage({ type: "init", config }));
          await Promise.all(ready);
          workers.forEach((worker) => worker.postMessage({ type: "GO" }));
          return await Promise.all(results);
        } finally {
          workers.forEach((worker) => worker.terminate());
          URL.revokeObjectURL(url);
        }
      },
    });
  });

  const offer = await owned.acceptanceFixture.controls.createOffer();
  assert.equal(offer.gateway_origin, tlsFixture.ready.gateway_origin);
  assert.equal(offer.client_origin, clientOrigin);
  const requestedScopes = ["agents:read", "chat:read", "chat:send"];
  const pairingCursor = testSupport.responseCursor();
  const browserSubmission = await browserPage.evaluate(async ({ gatewayOrigin, pairingOffer, deviceLabel, scopes }) => (
    window.__wardianClientAcceptance.submitPairing({
      gatewayOrigin,
      offer: pairingOffer,
      deviceLabel,
      requestedScopes: scopes,
    })
  ), {
    gatewayOrigin: tlsFixture.ready.gateway_origin,
    hudOrigin: clientOrigin,
    pairingOffer: offer,
    deviceLabel: `Conformance client ${RUN_ID}`,
    scopes: requestedScopes,
  });
  assert.equal(browserSubmission.privateKeyExtractable, false);
  assert.equal(browserSubmission.submitStatus, 202);
  assert.equal(browserSubmission.submitCacheControl, "no-store");
  const pairingResponse = testSupport.responsesSince(pairingCursor)
    .find((item) => item.method === "POST" && item.path === "/remote/client/v1/pairing/submit");
  assert.ok(pairingResponse, "TLS fixture must observe the browser pairing submission");
  assertClientBoundary(pairingResponse, clientOrigin);
  const pending = browserSubmission.submission;
  assert.equal(pending.status, "pending");
  const approval = await owned.acceptanceFixture.controls.approvePending({
    requestId: pending.pairing_request_id,
    publicKeyFingerprint: pending.public_key_fingerprint,
    readGrants: ["agents:read", "chat:read"],
    sendGrants: ["chat:send"],
  });
  assert.equal(approval.device_id, pending.device_id);
  const authenticationCursor = testSupport.responseCursor();
  const browserAuthentication = await browserPage.evaluate((gatewayOrigin) => (
    window.__wardianClientAcceptance.authenticate(gatewayOrigin)
  ), tlsFixture.ready.gateway_origin);
  assert.equal(browserAuthentication.challengeStatus, 200);
  assert.equal(browserAuthentication.challengeCacheControl, "no-store");
  assert.equal(browserAuthentication.tokenStatus, 200);
  assert.equal(browserAuthentication.tokenCacheControl, "no-store");
  const authenticationResponses = testSupport.responsesSince(authenticationCursor)
    .filter((item) => [
      "/remote/client/v1/auth/challenge",
      "/remote/client/v1/auth/token",
    ].includes(item.path));
  assert.equal(authenticationResponses.length, 2);
  for (const response of authenticationResponses) assertClientBoundary(response, clientOrigin);
  const challenge = browserAuthentication.challenge;
  assert.equal(challenge.origin, tlsFixture.ready.gateway_origin);
  assert.equal(challenge.client_origin, clientOrigin);
  assert.equal(challenge.device_id, pending.device_id);
  assert.equal(challenge.audience, "wardian_remote_client_v1");
  const expectedPayload = [
    "wardian.remote.client.auth.v1",
    `origin:${tlsFixture.ready.gateway_origin}`,
    `client_origin:${clientOrigin}`,
    "audience:wardian_remote_client_v1",
    `device:${pending.device_id}`,
    `challenge:${challenge.challenge_id}`,
    `nonce:${challenge.nonce}`,
  ].join("\n");
  assert.equal(challenge.signing_payload, expectedPayload);
  const tokenRequestStartedAt = browserAuthentication.tokenRequestStartedAt;
  const token = browserAuthentication.token;
  assert.deepEqual(Object.keys(token).sort(), [
    "access_token",
    "device_id",
    "expires_at",
    "idempotency_expires_at",
    "idempotency_namespace",
    "scopes",
    "token_type",
  ]);
  assert.equal(token.token_type, "Bearer");
  assert.equal(token.device_id, pending.device_id);
  assert.deepEqual(token.scopes, ["agents:read", "chat:read", "chat:send"]);
  assert.equal(browserAuthentication.tokenShapeValid, true, "browser token shape is invalid");
  assert.equal(/^[A-Za-z0-9_-]{43}$/.test(token.idempotency_namespace), true);
  const idempotencyExpiresAt = Date.parse(token.idempotency_expires_at);
  assert.ok(Number.isFinite(idempotencyExpiresAt));
  assert.ok(idempotencyExpiresAt > Date.now() + 23 * 60 * 60_000);
  assert.ok(idempotencyExpiresAt <= tokenRequestStartedAt + 24 * 60 * 60_000 + 1_000);
  const tokenExpiresAt = Date.parse(token.expires_at);
  assert.ok(Number.isFinite(tokenExpiresAt));
  assert.ok(tokenExpiresAt > Date.now() + 14 * 60_000);
  assert.ok(tokenExpiresAt <= tokenRequestStartedAt + 15 * 60_000 + 1_000);

  const sharing = await tlsRequest(tlsFixture, "/remote/client/v1/sharing", {
    headers: {
      authorization: `Bearer ${token.access_token}`,
      origin: clientOrigin,
    },
  });
  assert.equal(sharing.status, 200);
  assertClientBoundary(sharing, clientOrigin);
  const sharingStatus = JSON.parse(sharing.body);
  assert.equal(sharingStatus.device_id, pending.device_id);
  assert.equal(sharingStatus.expires_at, token.expires_at);

  const expectedRosterWorkspace = testSupport.workspacePath("RemoteRosterFixture");
  const rosterAgent = await testSupport.spawnAgent({
    sessionName: "RemoteRosterFixture",
    isOff: true,
  });
  assert.ok(rosterAgent.session_id);
  assert.equal(rosterAgent.session_name, "RemoteRosterFixture");
  assert.equal(rosterAgent.agent_class, "Coder");
  assert.equal(rosterAgent.provider, "mock");
  assert.equal(rosterAgent.folder, expectedRosterWorkspace);
  assert.equal(rosterAgent.is_off, true);

  const roster = await tlsRequest(tlsFixture, "/remote/client/v1/agents", {
    headers: {
      authorization: `Bearer ${token.access_token}`,
      origin: clientOrigin,
    },
  });
  assert.equal(roster.status, 200);
  assertClientBoundary(roster, clientOrigin);
  const rosterBody = JSON.parse(roster.body);
  assert.deepEqual(Object.keys(rosterBody).sort(), ["agents"]);
  assert.ok(Array.isArray(rosterBody.agents));
  assert.deepEqual(rosterBody.agents.map(({ session_id }) => session_id), [rosterAgent.session_id]);
  assert.deepEqual(rosterBody.agents, [{
    session_id: rosterAgent.session_id,
    session_name: "RemoteRosterFixture",
    agent_class: "Coder",
    provider: "mock",
    workspace: expectedRosterWorkspace,
    status: "Off",
    latest_text: null,
  }]);
  const rosterResponseVerified = roster.status === 200
    && rosterBody.agents.some((agent) => agent.session_id === rosterAgent.session_id);

  const chatAgent = await testSupport.spawnAgent({
    sessionName: "RemotePagedChatFixture",
    isOff: false,
    scenario: "paged_chat",
  });
  assert.ok(chatAgent.session_id);
  const clientChatPath = `/remote/client/v1/agents/${chatAgent.session_id}/chat`;
  const longDetail = "Paged client chat detail body. ".repeat(1_400);
  const clientChatPage = await waitForRemoteClientCondition({
    timeoutMs: REMOTE_CHAT_READY_TIMEOUT_MS,
    intervalMs: 250,
    label: "paired-client Chat readiness",
    request: ({ signal, timeoutMs }) => tlsRequest(tlsFixture, clientChatPath, {
      headers: clientBearerHeaders(token.access_token, clientOrigin),
      signal,
      timeoutMs,
    }),
    inspect: (response) => {
      assert.equal(response.status, 200, response.body);
      assertClientBoundary(response, clientOrigin);
      const candidate = JSON.parse(response.body);
      return {
        ready: candidate.events?.length === 80
          && candidate.progress === "ready"
          && typeof candidate.next_before === "string"
          && candidate.events.some((event) => typeof event.metadata?.chat_detail_ref === "string"),
        value: candidate,
      };
    },
  });
  assert.equal(clientChatPage.session_id, chatAgent.session_id);
  assert.ok(clientChatPage.revision);
  assert.equal(clientChatPage.events.length, 80);
  assert.ok(!JSON.stringify(clientChatPage).includes(longDetail));
  const largeChatEvent = clientChatPage.events.find(
    (event) => typeof event.metadata?.chat_detail_ref === "string",
  );
  assert.ok((largeChatEvent.text ?? "").length <= 1_024);

  let detailCursor = largeChatEvent.metadata.chat_detail_ref;
  let assembledDetail = "";
  let detailComplete = false;
  for (let chunk = 0; chunk < 8; chunk++) {
    const detailQuery = [
      `revision=${encodeRemoteChatQueryComponent(clientChatPage.revision)}`,
      `detail=${encodeRemoteChatQueryComponent(detailCursor)}`,
    ].join("&");
    const response = await tlsRequest(tlsFixture, `${clientChatPath}?${detailQuery}`, {
      headers: clientBearerHeaders(token.access_token, clientOrigin),
    });
    assert.equal(response.status, 200, response.body);
    const detailPage = JSON.parse(response.body);
    assert.equal(detailPage.detail.event_id, largeChatEvent.id);
    assembledDetail += detailPage.detail.text;
    if (detailPage.detail.complete) {
      detailComplete = true;
      break;
    }
    detailCursor = detailPage.detail.next;
  }
  assert.ok(detailComplete, "lazy Chat detail should finish in bounded chunks");
  assert.equal(assembledDetail, longDetail);

  const olderQuery = [
    `before=${encodeRemoteChatQueryComponent(clientChatPage.next_before)}`,
    `revision=${encodeRemoteChatQueryComponent(clientChatPage.revision)}`,
  ].join("&");
  const olderResponse = await tlsRequest(tlsFixture, `${clientChatPath}?${olderQuery}`, {
    headers: clientBearerHeaders(token.access_token, clientOrigin),
  });
  assert.equal(olderResponse.status, 200, olderResponse.body);
  const olderPage = JSON.parse(olderResponse.body);
  assert.equal(olderPage.older_page, true);
  assert.ok(olderPage.events.length > 0);

  const sourceRef = clientChatPage.events.find(
    (event) => typeof event.metadata?.chat_source_ref === "string",
  )?.metadata.chat_source_ref;
  assert.ok(sourceRef, "recent Chat page should identify an admitted source row for renewal");
  const sourceIds = encodeRemoteChatQueryComponent(JSON.stringify([sourceRef]));
  const renewalQuery = [
    `revision=${encodeRemoteChatQueryComponent(clientChatPage.revision)}`,
    `revalidate_ids=${sourceIds}`,
  ].join("&");
  const renewalResponse = await tlsRequest(tlsFixture, `${clientChatPath}?${renewalQuery}`, {
    headers: clientBearerHeaders(token.access_token, clientOrigin),
  });
  assert.equal(renewalResponse.status, 200, renewalResponse.body);
  const renewalPage = JSON.parse(renewalResponse.body);
  assert.deepEqual(renewalPage.revalidated_ids, [sourceRef]);

  const requestId = crypto.randomUUID();
  const idempotencyKey = `${token.idempotency_namespace}.${requestId}`;
  const messageText = "  M2 exact text stays unchanged.\nsecond line 🛰️  ";
  const messagePath = `/remote/client/v1/agents/${rosterAgent.session_id}/messages`;
  const submit = (sessionId, text, key = idempotencyKey) => tlsRequest(
    tlsFixture,
    `/remote/client/v1/agents/${sessionId}/messages`,
    {
      method: "POST",
      headers: {
        ...clientBearerHeaders(token.access_token, clientOrigin),
        "content-type": "application/json",
        "idempotency-key": key,
      },
      body: JSON.stringify({ text, input_mode: "message" }),
    },
  );
  const submitted = await submit(rosterAgent.session_id, messageText);
  assert.equal(submitted.status, 202, submitted.body);
  assertClientBoundary(submitted, clientOrigin);
  const operation = JSON.parse(submitted.body);
  assert.deepEqual(Object.keys(operation).sort(), [
    "accepted_at",
    "delivery_state",
    "error_code",
    "idempotency_expires_at",
    "operation_id",
    "request_id",
    "session_id",
    "updated_at",
  ]);
  assert.equal(operation.request_id, requestId);
  assert.equal(operation.session_id, rosterAgent.session_id);
  assert.equal(typeof operation.operation_id, "string");
  assert.equal(typeof operation.accepted_at, "string");
  assert.equal(typeof operation.updated_at, "string");
  assert.equal(typeof operation.delivery_state, "string");
  assert.equal(operation.error_code, null);
  assert.equal(operation.idempotency_expires_at, token.idempotency_expires_at);

  const duplicate = await submit(rosterAgent.session_id, messageText);
  assert.equal(duplicate.status, 202, duplicate.body);
  assert.equal(JSON.parse(duplicate.body).operation_id, operation.operation_id);
  const changedBody = await submit(rosterAgent.session_id, `${messageText}changed`);
  assert.equal(changedBody.status, 409);
  assert.deepEqual(JSON.parse(changedBody.body), { error: "idempotency_conflict" });
  const changedTarget = await submit("missing-target", messageText);
  assert.equal(changedTarget.status, 409);
  assert.deepEqual(JSON.parse(changedTarget.body), { error: "idempotency_conflict" });
  const changedMode = await tlsRequest(tlsFixture, messagePath, {
    method: "POST",
    headers: {
      ...clientBearerHeaders(token.access_token, clientOrigin),
      "content-type": "application/json",
      "idempotency-key": idempotencyKey,
    },
    body: JSON.stringify({ text: messageText, input_mode: "command" }),
  });
  assert.equal(changedMode.status, 409);
  assert.deepEqual(JSON.parse(changedMode.body), { error: "idempotency_conflict" });
  const changedInvalidBody = await submit(rosterAgent.session_id, "");
  assert.equal(changedInvalidBody.status, 409);
  assert.deepEqual(JSON.parse(changedInvalidBody.body), { error: "idempotency_conflict" });

  const statusPath = "/remote/client/v1/message-operations/by-key";
  const status = await tlsRequest(tlsFixture, statusPath, {
    headers: {
      ...clientBearerHeaders(token.access_token, clientOrigin),
      "idempotency-key": idempotencyKey,
    },
  });
  assert.equal(status.status, 200, status.body);
  assert.equal(JSON.parse(status.body).operation_id, operation.operation_id);
  const statusWithQuery = await tlsRequest(tlsFixture, `${statusPath}?extra=1`, {
    headers: {
      ...clientBearerHeaders(token.access_token, clientOrigin),
      "idempotency-key": idempotencyKey,
    },
  });
  assert.equal(statusWithQuery.status, 400);
  assert.deepEqual(JSON.parse(statusWithQuery.body), { error: "invalid_request" });
  const statusWithEmptyQuery = await tlsRequest(tlsFixture, `${statusPath}?`, {
    headers: {
      ...clientBearerHeaders(token.access_token, clientOrigin),
      "idempotency-key": idempotencyKey,
    },
  });
  assert.equal(statusWithEmptyQuery.status, 400);
  assert.deepEqual(JSON.parse(statusWithEmptyQuery.body), { error: "invalid_request" });
  // GET bodies are rejected by browser fetch before HTTP. The router contract
  // covers that malformed request without exporting browser-held credentials.
  const unknownOperation = await tlsRequest(tlsFixture, statusPath, {
    headers: {
      ...clientBearerHeaders(token.access_token, clientOrigin),
      "idempotency-key": `${token.idempotency_namespace}.${crypto.randomUUID()}`,
    },
  });
  assert.equal(unknownOperation.status, 404);
  assert.deepEqual(JSON.parse(unknownOperation.body), { error: "operation_not_found" });
  const unknownNamespaceKey = `${"u".repeat(43)}.${crypto.randomUUID()}`;
  const expiredNamespacePost = await tlsRequest(
    tlsFixture,
    `/remote/client/v1/agents/${rosterAgent.session_id}/messages`,
    {
      method: "POST",
      headers: {
        ...clientBearerHeaders(token.access_token, clientOrigin),
        "content-type": "application/json",
        "idempotency-key": unknownNamespaceKey,
      },
      body: JSON.stringify({ text: "cannot create a new namespace", input_mode: "message" }),
    },
  );
  assert.equal(expiredNamespacePost.status, 410);
  assert.deepEqual(JSON.parse(expiredNamespacePost.body), {
    error: "idempotency_namespace_expired_or_unknown",
  });
  const expiredNamespaceStatus = await tlsRequest(tlsFixture, statusPath, {
    headers: {
      ...clientBearerHeaders(token.access_token, clientOrigin),
      "idempotency-key": unknownNamespaceKey,
    },
  });
  assert.equal(expiredNamespaceStatus.status, 410);
  assert.deepEqual(JSON.parse(expiredNamespaceStatus.body), {
    error: "idempotency_namespace_expired_or_unknown",
  });
  const blankMessage = await submit(
    rosterAgent.session_id,
    "  \n",
    `${token.idempotency_namespace}.${crypto.randomUUID()}`,
  );
  assert.equal(blankMessage.status, 400);
  assert.deepEqual(JSON.parse(blankMessage.body), { error: "invalid_request" });
  const oversizedUtf8 = await submit(
    rosterAgent.session_id,
    "é".repeat(32_769),
    `${token.idempotency_namespace}.${crypto.randomUUID()}`,
  );
  assert.equal(oversizedUtf8.status, 400);
  assert.deepEqual(JSON.parse(oversizedUtf8.body), { error: "invalid_request" });
  const extraMessageField = await tlsRequest(tlsFixture, messagePath, {
    method: "POST",
    headers: {
      ...clientBearerHeaders(token.access_token, clientOrigin),
      "content-type": "application/json",
      "idempotency-key": `${token.idempotency_namespace}.${crypto.randomUUID()}`,
    },
    body: JSON.stringify({
      text: "unexpected target fields must be rejected",
      input_mode: "message",
      target_session_id: "another-agent",
    }),
  });
  assert.equal(extraMessageField.status, 400);
  assert.deepEqual(JSON.parse(extraMessageField.body), { error: "invalid_request" });
  const oversizedRawBody = await tlsRequest(tlsFixture, messagePath, {
    method: "POST",
    headers: {
      ...clientBearerHeaders(token.access_token, clientOrigin),
      "content-type": "application/json",
      "idempotency-key": `${token.idempotency_namespace}.${crypto.randomUUID()}`,
    },
    body: JSON.stringify({ text: "x".repeat(512 * 1024), input_mode: "message" }),
  });
  assert.equal(oversizedRawBody.status, 413);
  assert.deepEqual(JSON.parse(oversizedRawBody.body), { error: "request_too_large" });

  const sinkAgent = await testSupport.spawnAgent({
    sessionName: "RemoteClientSendFixture",
    isOff: false,
    scenario: "interactive_idle_echo",
    delayMs: 10,
  });
  assert.ok(sinkAgent.session_id);

  await waitForRemoteClientCondition({
    timeoutMs: 10_000,
    intervalMs: 100,
    label: "sink mock agent readiness",
    request: ({ signal, timeoutMs }) => tlsRequest(tlsFixture, "/remote/client/v1/agents", {
      headers: clientBearerHeaders(token.access_token, clientOrigin),
      signal,
      timeoutMs,
    }),
    inspect: (response) => {
      assert.equal(response.status, 200, response.body);
      const currentRoster = JSON.parse(response.body);
      const currentAgent = currentRoster.agents.find(
        (agent) => agent.session_id === sinkAgent.session_id,
      );
      return { ready: currentAgent?.status?.toLowerCase() === "idle", value: currentAgent };
    },
  });

  owned.acceptanceFixture.ready({
    authorized_roster: rosterResponseVerified,
    chat_ready: clientChatPage.events.length === 80 && clientChatPage.progress === "ready",
    test_agent_ids: [rosterAgent.session_id, chatAgent.session_id, sinkAgent.session_id],
  });
  hudHoldOutcome = owned.acceptanceFixture.holdUntilReleased()
    .then(() => null, (error) => error);

  const sinkText = `WARDIAN_M2_SINK_${RUN_ID.replace(/[^A-Za-z0-9_]/gu, "_")}`;
  const sinkRequestId = crypto.randomUUID();
  const sinkIdempotencyKey = `${token.idempotency_namespace}.${sinkRequestId}`;
  const sinkMessagePath = `/remote/client/v1/agents/${sinkAgent.session_id}/messages`;
  const submitToSink = (accessToken = token.access_token) => tlsRequest(tlsFixture, sinkMessagePath, {
    method: "POST",
    headers: {
      ...clientBearerHeaders(accessToken, clientOrigin),
      "content-type": "application/json",
      "idempotency-key": sinkIdempotencyKey,
    },
    body: JSON.stringify({ text: sinkText, input_mode: "message" }),
  });
  const duplicateCursor = testSupport.responseCursor();
  const concurrentSinkResponses = await browserPage.evaluate((request) => (
    window.__wardianClientAcceptance.duplicateRequests(request)
  ), {
    gatewayOrigin: tlsFixture.ready.gateway_origin,
    requestPath: sinkMessagePath,
    tokenKey: token.access_token,
    headers: {
      "content-type": "application/json",
      "idempotency-key": sinkIdempotencyKey,
    },
    body: { text: sinkText, input_mode: "message" },
  });
  const duplicateObservations = testSupport.responsesSince(duplicateCursor)
    .filter((item) => item.method === "POST" && item.path === sinkMessagePath);
  assert.equal(duplicateObservations.length, 2, "both browser workers must reach the native gateway through TLS");
  assert.deepEqual(
    duplicateObservations.map((item) => item.status).sort(),
    concurrentSinkResponses.map((item) => item.status).sort(),
  );
  for (const observation of duplicateObservations) assertClientBoundary(observation, clientOrigin);
  const concurrentSinkOperations = concurrentSinkResponses.map((response) => {
    assert.equal(response.status, 202, response.body);
    return JSON.parse(response.body);
  });
  assert.equal(
    new Set(concurrentSinkOperations.map((item) => item.operation_id)).size,
    1,
    "concurrent identical requests must reserve one durable operation",
  );
  assert.equal(concurrentSinkOperations[0].request_id, sinkRequestId);

  const sinkChatPath = `/remote/client/v1/agents/${sinkAgent.session_id}/chat`;
  const sinkChatPage = await waitForRemoteClientCondition({
    timeoutMs: 20_000,
    intervalMs: 100,
    label: "sink provider transcript visibility",
    request: ({ signal, timeoutMs }) => tlsRequest(tlsFixture, sinkChatPath, {
      headers: clientBearerHeaders(token.access_token, clientOrigin),
      signal,
      timeoutMs,
    }),
    inspect: (response) => {
      assert.equal(response.status, 200, response.body);
      const candidate = JSON.parse(response.body);
      return {
        ready: candidate.events?.some((event) => event.text === sinkText) === true,
        value: candidate,
      };
    },
  });
  assert.equal(
    sinkChatPage.events.filter((event) => event.text === sinkText).length,
    1,
    "concurrent same-key requests must cross the provider boundary once",
  );
  const settledSinkStatus = await waitForRemoteClientCondition({
    timeoutMs: 30_000,
    intervalMs: 50,
    label: "sink intent settlement",
    request: ({ signal, timeoutMs }) => tlsRequest(tlsFixture, statusPath, {
      headers: {
        ...clientBearerHeaders(token.access_token, clientOrigin),
        "idempotency-key": sinkIdempotencyKey,
      },
      signal,
      timeoutMs,
    }),
    inspect: (response) => {
      assert.equal(response.status, 200, response.body);
      const candidate = JSON.parse(response.body);
      return {
        ready: ["provider_accepted", "turn_started", "completed"].includes(candidate.delivery_state),
        value: candidate,
      };
    },
  });
  assert.equal(settledSinkStatus.operation_id, concurrentSinkOperations[0].operation_id);
  assert.ok(
    ["provider_accepted", "turn_started", "completed"].includes(settledSinkStatus.delivery_state),
    `sink delivery must have positive provider acceptance evidence, got ${settledSinkStatus.delivery_state}`,
  );

  const settledDuplicate = await submitToSink();
  assert.equal(settledDuplicate.status, 202, settledDuplicate.body);
  assert.equal(JSON.parse(settledDuplicate.body).operation_id, settledSinkStatus.operation_id);
  await new Promise((resolve) => setTimeout(resolve, 250));
  const afterDuplicate = await tlsRequest(tlsFixture, sinkChatPath, {
    headers: clientBearerHeaders(token.access_token, clientOrigin),
  });
  assert.equal(afterDuplicate.status, 200, afterDuplicate.body);
  const afterDuplicatePage = JSON.parse(afterDuplicate.body);
  assert.equal(
    afterDuplicatePage.events.filter((event) => event.text === sinkText).length,
    1,
    "a completed same-key retry must not submit the message again",
  );

  const sinkTranscriptBeforeRestart = testSupport.readMockTranscript(sinkAgent.session_id);
  const sinkTranscriptBeforeCounts = countMockTranscriptNonceEvents(sinkTranscriptBeforeRestart, sinkText);
  assert.deepEqual(sinkTranscriptBeforeCounts, {
    user_input_count: 1,
    assistant_echo_count: 1,
  }, "the raw mock transcript must contain one input and one assistant echo");
  const sinkTranscriptBeforeSha256 = sha256Bytes(sinkTranscriptBeforeRestart);

  const restart = await testSupport.restart();
  assert.equal(restart.previous_session_joined, true);
  assert.equal(restart.backend_health, true);
  assert.equal(owned.acceptanceFixture.context.app_sha256, fixtureContext.app_sha256);
  assert.equal(owned.acceptanceFixture.context.cli_sha256, fixtureContext.cli_sha256);

  const staleBearer = await tlsRequest(tlsFixture, "/remote/client/v1/sharing", {
    headers: clientBearerHeaders(token.access_token, clientOrigin),
  });
  assert.equal(staleBearer.status, 401);
  assertClientBoundary(staleBearer, clientOrigin);
  assert.deepEqual(JSON.parse(staleBearer.body), { error: "auth_required" });

  const refreshCursor = testSupport.responseCursor();
  const browserRefresh = await browserPage.evaluate((gatewayOrigin) => (
    window.__wardianClientAcceptance.refresh(gatewayOrigin)
  ), tlsFixture.ready.gateway_origin);
  assert.equal(browserRefresh.challengeStatus, 200);
  assert.equal(browserRefresh.tokenStatus, 200);
  const refreshResponses = testSupport.responsesSince(refreshCursor)
    .filter((item) => [
      "/remote/client/v1/auth/challenge",
      "/remote/client/v1/auth/token",
    ].includes(item.path));
  assert.equal(refreshResponses.length, 2);
  for (const response of refreshResponses) assertClientBoundary(response, clientOrigin);
  const refreshedToken = browserRefresh.token;
  assert.notEqual(refreshedToken.access_token, token.access_token);
  assert.equal(refreshedToken.idempotency_namespace, token.idempotency_namespace);
  assert.equal(sinkIdempotencyKey, `${refreshedToken.idempotency_namespace}.${sinkRequestId}`);
  const refreshedStatus = await tlsRequest(tlsFixture, statusPath, {
    headers: {
      ...clientBearerHeaders(refreshedToken.access_token, clientOrigin),
      "idempotency-key": idempotencyKey,
    },
  });
  assert.equal(refreshedStatus.status, 200, refreshedStatus.body);
  assert.equal(JSON.parse(refreshedStatus.body).operation_id, operation.operation_id);

  const restartedSinkStatus = await tlsRequest(tlsFixture, statusPath, {
    headers: {
      ...clientBearerHeaders(refreshedToken.access_token, clientOrigin),
      "idempotency-key": sinkIdempotencyKey,
    },
  });
  assert.equal(restartedSinkStatus.status, 200, restartedSinkStatus.body);
  const restartedSinkOperation = JSON.parse(restartedSinkStatus.body);
  assert.equal(restartedSinkOperation.operation_id, settledSinkStatus.operation_id);
  assert.equal(restartedSinkOperation.request_id, sinkRequestId);
  assert.equal(restartedSinkOperation.idempotency_expires_at, token.idempotency_expires_at);

  const restartedDuplicate = await submitToSink(refreshedToken.access_token);
  assert.equal(restartedDuplicate.status, 202, restartedDuplicate.body);
  assertClientBoundary(restartedDuplicate, clientOrigin);
  assert.equal(JSON.parse(restartedDuplicate.body).operation_id, settledSinkStatus.operation_id);

  const sinkTranscriptAfterRestart = testSupport.readMockTranscript(sinkAgent.session_id);
  const sinkTranscriptAfterCounts = countMockTranscriptNonceEvents(sinkTranscriptAfterRestart, sinkText);
  const sinkTranscriptDelta = mockTranscriptDelta(sinkTranscriptBeforeRestart, sinkTranscriptAfterRestart);
  const sinkTranscriptDeltaCounts = countMockTranscriptNonceEvents(sinkTranscriptDelta, sinkText, {
    allowEmpty: true,
  });
  assert.deepEqual(sinkTranscriptAfterCounts, sinkTranscriptBeforeCounts);
  assert.deepEqual(sinkTranscriptDeltaCounts, {
    user_input_count: 0,
    assistant_echo_count: 0,
  }, "restart and duplicate lookup must add no provider input or assistant echo");
  process.stdout.write(`${JSON.stringify({
    event: "remote_client_mock_sink_transcript_evidence",
    before_sha256: sinkTranscriptBeforeSha256,
    after_sha256: sha256Bytes(sinkTranscriptAfterRestart),
    before_counts: sinkTranscriptBeforeCounts,
    restart_delta_counts: sinkTranscriptDeltaCounts,
    operation_id: restartedSinkOperation.operation_id,
  })}\n`);

  const rosterCookieOnly = await tlsRequest(tlsFixture, "/remote/client/v1/agents", {
    headers: { cookie: "wardian_client_token=ignored", origin: clientOrigin },
  });
  assert.equal(rosterCookieOnly.status, 401);
  assertClientBoundary(rosterCookieOnly, clientOrigin);
  assert.deepEqual(JSON.parse(rosterCookieOnly.body), { error: "auth_required" });

  const cookieOnly = await tlsRequest(tlsFixture, "/remote/client/v1/sharing", {
    headers: { cookie: "wardian_client_token=ignored", origin: clientOrigin },
  });
  assert.equal(cookieOnly.status, 401);
  assertClientBoundary(cookieOnly, clientOrigin);
  assert.deepEqual(JSON.parse(cookieOnly.body), { error: "auth_required" });

  const legacyPath = await tlsRequest(tlsFixture, "/remote/client/v1/auth/session", {
    method: "POST",
    headers: { "content-type": "application/json", origin: clientOrigin },
    body: "{}",
  });
  assert.equal(legacyPath.status, 404);
  assertClientBoundary(legacyPath, clientOrigin);
  assert.deepEqual(JSON.parse(legacyPath.body), { error: "not_found" });

  const forbiddenOrigin = await tlsRequest(tlsFixture, "/remote/client/v1/auth/challenge", {
    method: "POST",
    headers: {
      "content-type": "application/json",
      origin: "https://unapproved.example",
    },
    body: JSON.stringify({ device_id: pending.device_id }),
  });
  assert.equal(forbiddenOrigin.status, 403);
  assert.equal(forbiddenOrigin.headers["cache-control"], "no-store");
  assert.equal(forbiddenOrigin.headers["access-control-allow-origin"], undefined);
  assert.equal(forbiddenOrigin.headers["access-control-allow-credentials"], undefined);
  assert.equal(forbiddenOrigin.headers["set-cookie"], undefined);
  assert.deepEqual(JSON.parse(forbiddenOrigin.body), { error: "origin_forbidden" });

  const completedHudHold = hudHoldOutcome;
  owned.acceptanceFixture.controls.release();
  hudHoldOutcome = undefined;
  const holdError = await completedHudHold;
  if (holdError) throw holdError;

  await owned.acceptanceFixture.controls.revoke();
  const revokedBearer = await tlsRequest(tlsFixture, "/remote/client/v1/sharing", {
    headers: clientBearerHeaders(refreshedToken.access_token, clientOrigin),
  });
  assert.equal(revokedBearer.status, 403);
  assert.deepEqual(JSON.parse(revokedBearer.body), { error: "device_revoked" });
});
