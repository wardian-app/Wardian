import crypto from "node:crypto";
import fs from "node:fs";
import { performance } from "node:perf_hooks";

export const REMOTE_CHAT_READY_TIMEOUT_MS = 120_000;
export const REMOTE_CLIENT_REQUEST_TIMEOUT_MS = 15_000;

function abortReason(signal) {
  return signal.reason ?? Object.assign(new Error("Remote-client fixture operation was cancelled"), {
    name: "AbortError",
  });
}

function raceWithSignal(promise, signal) {
  if (signal.aborted) return Promise.reject(abortReason(signal));
  let onAbort;
  const aborted = new Promise((_, reject) => {
    onAbort = () => reject(abortReason(signal));
    signal.addEventListener("abort", onAbort, { once: true });
  });
  return Promise.race([promise, aborted]).finally(() => {
    signal.removeEventListener("abort", onAbort);
  });
}

function timeoutError(timeoutMs, label) {
  const error = new Error(`${label} exceeded its ${timeoutMs}ms absolute deadline`);
  error.name = "TimeoutError";
  error.code = "REMOTE_CLIENT_FIXTURE_TIMEOUT";
  return error;
}

function assertExactOrigin(hudOrigin) {
  if (typeof hudOrigin !== "string") throw new TypeError("hudOrigin must be an exact HTTP(S) origin");
  let parsed;
  try {
    parsed = new URL(hudOrigin);
  } catch {
    throw new TypeError("hudOrigin must be an exact HTTP(S) origin");
  }
  if (
    !["http:", "https:"].includes(parsed.protocol)
    || parsed.origin !== hudOrigin
    || parsed.username
    || parsed.password
    || parsed.pathname !== "/"
    || parsed.search
    || parsed.hash
  ) {
    throw new TypeError("hudOrigin must contain only the exact HTTP(S) origin");
  }
  return parsed.origin;
}

function requireText(value, label) {
  if (typeof value !== "string" || value.length === 0) throw new TypeError(`${label} is required`);
  return value;
}

function requireSha256(value, label) {
  if (typeof value !== "string" || !/^[0-9a-f]{64}$/u.test(value)) {
    throw new TypeError(`${label} must be a lowercase SHA-256 digest`);
  }
  return value;
}

function createPublicReadyRecord(hudOrigin, evidence, holdTimeoutMs) {
  if (
    evidence?.backend_health !== true
    || evidence?.authorized_roster !== true
    || evidence?.chat_ready !== true
  ) {
    throw new Error("Acceptance fixture cannot become ready before health, authorized roster, and Chat are ready");
  }
  const gatewayUrl = new URL(requireText(evidence.gateway_url, "gateway_url"));
  if (gatewayUrl.protocol !== "https:" || gatewayUrl.origin !== evidence.gateway_url) {
    throw new TypeError("gateway_url must be the exact HTTPS origin bound by the TLS fixture");
  }
  const agentIds = evidence.test_agent_ids;
  if (!Array.isArray(agentIds) || agentIds.length === 0 || agentIds.some((id) => typeof id !== "string" || !id)) {
    throw new TypeError("test_agent_ids must contain the owned fixture agent IDs");
  }
  const readyAt = new Date().toISOString();
  return Object.freeze({
    event: "remote_client_acceptance_ready",
    gateway_url: gatewayUrl.origin,
    gateway_host: gatewayUrl.host,
    hud_origin: hudOrigin,
    certificate_sha256: requireSha256(evidence.certificate_sha256, "certificate_sha256"),
    certificate_spki_sha256_base64: requireText(
      evidence.certificate_spki_sha256_base64,
      "certificate_spki_sha256_base64",
    ),
    source_commit: requireText(evidence.source_commit, "source_commit"),
    source_delta_sha256: requireSha256(evidence.source_delta_sha256, "source_delta_sha256"),
    app_sha256: requireSha256(evidence.app_sha256, "app_sha256"),
    cli_sha256: requireSha256(evidence.cli_sha256, "cli_sha256"),
    test_agent_ids: Object.freeze([...agentIds]),
    ownership: Object.freeze({
      run_id: requireText(evidence.ownership.run_id, "ownership.run_id"),
      owned_by_native_harness: true,
    }),
    ready_at_utc: readyAt,
    hold_deadline_utc: new Date(Date.now() + holdTimeoutMs).toISOString(),
  });
}

/**
 * Create the callback lifecycle for an owned native acceptance fixture.
 * Setup can register resources incrementally in `owned`, so close still runs
 * if setup throws. Only the sanitized public ready record is exposed to HUD
 * handoff callbacks.
 */
export function createRemoteClientAcceptanceFixture({
  hudOrigin,
  close,
  onReady,
  holdTimeoutMs = 180_000,
  now = () => performance.now(),
  schedule = setTimeout,
  cancel = clearTimeout,
}) {
  const exactHudOrigin = assertExactOrigin(hudOrigin);
  if (typeof close !== "function") throw new TypeError("close callback is required");
  if (!Number.isFinite(holdTimeoutMs) || holdTimeoutMs <= 0) {
    throw new TypeError("holdTimeoutMs must be a positive finite duration");
  }
  const owned = Object.create(null);
  let state = "created";
  let readyRecord;
  let closePromise;
  let activeHold;
  const activePairings = new Set();

  function assertPairingOpen(signal) {
    if (signal.aborted || !["setup", "ready"].includes(state)) {
      throw signal.aborted ? abortReason(signal) : Object.assign(
        new Error(`pairing was cancelled while fixture is ${state}`),
        { name: "AbortError" },
      );
    }
  }

  return Object.freeze({
    get state() { return state; },
    get hudOrigin() { return exactHudOrigin; },
    get owned() { return owned; },
    async setup(callback) {
      if (state !== "created") throw new Error(`setup is invalid while fixture is ${state}`);
      if (typeof callback !== "function") throw new TypeError("setup callback is required");
      state = "setting_up";
      try {
        const result = await callback({ hudOrigin: exactHudOrigin, owned });
        if (result && typeof result === "object") Object.assign(owned, result);
        state = "setup";
        return owned;
      } catch (error) {
        state = "setup_failed";
        try {
          await this.close();
        } catch (closeError) {
          throw new AggregateError([error, closeError], "Acceptance setup and owned cleanup both failed", { cause: closeError });
        }
        throw error;
      }
    },
    pairClient({
      clientOrigin = exactHudOrigin,
      keyPair,
      deviceLabel,
      requestedScopes,
      createOffer,
      submitPairing,
      approvePairing,
    }) {
      if (!["setup", "ready"].includes(state)) throw new Error(`pairing is invalid while fixture is ${state}`);
      const exactClientOrigin = assertExactOrigin(clientOrigin);
      for (const [callback, name] of [
        [createOffer, "createOffer"],
        [submitPairing, "submitPairing"],
        [approvePairing, "approvePairing"],
      ]) {
        if (typeof callback !== "function") throw new TypeError(`${name} callback is required`);
      }
      requireText(deviceLabel, "deviceLabel");
      if (!Array.isArray(requestedScopes) || requestedScopes.length === 0) {
        throw new TypeError("requestedScopes must be a non-empty array");
      }
      const controller = new AbortController();
      const ownedKeyPair = keyPair ?? crypto.generateKeyPairSync("ec", { namedCurve: "P-256" });
      if (ownedKeyPair?.publicKey?.asymmetricKeyType !== "ec"
        || ownedKeyPair.publicKey.asymmetricKeyDetails?.namedCurve !== "prime256v1") {
        throw new TypeError("keyPair must contain a P-256 public key");
      }
      const publicKeySpkiDer = ownedKeyPair.publicKey.export({ type: "spki", format: "der" });
      const entry = { controller, promise: undefined };
      activePairings.add(entry);
      const operation = (async () => {
        assertPairingOpen(controller.signal);
        const offer = await raceWithSignal(
          Promise.resolve().then(() => {
            assertPairingOpen(controller.signal);
            return createOffer({ clientOrigin: exactClientOrigin, signal: controller.signal });
          }),
          controller.signal,
        );
        assertPairingOpen(controller.signal);
        if (offer?.client_origin !== exactClientOrigin) {
          throw new Error("pairing offer origin did not match the owned client origin");
        }
        const pending = await raceWithSignal(
          Promise.resolve().then(() => submitPairing({
            clientOrigin: exactClientOrigin,
            offer,
            deviceLabel,
            publicKeySpkiDerBase64: publicKeySpkiDer.toString("base64"),
            signal: controller.signal,
          })),
          controller.signal,
        );
        assertPairingOpen(controller.signal);
        if (pending?.status !== "pending" || !pending.pairing_request_id || !pending.device_id) {
          throw new Error("pairing submission did not return a pending owned request");
        }
        await raceWithSignal(
          Promise.resolve().then(() => approvePairing({
            clientOrigin: exactClientOrigin,
            offer,
            pending,
            signal: controller.signal,
          })),
          controller.signal,
        );
        assertPairingOpen(controller.signal);
        const pairing = Object.freeze({ clientOrigin: exactClientOrigin, offer, pending, keyPair: ownedKeyPair });
        owned.pairing = pairing;
        return pairing;
      })();
      entry.promise = operation.finally(() => activePairings.delete(entry));
      return entry.promise;
    },
    ready(evidence) {
      if (state !== "setup") throw new Error(`ready is invalid while fixture is ${state}`);
      readyRecord = createPublicReadyRecord(exactHudOrigin, evidence, holdTimeoutMs);
      state = "ready";
      onReady?.(readyRecord);
      return readyRecord;
    },
    holdUntilReleased(waitForRelease) {
      if (state !== "ready") throw new Error(`hold is invalid while fixture is ${state}`);
      if (typeof waitForRelease !== "function") throw new TypeError("hold release callback is required");
      state = "holding";
      const deadlineAt = now() + holdTimeoutMs;
      const controller = new AbortController();
      const deadlineError = timeoutError(holdTimeoutMs, "HUD acceptance hold");
      const timer = schedule(() => controller.abort(deadlineError), holdTimeoutMs);
      const promise = (async () => {
       try {
        const release = Promise.resolve().then(() => waitForRelease({
          hudOrigin: exactHudOrigin,
          ready: readyRecord,
          signal: controller.signal,
          deadlineAt,
        }));
        const value = await raceWithSignal(release, controller.signal);
        if (now() >= deadlineAt) {
          controller.abort(deadlineError);
          throw deadlineError;
        }
        if (state !== "holding") throw abortReason(controller.signal);
        state = "released";
        return value;
      } catch (error) {
        if (state === "holding") state = controller.signal.aborted ? "hold_timed_out" : "hold_failed";
        throw controller.signal.aborted ? abortReason(controller.signal) : error;
      } finally {
        cancel(timer);
        if (activeHold?.promise === promise) activeHold = undefined;
      }
      })();
      activeHold = { controller, promise };
      return promise;
    },
    waitForExternalRelease(waitForRelease) {
      return this.holdUntilReleased(waitForRelease);
    },
    close() {
      if (closePromise) return closePromise;
      state = "closing";
      const hold = activeHold;
      const pairings = [...activePairings];
      for (const pairing of pairings) {
        pairing.controller.abort(Object.assign(new Error("Acceptance fixture closed during pairing"), {
          name: "AbortError",
        }));
      }
      hold?.controller.abort(Object.assign(new Error("Acceptance fixture closed during external release hold"), {
        name: "AbortError",
      }));
      closePromise = Promise.resolve().then(async () => {
        await Promise.allSettled(pairings.map(({ promise }) => promise));
        if (hold) await hold.promise.catch(() => {});
        await close({ hudOrigin: exactHudOrigin, owned, ready: readyRecord });
      }).then(() => {
        state = "closed";
      });
      return closePromise;
    },
  });
}

/** Poll an asynchronous readiness condition under one monotonic deadline. */
export async function waitForRemoteClientCondition({
  request,
  inspect,
  timeoutMs = REMOTE_CHAT_READY_TIMEOUT_MS,
  intervalMs = 250,
  signal,
  now = () => performance.now(),
  schedule = setTimeout,
  cancel = clearTimeout,
  label = "Remote-client readiness",
}) {
  if (typeof request !== "function" || typeof inspect !== "function") {
    throw new TypeError("request and inspect callbacks are required");
  }
  if (!Number.isFinite(timeoutMs) || timeoutMs <= 0 || !Number.isFinite(intervalMs) || intervalMs < 0) {
    throw new TypeError("timeoutMs must be positive and intervalMs must be non-negative");
  }
  const deadlineAt = now() + timeoutMs;
  const controller = new AbortController();
  const error = timeoutError(timeoutMs, label);
  const timer = schedule(() => controller.abort(error), timeoutMs);
  const forwardAbort = () => controller.abort(abortReason(signal));
  if (signal?.aborted) forwardAbort();
  else signal?.addEventListener("abort", forwardAbort, { once: true });

  const assertBeforeDeadline = () => {
    if (controller.signal.aborted) throw abortReason(controller.signal);
    if (now() >= deadlineAt) {
      controller.abort(error);
      throw error;
    }
  };

  try {
    while (true) {
      assertBeforeDeadline();
      const remainingMs = deadlineAt - now();
      const requestTimeoutMs = Math.max(1, Math.min(
        REMOTE_CLIENT_REQUEST_TIMEOUT_MS,
        Math.ceil(remainingMs),
      ));
      const pendingRequest = Promise.resolve().then(() => request({
        signal: controller.signal,
        timeoutMs: requestTimeoutMs,
        deadlineAt,
      }));
      const response = await raceWithSignal(pendingRequest, controller.signal);
      assertBeforeDeadline();
      const observation = await inspect(response);
      assertBeforeDeadline();
      if (observation?.ready === true) return observation.value;
      const sleepMs = Math.min(intervalMs, deadlineAt - now());
      if (sleepMs <= 0) {
        controller.abort(error);
        throw error;
      }
      let sleepTimer;
      const sleep = new Promise((resolve) => {
        sleepTimer = schedule(resolve, sleepMs);
      });
      try {
        await raceWithSignal(sleep, controller.signal);
      } finally {
        cancel(sleepTimer);
      }
      assertBeforeDeadline();
    }
  } catch (caught) {
    if (controller.signal.aborted) throw abortReason(controller.signal);
    if (now() >= deadlineAt) {
      controller.abort(error);
      throw error;
    }
    throw caught;
  } finally {
    cancel(timer);
    signal?.removeEventListener("abort", forwardAbort);
  }
}

export function readMockTranscript(logPath) {
  return fs.readFileSync(logPath);
}

export function countMockTranscriptNonceEvents(bytes, nonce, { allowEmpty = false } = {}) {
  if (!Buffer.isBuffer(bytes)) throw new TypeError("mock transcript must be read as raw bytes");
  if (typeof nonce !== "string" || nonce.length === 0) throw new TypeError("nonce is required");
  const text = bytes.toString("utf8");
  if (text.length === 0) {
    if (allowEmpty) return { user_input_count: 0, assistant_echo_count: 0 };
    throw new Error("mock transcript is empty");
  }
  const lines = text.split(/\r?\n/u).filter((line) => line.length > 0);
  const events = lines.map((line, index) => {
    let event;
    try {
      event = JSON.parse(line);
    } catch (error) {
      throw new Error(`mock transcript line ${index + 1} is malformed`, { cause: error });
    }
    if (!event || typeof event !== "object" || Array.isArray(event) || typeof event.type !== "string") {
      throw new Error(`mock transcript line ${index + 1} is not a provider event`);
    }
    return event;
  });
  return {
    user_input_count: events.filter((event) => event.type === "user" && event.content === nonce).length,
    assistant_echo_count: events.filter((event) => event.type === "model" && event.content === `Echo: ${nonce}`).length,
  };
}

export function mockTranscriptDelta(before, after) {
  if (!Buffer.isBuffer(before) || !Buffer.isBuffer(after)) {
    throw new TypeError("mock transcript snapshots must be raw Buffers");
  }
  if (before.equals(after)) return Buffer.alloc(0);
  if (after.length >= before.length && after.subarray(0, before.length).equals(before)) {
    return after.subarray(before.length);
  }
  return after;
}

const DUPLICATE_REQUEST_WORKER = String.raw`
import https from "node:https";
import readline from "node:readline";
const input = readline.createInterface({ input: process.stdin, crlfDelay: Infinity });
const lines = input[Symbol.asyncIterator]();
const configLine = await lines.next();
if (configLine.done) throw new Error("duplicate client config was not provided");
const config = JSON.parse(configLine.value);
process.stdout.write("READY\n");
const releaseLine = await lines.next();
if (releaseLine.done || releaseLine.value !== "GO") throw new Error("duplicate client barrier was not released");
input.close();
const body = JSON.stringify(config.body);
const request = https.request({
  hostname: config.hostname,
  port: config.port,
  path: config.path,
  method: "POST",
  headers: config.headers,
  ca: config.ca,
  rejectUnauthorized: true,
  timeout: config.timeoutMs,
}, (response) => {
  const chunks = [];
  response.on("data", (chunk) => chunks.push(chunk));
  response.once("error", (error) => {
    process.stderr.write(JSON.stringify({ name: error.name, code: error.code }) + "\n");
    process.exitCode = 1;
  });
  response.once("end", () => {
    process.stdout.write(JSON.stringify({ status: response.statusCode, headers: response.headers, body: Buffer.concat(chunks).toString("utf8") }) + "\n");
  });
});
request.once("timeout", () => request.destroy(Object.assign(new Error("duplicate client request timed out"), { code: "ETIMEDOUT" })));
request.once("error", (error) => {
  process.stderr.write(JSON.stringify({ name: error.name, code: error.code }) + "\n");
  process.exitCode = 1;
});
request.end(body);
`;

/** Send one identical request from each of two separately owned Node processes. */
export async function submitDuplicateRemoteClientRequests({
  request,
  spawn = (command, args, options) => import("node:child_process").then(({ spawn: launch }) => launch(command, args, options)),
  timeoutMs = REMOTE_CLIENT_REQUEST_TIMEOUT_MS,
  now = () => performance.now(),
  schedule = setTimeout,
  cancel = clearTimeout,
}) {
  const url = new URL(request.url);
  if (url.protocol !== "https:") throw new TypeError("duplicate client requests require HTTPS");
  const workerInput = {
    hostname: url.hostname,
    port: Number(url.port || 443),
    path: `${url.pathname}${url.search}`,
    headers: request.headers,
    ca: request.ca,
    body: request.body,
    timeoutMs,
  };
  const children = [];
  const outputs = [];
  const waitForChildLine = (child, stream, label) => new Promise((resolve, reject) => {
    let buffered = "";
    let settled = false;
    const cleanup = () => {
      stream.removeListener("data", onData);
      child.removeListener("error", onError);
      child.removeListener("close", onClose);
    };
    const finish = (callback, value) => {
      if (settled) return;
      settled = true;
      cleanup();
      callback(value);
    };
    const onData = (chunk) => {
      buffered += chunk.toString("utf8");
      const newline = buffered.indexOf("\n");
      if (newline < 0) return;
      finish(resolve, buffered.slice(0, newline));
    };
    const onError = (error) => finish(reject, error);
    const onClose = (code, signal) => finish(reject, new Error(
      `${label} exited before its response (exit ${code ?? "signal"} ${signal ?? ""})`,
    ));
    stream.on("data", onData);
    child.once("error", onError);
    child.once("close", onClose);
  });
  const stopOwnedChildren = async () => {
    for (const child of children) {
      if (child.exitCode === null && child.signalCode === null) child.kill();
    }
    await Promise.all(children.map((child) => new Promise((resolve) => {
      if (child.exitCode !== null || child.signalCode !== null) resolve();
      else child.once("close", resolve);
    })));
  };
  const deadlineAt = now() + timeoutMs;
  const timer = schedule(() => {
    for (const child of children) {
      if (child.exitCode === null && child.signalCode === null) child.kill();
    }
  }, timeoutMs);
  try {
    for (let index = 0; index < 2; index += 1) {
      const child = await spawn(process.execPath, ["--input-type=module", "-e", DUPLICATE_REQUEST_WORKER], {
        cwd: process.cwd(),
        stdio: ["pipe", "pipe", "pipe"],
        windowsHide: true,
      });
      children.push(child);
      child.stdin.write(`${JSON.stringify(workerInput)}\n`);
    }
    const readyLines = await Promise.all(children.map((child, index) => waitForChildLine(child, child.stdout, `client ${index + 1} readiness`)));
    if (readyLines.some((line) => line !== "READY")) throw new Error("duplicate client child did not reach the shared barrier");
    if (now() >= deadlineAt) throw timeoutError(timeoutMs, "Duplicate client race");
    const responseLines = children.map((child, index) => waitForChildLine(child, child.stdout, `client ${index + 1} response`));
    for (const child of children) child.stdin.end("GO\n");
    for (let index = 0; index < children.length; index += 1) {
      const line = await responseLines[index];
      outputs.push(JSON.parse(line));
    }
    await Promise.all(children.map((child) => new Promise((resolve, reject) => {
      if (child.exitCode !== null || child.signalCode !== null) {
        if (child.exitCode === 0) resolve();
        else reject(new Error(`duplicate client child exited ${child.exitCode ?? child.signalCode}`));
        return;
      }
      child.once("close", (code) => code === 0 ? resolve() : reject(new Error(`duplicate client child exited ${code}`)));
    })));
    if (now() >= deadlineAt) throw timeoutError(timeoutMs, "Duplicate client race");
    return outputs;
  } catch (error) {
    await stopOwnedChildren();
    throw error;
  } finally {
    cancel(timer);
  }
}

export function sha256Bytes(bytes) {
  if (!Buffer.isBuffer(bytes)) throw new TypeError("sha256Bytes requires a Buffer");
  return crypto.createHash("sha256").update(bytes).digest("hex");
}
