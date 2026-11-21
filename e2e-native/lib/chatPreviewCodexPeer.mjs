// Synthetic startup/composer fixture, never evidence of real Codex behavior.
import fs from "node:fs";
import path from "node:path";
import http from "node:http";
import { createHash } from "node:crypto";
import { createRequire } from "node:module";
import { execFileSync, spawn } from "node:child_process";
import { Duplex } from "node:stream";
import { createInterface } from "node:readline";
import { fileURLToPath } from "node:url";

export const CHAT_PREVIEW_CODEX_PEER_VERSION = 1;
const MODULE = fileURLToPath(import.meta.url);
const TRANSPORT = fileURLToPath(new URL("./chat_preview_codex_transport.py", import.meta.url));
const ENV = "WARDIAN_CHAT_PREVIEW_CODEX_PEER";
const MAX_BYTES = 1024 * 1024;
const hash = (bytes) => createHash("sha256").update(bytes).digest("hex");
const require = createRequire(import.meta.url);
const need = (ok, code) => { if (!ok) throw new Error(code); };
const inside = (root, value) => { const p = path.relative(root, value); return p !== "" && !p.startsWith(`..${path.sep}`) && p !== ".." && !path.isAbsolute(p); };

/** Check the original source through an open handle; never repair or rewrite it. */
export function verifyChatPreviewPeerSource(manifest) {
  const source = manifest.source;
  need(fs.lstatSync(source.path).isFile() && !fs.lstatSync(source.path).isSymbolicLink(), "peer_source_not_plain");
  const fd = fs.openSync(source.path, "r");
  try {
    const before = fs.fstatSync(fd, { bigint: true });
    need(String(before.dev) === source.dev && String(before.ino) === source.ino
      && Number(before.size) === source.bytes && before.size <= 64n * 1024n * 1024n, "peer_source_identity_changed");
    const bytes = fs.readFileSync(fd);
    const after = fs.fstatSync(fd, { bigint: true });
    need(before.dev === after.dev && before.ino === after.ino && before.size === after.size
      && before.mtimeNs === after.mtimeNs && hash(bytes) === source.sha256, "peer_source_bytes_changed");
    const first = JSON.parse(bytes.subarray(0, bytes.indexOf(10)).toString("utf8"));
    need(first.type === "session_meta" && first.payload?.id === manifest.nativeSessionId, "peer_source_session_changed");
    return true;
  } finally { fs.closeSync(fd); }
}

/** Stage only a PATH shim and immutable fixture manifest in an already owned home. */
export function prepareChatPreviewCodexPeer({ root, sourcePath, sessionId, nativeSessionId, pythonExecutable, transport, model = "fixture-model" }) {
  root = fs.realpathSync(root);
  const owner = JSON.parse(fs.readFileSync(path.join(root, ".native-e2e-lock", "owner.json"), "utf8"));
  need(typeof owner.runId === "string" && owner.runId.length > 0 && Number.isSafeInteger(owner.pid) && owner.pid > 0, "peer_requires_owned_home");
  const uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
  need(uuid.test(sessionId) && uuid.test(nativeSessionId), "peer_requires_native_ids");
  sourcePath = path.resolve(sourcePath);
  need(inside(path.join(root, "agents", sessionId), sourcePath), "peer_source_outside_agent");
  need(path.isAbsolute(pythonExecutable) && fs.statSync(pythonExecutable).isFile(), "peer_requires_pinned_python");
  need(["python-af-unix", "winsock-af-unix"].includes(transport), "peer_requires_explicit_transport");
  const python = JSON.parse(execFileSync(pythonExecutable, ["-I", "-u", TRANSPORT, "probe", transport],
    { encoding: "utf8", timeout: 5_000, windowsHide: true, maxBuffer: 4096 }));
  need(python.transport === transport && python.version[0] === 3 && python.version[1] >= 11, "peer_unsupported_python");
  need(typeof model === "string" && model.length > 0 && model.length <= 256, "peer_invalid_model");
  const stat = fs.statSync(sourcePath, { bigint: true });
  need(stat.isFile() && stat.size > 0n && stat.size <= 64n * 1024n * 1024n, "peer_source_bound");
  const wsEntry = require.resolve("ws");
  const manifest = { version: CHAT_PREVIEW_CODEX_PEER_VERSION, root, sessionId, nativeSessionId, model,
    peerSha256: hash(fs.readFileSync(MODULE)), nodeVersion: process.version, nodeSha256: hash(fs.readFileSync(process.execPath)),
    pythonExecutable, pythonSha256: hash(fs.readFileSync(pythonExecutable)), python, transport,
    transportPath: TRANSPORT, transportSha256: hash(fs.readFileSync(TRANSPORT)),
    wsEntry, wsSha256: hash(fs.readFileSync(wsEntry)),
    source: { path: sourcePath, bytes: Number(stat.size), dev: String(stat.dev), ino: String(stat.ino), sha256: hash(fs.readFileSync(sourcePath)) } };
  verifyChatPreviewPeerSource(manifest);
  const directory = path.join(root, "chat-preview-codex-peer-v1");
  fs.mkdirSync(directory); // An existing peer is never silently replaced.
  const entryPath = path.join(directory, "chatPreviewCodexPeer.mjs");
  const transportPath = path.join(directory, "chat_preview_codex_transport.py");
  fs.copyFileSync(MODULE, entryPath, fs.constants.COPYFILE_EXCL);
  fs.copyFileSync(TRANSPORT, transportPath, fs.constants.COPYFILE_EXCL);
  need(hash(fs.readFileSync(entryPath)) === manifest.peerSha256
    && hash(fs.readFileSync(transportPath)) === manifest.transportSha256, "peer_freeze_copy_changed");
  manifest.transportPath = transportPath;
  const manifestPath = path.join(directory, "manifest.json");
  fs.writeFileSync(manifestPath, JSON.stringify(manifest), { flag: "wx" });
  const executable = process.execPath;
  need(!/[\r\n"%]/.test(executable + entryPath), "peer_unsafe_shim_path");
  if (process.platform === "win32") {
    fs.writeFileSync(path.join(directory, "codex.cmd"), `@echo off\r\n"${executable}" "${entryPath}" %*\r\n`, { flag: "wx" });
  } else {
    const quote = (s) => `'${s.replaceAll("'", "'\\''")}'`;
    fs.writeFileSync(path.join(directory, "codex"), `#!/bin/sh\nexec ${quote(executable)} ${quote(entryPath)} "$@"\n`, { flag: "wx", mode: 0o700 });
  }
  return { manifestPath, manifest, entryPath, environment: { [ENV]: manifestPath,
    PATH: `${directory}${path.delimiter}${process.env.PATH ?? ""}`,
    ...(process.platform === "win32" ? { PATHEXT: ".CMD;.EXE;.BAT" } : {}) } };
}

/** Only launch policy is simulated. No prompt/model turn or archive API exists. */
export function createChatPreviewCodexProtocol({ nativeSessionId, codexHome, model, policy = {} }) {
  const clients = new Map();
  const loaded = new Set();
  let requests = 0;
  const selected = { model, approvalPolicy: "on-request", approvalsReviewer: "user", sandbox: { type: "workspaceWrite" }, reasoningEffort: "medium", ...policy };
  const thread = () => ({ id: nativeSessionId, model: selected.model, status: { type: "idle" }, canAcceptDirectInput: true, turns: [] });
  const response = () => ({ thread: thread(), ...selected });
  const identify = (params) => need(params?.threadId === nativeSessionId, "peer_foreign_thread");
  return {
    close(client) { clients.delete(client); loaded.delete(client); },
    dispatch(client, request) {
      need(++requests <= 1024, "peer_request_bound");
      need(request && typeof request.method === "string" && request.method.length <= 128, "peer_invalid_rpc");
      const { method, params = {} } = request;
      if (method === "initialize") {
        need(clients.size < 4 && !clients.has(client), "peer_client_bound_or_reinitialized");
        const tui = params.clientInfo?.name === "wardian-preview-fixture-tui";
        clients.set(client, { tui, initialized: false });
        return { result: { userAgent: "wardian/0.160.0 (Wardian synthetic fixture peer v1)", codexHome } };
      }
      const state = clients.get(client);
      need(state, "peer_initialize_required");
      if (method === "initialized") { state.initialized = true; return {}; }
      need(state.initialized, "peer_initialized_required");
      if (method === "config/read") return { result: { config: { model, windows: { sandbox: "unelevated" }, features: { prefer_mxc: false } }, origins: {} } };
      if (method === "model/list") return { result: { data: [{ id: model, model, isDefault: true }], nextCursor: null } };
      if (method === "thread/loaded/list") return { result: { data: loaded.size ? [nativeSessionId] : [], nextCursor: null } };
      if (method === "thread/read") { identify(params); return { result: { thread: thread() } }; }
      if (method === "thread/resume") {
        identify(params);
        need(loaded.size === 0 || loaded.has(client) || [...loaded].some((c) => clients.get(c)?.tui), "peer_other_live_loader");
        for (const key of ["model", "approvalPolicy", "approvalsReviewer", "reasoningEffort"]) {
          if (params[key] !== undefined) need(params[key] === selected[key], "peer_policy_mismatch");
        }
        if (params.sandbox !== undefined) {
          const type = { "workspace-write": "workspaceWrite", "danger-full-access": "dangerFullAccess", "read-only": "readOnly" }[params.sandbox];
          need(type === selected.sandbox.type, "peer_sandbox_mismatch");
        }
        loaded.add(client);
        return { result: response() };
      }
      if (method === "thread/unsubscribe") {
        identify(params); need(loaded.delete(client), "peer_not_subscribed");
        return { result: { status: "unsubscribed" }, notifications: loaded.size ? [] : [{ method: "thread/closed", params: { threadId: nativeSessionId } }] };
      }
      if (method === "thread/inject_items") {
        identify(params);
        need(loaded.size > 0 && Array.isArray(params.items) && params.items.length === 1
          && params.items[0]?.type === "message" && params.items[0].role === "developer", "peer_only_startup_context_supported");
        return { result: {} }; // Startup context is consumed in memory, never appended to the stopped source.
      }
      throw new Error(`peer_unsupported_method:${method}`);
    },
  };
}

function python(manifest, mode, socket) {
  const child = spawn(manifest.pythonExecutable, ["-I", "-u", manifest.transportPath, mode, socket, manifest.transport], { windowsHide: true, stdio: ["pipe", "pipe", "pipe"] });
  child.on("error", () => {});
  child.stdin.on("error", () => {});
  child.stderr.on("data", () => {}); // Drain bounded, content-free Python failures without leaking paths.
  return child;
}

function proxyStream(manifest, socket) {
  const child = python(manifest, "proxy", socket);
  const stream = Duplex.from({ readable: child.stdout, writable: child.stdin });
  stream.setTimeout = stream.setNoDelay = stream.setKeepAlive = () => stream;
  child.on("error", () => stream.destroy(new Error("peer_transport_spawn_failed")));
  child.on("exit", () => stream.destroy());
  return stream;
}

function parsePolicy(args, model) {
  const policy = { model };
  const keys = { model: "model", approval_policy: "approvalPolicy", approvals_reviewer: "approvalsReviewer", model_reasoning_effort: "reasoningEffort" };
  for (let i = 0; i < args.length; i++) {
    if (args[i] !== "-c") continue;
    const match = /^([a-z_]+)=(?:"([a-z0-9_.-]+)"|([a-z0-9_.-]+))$/i.exec(args[++i] ?? "");
    const value = match?.[2] ?? match?.[3];
    if (match && keys[match[1]]) policy[keys[match[1]]] = value;
    if (match?.[1] === "sandbox_mode") {
      const type = { "workspace-write": "workspaceWrite", "danger-full-access": "dangerFullAccess", "read-only": "readOnly" }[value];
      need(type, "peer_unsupported_sandbox"); policy.sandbox = { type };
    }
  }
  return policy;
}

// Validate every existing path component before creating control state or
// spawning a proxy: an absolute CODEX_HOME alone does not establish ownership.
function ownedCodexHome(manifest, home) {
  try {
    need(typeof home === "string" && path.isAbsolute(home) && path.isAbsolute(manifest.root), "peer_unowned_codex_home");
    const root = path.resolve(manifest.root), resolved = path.resolve(home);
    need(path.relative(root, fs.realpathSync(root)) === "" && inside(root, resolved), "peer_unowned_codex_home");
    const parts = [...path.relative(root, resolved).split(path.sep), "app-server-control"];
    let current = root;
    for (const [index, part] of parts.entries()) {
      current = path.join(current, part);
      const stat = fs.lstatSync(current, { throwIfNoEntry: false });
      if (!stat && index === parts.length - 1) break; // Only the control directory may be created later.
      need(stat?.isDirectory() && !stat.isSymbolicLink()
        && path.relative(current, fs.realpathSync(current)) === "", "peer_unowned_codex_home");
    }
    const socketPath = path.join(resolved, "app-server-control", "app-server-control.sock");
    try {
      const socket = fs.lstatSync(socketPath, { throwIfNoEntry: false });
      need(!socket?.isSymbolicLink() && !socket?.isDirectory(), "peer_unowned_codex_home");
    } catch (error) {
      // Node reports EACCES for genuine Windows AF_UNIX reparse points. Check
      // that exact tag with the pinned interpreter; never treat denial as proof.
      need(process.platform === "win32" && error.code === "EACCES", "peer_unowned_codex_home");
      execFileSync(manifest.pythonExecutable, ["-I", "-u", manifest.transportPath, "check-socket", socketPath],
        { timeout: 5_000, windowsHide: true, maxBuffer: 4096, stdio: ["ignore", "pipe", "pipe"] });
    }
    return resolved;
  } catch { throw new Error("peer_unowned_codex_home"); }
}

/** Start the real local transport; callers own the returned bounded close. */
export async function startChatPreviewCodexPeer({ manifest, codexHome, args = [] }) {
  verifyRuntime(manifest);
  verifyChatPreviewPeerSource(manifest);
  codexHome = ownedCodexHome(manifest, codexHome);
  const socket = path.join(codexHome, "app-server-control", "app-server-control.sock");
  fs.mkdirSync(path.dirname(socket), { recursive: true });
  const { WebSocketServer } = require(manifest.wsEntry);
  const protocol = createChatPreviewCodexProtocol({ nativeSessionId: manifest.nativeSessionId, codexHome, model: manifest.model, policy: parsePolicy(args, manifest.model) });
  const server = http.createServer((_req, response) => { response.writeHead(426); response.end(); });
  const webSockets = new WebSocketServer({ noServer: true, maxPayload: MAX_BYTES, perMessageDeflate: false });
  server.on("upgrade", (request, stream, head) => webSockets.handleUpgrade(request, stream, head, (ws) => {
    ws.on("error", () => ws.terminate());
    ws.on("close", () => protocol.close(ws));
    ws.on("message", (bytes, binary) => {
      try {
        need(!binary && bytes.length <= MAX_BYTES, "peer_rpc_bound");
        const request = JSON.parse(bytes.toString());
        const result = protocol.dispatch(ws, request);
        if (Object.hasOwn(request, "id")) ws.send(JSON.stringify({ id: request.id, result: result.result }));
        for (const note of result.notifications ?? []) for (const peer of webSockets.clients) peer.send(JSON.stringify(note));
      } catch (error) {
        // Fail closed; never echo a request, initial context or source identifiers.
        process.stderr.write(`${String(error.message).split(":")[0]}\n`);
        ws.close(1008, "fixture protocol rejected");
      }
    });
  }));
  const transport = python(manifest, "server", socket);
  let ready;
  let rejectReady;
  const started = new Promise((resolve, reject) => { ready = resolve; rejectReady = reject; });
  const timeout = setTimeout(() => rejectReady(new Error("peer_startup_timeout")), 5_000);
  transport.once("error", () => rejectReady(new Error("peer_transport_spawn_failed")));
  const exited = new Promise((resolve) => {
    transport.once("exit", (code) => { rejectReady(new Error("peer_transport_exited")); resolve(code); });
    transport.once("error", () => resolve(1));
  });
  const channels = new Map();
  const send = (value, done = () => {}) => {
    if (transport.stdin.destroyed || transport.stdin.writableEnded) { done(); return; }
    transport.stdin.write(`${JSON.stringify(value)}\n`, done);
  };
  const lines = createInterface({ input: transport.stdout, crlfDelay: Infinity });
  lines.on("line", (line) => {
    try {
      need(line.length <= 2 * MAX_BYTES, "peer_transport_bound");
      const message = JSON.parse(line);
      if (message.ready === true) { ready(); return; }
      if (message.open) {
        need(channels.size < 4, "peer_channel_bound");
        const stream = new Duplex({ read() {}, write(data, _encoding, done) { send({ id: message.id, data: data.toString("base64") }, done); } });
        stream.setTimeout = stream.setNoDelay = stream.setKeepAlive = () => stream;
        stream.on("error", () => {});
        stream.once("close", () => { channels.delete(message.id); send({ id: message.id, end: true }); });
        channels.set(message.id, stream); server.emit("connection", stream);
      } else if (message.end) channels.get(message.id)?.destroy();
      else channels.get(message.id)?.push(Buffer.from(message.data, "base64"));
    } catch { transport.kill(); rejectReady(new Error("peer_transport_invalid")); }
  });
  transport.once("exit", () => { for (const stream of channels.values()) stream.destroy(); server.close(); });
  let closing;
  const close = () => closing ??= (async () => {
    clearTimeout(timeout);
    for (const ws of webSockets.clients) ws.terminate();
    for (const stream of channels.values()) stream.destroy();
    transport.stdin.end();
    let forced = false;
    const kill = setTimeout(() => { forced = true; transport.kill(); }, 2_000);
    let deadline;
    try {
      const exit = await Promise.race([exited, new Promise((_, reject) => { deadline = setTimeout(() => reject(new Error("peer_cleanup_timeout")), 3500); })]);
      return { exit, forced };
    } finally { clearTimeout(kill); clearTimeout(deadline); lines.close(); server.close(); }
  })();
  try { await started; clearTimeout(timeout); }
  catch (error) { await close(); throw error; }
  return { socket, transportPid: transport.pid, close };
}

async function tui(manifest, socket) {
  const WebSocket = require(manifest.wsEntry);
  const stream = proxyStream(manifest, socket);
  const ws = new WebSocket("ws://localhost", { createConnection: () => stream, perMessageDeflate: false, maxPayload: MAX_BYTES });
  const request = (id, method, params) => ws.send(JSON.stringify({ id, method, params }));
  ws.on("error", () => { stream.destroy(); process.exitCode = 1; });
  ws.on("open", () => request(1, "initialize", { clientInfo: { name: "wardian-preview-fixture-tui" } }));
  const deadline = setTimeout(() => { stream.destroy(); process.exitCode = 1; }, 5_000);
  ws.on("message", (data) => {
    const response = JSON.parse(data.toString());
    if (response.id === 1) {
      ws.send(JSON.stringify({ method: "initialized" }));
      request(2, "thread/resume", { threadId: manifest.nativeSessionId });
    } else if (response.id === 2) {
      need(response.result?.thread?.id === manifest.nativeSessionId, "peer_tui_resume_mismatch");
      clearTimeout(deadline);
      process.stdin.resume(); // A real, quiet interactive child, not a status-file fixture.
      process.stdin.once("end", () => ws.close());
    }
  });
  ws.on("close", () => { clearTimeout(deadline); stream.destroy(); });
}

function verifyRuntime(manifest) {
  need(manifest.version === 1 && hash(fs.readFileSync(MODULE)) === manifest.peerSha256, "peer_version_or_hash_changed");
  need(process.version === manifest.nodeVersion && hash(fs.readFileSync(process.execPath)) === manifest.nodeSha256
    && hash(fs.readFileSync(manifest.pythonExecutable)) === manifest.pythonSha256
    && hash(fs.readFileSync(manifest.transportPath)) === manifest.transportSha256
    && hash(fs.readFileSync(manifest.wsEntry)) === manifest.wsSha256, "peer_runtime_changed");
}

async function main(args) {
  const manifest = JSON.parse(fs.readFileSync(process.env[ENV], "utf8"));
  verifyRuntime(manifest);
  verifyChatPreviewPeerSource(manifest);
  if (args.includes("--version")) { process.stdout.write("codex-cli 0.160.0 (Wardian synthetic fixture peer v1)\n"); return; }
  if (args[0] === "debug" && args[1] === "models") {
    process.stdout.write(`${JSON.stringify({ models: [{ id: manifest.model, display_name: "Synthetic fixture model" }] })}\n`); return;
  }
  need(process.env.WARDIAN_SESSION_ID === manifest.sessionId, "peer_wrong_agent");
  const home = ownedCodexHome(manifest, process.env.CODEX_HOME);
  const socket = path.join(home, "app-server-control", "app-server-control.sock");
  if (args[0] === "app-server" && args[1] === "proxy") {
    const supplied = args[args.indexOf("--sock") + 1];
    need(path.resolve(supplied) === path.resolve(socket), "peer_foreign_socket");
    const child = python(manifest, "proxy", socket); process.stdin.pipe(child.stdin); child.stdout.pipe(process.stdout);
    child.once("exit", (code) => { process.exitCode = code ?? 1; }); return;
  }
  if (args[0] === "app-server") {
    need(args[args.indexOf("--listen") + 1] === "unix://", "peer_only_local_transport");
    const peer = await startChatPreviewCodexPeer({ manifest, codexHome: home, args });
    process.once("SIGTERM", () => { void peer.close(); }); return;
  }
  need(args[0] === "resume" && args[1] === manifest.nativeSessionId, "peer_only_exact_resume");
  await tui(manifest, socket);
}

if (process.argv[1] && path.resolve(process.argv[1]) === MODULE) {
  main(process.argv.slice(2)).catch((error) => { process.stderr.write(`${String(error.message).split(":")[0]}\n`); process.exitCode = 1; });
}
