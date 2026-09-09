// @tier nightly — Inert child processes only; no native app or provider.
import test from "node:test";
import assert from "node:assert/strict";
import path from "node:path";
import { randomUUID } from "node:crypto";
import childProcess from "node:child_process";
import { syncBuiltinESMExports } from "node:module";
import { EventEmitter } from "node:events";
import { PassThrough } from "node:stream";
import { startStdioRpc } from "../lib/stdio-json-rpc.mjs";

test("spawn failure rejects pending and later requests and close settles without an exit event", { timeout: 5000 }, async () => {
  const missing = path.join(import.meta.dirname, `missing-rpc-${randomUUID()}`);
  const client = startStdioRpc(missing, [], { cwd: import.meta.dirname, env: process.env });
  await assert.rejects(client.request("initialize"), { code: "ENOENT" });
  await assert.rejects(client.request("after-failed-spawn"), { code: "ENOENT" });
  const result = await client.close();
  assert.deepEqual(result, { code: null, signal: null, spawn_error: "ENOENT", stderr_bytes: 0 });
  assert.deepEqual(await client.close(), result);
});

test("ordinary RPC completion and EOF shutdown retain the observed exit result", { timeout: 8000 }, async () => {
  const script = `
    const lines = require('node:readline').createInterface({ input: process.stdin });
    lines.on('line', line => {
      const request = JSON.parse(line);
      process.stdout.write(JSON.stringify({ jsonrpc: '2.0', id: request.id, result: request.params }) + '\\n');
    });
  `;
  const client = startStdioRpc(process.execPath, ["-e", script], { cwd: import.meta.dirname, env: process.env });
  try {
    assert.deepEqual(await client.request("echo", { marker: "exact" }), { marker: "exact" });
  } finally {
    assert.deepEqual(await client.close(), { code: 0, signal: null, stderr_bytes: 0 });
  }
  await assert.rejects(client.request("after-exit"), /already exited/);
});

test("close forcibly stops and observes an inert child that stays alive after EOF", { timeout: 12000 }, async () => {
  const script = `
    setInterval(() => {}, 1000);
    process.stdin.resume();
  `;
  const client = startStdioRpc(process.execPath, ["-e", script], { cwd: import.meta.dirname, env: process.env });
  const result = await client.close();
  assert.ok(result.code !== null || result.signal !== null, "Forced cleanup must observe a real exit");
  assert.equal(result.spawn_error, undefined);
  await assert.rejects(client.request("after-kill"), /already exited/);
});

test("failed termination has a bounded close and never claims the child exited", async (t) => {
  const child = new EventEmitter();
  child.pid = 123;
  child.stdin = new PassThrough();
  child.stdout = new PassThrough();
  child.stderr = new PassThrough();
  const denied = Object.assign(new Error("fixture termination denied"), { code: "EPERM" });
  child.kill = () => { child.emit("error", denied); return false; };
  const spawnMock = t.mock.method(childProcess, "spawn", () => child);
  const killMock = t.mock.method(childProcess, "execFile", (_command, _args, _options, callback) => {
    child.emit("error", denied);
    callback(denied);
  });
  syncBuiltinESMExports();
  t.mock.timers.enable({ apis: ["setTimeout"] });
  try {
    const client = startStdioRpc("fixture-only", [], { cwd: import.meta.dirname, env: {} });
    const closing = client.close();
    let settled = false;
    const rejected = assert.rejects(closing, /termination was not confirmed/).then(() => { settled = true; });
    t.mock.timers.tick(3000);
    await new Promise(setImmediate);
    assert.equal(settled, false, "A kill error on an existing PID is not observed exit");
    t.mock.timers.tick(3000);
    await rejected;
    await assert.rejects(client.request("after-termination-error"), { code: "EPERM" });
  } finally {
    t.mock.timers.reset();
    spawnMock.mock.restore();
    killMock.mock.restore();
    syncBuiltinESMExports();
    for (const stream of [child.stdin, child.stdout, child.stderr]) stream.destroy();
  }
});
