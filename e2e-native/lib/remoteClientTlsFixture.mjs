import crypto from "node:crypto";
import http from "node:http";
import https from "node:https";
import { once } from "node:events";

const SOURCE_COMMIT = /^[0-9a-f]{40,64}$/i;
const SOURCE_DELTA_DIGEST = /^[0-9a-f]{64}$/i;
const PWA_UPGRADE_HANDSHAKE_TIMEOUT_MS = 10_000;
const HOP_BY_HOP_HEADERS = new Set([
  "connection",
  "keep-alive",
  "proxy-authenticate",
  "proxy-authorization",
  "te",
  "trailer",
  "transfer-encoding",
  "upgrade",
]);
const WEBSOCKET_REQUEST_HEADERS = [
  "host",
  "origin",
  "cookie",
  "authorization",
  "sec-websocket-key",
  "sec-websocket-version",
  "sec-websocket-protocol",
  "sec-websocket-extensions",
];

function derLength(length) {
  if (length < 0x80) return Buffer.from([length]);
  const bytes = [];
  for (let value = length; value > 0; value >>>= 8) bytes.unshift(value & 0xff);
  return Buffer.from([0x80 | bytes.length, ...bytes]);
}

function der(tag, ...values) {
  const contents = Buffer.concat(values);
  return Buffer.concat([Buffer.from([tag]), derLength(contents.length), contents]);
}

function derSequence(...values) {
  return der(0x30, ...values);
}

function derOid(hex) {
  return der(0x06, Buffer.from(hex, "hex"));
}

function derInteger(value) {
  let bytes = Buffer.from(value);
  while (bytes.length > 1 && bytes[0] === 0) bytes = bytes.subarray(1);
  if (bytes[0] & 0x80) bytes = Buffer.concat([Buffer.from([0]), bytes]);
  return der(0x02, bytes);
}

function derBitString(value, unusedBits = 0) {
  return der(0x03, Buffer.concat([Buffer.from([unusedBits]), value]));
}

function derUtcTime(value) {
  const encoded = value.toISOString().slice(2, 19).replace(/[-:T]/g, "");
  return der(0x17, Buffer.from(`${encoded}Z`));
}

function derExtension(oidHex, value, critical = false) {
  return derSequence(
    derOid(oidHex),
    ...(critical ? [der(0x01, Buffer.from([0xff]))] : []),
    der(0x04, value),
  );
}

function certificateName() {
  return derSequence(der(0x31, derSequence(derOid("550403"), der(0x0c, Buffer.from("localhost")))));
}

/** Build an in-memory self-signed localhost certificate without changing trust stores. */
function makeFixtureCertificate() {
  const { privateKey, publicKey } = crypto.generateKeyPairSync("ec", {
    namedCurve: "prime256v1",
  });
  const signatureAlgorithm = derSequence(derOid("2a8648ce3d040302"));
  const serial = crypto.randomBytes(16);
  serial[0] &= 0x7f;
  if (serial.every((byte) => byte === 0)) serial[serial.length - 1] = 1;
  const subject = certificateName();
  const notBefore = new Date(Date.now() - 60_000);
  const notAfter = new Date(Date.now() + 24 * 60 * 60 * 1000);
  const subjectAltNames = derSequence(
    der(0x82, Buffer.from("localhost")),
    der(0x87, Buffer.from([127, 0, 0, 1])),
  );
  const extensions = derSequence(
    derExtension("551d13", derSequence(der(0x01, Buffer.from([0xff]))), true),
    derExtension("551d0f", derBitString(Buffer.from([0x84]), 2), true),
    derExtension("551d11", subjectAltNames),
    derExtension("551d25", derSequence(derOid("2b06010505070301"))),
  );
  const tbsCertificate = derSequence(
    der(0xa0, derInteger(Buffer.from([2]))),
    derInteger(serial),
    signatureAlgorithm,
    subject,
    derSequence(derUtcTime(notBefore), derUtcTime(notAfter)),
    subject,
    publicKey.export({ type: "spki", format: "der" }),
    der(0xa3, extensions),
  );
  const signature = crypto.sign("sha256", tbsCertificate, privateKey);
  const certificate = derSequence(
    tbsCertificate,
    signatureAlgorithm,
    derBitString(signature),
  ).toString("base64");
  const certificatePem = `-----BEGIN CERTIFICATE-----\n${certificate.match(/.{1,64}/g).join("\n")}\n-----END CERTIFICATE-----\n`;
  const privateKeyPem = privateKey.export({ type: "pkcs8", format: "pem" });
  return { certificatePem, privateKeyPem };
}

function forwardedHeaders(headers) {
  const result = { ...headers };
  const connectionTokens = String(headers.connection ?? "")
    .split(",")
    .map((token) => token.trim().toLowerCase())
    .filter(Boolean);
  for (const name of [...HOP_BY_HOP_HEADERS, ...connectionTokens]) delete result[name];
  return result;
}

function headerHasToken(value, expected) {
  return String(value ?? "")
    .split(",")
    .some((token) => token.trim().toLowerCase() === expected.toLowerCase());
}

function isPwaWebSocketPath(requestUrl) {
  if (typeof requestUrl !== "string") return false;
  const pathname = requestUrl.split("?", 1)[0];
  if (pathname === "/remote/api/status-stream") return true;

  const prefix = "/remote/api/agents/";
  const suffix = "/terminal-stream";
  if (!pathname.startsWith(prefix) || !pathname.endsWith(suffix)) return false;
  const sessionId = pathname.slice(prefix.length, -suffix.length);
  return sessionId.length > 0 && !sessionId.includes("/");
}

function isWebSocketUpgradeRequest(request) {
  return request.method === "GET"
    && String(request.headers.upgrade ?? "").toLowerCase() === "websocket"
    && headerHasToken(request.headers.connection, "upgrade");
}

function forwardedWebSocketHeaders(headers) {
  const result = forwardedHeaders(headers);
  for (const name of WEBSOCKET_REQUEST_HEADERS) {
    if (headers[name] !== undefined) result[name] = headers[name];
  }
  result.connection = "Upgrade";
  result.upgrade = "websocket";
  return result;
}

function rawHeaderPairs(rawHeaders, {
  preserveUpgrade = false,
  preserveWebSocketNegotiation = false,
} = {}) {
  const connectionTokens = new Set();
  for (let index = 0; index < rawHeaders.length; index += 2) {
    if (rawHeaders[index].toLowerCase() !== "connection") continue;
    for (const token of rawHeaders[index + 1].split(",")) {
      const normalized = token.trim().toLowerCase();
      if (normalized) connectionTokens.add(normalized);
    }
  }

  const pairs = [];
  for (let index = 0; index < rawHeaders.length; index += 2) {
    const name = rawHeaders[index];
    const normalized = name.toLowerCase();
    if (preserveUpgrade && (normalized === "connection" || normalized === "upgrade")) {
      pairs.push([name, rawHeaders[index + 1]]);
      continue;
    }
    if (preserveWebSocketNegotiation && normalized.startsWith("sec-websocket-")) {
      pairs.push([name, rawHeaders[index + 1]]);
      continue;
    }
    if (HOP_BY_HOP_HEADERS.has(normalized) || connectionTokens.has(normalized)) continue;
    pairs.push([name, rawHeaders[index + 1]]);
  }
  return pairs;
}

function serializeHttpResponseHead(httpVersion, statusCode, statusMessage, headerPairs) {
  const lines = [`HTTP/${httpVersion} ${statusCode} ${statusMessage}`];
  for (const [name, value] of headerPairs) lines.push(`${name}: ${value}`);
  return Buffer.from(`${lines.join("\r\n")}\r\n\r\n`);
}

function writeUpgradeError(socket, statusCode, statusMessage, code) {
  if (socket.destroyed || socket.writableEnded) return;
  const body = Buffer.from(JSON.stringify({ error: code }));
  const headers = [
    ["content-type", "application/json; charset=utf-8"],
    ["cache-control", "no-store"],
    ["content-length", String(body.length)],
    ["connection", "close"],
  ];
  socket.end(Buffer.concat([
    serializeHttpResponseHead("1.1", statusCode, statusMessage, headers),
    body,
  ]));
}

function isValidUpstreamWebSocketResponse(response) {
  return response.statusCode === 101
    && String(response.headers.upgrade ?? "").toLowerCase() === "websocket"
    && headerHasToken(response.headers.connection, "upgrade");
}

function writeUpstreamWebSocketResponse(response, socket) {
  const statusMessage = response.statusMessage || "Switching Protocols";
  const headers = rawHeaderPairs(response.rawHeaders, {
    preserveUpgrade: true,
    preserveWebSocketNegotiation: true,
  });
  socket.write(serializeHttpResponseHead(
    response.httpVersion,
    response.statusCode,
    statusMessage,
    headers,
  ));
}

function forwardNonUpgradeResponse(response, socket) {
  const statusCode = response.statusCode ?? 502;
  if (statusCode === 101) {
    response.destroy();
    socket.destroy();
    return;
  }

  const bodyless = statusCode === 204 || statusCode === 304 || (statusCode >= 100 && statusCode < 200);
  const headers = rawHeaderPairs(response.rawHeaders);
  const hasContentLength = headers.some(([name]) => name.toLowerCase() === "content-length");
  const chunked = !bodyless && !hasContentLength;
  if (chunked) headers.push(["transfer-encoding", "chunked"]);
  headers.push(["connection", "close"]);

  const statusMessage = response.statusMessage || "Upstream Error";
  if (!socket.write(serializeHttpResponseHead(
    response.httpVersion,
    statusCode,
    statusMessage,
    headers,
  ))) {
    response.pause();
  }

  if (bodyless) {
    response.once("end", () => socket.end());
    response.once("error", () => socket.destroy());
    response.resume();
    return;
  }

  let completed = false;
  const resumeResponse = () => response.resume();
  socket.on("drain", resumeResponse);
  response.on("data", (chunk) => {
    const frame = chunked
      ? Buffer.concat([Buffer.from(`${chunk.length.toString(16)}\r\n`), chunk, Buffer.from("\r\n")])
      : chunk;
    if (!socket.write(frame)) response.pause();
  });
  response.once("end", () => {
    completed = true;
    socket.removeListener("drain", resumeResponse);
    socket.end(chunked ? "0\r\n\r\n" : undefined);
  });
  response.once("close", () => {
    socket.removeListener("drain", resumeResponse);
    if (!completed && !socket.destroyed) socket.destroy();
  });
  response.once("error", () => socket.destroy());
  socket.once("close", () => response.destroy());
}

function isLoopbackHost(host) {
  return host === "localhost" || host === "127.0.0.1" || host === "::1";
}

/**
 * Start an HTTPS loopback front for a real ephemeral-loopback gateway.
 * The certificate is an in-memory, test-only self-signed CA leaf. Node clients
 * must trust it explicitly; the operating-system trust store is never changed.
 *
 * PWA WebSocket forwarding is opt-in and limited to the existing status and
 * terminal stream paths. The default rejects upgrades, including client v1.
 * The frozen ready record includes SHA-256 pins for the exact HTTPS leaf
 * certificate and its SPKI; these are public test metadata, not credentials.
 *
 * @param {{sourceCommit: string, sourceDeltaDigest: string, onReady?: (ready: object) => void, onResponse?: (observation: object) => void, allowPwaWebSocketUpgrades?: boolean}} options
 */
export async function startRemoteClientTlsFixture({
  sourceCommit,
  sourceDeltaDigest,
  onReady,
  onResponse,
  allowPwaWebSocketUpgrades = false,
}) {
  if (!SOURCE_COMMIT.test(sourceCommit ?? "")) {
    throw new TypeError("sourceCommit must be a full Git commit hash");
  }
  if (!SOURCE_DELTA_DIGEST.test(sourceDeltaDigest ?? "")) {
    throw new TypeError("sourceDeltaDigest must be a SHA-256 hex digest");
  }
  if (onResponse !== undefined && typeof onResponse !== "function") {
    throw new TypeError("onResponse must be a function when provided");
  }
  if (typeof allowPwaWebSocketUpgrades !== "boolean") {
    throw new TypeError("allowPwaWebSocketUpgrades must be a boolean");
  }

  const { certificatePem, privateKeyPem } = makeFixtureCertificate();
  const certificate = new crypto.X509Certificate(certificatePem);
  const certificateSha256 = crypto.createHash("sha256").update(certificate.raw).digest("hex");
  const certificateSpkiSha256Base64 = crypto.createHash("sha256")
    .update(certificate.publicKey.export({ type: "spki", format: "der" }))
    .digest("base64");
  let upstream = null;
  let closing = false;
  let closePromise = null;
  const sockets = new Set();
  const upstreamSockets = new Set();
  const upstreamRequests = new Set();
  const upstreamResponses = new Set();
  const handshakeTimers = new Set();
  const trackUpstreamSocket = (socket) => {
    if (upstreamSockets.has(socket)) return;
    upstreamSockets.add(socket);
    socket.once("close", () => upstreamSockets.delete(socket));
  };
  const server = https.createServer({
    key: privateKeyPem,
    cert: certificatePem,
    minVersion: "TLSv1.2",
    requestTimeout: 15_000,
    headersTimeout: 10_000,
    keepAliveTimeout: 1_000,
  }, (incoming, outgoing) => {
    if (!upstream || closing) {
      outgoing.writeHead(503, {
        "cache-control": "no-store",
        "content-type": "application/json; charset=utf-8",
      });
      outgoing.end(JSON.stringify({ error: "gateway_unavailable" }));
      return;
    }

    const request = http.request({
      host: upstream.host,
      port: upstream.port,
      method: incoming.method,
      path: incoming.url,
      headers: forwardedHeaders(incoming.headers),
      setHost: false,
      agent: false,
      timeout: 30_000,
    }, (response) => {
      upstreamResponses.add(response);
      response.once("close", () => upstreamResponses.delete(response));
      onResponse?.(Object.freeze({
        method: incoming.method,
        path: incoming.url,
        status: response.statusCode ?? 502,
        headers: Object.freeze({
          "cache-control": response.headers["cache-control"],
          "access-control-allow-origin": response.headers["access-control-allow-origin"],
          "access-control-allow-credentials": response.headers["access-control-allow-credentials"],
          vary: response.headers.vary,
          "set-cookie": response.headers["set-cookie"] === undefined ? undefined : true,
        }),
      }));
      outgoing.writeHead(response.statusCode ?? 502, forwardedHeaders(response.headers));
      response.pipe(outgoing);
    });
    upstreamRequests.add(request);
    request.once("socket", trackUpstreamSocket);
    request.once("close", () => upstreamRequests.delete(request));
    request.once("timeout", () => request.destroy(new Error("gateway upstream timed out")));
    request.once("error", () => {
      if (outgoing.destroyed || outgoing.writableEnded) return;
      if (outgoing.headersSent) {
        outgoing.destroy();
        return;
      }
      outgoing.writeHead(502, {
        "cache-control": "no-store",
        "content-type": "application/json; charset=utf-8",
      });
      outgoing.end(JSON.stringify({ error: "gateway_unavailable" }));
    });
    incoming.once("aborted", () => request.destroy());
    outgoing.once("close", () => {
      if (!outgoing.writableEnded) request.destroy();
    });
    incoming.pipe(request);
  });
  server.on("connection", (socket) => {
    sockets.add(socket);
    socket.once("close", () => sockets.delete(socket));
  });
  server.on("upgrade", (incoming, socket, clientHead) => {
    if (
      closing
      || !allowPwaWebSocketUpgrades
      || !isPwaWebSocketPath(incoming.url)
      || !isWebSocketUpgradeRequest(incoming)
    ) {
      socket.destroy();
      return;
    }
    if (!upstream) {
      writeUpgradeError(socket, 503, "Service Unavailable", "gateway_unavailable");
      return;
    }

    const request = http.request({
      host: upstream.host,
      port: upstream.port,
      method: incoming.method,
      path: incoming.url,
      headers: forwardedWebSocketHeaders(incoming.headers),
      setHost: false,
      agent: false,
      maxHeaderSize: 32 * 1024,
    });
    upstreamRequests.add(request);
    request.once("socket", trackUpstreamSocket);

    let settled = false;
    let handshakeTimer;
    const clearHandshakeTimer = () => {
      if (!handshakeTimer) return;
      clearTimeout(handshakeTimer);
      handshakeTimers.delete(handshakeTimer);
      handshakeTimer = undefined;
    };
    const rejectPendingHandshake = () => {
      if (settled) return;
      settled = true;
      clearHandshakeTimer();
      request.destroy();
    };
    socket.once("close", rejectPendingHandshake);
    request.once("close", () => {
      upstreamRequests.delete(request);
      clearHandshakeTimer();
    });
    request.once("upgrade", (response, upstreamSocket, upstreamHead) => {
      upstreamResponses.add(response);
      response.once("close", () => upstreamResponses.delete(response));
      trackUpstreamSocket(upstreamSocket);
      clearHandshakeTimer();
      if (settled || closing || socket.destroyed) {
        response.destroy();
        upstreamSocket.destroy();
        socket.destroy();
        return;
      }
      settled = true;
      if (!isValidUpstreamWebSocketResponse(response)) {
        upstreamSocket.destroy();
        socket.destroy();
        return;
      }

      writeUpstreamWebSocketResponse(response, socket);
      if (upstreamHead.length > 0) socket.write(upstreamHead);
      if (clientHead.length > 0) upstreamSocket.write(clientHead);
      socket.once("close", () => upstreamSocket.destroy());
      upstreamSocket.once("close", () => socket.destroy());
      socket.on("error", () => upstreamSocket.destroy());
      upstreamSocket.on("error", () => socket.destroy());
      socket.pipe(upstreamSocket);
      upstreamSocket.pipe(socket);
    });
    request.once("response", (response) => {
      clearHandshakeTimer();
      if (settled || closing || socket.destroyed) {
        response.destroy();
        return;
      }
      settled = true;
      upstreamResponses.add(response);
      response.once("close", () => upstreamResponses.delete(response));
      forwardNonUpgradeResponse(response, socket);
    });
    request.once("error", () => {
      clearHandshakeTimer();
      if (settled || closing || socket.destroyed) return;
      settled = true;
      writeUpgradeError(socket, 502, "Bad Gateway", "gateway_unavailable");
    });

    handshakeTimer = setTimeout(() => {
      if (settled || closing) return;
      settled = true;
      clearHandshakeTimer();
      writeUpgradeError(socket, 504, "Gateway Timeout", "gateway_upgrade_timeout");
      request.destroy();
    }, PWA_UPGRADE_HANDSHAKE_TIMEOUT_MS);
    handshakeTimers.add(handshakeTimer);
    request.end();
  });
  server.on("clientError", (_error, socket) => socket.destroy());

  const listening = once(server, "listening");
  server.listen(0, "127.0.0.1");
  await listening;
  const address = server.address();
  if (!address || typeof address === "string") {
    await new Promise((resolve) => server.close(resolve));
    throw new Error("TLS fixture did not bind a loopback address");
  }
  const ready = Object.freeze({
    event: "remote_client_tls_fixture_ready",
    source_commit: sourceCommit,
    source_delta_sha256: sourceDeltaDigest,
    address: Object.freeze({ host: "127.0.0.1", port: address.port }),
    gateway_origin: `https://127.0.0.1:${address.port}`,
    certificate_sha256: certificateSha256,
    certificate_spki_sha256_base64: certificateSpkiSha256Base64,
  });
  onReady?.(ready);

  return {
    ready,
    certificatePem,
    setUpstream({ host, port }) {
      if (!isLoopbackHost(host) || !Number.isInteger(port) || port < 1 || port > 65535) {
        throw new TypeError("TLS fixture upstream must be an explicit loopback host and port");
      }
      if (upstream) throw new Error("TLS fixture upstream is already set");
      if (closing) throw new Error("TLS fixture is closing");
      upstream = { host, port };
    },
    close() {
      if (closePromise) return closePromise;
      closing = true;
      for (const timer of handshakeTimers) clearTimeout(timer);
      handshakeTimers.clear();
      const serverClosed = new Promise((resolve, reject) => {
        server.close((error) => {
          if (error && error.code !== "ERR_SERVER_NOT_RUNNING") {
            reject(error);
            return;
          }
          resolve();
        });
        server.closeAllConnections();
      });
      const drainOwnedResources = async () => {
        while (true) {
          const resources = [
            ...upstreamRequests,
            ...upstreamResponses,
            ...upstreamSockets,
            ...sockets,
          ];
          if (resources.length === 0) return;
          const closed = resources.map((resource) => new Promise((resolve) => {
            resource.once("close", resolve);
          }));
          for (const resource of resources) resource.destroy();
          await Promise.all(closed);
        }
      };
      closePromise = Promise.all([serverClosed, drainOwnedResources()]).then(() => undefined);
      return closePromise;
    },
  };
}
