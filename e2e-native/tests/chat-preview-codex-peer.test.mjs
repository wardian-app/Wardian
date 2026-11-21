// @tier ci — App-free: real local fixture sockets/processes, never Wardian or Codex.
import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { randomUUID, createHash } from "node:crypto";
import { execFileSync, spawn } from "node:child_process";
import { Duplex } from "node:stream";
import { once } from "node:events";
import WebSocket from "ws";
import { createChatPreviewCodexProtocol, prepareChatPreviewCodexPeer,
  startChatPreviewCodexPeer, verifyChatPreviewPeerSource } from "../lib/chatPreviewCodexPeer.mjs";

const pythonExecutable = process.env.WARDIAN_CHAT_PREVIEW_PYTHON ?? execFileSync(process.platform === "win32" ? "python" : "python3",
  ["-c", "import sys,ctypes; b=ctypes.create_unicode_buffer(32768); ctypes.windll.kernel32.GetModuleFileNameW(None,b,len(b)) if sys.platform=='win32' else None; print(b.value if sys.platform=='win32' else sys.executable)"],
  { encoding: "utf8", timeout: 5000, windowsHide: true }).trim();
const transport = process.platform === "win32" ? "winsock-af-unix" : "python-af-unix";
const digest = (bytes) => createHash("sha256").update(bytes).digest("hex");
const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const bounded = (promise, ms = 5000) => {
  let timer;
  return Promise.race([promise, new Promise((_, reject) => { timer = setTimeout(() => reject(new Error("fixture_test_timeout")), ms); })])
    .finally(() => clearTimeout(timer));
};

function fixture(t) {
  // The short leaf keeps the actual AF_UNIX pathname inside the 108-byte bound.
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "wdcp-"));
  const cleanup = [];
  t.after(async () => {
    const failures = [];
    try { for (const action of cleanup.reverse()) { try { await action(); } catch (error) { failures.push(error); } } }
    finally {
      const relative = path.relative(os.tmpdir(), root);
      assert.ok(relative.startsWith("wdcp-") && !relative.includes(path.sep));
      fs.rmSync(root, { recursive: true, force: true });
    }
    if (failures.length) throw new AggregateError(failures, "fixture cleanup failed");
  });
  const sessionId = randomUUID(), nativeSessionId = randomUUID();
  fs.mkdirSync(path.join(root, ".native-e2e-lock"));
  fs.writeFileSync(path.join(root, ".native-e2e-lock", "owner.json"), JSON.stringify({ runId: "peer-contract", pid: process.pid }));
  const agent = path.join(root, "agents", sessionId);
  fs.mkdirSync(agent, { recursive: true });
  const sourcePath = path.join(agent, "source.jsonl");
  fs.writeFileSync(sourcePath, `${JSON.stringify({ type: "session_meta", payload: { id: nativeSessionId } })}\n`
    + `${JSON.stringify({ type: "event_msg", payload: { type: "task_complete", last_agent_message: "Synthetic source remains stopped." } })}\n`);
  const codexHome = path.join(root, "c"); fs.mkdirSync(codexHome);
  const inputs = { root, sourcePath, sessionId, nativeSessionId, pythonExecutable, transport };
  return { ...inputs, inputs, codexHome, cleanup };
}

function initialize(protocol, client, tui = false) {
  const result = protocol.dispatch(client, { method: "initialize", params: { clientInfo: { name: tui ? "wardian-preview-fixture-tui" : "wardian" } } });
  protocol.dispatch(client, { method: "initialized" });
  return result.result;
}

test("protocol requires initialization and rejects unknown or model-turn methods", () => {
  const protocol = createChatPreviewCodexProtocol({ nativeSessionId: "native", codexHome: "home", model: "fixture-model" });
  assert.throws(() => protocol.dispatch("owner", { method: "thread/loaded/list" }), /initialize_required/);
  initialize(protocol, "owner");
  assert.throws(() => protocol.dispatch("owner", { method: "turn/start", params: { threadId: "native" } }), /unsupported_method/);
  assert.throws(() => protocol.dispatch("owner", { method: "thread/start" }), /unsupported_method/);
});

test("preloader eviction and live TUI membership independently precede the exact metadata rejoin", () => {
  const protocol = createChatPreviewCodexProtocol({ nativeSessionId: "native", codexHome: "home", model: "fixture-model" });
  assert.equal(initialize(protocol, "owner").codexHome, "home");
  const call = (method, params) => protocol.dispatch("owner", { method, params });
  assert.deepEqual(call("thread/loaded/list").result.data, []);
  assert.throws(() => call("thread/resume", { threadId: "foreign" }), /foreign_thread/);
  assert.equal(call("thread/resume", { threadId: "native" }).result.approvalsReviewer, "user");
  const eviction = call("thread/unsubscribe", { threadId: "native" });
  assert.deepEqual(eviction.result, { status: "unsubscribed" });
  assert.deepEqual(eviction.notifications, [{ method: "thread/closed", params: { threadId: "native" } }]);
  assert.deepEqual(call("thread/loaded/list").result.data, []);
  initialize(protocol, "tui", true);
  protocol.dispatch("tui", { method: "thread/resume", params: { threadId: "native" } });
  assert.deepEqual(call("thread/loaded/list").result.data, ["native"]);
  assert.deepEqual(call("thread/inject_items", { threadId: "native", items: [{ type: "message", role: "developer" }] }).result, {});
  const rejoin = call("thread/resume", { threadId: "native", excludeTurns: true }).result;
  assert.equal(rejoin.thread.canAcceptDirectInput, true);
  assert.equal(rejoin.thread.status.type, "idle");
  assert.deepEqual(rejoin.thread.turns, []);
  protocol.close("tui"); protocol.close("owner");
  initialize(protocol, "new-owner");
  assert.deepEqual(protocol.dispatch("new-owner", { method: "thread/loaded/list" }).result.data, []);
});

test("policy mismatches, user injections, and extra clients fail closed", () => {
  const protocol = createChatPreviewCodexProtocol({ nativeSessionId: "native", codexHome: "home", model: "fixture-model" });
  initialize(protocol, 1);
  assert.throws(() => protocol.dispatch(1, { method: "thread/resume", params: { threadId: "native", approvalPolicy: "never" } }), /policy_mismatch/);
  assert.throws(() => protocol.dispatch(1, { method: "thread/resume", params: { threadId: "native", sandbox: "danger-full-access" } }), /sandbox_mismatch/);
  assert.throws(() => protocol.dispatch(1, { method: "thread/inject_items", params: { threadId: "native", items: [{ type: "message", role: "user" }] } }), /only_startup/);
  initialize(protocol, 2); initialize(protocol, 3); initialize(protocol, 4);
  assert.throws(() => initialize(protocol, 5), /client_bound/);
});

test("request accounting is bounded rather than silently resetting a peer", () => {
  const protocol = createChatPreviewCodexProtocol({ nativeSessionId: "native", codexHome: "home", model: "fixture-model" });
  initialize(protocol, "owner");
  for (let i = 0; i < 1022; i++) protocol.dispatch("owner", { method: "thread/loaded/list" });
  assert.throws(() => protocol.dispatch("owner", { method: "thread/loaded/list" }), /request_bound/);
});

test("staging pins interpreter, transport and code without touching native source identity or bytes", (t) => {
  const f = fixture(t), before = fs.statSync(f.sourcePath, { bigint: true });
  const sourceHash = digest(fs.readFileSync(f.sourcePath));
  const staged = prepareChatPreviewCodexPeer(f.inputs);
  assert.equal(staged.manifest.python.transport, transport);
  assert.ok(staged.manifest.python.version[1] >= 11);
  assert.equal(staged.manifest.transportSha256.length, 64);
  assert.equal(staged.manifest.pythonSha256.length, 64);
  assert.equal(verifyChatPreviewPeerSource(staged.manifest), true);
  const after = fs.statSync(f.sourcePath, { bigint: true });
  assert.deepEqual([after.dev, after.ino, after.size], [before.dev, before.ino, before.size]);
  assert.equal(digest(fs.readFileSync(f.sourcePath)), sourceHash);
  assert.deepEqual(fs.readdirSync(path.dirname(f.sourcePath)), ["source.jsonl"]);
  assert.throws(() => prepareChatPreviewCodexPeer(f.inputs), /EEXIST/);
});

test("source mutation, same-content replacement and foreign native identity are refused", (t) => {
  const f = fixture(t), staged = prepareChatPreviewCodexPeer(f.inputs);
  assert.throws(() => verifyChatPreviewPeerSource({ ...staged.manifest, nativeSessionId: randomUUID() }), /session_changed/);
  const bytes = fs.readFileSync(f.sourcePath);
  fs.renameSync(f.sourcePath, `${f.sourcePath}.prior`);
  fs.writeFileSync(f.sourcePath, bytes);
  assert.throws(() => verifyChatPreviewPeerSource(staged.manifest), /identity_changed/);
});

test("external Codex homes are rejected before shared startup or any CLI transport role creates control state", async (t) => {
  const f = fixture(t), external = fixture(t), staged = prepareChatPreviewCodexPeer(f.inputs);
  const control = path.join(external.codexHome, "app-server-control");
  let daemon;
  try {
    await assert.rejects(async () => { daemon = await startChatPreviewCodexPeer({ manifest: staged.manifest, codexHome: external.codexHome }); }, /peer_unowned_codex_home/);
  } finally { await daemon?.close(); }
  assert.equal(fs.existsSync(control), false);
  const traversed = `${f.root}${path.sep}..${path.sep}${path.basename(external.root)}${path.sep}c`;
  await assert.rejects(startChatPreviewCodexPeer({ manifest: staged.manifest, codexHome: traversed }), /peer_unowned_codex_home/);
  for (const args of [["app-server", "--listen", "unix://"], ["app-server", "proxy", "--sock", path.join(control, "app-server-control.sock")], ["resume", f.nativeSessionId]]) {
    const result = await new Promise((resolve, reject) => {
      const child = spawn(process.execPath, [staged.entryPath, ...args], { env: { ...process.env, ...staged.environment,
        WARDIAN_SESSION_ID: f.sessionId, CODEX_HOME: external.codexHome }, windowsHide: true, stdio: ["ignore", "pipe", "pipe"] });
      let error = ""; child.stderr.on("data", (bytes) => { error += bytes.toString(); }); child.stdout.resume();
      const timer = setTimeout(() => { child.kill(); reject(new Error("external_home_cli_did_not_reject")); }, 5000);
      child.once("error", (failure) => { clearTimeout(timer); reject(failure); });
      child.once("exit", (code) => { clearTimeout(timer); resolve({ code, error }); });
    });
    assert.equal(result.code, 1); assert.match(result.error, /peer_unowned_codex_home/);
    assert.equal(fs.existsSync(control), false);
  }
  assert.deepEqual(fs.readdirSync(external.codexHome), []);
  assert.equal(verifyChatPreviewPeerSource(staged.manifest), true);
});

test("owned-looking home, control and socket links cannot escape into an external home", async (t) => {
  const f = fixture(t), external = fixture(t), staged = prepareChatPreviewCodexPeer(f.inputs);
  const linkedHome = path.join(f.root, "linked-home");
  fs.symlinkSync(external.codexHome, linkedHome, process.platform === "win32" ? "junction" : "dir");
  await assert.rejects(startChatPreviewCodexPeer({ manifest: staged.manifest, codexHome: linkedHome }), /peer_unowned_codex_home/);
  const control = path.join(f.codexHome, "app-server-control");
  fs.symlinkSync(external.codexHome, control, process.platform === "win32" ? "junction" : "dir");
  await assert.rejects(startChatPreviewCodexPeer({ manifest: staged.manifest, codexHome: f.codexHome }), /peer_unowned_codex_home/);
  fs.unlinkSync(control); fs.mkdirSync(control);
  const outsideSocket = path.join(external.codexHome, "socket-sentinel");
  fs.writeFileSync(outsideSocket, "external sentinel");
  try { fs.symlinkSync(outsideSocket, path.join(control, "app-server-control.sock"), "file"); }
  catch (error) {
    if (process.platform !== "win32" || !["EPERM", "EACCES"].includes(error.code)) throw error;
    t.diagnostic("Windows file-symlink privilege unavailable; directory junction escapes were exercised");
    assert.deepEqual(fs.readdirSync(external.codexHome), ["socket-sentinel"]); return;
  }
  await assert.rejects(startChatPreviewCodexPeer({ manifest: staged.manifest, codexHome: f.codexHome }), /peer_unowned_codex_home/);
  assert.equal(fs.readFileSync(outsideSocket, "utf8"), "external sentinel");
  assert.deepEqual(fs.readdirSync(external.codexHome), ["socket-sentinel"]);
});

test("same-file byte mutation is rejected and a stale socket is never replaced", async (t) => {
  const f = fixture(t), staged = prepareChatPreviewCodexPeer(f.inputs);
  const original = fs.readFileSync(f.sourcePath);
  const changed = Buffer.from(original); changed[changed.length - 4] ^= 1;
  fs.writeFileSync(f.sourcePath, changed);
  assert.throws(() => verifyChatPreviewPeerSource(staged.manifest), /bytes_changed/);
  const other = fixture(t), current = prepareChatPreviewCodexPeer(other.inputs);
  const socket = path.join(other.codexHome, "app-server-control", "app-server-control.sock");
  fs.mkdirSync(path.dirname(socket)); fs.writeFileSync(socket, "owned sentinel");
  await assert.rejects(startChatPreviewCodexPeer({ manifest: { ...current.manifest, nodeVersion: "wrong" }, codexHome: other.codexHome }), /runtime_changed/);
  await assert.rejects(startChatPreviewCodexPeer({ manifest: current.manifest, codexHome: other.codexHome }), /transport_exited/);
  assert.equal(fs.readFileSync(socket, "utf8"), "owned sentinel");
});

test("transport selection is explicit and wrong-agent source cannot be staged", (t) => {
  const f = fixture(t);
  assert.throws(() => prepareChatPreviewCodexPeer({ ...f.inputs, transport: undefined }), /explicit_transport/);
  assert.throws(() => prepareChatPreviewCodexPeer({ ...f.inputs, sessionId: randomUUID() }), /outside_agent/);
});

test("CLI entry supplies the actual catalogue schema and rejects a foreign runtime without provider fallback", (t) => {
  const f = fixture(t), staged = prepareChatPreviewCodexPeer(f.inputs);
  const env = { ...process.env, ...staged.environment, WARDIAN_SESSION_ID: f.sessionId, CODEX_HOME: f.codexHome };
  const run = (args, overrides = {}) => execFileSync(process.execPath, [staged.entryPath, ...args],
    { env: { ...env, ...overrides }, encoding: "utf8", windowsHide: true, timeout: 5000, maxBuffer: 8192 });
  assert.match(run(["--version"]), /synthetic fixture peer v1/);
  assert.deepEqual(JSON.parse(run(["debug", "models"])).models.map((model) => model.id), ["fixture-model"]);
  assert.throws(() => run(["resume", f.nativeSessionId], { WARDIAN_SESSION_ID: randomUUID() }), /Command failed/);
  assert.throws(() => run(["exec", "do not run"]), /Command failed/);
});

test("real local AF_UNIX and raw WebSocket proxy perform the startup subset and leave the source unchanged", { timeout: 20_000 }, async (t) => {
  const f = fixture(t), staged = prepareChatPreviewCodexPeer(f.inputs);
  const sourceBefore = { hash: digest(fs.readFileSync(f.sourcePath)), stat: fs.statSync(f.sourcePath, { bigint: true }) };
  const daemon = await startChatPreviewCodexPeer({ manifest: staged.manifest, codexHome: f.codexHome,
    args: ["-c", 'model="fixture-model"', "-c", 'approval_policy="on-request"', "-c", 'sandbox_mode="workspace-write"'] });
  const socketListed = () => fs.readdirSync(path.dirname(daemon.socket)).includes(path.basename(daemon.socket));
  f.cleanup.push(async () => { const result = await daemon.close(); assert.deepEqual(result, { exit: 0, forced: false }); assert.equal(socketListed(), false); });
  // Node stat does not recognize Windows AF_UNIX reparse points. Directory
  // presence plus the real connection below do not substitute a marker file.
  assert.ok(socketListed());
  const env = { ...process.env, ...staged.environment, WARDIAN_SESSION_ID: f.sessionId, CODEX_HOME: f.codexHome };
  const proxy = spawn(process.execPath, [staged.entryPath, "app-server", "proxy", "--sock", daemon.socket], { env, windowsHide: true, stdio: ["pipe", "pipe", "pipe"] });
  let proxyError = ""; proxy.stderr.on("data", (b) => { proxyError += b.toString(); });
  const proxyExit = once(proxy, "exit");
  const stream = Duplex.from({ readable: proxy.stdout, writable: proxy.stdin });
  stream.setTimeout = stream.setNoDelay = stream.setKeepAlive = () => stream;
  const ws = new WebSocket("ws://localhost", { createConnection: () => stream, perMessageDeflate: false });
  ws.on("error", () => {});
  f.cleanup.push(async () => { ws.terminate(); proxy.stdin.end(); await bounded(proxyExit); assert.equal(proxyError, ""); });
  await bounded(once(ws, "open"));
  let sequence = 0; const pending = new Map(), notifications = [];
  ws.on("message", (data) => {
    const value = JSON.parse(data.toString());
    if (Object.hasOwn(value, "id")) { pending.get(value.id)?.(value.result); pending.delete(value.id); }
    else notifications.push(value);
  });
  const rpc = (method, params) => bounded(new Promise((resolve) => {
    const id = ++sequence; pending.set(id, resolve); ws.send(JSON.stringify({ id, method, params }));
  }));
  const initialized = await rpc("initialize", { clientInfo: { name: "wardian" } });
  assert.equal(initialized.userAgent, "wardian/0.160.0 (Wardian synthetic fixture peer v1)");
  assert.equal(initialized.codexHome, f.codexHome);
  ws.send(JSON.stringify({ method: "initialized" }));
  assert.deepEqual((await rpc("thread/loaded/list", { limit: 2 })).data, []);
  assert.equal((await rpc("config/read", { cwd: f.root })).config.windows.sandbox, "unelevated");
  assert.equal((await rpc("thread/resume", { threadId: f.nativeSessionId, excludeTurns: true })).thread.id, f.nativeSessionId);
  assert.equal((await rpc("thread/unsubscribe", { threadId: f.nativeSessionId })).status, "unsubscribed");
  assert.deepEqual((await rpc("thread/loaded/list", { limit: 2 })).data, []);
  assert.ok(notifications.some((note) => note.method === "thread/closed"));
  const tui = spawn(process.execPath, [staged.entryPath, "resume", f.nativeSessionId], { env, windowsHide: true, stdio: ["pipe", "pipe", "pipe"] });
  let tuiOutput = "", tuiError = "";
  tui.stdout.on("data", (b) => { tuiOutput += b.toString(); }); tui.stderr.on("data", (b) => { tuiError += b.toString(); });
  const tuiExit = once(tui, "exit");
  f.cleanup.push(async () => { tui.stdin.end(); await bounded(tuiExit); assert.equal(tuiOutput, ""); assert.equal(tuiError, ""); });
  let loaded;
  const deadline = Date.now() + 5000;
  do { loaded = await rpc("thread/loaded/list", { limit: 2 }); if (!loaded.data.length) await delay(20); } while (!loaded.data.length && Date.now() < deadline);
  assert.deepEqual(loaded.data, [f.nativeSessionId]);
  assert.deepEqual(await rpc("thread/inject_items", { threadId: f.nativeSessionId,
    items: [{ type: "message", role: "developer", content: [{ type: "input_text", text: "Synthetic startup context" }] }] }), {});
  const attached = await rpc("thread/resume", { threadId: f.nativeSessionId, excludeTurns: true });
  assert.equal(attached.thread.canAcceptDirectInput, true);
  assert.equal(attached.thread.status.type, "idle");
  assert.equal(attached.approvalsReviewer, "user");
  assert.equal(verifyChatPreviewPeerSource(staged.manifest), true);
  const after = fs.statSync(f.sourcePath, { bigint: true });
  assert.deepEqual([after.dev, after.ino, after.size], [sourceBefore.stat.dev, sourceBefore.stat.ino, sourceBefore.stat.size]);
  assert.equal(digest(fs.readFileSync(f.sourcePath)), sourceBefore.hash);
});
