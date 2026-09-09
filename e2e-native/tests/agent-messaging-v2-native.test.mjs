// @tier nightly — Paired MCP clients exercise the native store without provider turns.
import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { startStdioRpc } from "../lib/stdio-json-rpc.mjs";
import { createNativeHarness, ensureNativeAppBuilt, prepareIsolatedHome, startNativeSession, waitForAppShell, invokeTauri } from "../lib/harness.mjs";

const hash = (bytes) => createHash("sha256").update(bytes).digest("hex");
const TOOLS = ["followup_task", "interrupt_agent", "list_agents", "receive_messages", "reply", "send_message"];

function receipt(result) {
  const item = result?.content?.find((entry) => entry.type === "text");
  assert.equal(typeof item?.text, "string", "Expected a structured MCP receipt");
  const value = JSON.parse(item.text);
  assert.notEqual(result.isError, true, JSON.stringify(value));
  return value;
}

async function connect(cli, home, cwd, agentId) {
  const env = { ...process.env, WARDIAN_HOME: home, WARDIAN_SESSION_ID: agentId };
  const client = startStdioRpc(cli, ["mcp", "serve"], { cwd, env });
  await client.request("initialize", { protocolVersion: "2025-11-25", capabilities: {}, clientInfo: { name: "wardian-v2-native", version: "1" } });
  client.notify("notifications/initialized");
  const inventory = await client.request("tools/list");
  assert.deepEqual(inventory.tools.map((tool) => tool.name).sort(), TOOLS);
  return {
    close: () => client.close(),
    raw: (name, args) => client.request("tools/call", { name, arguments: args }, 70_000),
    async call(name, args = {}) { return receipt(await this.raw(name, args)); },
  };
}

function watch(cli, home, cwd, target) {
  const env = { ...process.env, WARDIAN_HOME: home };
  delete env.WARDIAN_SESSION_ID;
  return JSON.parse(execFileSync(cli, ["agent", "watch", target, "--include", "events,delivery", "--tail", "0", "--timeout", "5s"],
    { cwd, env, encoding: "utf8", timeout: 10_000, windowsHide: true }));
}

function nativeBinding(cli, home, cwd, target) {
  const env = { ...process.env, WARDIAN_HOME: home };
  delete env.WARDIAN_SESSION_ID;
  return JSON.parse(execFileSync(cli, ["delivery", "capabilities", target],
    { cwd, env, encoding: "utf8", timeout: 10_000, windowsHide: true })).binding ?? null;
}

test("v2 information has consistent sender and receiver semantics without starting a turn", { timeout: 180_000 }, async (t) => {
  const harness = await createNativeHarness();
  harness.watchMode = false;
  if (process.env.WARDIAN_NATIVE_SKIP_BUILD !== "1") await ensureNativeAppBuilt(harness);
  harness.isolatedHome = path.join(harness.repoRoot, ".tmp", "e2e-native", "messaging-v2", `${Date.now()}-${process.pid}`);
  prepareIsolatedHome(harness);
  const report = { schema: 2, issue: 1218, status: "running", started_at: new Date().toISOString(),
    artifact_sha256: { app: hash(await fs.readFile(harness.appPath)), test: hash(await fs.readFile(import.meta.filename)) }, cases: {} };
  const reportPath = path.join(harness.isolatedHome, "paired-messaging-report.json");
  const save = () => fs.writeFile(reportPath, `${JSON.stringify(report, null, 2)}\n`);
  let session;
  const clients = [];
  const previousEnv = new Map();
  try {
    await save();
    const profile = path.join(harness.isolatedHome, "fixture-profile");
    const codexHome = path.join(profile, ".codex");
    await fs.mkdir(codexHome, { recursive: true });
    const overrides = { HOME: profile, USERPROFILE: profile, CODEX_HOME: codexHome };
    for (const key of Object.keys(process.env)) {
      if (/^(OPENAI_|WARDIAN_SESSION_ID$|WARDIAN_MEMORY_CAPABILITY$)/u.test(key)) overrides[key] = undefined;
    }
    for (const [key, value] of Object.entries(overrides)) {
      previousEnv.set(key, process.env[key]);
      if (value === undefined) delete process.env[key]; else process.env[key] = value;
    }
    session = await startNativeSession(harness);
    await waitForAppShell(session.driver, 30_000);
    const installedCli = path.join(harness.isolatedHome, "bin", process.platform === "win32" ? "wardian-cli.exe" : "wardian-cli");
    const cli = process.env.WARDIAN_E2E_MESSAGING_CLI ?? installedCli;
    assert.ok(path.isAbsolute(cli));
    report.artifact_sha256.cli = hash(await fs.readFile(cli));
    assert.equal(hash(await fs.readFile(installedCli)), report.artifact_sha256.cli,
      "The tested MCP CLI must match the application's installed CLI");
    const agents = [];
    for (const name of ["Message-Sender", "Message-Receiver"]) {
      const workspace = path.join(harness.isolatedHome, "workspaces", name);
      await fs.mkdir(workspace, { recursive: true });
      agents.push(await invokeTauri(session.driver, "spawn_agent", { req: {
        sessionName: name, agentClass: "TestClass", folder: workspace, isOff: true,
        resumeSession: null, configOverride: { provider: "codex", model: "gpt-5.4-mini", conversation_logging: "enabled" },
      } }));
    }
    const [sender, receiver] = agents;
    const senderClient = await connect(cli, harness.isolatedHome, harness.repoRoot, sender.session_id);
    clients.push(senderClient);
    const receiverClient = await connect(cli, harness.isolatedHome, harness.repoRoot, receiver.session_id);
    clients.push(receiverClient);
    const before = watch(cli, harness.isolatedHome, harness.repoRoot, receiver.session_id);
    assert.equal(before.agent.status.toLowerCase(), "off");
    assert.equal(nativeBinding(cli, harness.isolatedHome, harness.repoRoot, receiver.session_id), null);
    const initialConfig = (await invokeTauri(session.driver, "list_agents")).find((agent) => agent.session_id === receiver.session_id);
    assert.ok(initialConfig);
    const initialProviderSession = initialConfig.resume_session ?? null;
    const initial = await receiverClient.call("receive_messages", { timeout_ms: 0 });
    assert.deepEqual(initial.messages, []);
    const literal = "Information only. Do not start work.\ncafé ✓; `x`; $(literal).\n";
    const sent = await senderClient.call("send_message", { target: receiver.session_id, message: literal });
    assert.equal(typeof sent.interaction_id, "string");
    const page = await receiverClient.call("receive_messages", { cursor: initial.next_cursor, timeout_ms: 0, limit: 1 });
    assert.equal(page.messages.length, 1);
    assert.equal(page.messages[0].interaction_id, sent.interaction_id);
    assert.equal(page.messages[0].kind, "message");
    assert.equal(page.messages[0].sender, sender.session_id);
    assert.equal(page.messages[0].message, literal);
    const replay = await receiverClient.call("receive_messages", { cursor: initial.next_cursor, timeout_ms: 0, limit: 1 });
    assert.deepEqual(replay.messages, page.messages, "Cursor replay must preserve identities and literal content");
    const foreign = await senderClient.raw("receive_messages", { cursor: page.next_cursor, timeout_ms: 0 });
    assert.equal(foreign.isError, true, "Another agent's cursor must fail closed");
    const acknowledged = await receiverClient.call("receive_messages", { ack_cursor: page.ack_cursor, timeout_ms: 0 });
    assert.deepEqual(acknowledged.messages, []);
    const lateBody = "Later information remains available after a bounded empty wait.";
    const empty = await receiverClient.call("receive_messages", { timeout_ms: 100 });
    assert.equal(empty.timed_out, true);
    const lateSent = await senderClient.call("send_message", { target: receiver.session_id, message: lateBody });
    const late = await receiverClient.call("receive_messages", { cursor: empty.next_cursor, timeout_ms: 0 });
    assert.equal(late.messages[0].interaction_id, lateSent.interaction_id);
    assert.equal(late.messages[0].message, lateBody);
    const after = watch(cli, harness.isolatedHome, harness.repoRoot, receiver.session_id);
    assert.equal(after.agent.status.toLowerCase(), "off");
    assert.equal(nativeBinding(cli, harness.isolatedHome, harness.repoRoot, receiver.session_id), null,
      "Information must not create a native provider owner");
    const finalConfig = (await invokeTauri(session.driver, "list_agents")).find((agent) => agent.session_id === receiver.session_id);
    assert.ok(finalConfig);
    assert.equal(finalConfig.resume_session ?? null, initialProviderSession,
      "Information must not replace or create provider identity after off-agent preparation");
    report.cases.paired_information = { status: "pass", sent, page, replay, acknowledged, empty_wait: empty, late, before, after };
    report.status = "pass";
  } catch (error) {
    report.status = "fail";
    report.error = error.message;
    throw error;
  } finally {
    for (const client of clients.reverse()) await client.close();
    if (session) await session.close();
    for (const [key, value] of previousEnv) {
      if (value === undefined) delete process.env[key]; else process.env[key] = value;
    }
    report.finished_at = new Date().toISOString();
    await save();
    t.diagnostic(`Paired native messaging ${report.status}; evidence retained in isolated test home`);
  }
});
