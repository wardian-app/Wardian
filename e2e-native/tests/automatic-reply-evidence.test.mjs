// @tier nightly — Read-only observer, owned gate, and deadline checks; no app or providers.
import test from "node:test";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { DatabaseSync } from "node:sqlite";
import {
  activeConversationLeaseConflicts, appendOwnedReplyToolApproval, assertPromptDeliveryAccepted,
  assertAutomaticReplyGateInvocation, assertRequesterOriginMatchesProviderInput,
  automaticReplyDeadlines, automaticReplyExecPowerShell, automaticReplyExecScript,
  automaticReplyExecShell,
  automaticReplyGateCommand, automaticReplyGateToken, automaticReplyTaskInstructions,
  collectAutomaticReplyExecAttempts,
  completedAssistantRows, qualifyAutomaticReplyExecEnvironment,
  exactCurrentCascade, persistAutomaticReplyFailure, providerInputIsReady,
  readCanonicalTaskReply, readConversationLeaseSnapshot, readProviderInputState,
  readAutomaticReplyProviderLogEvidence, recordConversationLeaseObservation, writeOwnedReplyReport,
} from "../lib/automatic-reply-evidence.mjs";
import { acquireHomeLock, readHomeLock, releaseHomeLock } from "../lib/sessionHome.mjs";
import { waitForReplyRelease } from "./fixtures/automatic-reply-gate.mjs";

async function fixture(t) {
  const parent = path.resolve(os.tmpdir());
  const home = await fs.mkdtemp(path.join(parent, "wardian-automatic-reply-"));
  t.after(async () => {
    const relative = path.relative(parent, path.resolve(home));
    assert.ok(relative && !relative.startsWith("..") && !path.isAbsolute(relative));
    await fs.rm(home, { recursive: true, force: true });
  });
  return home;
}

async function ownedCodexFixture(t, configSuffix = "") {
  const wardianHome = await fixture(t);
  const agentId = "123e4567-e89b-42d3-a456-426614174000";
  const codexHome = path.join(wardianHome, "agents", agentId, "habitat", ".codex");
  const bin = path.join(wardianHome, "bin");
  await fs.mkdir(codexHome, { recursive: true });
  await fs.mkdir(bin, { recursive: true });
  const commandPath = path.join(bin, process.platform === "win32" ? "wardian-mcp.exe" : "wardian-cli");
  await fs.writeFile(commandPath, "fixture executable\n");
  const command = await fs.realpath(commandPath);
  const owner = { version: 1, agent_id: agentId, command, wardian_home: wardianHome };
  const config = `model = "gpt-6-luna"\napproval_policy = "never"\nsandbox_mode = "workspace-write"\n` +
    `[mcp_servers.other]\nlabel = "preserved"\n` +
    `[mcp_servers.wardian]\ncommand = ${JSON.stringify(command)}\nargs = ["mcp", "serve"]\n` +
    `tool_timeout_sec = 70\n` +
    `[mcp_servers.wardian.env]\nWARDIAN_HOME = ${JSON.stringify(wardianHome)}\n` +
    `WARDIAN_SESSION_ID = "${agentId}"\n` + configSuffix;
  await fs.writeFile(path.join(codexHome, ".wardian-messaging.json"), JSON.stringify(owner));
  await fs.writeFile(path.join(codexHome, "config.toml"), config);
  return { wardianHome, agentId, codexHome, commandPath, command, owner, config };
}

test("fixture reply-tool grant is scoped to the exact managed registration and preserves config", async (t) => {
  const owned = await ownedCodexFixture(t,
    "\n[mcp_servers.wardian.tools.review]\napproval_mode = \"prompt\"\n");
  const result = await appendOwnedReplyToolApproval({
    wardianHome: owned.wardianHome,
    agentId: owned.agentId,
    expectedCommand: owned.commandPath,
  });
  const after = await fs.readFile(path.join(owned.codexHome, "config.toml"), "utf8");
  assert.equal(after.slice(0, owned.config.length), owned.config,
    "The fixture must preserve every existing managed and unrelated config key");
  assert.equal(after.slice(owned.config.length),
    "\n[mcp_servers.wardian.tools.reply]\napproval_mode = \"approve\"\n");
  assert.equal(result.agent_id, owned.agentId);
  assert.equal(result.fixture_permission_delta,
    "[mcp_servers.wardian.tools.reply] approval_mode=approve");
  assert.equal(result.command_file, path.basename(owned.commandPath));
});

test("fixture reply-tool grant rejects conflicting or mismatched ownership before writing", async (t) => {
  for (const policy of [
    "\n[mcp_servers.wardian.tools.reply]\napproval_mode = \"deny\"\n",
    "\n[mcp_servers.wardian.tools]\nreply = { approval_mode = \"prompt\" }\n",
    "\n[mcp_servers.wardian.tools] # policy\nreply = { approval_mode = \"prompt\" }\n",
    "\n[\"mcp_servers\".\"wardian\".\"tools\"] # policy\nreply = { approval_mode = \"prompt\" }\n",
    "\n[mcp_servers.wardian.tools.\"reply\"]\napproval_mode = \"prompt\"\n",
  ]) {
    const conflict = await ownedCodexFixture(t, policy);
    const conflictPath = path.join(conflict.codexHome, "config.toml");
    const conflictBefore = await fs.readFile(conflictPath, "utf8");
    await assert.rejects(appendOwnedReplyToolApproval({
      wardianHome: conflict.wardianHome,
      agentId: conflict.agentId,
      expectedCommand: conflict.commandPath,
    }), /reply-tool policy must be reviewed/u);
    assert.equal(await fs.readFile(conflictPath, "utf8"), conflictBefore);
  }

  const invalidIds = await ownedCodexFixture(t);
  for (const agentId of [".", "..", "../other", "fixture-worker-1"]) {
    await assert.rejects(appendOwnedReplyToolApproval({
      wardianHome: invalidIds.wardianHome,
      agentId,
      expectedCommand: invalidIds.commandPath,
    }), /canonical UUID/u);
  }
  assert.equal(await fs.readFile(path.join(invalidIds.codexHome, "config.toml"), "utf8"), invalidIds.config);

  const wrongAgent = await ownedCodexFixture(t);
  const wrongOwner = { ...wrongAgent.owner, agent_id: "foreign-agent" };
  await fs.writeFile(path.join(wrongAgent.codexHome, ".wardian-messaging.json"), JSON.stringify(wrongOwner));
  const wrongAgentPath = path.join(wrongAgent.codexHome, "config.toml");
  const wrongAgentBefore = await fs.readFile(wrongAgentPath, "utf8");
  await assert.rejects(appendOwnedReplyToolApproval({
    wardianHome: wrongAgent.wardianHome,
    agentId: wrongAgent.agentId,
    expectedCommand: wrongAgent.commandPath,
  }), /different agent/u);
  assert.equal(await fs.readFile(wrongAgentPath, "utf8"), wrongAgentBefore);

  const wrongCommand = await ownedCodexFixture(t);
  const otherCommand = path.join(wrongCommand.wardianHome, "bin", "other-mcp");
  await fs.writeFile(otherCommand, "foreign executable\n");
  const wrongCommandPath = path.join(wrongCommand.codexHome, "config.toml");
  const wrongCommandBefore = await fs.readFile(wrongCommandPath, "utf8");
  await assert.rejects(appendOwnedReplyToolApproval({
    wardianHome: wrongCommand.wardianHome,
    agentId: wrongCommand.agentId,
    expectedCommand: otherCommand,
  }), /different executable/u);
  assert.equal(await fs.readFile(wrongCommandPath, "utf8"), wrongCommandBefore);

  const wrongArgs = await ownedCodexFixture(t);
  const wrongArgsPath = path.join(wrongArgs.codexHome, "config.toml");
  const wrongArgsBefore = wrongArgs.config.replace('args = ["mcp", "serve"]', 'args = ["mcp", "receive"]');
  await fs.writeFile(wrongArgsPath, wrongArgsBefore);
  await assert.rejects(appendOwnedReplyToolApproval({
    wardianHome: wrongArgs.wardianHome,
    agentId: wrongArgs.agentId,
    expectedCommand: wrongArgs.commandPath,
  }), /args differ/u);
  assert.equal(await fs.readFile(wrongArgsPath, "utf8"), wrongArgsBefore);

  const wrongEnvironment = await ownedCodexFixture(t);
  const wrongEnvironmentPath = path.join(wrongEnvironment.codexHome, "config.toml");
  const wrongEnvironmentBefore = wrongEnvironment.config.replace(
    `WARDIAN_HOME = ${JSON.stringify(wrongEnvironment.wardianHome)}`,
    'WARDIAN_HOME = "C:/foreign-home"',
  );
  await fs.writeFile(wrongEnvironmentPath, wrongEnvironmentBefore);
  await assert.rejects(appendOwnedReplyToolApproval({
    wardianHome: wrongEnvironment.wardianHome,
    agentId: wrongEnvironment.agentId,
    expectedCommand: wrongEnvironment.commandPath,
  }), /different Wardian home/u);
  assert.equal(await fs.readFile(wrongEnvironmentPath, "utf8"), wrongEnvironmentBefore);
});

test("fixture reply-tool grant refuses a linked managed Codex profile", async (t) => {
  const owned = await ownedCodexFixture(t);
  const movedProfile = owned.codexHome + "-owned-target";
  await fs.rename(owned.codexHome, movedProfile);
  try {
    await fs.symlink(movedProfile, owned.codexHome, process.platform === "win32" ? "junction" : "dir");
  } catch (error) {
    await fs.rename(movedProfile, owned.codexHome);
    if (["EPERM", "EACCES", "ENOTSUP"].includes(error.code)) {
      t.skip("This host does not permit creating a directory link for the ownership guard test");
      return;
    }
    throw error;
  }
  await assert.rejects(appendOwnedReplyToolApproval({
    wardianHome: owned.wardianHome,
    agentId: owned.agentId,
    expectedCommand: owned.commandPath,
  }), /must not be a link/u);
});

test("automatic reply uses an explicit System32 shell on Windows and the portable path elsewhere", () => {
  const gateCommand = "node .wardian-e2e-gate/reply-gate.mjs --deadline \"2030-01-01T00:00:00.000Z\"";
  const windowsShell = automaticReplyExecShell("win32", "C:\\Windows");
  assert.equal(windowsShell, "C:\\Windows\\System32\\cmd.exe");
  const windowsPowerShell = automaticReplyExecPowerShell("win32", "C:\\Windows");
  assert.equal(windowsPowerShell, "C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe");
  const windowsInstructions = automaticReplyTaskInstructions(gateCommand, windowsShell, "C:\\Windows");
  const execScript = automaticReplyExecScript(gateCommand, windowsShell);
  assert.equal(execScript,
    `let result=await tools.exec_command({cmd:${JSON.stringify(gateCommand)},shell:${JSON.stringify(windowsShell)},yield_time_ms:1000});` +
    "text(JSON.stringify(result));while(result.session_id){" +
    "result=await tools.write_stdin({session_id:result.session_id,chars:'',yield_time_ms:1000});" +
    "text(JSON.stringify(result));}");
  assert.match(windowsInstructions, /functions\.exec/u);
  assert.match(windowsInstructions, /shell/u);
  assert.ok(windowsInstructions.includes(execScript));
  assert.ok(execScript.includes(`cmd:${JSON.stringify(gateCommand)}`));
  assert.ok(execScript.includes(`shell:${JSON.stringify(windowsShell)}`));
  assert.ok(execScript.includes("yield_time_ms:1000"));
  assert.ok(execScript.includes("tools.write_stdin({session_id:result.session_id,chars:'',yield_time_ms:1000})"));
  assert.ok(!/WindowsApps/iu.test(windowsInstructions));
  assert.throws(() => automaticReplyTaskInstructions(
    gateCommand, "C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe", "C:\\Windows",
  ), /System32 Cmd/u);
  assert.throws(() => automaticReplyTaskInstructions(gateCommand, "cmd.exe", "C:\\Windows"), /System32 Cmd/u);

  assert.equal(automaticReplyExecShell("linux", "/usr"), null);
  assert.equal(automaticReplyExecShell("darwin", "/Applications"), null);
  assert.equal(automaticReplyExecPowerShell("linux", "/usr"), null);
  const portableInstructions = automaticReplyTaskInstructions(gateCommand);
  const portableScript = automaticReplyExecScript(gateCommand, null);
  assert.match(portableInstructions, /functions\.exec/u);
  assert.ok(portableInstructions.includes(portableScript));
  assert.ok(portableScript.includes(`cmd:${JSON.stringify(gateCommand)},yield_time_ms:1000`));
  assert.ok(!portableScript.includes("shell:"), "Portable execution must leave shell selection to the host");
  assert.throws(() => automaticReplyExecShell("win32", null), /SystemRoot must be absolute/u);
  assert.throws(() => automaticReplyExecShell("win32", "C:\\WindowsApps"), /must not resolve/u);
  assert.throws(() => automaticReplyExecPowerShell("win32", "C:\\Windows Root"), /free of Cmd metacharacters/u);
  assert.throws(() => automaticReplyExecPowerShell("win32", "C:\\Win&dows"), /free of Cmd metacharacters/u);
  assert.throws(() => automaticReplyExecPowerShell("win32", 'C:\\Win"dows'), /free of Cmd metacharacters/u);
});

test("gate invocation assertion accepts only the exact wrapper or exact native exec arguments", () => {
  const gateCommand = '"C:\\Program Files\\Node\\node.exe" .wardian-e2e-gate/reply-gate.mjs --deadline 2030-01-01T00:00:00.000Z';
  const shell = "C:\\Windows\\System32\\cmd.exe";
  const script = automaticReplyExecScript(gateCommand, shell);
  const wrapped = { type: "custom_tool_call", name: "exec", raw_input: ` \n${script}\n ` };
  assert.equal(assertAutomaticReplyGateInvocation(wrapped, { gateCommand, shell }), true);
  assert.throws(() => assertAutomaticReplyGateInvocation({ ...wrapped, raw_input: script + "text('extra');" }, {
    gateCommand, shell,
  }), /exact reply gate wrapper/u);
  assert.throws(() => assertAutomaticReplyGateInvocation({ ...wrapped, name: "read_file" }, {
    gateCommand, shell,
  }), /captured tool call type is unsupported/u);

  const direct = {
    type: "function_call", name: "exec_command",
    raw_input: JSON.stringify({ cmd: gateCommand, shell, yield_time_ms: 1000 }),
  };
  assert.equal(assertAutomaticReplyGateInvocation(direct, { gateCommand, shell }), true);
  for (const args of [
    { cmd: gateCommand + " ", shell },
    { cmd: gateCommand, shell: "C:\\WindowsApps\\pwsh.exe" },
    { cmd: gateCommand },
  ]) {
    assert.throws(() => assertAutomaticReplyGateInvocation({ ...direct, raw_input: JSON.stringify(args) }, {
      gateCommand, shell,
    }), assert.AssertionError);
  }
  assert.throws(() => assertAutomaticReplyGateInvocation({ ...direct, raw_input: "not JSON" }, {
    gateCommand, shell,
  }), /valid JSON/u);
  for (const override of [{ workdir: "C:\\foreign" }, { cwd: "C:\\foreign" }]) {
    assert.throws(() => assertAutomaticReplyGateInvocation({
      ...direct, raw_input: JSON.stringify({ cmd: gateCommand, shell, ...override }),
    }, { gateCommand, shell }), /workspace/u);
  }
  const portableDirect = {
    type: "function_call", name: "exec_command",
    raw_input: JSON.stringify({ cmd: gateCommand }),
  };
  assert.equal(assertAutomaticReplyGateInvocation(portableDirect, { gateCommand, shell: null }), true);
  assert.throws(() => assertAutomaticReplyGateInvocation({
    ...portableDirect, raw_input: JSON.stringify({ cmd: gateCommand, shell: null }),
  }, { gateCommand, shell: null }), /omit the shell override/u);
});

test("Windows gate command pins PowerShell and encodes the exact Node path and deadline", () => {
  const deadlineAt = Date.parse("2030-01-01T00:00:00.000Z");
  const nodeExecutable = "C:\\Program Files\\Node Ω's\\node.exe";
  const systemRoot = "C:\\Windows";
  const powershellExecutable = automaticReplyExecPowerShell("win32", systemRoot);
  const command = automaticReplyGateCommand(deadlineAt, {
    nodeExecutable, powershellExecutable, platform: "win32", systemRoot,
  });
  const encodedPrefix = powershellExecutable + " -NoProfile -NonInteractive -EncodedCommand ";
  assert.ok(command.startsWith(encodedPrefix));
  assert.equal((command.match(/"/gu) ?? []).length, 0);
  const encodedScript = command.slice(encodedPrefix.length);
  assert.match(encodedScript, /^[A-Za-z0-9+/]+=*$/u);
  const decodedScript = Buffer.from(encodedScript, "base64").toString("utf16le");
  assert.ok(decodedScript.includes("$node = 'C:\\Program Files\\Node Ω''s\\node.exe'"));
  assert.ok(decodedScript.includes(
    "@('.\\.wardian-e2e-gate\\reply-gate.mjs','--deadline','2030-01-01T00:00:00.000Z')"));
  assert.ok(decodedScript.includes("[Console]::OutputEncoding = $utf8"));
  for (const unsafePath of [
    "C:\\Node%HOME%\\node.exe", "C:\\Node!\\node.exe", "C:\\Node^\\node.exe",
    "C:\\Node&tools\\node.exe", "C:\\Node|tools\\node.exe", "C:\\Node<tools\\node.exe",
    "C:\\Node>tools\\node.exe", "C:\\Node\\(tools)\\node.exe", "C:\\Node\\node.exe\r\n",
    "C:\\Node\\\"quoted\\node.exe",
  ]) {
    assert.throws(() => automaticReplyGateCommand(deadlineAt, {
      nodeExecutable: unsafePath, powershellExecutable, platform: "win32", systemRoot,
    }), /Cmd metacharacter/u);
  }
  assert.throws(() => automaticReplyGateCommand(deadlineAt, {
    nodeExecutable,
    powershellExecutable: "C:\\Windows Root\\System32\\WindowsPowerShell\\v1.0\\powershell.exe",
    platform: "win32", systemRoot,
  }), /exact System32 Windows PowerShell/u);
  assert.equal(automaticReplyGateCommand(deadlineAt, { platform: "linux" }),
    "node .wardian-e2e-gate/reply-gate.mjs --deadline \"2030-01-01T00:00:00.000Z\"");
  assert.throws(() => automaticReplyGateCommand(Number.MAX_SAFE_INTEGER, { platform: "win32" }),
    /supported date range/u);
});

if (process.platform === "win32") {
  test("Cmd /c launches the qualified Node executable through encoded System32 PowerShell", async (t) => {
    const root = await fixture(t);
    const nodeDirectory = path.join(root, "Node runtime Ω's with spaces");
    await fs.symlink(await fs.realpath(path.dirname(process.execPath)), nodeDirectory, "junction");

    const qualifiedNode = path.join(nodeDirectory, path.basename(process.execPath));
    assert.equal(await fs.realpath(qualifiedNode), await fs.realpath(process.execPath));

    const gateDirectory = path.join(root, ".wardian-e2e-gate");
    await fs.mkdir(gateDirectory);
    await fs.writeFile(path.join(gateDirectory, "reply-gate.mjs"),
      "process.stdout.write(JSON.stringify({execPath:process.execPath,argv:process.argv.slice(2),marker:\"Ω雪\"}));\n");

    const shell = automaticReplyExecShell();
    const systemRoot = process.env.SystemRoot;
    const powershellExecutable = automaticReplyExecPowerShell("win32", systemRoot);
    const expectedDeadline = "2030-01-01T00:00:00.000Z";
    const command = automaticReplyGateCommand(Date.parse(expectedDeadline), {
      nodeExecutable: qualifiedNode,
      powershellExecutable,
      platform: "win32",
      systemRoot,
    });
    assert.ok(!command.includes(qualifiedNode), "Cmd must see only the encoded Node path");
    const result = spawnSync(shell, ["/c", command], {
      cwd: root,
      encoding: "utf8",
      timeout: 10_000,
      windowsHide: true,
    });
    assert.ifError(result.error);
    assert.equal(result.status, 0, result.stderr);
    const child = JSON.parse(result.stdout);
    assert.equal(await fs.realpath(child.execPath), await fs.realpath(qualifiedNode));
    assert.deepEqual(child.argv, ["--deadline", expectedDeadline]);
    assert.equal(child.marker, "Ω雪", "PowerShell must preserve Node UTF-8 stdout");
  });
}

test("exec attempt collector preserves raw native inputs and matches both output event forms by call_id", () => {
  const wrappedInput =
    "const x = await tools.exec_command({cmd:'node reply-gate.mjs',shell:'C:\\\\WINDOWS\\\\System32\\\\cmd.exe'});text(JSON.stringify(x));";
  const rows = [
    { type: "response_item", timestamp: "2030-01-01T00:00:01.000Z", payload: {
      type: "function_call", namespace: "functions", name: "exec_command",
      call_id: "native-call-1", arguments: "{\"cmd\":\"node reply-gate.mjs\"}",
    } },
    { type: "response_item", timestamp: "2030-01-01T00:00:02.000Z", payload: {
      type: "function_call", namespace: "functions", name: "reply",
      call_id: "wardian-reply", arguments: "{\"status\":\"done\"}",
    } },
    { type: "response_item", timestamp: "2030-01-01T00:00:03.000Z", payload: {
      type: "custom_tool_call", namespace: "functions", name: "exec",
      call_id: "native-call-2", input: wrappedInput,
    } },
    { type: "response_item", timestamp: "2030-01-01T00:00:04.000Z", payload: {
      type: "custom_tool_call_output", call_id: "native-call-2", output: "custom exec completed",
    } },
    { type: "response_item", timestamp: "2030-01-01T00:00:05.000Z", payload: {
      type: "function_call_output", call_id: "native-call-1",
      output: "Script running with cell ID 9\nWall time 1.0 seconds\nOutput:\n",
    } },
  ];
  const attempts = collectAutomaticReplyExecAttempts(rows);
  assert.equal(attempts.length, 2, "Wardian reply calls are not shell attempts");
  assert.deepEqual(attempts.map(({ call_id, ordinal, row_ordinal, timestamp }) =>
    ({ call_id, ordinal, row_ordinal, timestamp })), [
    { call_id: "native-call-1", ordinal: 1, row_ordinal: 0, timestamp: "2030-01-01T00:00:01.000Z" },
    { call_id: "native-call-2", ordinal: 2, row_ordinal: 2, timestamp: "2030-01-01T00:00:03.000Z" },
  ]);
  assert.equal(attempts[0].raw_input, "{\"cmd\":\"node reply-gate.mjs\"}");
  assert.equal(attempts[0].namespace, "functions");
  assert.deepEqual(attempts[0].output, {
    row_ordinal: 4, timestamp: "2030-01-01T00:00:05.000Z",
    raw: "Script running with cell ID 9\nWall time 1.0 seconds\nOutput:\n",
  });
  assert.equal(attempts[1].raw_input, wrappedInput);
  assert.deepEqual(attempts[1].output, {
    row_ordinal: 3, timestamp: "2030-01-01T00:00:04.000Z", raw: "custom exec completed",
  });
});

test("exec attempt collector leaves pending calls visible and rejects ambiguous call/output identities", () => {
  const pending = collectAutomaticReplyExecAttempts([
    { type: "response_item", timestamp: "2030-01-01T00:00:01Z", payload: {
      type: "custom_tool_call", name: "exec", call_id: "pending-1", input: "const value = 'opaque';",
    } },
    { type: "response_item", timestamp: "2030-01-01T00:00:02Z", payload: {
      type: "function_call", name: "exec_command", call_id: "pending-2", arguments: "not-json { raw input",
    } },
    { type: "response_item", payload: {
      type: "function_call", name: "unrelated_tool", call_id: "ignored", arguments: "{}",
    } },
  ]);
  assert.equal(pending.length, 2);
  assert.equal(pending[0].call_id, "pending-1");
  assert.equal(pending[0].output, null);
  assert.equal(pending[0].raw_input, "const value = 'opaque';");
  assert.equal(pending[1].call_id, "pending-2");
  assert.equal(pending[1].namespace, null, "The collector accepts function calls without a namespace field");
  assert.equal(pending[1].raw_input, "not-json { raw input");

  const duplicateCalls = [
    { type: "response_item", payload: {
      type: "function_call", name: "exec_command", call_id: "same-id", arguments: "first",
    } },
    { type: "response_item", payload: {
      type: "custom_tool_call", name: "exec", call_id: "same-id", input: "second",
    } },
  ];
  assert.throws(() => collectAutomaticReplyExecAttempts(duplicateCalls), /Duplicate exec attempt call_id/u);

  const foreignCollision = [
    { type: "response_item", payload: {
      type: "function_call", name: "exec_command", call_id: "foreign-id", arguments: "opaque",
    } },
    { type: "response_item", payload: {
      type: "custom_tool_call", name: "read_file", call_id: "foreign-id", input: "other tool",
    } },
  ];
  assert.throws(() => collectAutomaticReplyExecAttempts(foreignCollision), /foreign tool call_id/u);

  const duplicateOutputs = [
    { type: "response_item", payload: {
      type: "function_call", name: "exec_command", call_id: "output-id", arguments: "opaque",
    } },
    { type: "response_item", payload: {
      type: "function_call_output", call_id: "output-id", output: "first",
    } },
    { type: "response_item", payload: {
      type: "function_call_output", call_id: "output-id", output: "second",
    } },
  ];
  assert.throws(() => collectAutomaticReplyExecAttempts(duplicateOutputs), /Duplicate exec attempt outputs/u);
  const wrongOutput = [
    duplicateOutputs[0],
    { type: "response_item", payload: {
      type: "custom_tool_call_output", call_id: "output-id", output: "foreign output",
    } },
  ];
  assert.throws(() => collectAutomaticReplyExecAttempts(wrongOutput), /output type does not match/u);
});

test("gate token comes only from the matching completed functions.wait cell output", () => {
  const token = "WORKER_ONLY_123e4567-e89b-42d3-a456-426614174000";
  const firstResult = JSON.stringify({ output: "", exit_code: null, session_id: 17 });
  const firstWaitResult = JSON.stringify({
    output: "WORKER_ONLY_123e4567-e89b-42d3-", exit_code: null, session_id: 17,
  });
  const terminalWaitResult = JSON.stringify({ output: "a456-426614174000\n", exit_code: 0, session_id: null });
  const completedWaitText = "Script completed\nWall time 1.0 seconds\nOutput:\n" +
    firstWaitResult + "\n" + terminalWaitResult;
  const rows = [
    { type: "response_item", timestamp: "2030-01-01T00:00:01Z", payload: {
      type: "custom_tool_call", namespace: "functions", name: "exec", call_id: "outer-exec",
      input: "exact wrapper source",
    } },
    { type: "response_item", timestamp: "2030-01-01T00:00:02Z", payload: {
      type: "custom_tool_call_output", call_id: "outer-exec",
      output: `Script running with cell ID 17\nWall time 1.0 seconds\nOutput:\n${firstResult}`,
    } },
    { type: "response_item", timestamp: "2030-01-01T00:00:03Z", payload: {
      type: "function_call", namespace: "functions", name: "wait", call_id: "outer-wait",
      arguments: JSON.stringify({ cell_id: "17" }),
    } },
    { type: "response_item", timestamp: "2030-01-01T00:00:04Z", payload: {
      type: "function_call_output", call_id: "outer-wait", output: [
        { type: "input_text", text: completedWaitText },
      ],
    } },
  ];
  const [attempt] = collectAutomaticReplyExecAttempts(rows);
  assert.equal(attempt.cell_id, "17");
  assert.deepEqual(attempt.continuations, [{
    exec_call_id: "outer-exec", cell_id: "17", call_id: "outer-wait", ordinal: 1, row_ordinal: 2,
    timestamp: "2030-01-01T00:00:03Z", type: "function_call", namespace: "functions", name: "wait",
    raw_input: JSON.stringify({ cell_id: "17" }),
    output: { row_ordinal: 3, timestamp: "2030-01-01T00:00:04Z", raw: [
      { type: "input_text", text: completedWaitText },
    ] },
  }]);
  assert.equal(automaticReplyGateToken(attempt), token);
  assert.equal(automaticReplyGateToken(attempt), "WORKER_ONLY_123e4567-e89b-42d3-a456-426614174000");
  assert.throws(() => assert.equal(automaticReplyGateToken(attempt),
    "WORKER_ONLY_123e4567-e89b-42d3-a456-426614174001"), assert.AssertionError,
  "A canonical reply with a different token must fail exact matching");

  const customWaitRows = rows.map((row, index) => {
    if (index === 2) return { ...row, payload: {
      type: "custom_tool_call", namespace: "functions", name: "wait", call_id: "outer-wait",
      input: JSON.stringify({ cell_id: "17", yield_time_ms: 30_000, max_tokens: 1_000 }),
    } };
    if (index === 3) return { ...row, payload: {
      type: "custom_tool_call_output", call_id: "outer-wait", output: [
        { type: "text", text: completedWaitText },
      ],
    } };
    return row;
  });
  const [customWaitAttempt] = collectAutomaticReplyExecAttempts(customWaitRows);
  assert.equal(automaticReplyGateToken(customWaitAttempt), token);

  const afterReplyRows = [
    ...rows,
    { type: "response_item", timestamp: "2030-01-01T00:00:04.500Z", payload: {
      type: "function_call", namespace: "functions", name: "reply", call_id: "canonical-reply",
      arguments: JSON.stringify({ status: "done", token }),
    } },
    { type: "response_item", timestamp: "2030-01-01T00:00:04.750Z", payload: {
      type: "function_call_output", call_id: "canonical-reply", output: `accepted ${token}`,
    } },
    { type: "response_item", timestamp: "2030-01-01T00:00:05Z", payload: {
      type: "custom_tool_call", namespace: "functions", name: "exec", call_id: "reply-only-exec",
      input: "reply-only wrapper source",
    } },
    { type: "response_item", timestamp: "2030-01-01T00:00:06Z", payload: {
      type: "custom_tool_call_output", call_id: "reply-only-exec", output: "reply accepted",
    } },
  ];
  const attemptsAfterReply = collectAutomaticReplyExecAttempts(afterReplyRows);
  assert.equal(attemptsAfterReply.length, 2, "Later reply-only exec wrappers remain visible without becoming gate evidence");
  assert.equal(automaticReplyGateToken(attemptsAfterReply[0]), token);
  assert.equal(attemptsAfterReply[1].call_id, "reply-only-exec");
  assert.ok(!JSON.stringify(attemptsAfterReply[1].output.raw).includes("WORKER_ONLY_"));
});

test("repeated waits merge pending-cell stdout through the terminal result", () => {
  const token = "WORKER_ONLY_123e4567-e89b-42d3-a456-426614174000";
  const rows = [
    { type: "response_item", payload: {
      type: "custom_tool_call", namespace: "functions", name: "exec", call_id: "exec-repeated",
      input: "exact wrapper source",
    } },
    { type: "response_item", payload: {
      type: "custom_tool_call_output", call_id: "exec-repeated",
      output: "Script running with cell ID 41\nWall time 31.0 seconds\nOutput:\n" +
        JSON.stringify({ output: "WORKER_ONLY_123e4567-e89b-42d3-", exit_code: null, session_id: 41 }),
    } },
    { type: "response_item", payload: {
      type: "function_call", namespace: "functions", name: "wait", call_id: "wait-repeated-1",
      arguments: JSON.stringify({ cell_id: "41", yield_time_ms: 30_000, max_tokens: 1_000 }),
    } },
    { type: "response_item", payload: {
      type: "function_call_output", call_id: "wait-repeated-1", output: [
        { type: "input_text", text: "Script running with cell ID 41\nWall time 14.8 seconds\nOutput:\n" +
          JSON.stringify({ output: "a456-426614174000", exit_code: null, session_id: 41 }) },
      ],
    } },
    { type: "response_item", payload: {
      type: "custom_tool_call", namespace: "functions", name: "wait", call_id: "wait-repeated-2",
      input: JSON.stringify({ cell_id: "41", yield_time_ms: 30_000, max_tokens: 1_000 }),
    } },
    { type: "response_item", payload: {
      type: "custom_tool_call_output", call_id: "wait-repeated-2", output: [
        { type: "input_text", text: "Script completed\nWall time 0.4 seconds\nOutput:\n" +
          JSON.stringify({ output: "\n", exit_code: 0, session_id: null }) },
      ],
    } },
  ];
  const [attempt] = collectAutomaticReplyExecAttempts(rows);
  assert.equal(attempt.continuations.length, 2);
  assert.deepEqual(attempt.continuations.map(({ call_id, cell_id }) => ({ call_id, cell_id })), [
    { call_id: "wait-repeated-1", cell_id: "41" },
    { call_id: "wait-repeated-2", cell_id: "41" },
  ]);
  assert.equal(automaticReplyGateToken(attempt), token,
    "Initial and repeated-wait result.output chunks combine only after exit_code 0");

  const finishedBeforeSecondWait = rows.map((row, index) => index === 3 ? {
    ...row,
    payload: { ...row.payload, output: [
      { type: "input_text", text: "Script completed\nWall time 0.4 seconds\nOutput:\n" +
        JSON.stringify({ output: "", exit_code: 0, session_id: null }) },
    ] },
  } : row);
  assert.throws(() => collectAutomaticReplyExecAttempts(finishedBeforeSecondWait),
    /same cell to remain pending/u);

  const waitBeforePriorOutput = [...rows];
  const secondWaitCall = waitBeforePriorOutput.splice(4, 1)[0];
  waitBeforePriorOutput.splice(3, 0, secondWaitCall);
  assert.throws(() => collectAutomaticReplyExecAttempts(waitBeforePriorOutput),
    /follow the previous output/u);

  const foreignCellWait = rows.map((row, index) => index === 3 ? {
    ...row,
    payload: { ...row.payload, output: [
      { type: "input_text", text: "Script running with cell ID 42\nWall time 14.8 seconds\nOutput:\n" +
        JSON.stringify({ output: "a456-426614174000", exit_code: null, session_id: 42 }) },
    ] },
  } : row);
  assert.throws(() => collectAutomaticReplyExecAttempts(foreignCellWait), /foreign cell/u);
});

test("direct exec_command gate token is bound to its own output call_id", () => {
  const token = "WORKER_ONLY_123e4567-e89b-42d3-a456-426614174000";
  const args = JSON.stringify({
    cmd: '"C:\\Program Files\\Node\\node.exe" gate.mjs --deadline 2030-01-01T00:00:00.000Z',
    shell: "C:\\Windows\\System32\\cmd.exe",
  });
  const rows = [
    { type: "response_item", timestamp: "2030-01-01T00:00:01Z", payload: {
      type: "function_call", namespace: "functions", name: "exec_command", call_id: "direct-exec",
      arguments: args,
    } },
    { type: "response_item", timestamp: "2030-01-01T00:00:03Z", payload: {
      type: "function_call_output", call_id: "direct-exec",
      output: JSON.stringify({ output: `${token}\n`, exit_code: 0 }),
    } },
    { type: "response_item", payload: {
      type: "function_call", namespace: "functions", name: "reply", call_id: "canonical-reply",
      arguments: JSON.stringify({ token }),
    } },
  ];
  const [attempt] = collectAutomaticReplyExecAttempts(rows);
  assert.equal(attempt.raw_input, args, "The exact native JSON arguments remain available for validation");
  assert.equal(automaticReplyGateToken(attempt), token);
  assert.notEqual(automaticReplyGateToken(attempt), "WORKER_ONLY_123e4567-e89b-42d3-a456-426614174001",
    "A mismatched canonical reply token cannot equal the observed gate token");
  assert.throws(() => collectAutomaticReplyExecAttempts([
    rows[0],
    { type: "response_item", payload: {
      type: "function_call_output", call_id: "foreign-output", output: token,
    } },
  ]), /foreign native tool output/u);
});

test("gate token remains pending without the observed cell output and rejects foreign token output", () => {
  const token = "WORKER_ONLY_123e4567-e89b-42d3-a456-426614174000";
  const execCall = { type: "response_item", payload: {
    type: "custom_tool_call", name: "exec", call_id: "exec-cell", input: "wrapper",
  } };
  const execOutput = { type: "response_item", payload: {
    type: "custom_tool_call_output", call_id: "exec-cell",
    output: "Script running with cell ID 22\nWall time 1.0 seconds\nOutput:\n",
  } };
  const foreignWait = [
    { type: "response_item", payload: {
      type: "custom_tool_call", name: "wait", call_id: "wait-foreign", input: JSON.stringify({ cell_id: "23" }),
    } },
    { type: "response_item", payload: {
      type: "custom_tool_call_output", call_id: "wait-foreign", output: token,
    } },
  ];
  const [pending] = collectAutomaticReplyExecAttempts([execCall, execOutput]);
  assert.equal(automaticReplyGateToken(pending), null);
  assert.throws(() => collectAutomaticReplyExecAttempts([execCall, execOutput, ...foreignWait]),
    /foreign functions\.wait output/u);

  const matchingWaitPending = [
    ...[execCall, execOutput],
    { type: "response_item", payload: {
      type: "function_call", name: "wait", call_id: "wait-pending", arguments: JSON.stringify({ cell_id: 22 }),
    } },
  ];
  const [stillPending] = collectAutomaticReplyExecAttempts(matchingWaitPending);
  assert.equal(stillPending.continuations[0].output, null);
  assert.equal(automaticReplyGateToken(stillPending), null);
});

test("gate token collector rejects duplicate cells, duplicate wait outputs, and malformed markers", () => {
  const execRow = (callId, cellId) => [
    { type: "response_item", payload: {
      type: "function_call", name: "exec_command", call_id: callId, arguments: "{}",
    } },
    { type: "response_item", payload: {
      type: "function_call_output", call_id: callId,
      output: `Script running with cell ID ${cellId}\nWall time 1.0 seconds\nOutput:\n`,
    } },
  ];
  assert.throws(() => collectAutomaticReplyExecAttempts([
    ...execRow("exec-a", "same-cell"), ...execRow("exec-b", "same-cell"),
  ]), /Duplicate functions\.exec cell IDs/u);

  const base = [
    ...execRow("exec-one", "31"),
    { type: "response_item", payload: {
      type: "function_call", name: "wait", call_id: "wait-one", arguments: JSON.stringify({ cell_id: "31" }),
    } },
    { type: "response_item", payload: {
      type: "function_call_output", call_id: "wait-one", output: "first",
    } },
  ];
  assert.throws(() => collectAutomaticReplyExecAttempts([
    ...base,
    { type: "response_item", payload: {
      type: "function_call_output", call_id: "wait-one", output: "duplicate",
    } },
  ]), /Duplicate functions\.wait outputs/u);
  assert.throws(() => collectAutomaticReplyExecAttempts([
    ...base,
    { type: "response_item", payload: {
      type: "custom_tool_call", name: "wait", call_id: "wait-one", input: JSON.stringify({ cell_id: "31" }),
    } },
  ]), /Duplicate functions\.wait call_id/u);

  const attempt = { cell_id: null, output: { raw: JSON.stringify({ output: "WORKER_ONLY_not-a-uuid", exit_code: 0 }) } };
  assert.throws(() => automaticReplyGateToken(attempt), /exactly one strict worker UUID/u);
  assert.throws(() => automaticReplyGateToken({
    cell_id: null, output: { raw: JSON.stringify({
      output: "WORKER_ONLY_123e4567-e89b-42d3-a456-426614174000\nWORKER_ONLY_123e4567-e89b-42d3-a456-426614174001",
      exit_code: 0,
    }) },
  }), /exactly one strict worker UUID/u);
  assert.throws(() => automaticReplyGateToken({
    cell_id: null, output: { raw: [
      JSON.stringify({ output: "WORKER_ONLY_123e4567-e89b-42d3-a456-426614174000", exit_code: 0 }),
      JSON.stringify({ output: "", exit_code: 0 }),
    ].join("\n") },
  }), /multiple terminal results/u);
  assert.throws(() => automaticReplyGateToken({
    cell_id: null, raw_input: "WORKER_ONLY_123e4567-e89b-42d3-a456-426614174000",
    output: { raw: JSON.stringify({ output: "", error: "WORKER_ONLY_123e4567-e89b-42d3-a456-426614174000", exit_code: 0 }) },
  }), /exactly one strict worker UUID/u, "A UUID in raw input or an error is not stdout evidence");
  assert.throws(() => automaticReplyGateToken({
    cell_id: null, output: { raw: "not JSON\nWORKER_ONLY_123e4567-e89b-42d3-a456-426614174000" },
  }), /non-JSON result line/u);
  assert.throws(() => automaticReplyGateToken({
    cell_id: null,
    output: { raw: JSON.stringify({ output: "WORKER_ONLY_123e4567-e89b-42d3-a456-426614174000" }) },
  }), /missing a terminal result/u);
  assert.equal(automaticReplyGateToken({
    cell_id: null,
    output: { raw: JSON.stringify({ output: "partial", exit_code: null, session_id: 4 }) },
  }), null, "A process with a live session remains pending without a terminal result");
  assert.throws(() => automaticReplyGateToken({
    cell_id: null, output: { raw: JSON.stringify({
      output: "WORKER_ONLY_123e4567-e89b-42d3-a456-426614174000", exit_code: 1,
    }) },
  }), /did not exit successfully/u);
  assert.throws(() => automaticReplyGateToken({
    cell_id: "41",
    output: { raw: "Script running with cell ID 41\nWall time 31.0 seconds\nOutput:\n" },
    continuations: [{ output: { raw: [
      { type: "input_text", text: "Script failed\nWall time 14.8 seconds\nOutput:\n" },
      { type: "input_text", text: "Script error: command failed" },
    ] } }],
  }), /outer functions\.exec wrapper failed/u);
  assert.throws(() => automaticReplyGateToken({
    cell_id: null, output: { raw: [{ type: "unsupported_text", text: "WORKER_ONLY_123e4567-e89b-42d3-a456-426614174000" }] },
  }), /unsupported text representation/u);
});

test("non-Windows exec environment qualification keeps the portable path", async () => {
  assert.equal(await qualifyAutomaticReplyExecEnvironment({ platform: "linux" }), null);
  assert.equal(await qualifyAutomaticReplyExecEnvironment({ platform: "darwin" }), null);
});

const windowsOnly = process.platform === "win32" ? test : test.skip;
windowsOnly("Windows exec environment qualification hashes exact binaries and rejects command shadows", async (t) => {
  const wardianHome = await fixture(t);
  const workerWorkspace = path.join(wardianHome, "workspace");
  const wardianBin = path.join(wardianHome, "bin");
  const systemRoot = process.env.SystemRoot;
  const system32 = path.win32.join(systemRoot, "System32");
  const nodeDirectory = path.win32.dirname(process.execPath);
  await fs.mkdir(workerWorkspace);
  await fs.mkdir(wardianBin);
  const inheritedPath = [wardianBin + "\\", nodeDirectory, system32].join(";");
  const env = {
    WARDIAN_HOME: wardianHome,
    SystemRoot: systemRoot,
    Path: inheritedPath,
    PATHEXT: process.env.PATHEXT ?? ".COM;.EXE;.BAT;.CMD",
  };
  const qualify = (pathValue = inheritedPath) => qualifyAutomaticReplyExecEnvironment({
    platform: "win32",
    wardianHome,
    workerWorkspace,
    env: { ...env, Path: pathValue },
    nodeExecutable: process.execPath,
  });
  const receipt = await qualify();
  assert.equal(receipt.shell, path.win32.join(systemRoot, "System32", "cmd.exe"));
  assert.equal(receipt.powershell, automaticReplyExecPowerShell("win32", systemRoot));
  assert.equal(receipt.node, process.execPath);
  assert.equal(receipt.node_realpath, await fs.realpath(process.execPath));
  assert.match(receipt.shell_sha256, /^[0-9a-f]{64}$/u);
  assert.match(receipt.powershell_sha256, /^[0-9a-f]{64}$/u);
  assert.match(receipt.node_sha256, /^[0-9a-f]{64}$/u);
  assert.match(receipt.child_path.sha256, /^[0-9a-f]{64}$/u);
  assert.equal(receipt.child_path.entry_count, 3, "The duplicate Wardian bin entry is removed");
  assert.equal(receipt.child_path.cmd_resolution, "system32");
  assert.equal(receipt.child_path.node_resolution, "exact");
  assert.equal(receipt.child_path.workspace_command_shadows, 0);
  assert.equal(Object.hasOwn(receipt.child_path, "value"), false, "The receipt must not expose PATH contents");

  const shadowDirectory = path.join(wardianHome, "shadow");
  await fs.mkdir(shadowDirectory);
  await fs.writeFile(path.join(shadowDirectory, "cmd.exe"), "shadow");
  await assert.rejects(qualify(shadowDirectory + ";" + inheritedPath), /resolves cmd/u);
  await fs.rm(shadowDirectory, { recursive: true });

  await fs.writeFile(path.join(wardianBin, "node.exe"), "shadow");
  await assert.rejects(qualify(), /resolves Node/u);
  await fs.rm(path.join(wardianBin, "node.exe"));

  await fs.writeFile(path.join(workerWorkspace, "cmd.exe"), "shadow");
  await assert.rejects(qualify(), /workspace contains a cmd command shadow/u);
});

windowsOnly("Node qualification accepts its existing parent junction and rejects a foreign PATH shadow", async (t) => {
  const wardianHome = await fixture(t);
  const workerWorkspace = path.join(wardianHome, "workspace");
  const wardianBin = path.join(wardianHome, "bin");
  const aliasDirectory = path.join(wardianHome, "node-alias");
  const nodeDirectory = path.win32.dirname(process.execPath);
  const linkedNode = path.win32.join(aliasDirectory, path.win32.basename(process.execPath));
  const systemRoot = process.env.SystemRoot;
  const system32 = path.win32.join(systemRoot, "System32");
  await fs.mkdir(workerWorkspace);
  await fs.mkdir(wardianBin);
  try {
    await fs.symlink(nodeDirectory, aliasDirectory, "junction");
  } catch (error) {
    if (["EPERM", "EACCES", "ENOTSUP"].includes(error.code)) {
      t.skip("This host does not permit creating a directory junction for the Node path test");
      return;
    }
    throw error;
  }
  const env = {
    WARDIAN_HOME: wardianHome,
    SystemRoot: systemRoot,
    Path: [wardianBin, aliasDirectory, system32].join(";"),
    PATHEXT: process.env.PATHEXT ?? ".COM;.EXE;.BAT;.CMD",
  };
  const qualify = (pathValue = env.Path) => qualifyAutomaticReplyExecEnvironment({
    platform: "win32",
    wardianHome,
    workerWorkspace,
    env: { ...env, Path: pathValue },
    nodeExecutable: linkedNode,
  });
  const receipt = await qualify();
  assert.equal(receipt.node, linkedNode);
  assert.equal(receipt.node_realpath, await fs.realpath(process.execPath));

  const foreignDirectory = path.join(wardianHome, "foreign-node");
  await fs.mkdir(foreignDirectory);
  await fs.writeFile(path.join(foreignDirectory, "node.exe"), "foreign executable");
  await assert.rejects(qualify([wardianBin, foreignDirectory, aliasDirectory, system32].join(";")),
    /resolves Node/u);
});

function database(home, configure = () => {}) {
  const db = new DatabaseSync(path.join(home, "state.db"));
  try {
    db.exec(`
      CREATE TABLE interactions (
        id TEXT PRIMARY KEY, kind TEXT, sender_session_id TEXT, target_session_ids TEXT,
        parent_interaction_id TEXT, body_ref TEXT, status TEXT
      );
      CREATE TABLE structured_replies (request_id TEXT, status TEXT);
      CREATE TABLE agent_message_delivery (
        interaction_id TEXT, operation TEXT, requester_provider TEXT,
        requester_conversation_id TEXT, requester_generation INTEGER
      );
      CREATE TABLE provider_input_state (session_id TEXT PRIMARY KEY, state TEXT, generation INTEGER);
    `);
    configure(db);
  } finally { db.close(); }
}

test("report persistence preserves unowned homes and uses the actual acquired run claim", async (t) => {
  const home = await fixture(t);
  const reportPath = path.join(home, "automatic-inbound-reply-report.json");
  await fs.writeFile(reportPath, "operator evidence\n", { flag: "wx" });
  assert.equal(await writeOwnedReplyReport({ isolatedHome: home }, {}), false);
  const runId = "report-owner-" + path.basename(home);
  const lock = acquireHomeLock({ home, runId }).lock;
  try {
    assert.equal(await writeOwnedReplyReport({ isolatedHome: home, runId: "other-run" }, {}), false);
    assert.equal(await writeOwnedReplyReport({ isolatedHome: home, runId: "other-run", homeLock: lock }, {}), false);
    assert.equal(await fs.readFile(reportPath, "utf8"), "operator evidence\n");
    assert.deepEqual(readHomeLock(home), lock);
    const report = { status: "fail", cleanup: { shutdown_confirmed: false } };
    assert.equal(await writeOwnedReplyReport({ isolatedHome: home, runId, homeLock: lock }, report), true);
    assert.deepEqual(JSON.parse(await fs.readFile(reportPath, "utf8")), report);
  } finally { releaseHomeLock({ home, runId }); }
});

function row(db, { id = "task-1", kind = "task", sender = "requester", targets = ["worker"],
  parent = null, body = "gate instruction", status = "pending" } = {}) {
  db.prepare("INSERT INTO interactions VALUES (?,?,?,?,?,?,?)").run(
    id, kind, sender, JSON.stringify(targets), parent,
    JSON.stringify({ storage: "inline", body }), status,
  );
}

function task(db, { id = "task-1", sender = "requester", targets = ["worker"],
  provider = "antigravity", conversationId = "conversation-1", requesterGeneration = 7,
  providerInputGeneration = requesterGeneration } = {}) {
  row(db, { id, sender, targets });
  db.prepare("INSERT INTO agent_message_delivery VALUES (?,?,?,?,?)").run(
    id, "followup_task", provider, conversationId, requesterGeneration,
  );
  if (providerInputGeneration !== null) {
    db.prepare("INSERT INTO provider_input_state (session_id,state,generation) VALUES (?,?,?) " +
      "ON CONFLICT(session_id) DO UPDATE SET state=excluded.state,generation=excluded.generation")
      .run(sender, "ready", providerInputGeneration);
  }
}

function reply(db, overrides = {}) {
  row(db, { id: "reply-1", kind: "reply", sender: "worker", targets: ["requester"],
    parent: "task-1", body: "WORKER_ONLY_unpredictable", status: "completed", ...overrides });
  db.prepare("INSERT INTO structured_replies VALUES (?,?)").run("task-1", "done");
}

function lease(overrides = {}) {
  return {
    agent_id: "agent-1",
    provider: "antigravity",
    resume_session: "resume-1",
    owner_kind: "agent_lifecycle",
    owner_id: "lifecycle-1",
    acquisition_id: "opaque-acquisition-token",
    mode: "lifecycle_transition",
    started_at: "2026-10-24T19:00:00Z",
    heartbeat_at: "2026-10-24T19:00:01Z",
    expires_at: "2026-10-24T20:00:00Z",
    ...overrides,
  };
}

async function writeLeaseFile(home, document) {
  const runtime = path.join(home, "runtime");
  await fs.mkdir(runtime, { recursive: true });
  const leasePath = path.join(runtime, "conversation-leases.json");
  await fs.writeFile(leasePath, typeof document === "string" ? document : JSON.stringify(document));
  return leasePath;
}

const LEASE_CHECK_TIME = Date.parse("2026-10-24T19:30:00Z");

test("observer waits for missing database or unadmitted task without creating state", async (t) => {
  const home = await fixture(t);
  assert.equal(await readCanonicalTaskReply(home, "requester", "worker"), null);
  assert.deepEqual(await fs.readdir(home), []);
  database(home);
  assert.equal(await readCanonicalTaskReply(home, "requester", "worker"), null);
});

test("provider readiness observer distinguishes Booting, Ready, and missing state", async (t) => {
  const home = await fixture(t);
  database(home);
  assert.equal(await readProviderInputState(home, "requester"), null);
  assert.equal(providerInputIsReady(null, "requester"), false);

  const db = new DatabaseSync(path.join(home, "state.db"));
  try {
    db.prepare("INSERT INTO provider_input_state VALUES (?,?,?)").run("requester", "booting", 1);
  } finally { db.close(); }
  const beforeBootingRead = await fs.readFile(path.join(home, "state.db"));
  const booting = await readProviderInputState(home, "requester");
  assert.deepEqual({ ...booting }, { session_id: "requester", state: "booting", generation: 1 });
  assert.equal(providerInputIsReady(booting, "requester"), false);
  assert.deepEqual(await fs.readFile(path.join(home, "state.db")), beforeBootingRead);

  const readyDb = new DatabaseSync(path.join(home, "state.db"));
  try {
    readyDb.prepare("UPDATE provider_input_state SET state='ready' WHERE session_id='requester'").run();
  } finally { readyDb.close(); }
  const beforeReadyRead = await fs.readFile(path.join(home, "state.db"));
  const ready = await readProviderInputState(home, "requester");
  assert.equal(providerInputIsReady(ready, "requester"), true);
  assert.equal(providerInputIsReady({ ...ready, generation: 0 }, "requester"), false);
  assert.equal(providerInputIsReady(ready, "another-requester"), false);
  assert.deepEqual(await fs.readFile(path.join(home, "state.db")), beforeReadyRead);
});

test("lease predicate matches exact requester identity, optional session, and ordinary expiry", () => {
  assert.equal(activeConversationLeaseConflicts(
    [lease({ agent_id: "requester", resume_session: "" })], "requester", "", LEASE_CHECK_TIME,
  ).length, 1, "The same native agent conflicts even when it has no resume id");
  assert.equal(activeConversationLeaseConflicts(
    [lease({ agent_id: "another-agent", resume_session: "another-session" })],
    "requester", "", LEASE_CHECK_TIME,
  ).length, 0, "An unrelated agent and absent session do not collide");
  assert.equal(activeConversationLeaseConflicts(
    [lease({ agent_id: "another-agent", resume_session: "requester-session" })],
    "requester", "requester-session", LEASE_CHECK_TIME,
  ).length, 1, "A nonempty exact resume id conflicts across agent ids");
  assert.equal(activeConversationLeaseConflicts(
    [lease({ agent_id: "requester", expires_at: "2026-10-24T19:29:59Z" })],
    "requester", "", LEASE_CHECK_TIME,
  ).length, 0, "Expired ordinary rows do not block");
  assert.equal(activeConversationLeaseConflicts(
    [lease({ agent_id: "another-agent", resume_session: "requester-session",
      expires_at: "2026-10-24T19:29:59Z" })],
    "requester", "requester-session", LEASE_CHECK_TIME,
  ).length, 0, "Expired ordinary rows do not block by session either");
  assert.throws(() => activeConversationLeaseConflicts([], "", "", LEASE_CHECK_TIME), /nonempty requester id/u);
});

test("uncertain previous-provider holds are indefinite only for an exact nonempty session", () => {
  const hold = lease({
    agent_id: "another-agent",
    resume_session: "held-session",
    owner_kind: "prior_provider_hold",
    owner_id: "hold-1",
    mode: "uncertain_previous_provider",
    expires_at: "2020-01-01T00:00:00Z",
  });
  assert.equal(activeConversationLeaseConflicts([hold], "requester", "held-session", LEASE_CHECK_TIME).length, 1);
  assert.equal(activeConversationLeaseConflicts([hold], "requester", "different-session", LEASE_CHECK_TIME).length, 0);
  assert.equal(activeConversationLeaseConflicts([hold], "another-agent", "", LEASE_CHECK_TIME).length, 0,
    "An uncertain hold is not an agent-wide TTL lease");
});

test("lease snapshot reads are missing-as-empty, read-only, hashed, and owner-bounded", async (t) => {
  const home = await fixture(t);
  await assert.rejects(readConversationLeaseSnapshot(home, "", "", LEASE_CHECK_TIME), /nonempty requester id/u);
  const missing = await readConversationLeaseSnapshot(home, "requester", "", LEASE_CHECK_TIME);
  assert.equal(missing.exists, false);
  assert.equal(missing.has_active_conflict, false);
  assert.equal(missing.lease_count, 0);
  assert.equal(missing.snapshot_sha256,
    "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
  assert.deepEqual(await fs.readdir(home), [], "A missing lease file remains missing after observation");

  const leasePath = await writeLeaseFile(home, { schema: 1, leases: [lease({
    agent_id: "requester", resume_session: "", owner_id: "startup-owner",
  })] });
  const before = await fs.readFile(leasePath);
  const snapshot = await readConversationLeaseSnapshot(home, "requester", "", LEASE_CHECK_TIME);
  assert.equal(snapshot.has_active_conflict, true);
  assert.equal(snapshot.active_conflict_count, 1);
  assert.equal(snapshot.active_conflicts[0].owner_kind, "agent_lifecycle");
  assert.equal(snapshot.active_conflicts[0].owner_id, "startup-owner");
  assert.equal(snapshot.active_conflicts[0].mode, "lifecycle_transition");
  assert.equal(snapshot.active_conflicts[0].agent_id, "requester");
  assert.equal(snapshot.active_conflicts[0].provider, "antigravity");
  assert.equal(snapshot.active_conflicts[0].resume_session, "");
  assert.equal(snapshot.active_conflicts[0].expires_at, "2026-10-24T20:00:00Z");
  assert.equal("acquisition_id" in snapshot.active_conflicts[0], false);
  assert.equal(snapshot.snapshot_sha256.length, 64);
  assert.deepEqual(await fs.readFile(leasePath), before, "Observation must not alter the authoritative file");

  const evidence = { first_blocking: null, last_blocking: null, final_snapshot: null };
  recordConversationLeaseObservation(evidence, snapshot);
  const clear = await readConversationLeaseSnapshot(home, "other-agent", "", LEASE_CHECK_TIME);
  recordConversationLeaseObservation(evidence, clear);
  await writeLeaseFile(home, { schema: 1, leases: [lease({
    agent_id: "requester", resume_session: "", owner_id: "later-startup-owner",
  })] });
  const laterBlocking = await readConversationLeaseSnapshot(
    home, "requester", "", LEASE_CHECK_TIME + 1_000,
  );
  recordConversationLeaseObservation(evidence, laterBlocking);
  assert.equal(evidence.first_blocking.observed_at, snapshot.observed_at);
  assert.equal(evidence.last_blocking.snapshot_sha256, laterBlocking.snapshot_sha256);
  assert.equal(evidence.last_blocking.active_conflicts[0].owner_id, "later-startup-owner");
  assert.equal(evidence.final_snapshot.has_active_conflict, true);
  assert.equal("poll_history" in evidence, false);

  await writeLeaseFile(home, { schema: 1, leases: Array.from({ length: 6 }, (_, index) => lease({
    agent_id: "requester", resume_session: "", owner_id: `owner-${index}`,
  })) });
  const capped = await readConversationLeaseSnapshot(home, "requester", "", LEASE_CHECK_TIME);
  assert.equal(capped.active_conflict_count, 6);
  assert.equal(capped.active_conflicts.length, 4);
  assert.equal(capped.omitted_active_conflicts, 2);
});

test("lease observer fails closed on malformed JSON, unknown schema, and invalid lease dates", async (t) => {
  const cases = [
    "{ malformed",
    { schema: 2, leases: [] },
    { schema: 1, leases: [lease({ expires_at: "not-a-date" })] },
    { schema: 1, leases: [lease({ started_at: "2026-02-30T00:00:00Z" })] },
  ];
  for (const input of cases) {
    const home = await fixture(t);
    const leasePath = await writeLeaseFile(home, input);
    const before = await fs.readFile(leasePath);
    await assert.rejects(readConversationLeaseSnapshot(home, "requester", "", LEASE_CHECK_TIME), (error) => {
      assert.ok(error.lease_snapshot);
      assert.equal(error.lease_snapshot.valid, false);
      assert.equal(error.lease_snapshot.snapshot_sha256.length, 64);
      return true;
    });
    assert.deepEqual(await fs.readFile(leasePath), before, "Fail-closed parsing must remain read-only");
  }
});

test("core failure reporting preserves the first error and does not mask it with report I/O", async () => {
  const firstError = new Error("conversation_leased");
  const report = { cases: { delayed_reply: { status: "running", initial_prompt_attempts: 1 } } };
  const returnedError = await persistAutomaticReplyFailure(report, "delayed_reply", firstError, async () => {
    throw new Error("report disk unavailable");
  });
  assert.equal(returnedError, firstError);
  assert.equal(report.cases.delayed_reply.status, "fail");
  assert.equal(report.cases.delayed_reply.error, "conversation_leased");
  assert.equal(report.error, "conversation_leased");
  assert.equal(report.report_persistence_error, "report disk unavailable");
});

test("initial prompt delivery accepts only an accepted receipt for the exact requester", () => {
  assert.doesNotThrow(() => assertPromptDeliveryAccepted(
    { uuid: "requester", provider: "antigravity", delivery_state: "provider_accepted" },
    "requester", "antigravity",
  ));
  assert.doesNotThrow(() => assertPromptDeliveryAccepted(
    { uuid: "requester", provider: "antigravity", delivery_state: "queued" },
    "requester", "antigravity",
  ));
  for (const receipt of [
    { uuid: "requester", provider: "antigravity", delivery_state: "provider_input_not_ready" },
    { uuid: "requester", provider: "antigravity", delivery_state: "failed", error: { code: "provider_input_not_ready" } },
    { uuid: "other", provider: "antigravity", delivery_state: "provider_accepted" },
    { uuid: "requester", provider: "codex", delivery_state: "provider_accepted" },
  ]) assert.throws(() => assertPromptDeliveryAccepted(receipt, "requester", "antigravity"), assert.AssertionError);
});

test("observer binds pending and completed replies to one exact request without writing", async (t) => {
  const home = await fixture(t);
  database(home, (db) => task(db));
  const pending = await readCanonicalTaskReply(home, "requester", "worker", "task-1");
  assert.equal(pending.reply_count, 0);
  assert.equal(pending.task_status, "pending");
  assertRequesterOriginMatchesProviderInput(pending, "antigravity", "conversation-1");
  assert.throws(() => assertRequesterOriginMatchesProviderInput(pending, "codex", "conversation-1"),
    /provider changed/u);
  assert.throws(() => assertRequesterOriginMatchesProviderInput(pending, "antigravity", "other-conversation"),
    /captured provider conversation/u);
  const db = new DatabaseSync(path.join(home, "state.db"));
  try {
    db.exec("UPDATE interactions SET status='completed' WHERE id='task-1'");
    reply(db);
  } finally { db.close(); }
  const before = await fs.readFile(path.join(home, "state.db"));
  const proof = await readCanonicalTaskReply(home, "requester", "worker", "task-1");
  assert.equal(proof.reply_count, 1);
  assert.equal(proof.reply.status, "done");
  assert.equal(proof.reply.sender, "worker");
  assert.equal(proof.reply.parent_interaction_id, "task-1");
  assert.equal(proof.requester_reply_count, 0);
  assert.equal(proof.reply_to_reply_count, 0);
  assert.deepEqual(await fs.readFile(path.join(home, "state.db")), before);
  assert.deepEqual(await fs.readdir(home), ["state.db"]);
  await assert.rejects(readCanonicalTaskReply(home, "requester", "worker", "other-task"), /owned request changed/u);
});

test("observer rejects wrong recipients, extra tasks, duplicate replies, and spoofed reply origin", async (t) => {
  const cases = [
    (db) => task(db, { targets: ["foreign-worker"] }),
    (db) => { task(db); task(db, { id: "task-2" }); },
    (db) => { task(db); reply(db); reply(db, { id: "reply-2" }); },
    (db) => { task(db); reply(db, { sender: "foreign-worker" }); },
    (db) => { task(db); reply(db, { targets: ["foreign-requester"] }); },
    (db) => { task(db); reply(db); db.exec("DELETE FROM structured_replies"); },
  ];
  for (const configure of cases) {
    const home = await fixture(t);
    database(home, configure);
    await assert.rejects(readCanonicalTaskReply(home, "requester", "worker"), assert.AssertionError);
  }
});

test("observer exposes requester obligations and reply-to-reply loops to the acceptance oracle", async (t) => {
  const home = await fixture(t);
  database(home, (db) => {
    task(db); reply(db);
    row(db, { id: "loop", kind: "reply", sender: "requester", targets: ["worker"], parent: "reply-1" });
  });
  const proof = await readCanonicalTaskReply(home, "requester", "worker");
  assert.equal(proof.requester_reply_count, 1);
  assert.equal(proof.reply_to_reply_count, 1);
});

test("observer fails closed for missing requester origin and checks provider-input generation", async (t) => {
  const missingHome = await fixture(t);
  database(missingHome, (db) => row(db));
  await assert.rejects(readCanonicalTaskReply(missingHome, "requester", "worker"), /requester-origin row/u);

  const readyBeforePrompt = { session_id: "requester", state: "ready", generation: 7 };
  assert.equal(providerInputIsReady(readyBeforePrompt, "requester"), true);
  const advancedHome = await fixture(t);
  database(advancedHome, (db) => task(db, { requesterGeneration: 8, providerInputGeneration: 8 }));
  const advanced = await readCanonicalTaskReply(advancedHome, "requester", "worker");
  assertRequesterOriginMatchesProviderInput(advanced, "antigravity", "conversation-1");

  const staleHome = await fixture(t);
  database(staleHome, (db) => task(db, { requesterGeneration: 6, providerInputGeneration: 7 }));
  const stale = await readCanonicalTaskReply(staleHome, "requester", "worker");
  assert.throws(() => assertRequesterOriginMatchesProviderInput(stale, "antigravity", "conversation-1"),
    /match the current persisted provider-input generation/u);

  const missingGenerationHome = await fixture(t);
  database(missingGenerationHome, (db) => task(db, { providerInputGeneration: null }));
  const missingGeneration = await readCanonicalTaskReply(missingGenerationHome, "requester", "worker");
  assert.throws(() => assertRequesterOriginMatchesProviderInput(
    missingGeneration, "antigravity", "conversation-1",
  ), /current provider-input generation/u);
});

test("deadlines preserve the original absolute limit and require a positive cleanup reserve", () => {
  assert.deepEqual(automaticReplyDeadlines("2030-01-01T00:00:10Z", 2_000, Date.parse("2030-01-01T00:00:00Z")), {
    deadlineAt: Date.parse("2030-01-01T00:00:10Z"),
    workDeadlineAt: Date.parse("2030-01-01T00:00:08Z"), cleanupReserveMs: 2_000,
  });
  for (const [date, reserve, now] of [
    [undefined, 1, 0], ["2030-01-01T00:00:10", 1, 0], ["invalidZ", 1, 0],
    ["2030-01-01T00:00:10Z", 0, 0], ["2030-01-01T00:00:10Z", 1.5, 0],
    ["2030-01-01T00:00:10Z", 2_000, Date.parse("2030-01-01T00:00:08Z")],
  ]) assert.throws(() => automaticReplyDeadlines(date, reserve, now), assert.AssertionError);
});

test("native evidence requires exact direct workspace ownership and both completion signals", () => {
  const cascade = { id: "owned", ownership_basis: "workspace_uri", records: [] };
  assert.equal(exactCurrentCascade({ provider: "antigravity", sessions: [cascade] }, "owned"), cascade);
  for (const snapshot of [
    { provider: "codex", sessions: [cascade] },
    { provider: "antigravity", sessions: [cascade, cascade] },
    { provider: "antigravity", sessions: [{ ...cascade, id: "foreign" }] },
    { provider: "antigravity", sessions: [{ ...cascade, ownership_basis: "workspace_alias" }] },
  ]) assert.throws(() => exactCurrentCascade(snapshot, "owned"), assert.AssertionError);
  const complete = { role: "assistant", complete: true, native_status: 3 };
  assert.deepEqual(completedAssistantRows([
    complete, { ...complete, complete: false }, { ...complete, native_status: 2 }, { ...complete, role: "user" },
  ]), [complete]);
});

test("worker gate discloses one unpredictable token only after operator release", async (t) => {
  const home = await fixture(t);
  const readyPath = path.join(home, "ready");
  const releasePath = path.join(home, "release");
  let settled = false;
  const pending = waitForReplyRelease({ readyPath, releasePath, deadlineAt: Date.now() + 3_000, pollMs: 5 });
  pending.then(() => { settled = true; }, () => { settled = true; });
  const readyDeadline = Date.now() + 2_000;
  while (Date.now() < readyDeadline) {
    try { await fs.access(readyPath); break; }
    catch (error) { if (error.code !== "ENOENT") throw error; }
    await new Promise((resolve) => setTimeout(resolve, 5));
  }
  assert.equal(await fs.readFile(readyPath, "utf8"), "ready\n");
  assert.equal(settled, false);
  await assert.rejects(waitForReplyRelease({ readyPath, releasePath, deadlineAt: Date.now() + 1_000 }), /EEXIST/u);
  await fs.writeFile(releasePath, "release\n", { flag: "wx" });
  assert.match(await pending, /^WORKER_ONLY_[0-9a-f-]{36}$/u);
  assert.equal(await fs.readFile(readyPath + ".consumed", "utf8"), "consumed\n");
  await assert.rejects(waitForReplyRelease({ readyPath, releasePath, deadlineAt: Date.now() + 1_000 }), /released before/u);
});

test("worker gate fails at the original deadline without release or token", async (t) => {
  const home = await fixture(t);
  const readyPath = path.join(home, "ready");
  const releasePath = path.join(home, "release");
  await assert.rejects(waitForReplyRelease({ readyPath, releasePath, deadlineAt: Date.now() + 30, pollMs: 5 }), /deadline expired/u);
  await assert.rejects(fs.access(readyPath + ".consumed"), /ENOENT/u);
  await assert.rejects(waitForReplyRelease({ readyPath, releasePath, deadlineAt: Date.now() - 1 }), /deadline expired/u);
});

async function providerLogFixture(t, { sessionsProjection = "plain" } = {}) {
  const owned = await ownedCodexFixture(t);
  const workspace = path.join(owned.wardianHome, "workspaces", "worker");
  await fs.mkdir(workspace, { recursive: true });
  const nativeSession = "c3a45678-e89b-42d3-a456-426614174001";
  const projectedSessions = path.join(owned.codexHome, "sessions");
  let codexHome = owned.codexHome;
  if (sessionsProjection === "matching" || sessionsProjection === "foreign") {
    codexHome = path.join(owned.wardianHome, "qualified-codex-home");
    const explicitSessions = path.join(codexHome, "sessions");
    const target = sessionsProjection === "matching"
      ? explicitSessions
      : path.join(owned.wardianHome, "foreign-codex-home", "sessions");
    await fs.mkdir(explicitSessions, { recursive: true });
    if (sessionsProjection === "foreign") await fs.mkdir(target, { recursive: true });
    try {
      await fs.symlink(target, projectedSessions, process.platform === "win32" ? "junction" : "dir");
    } catch (error) {
      if (["EPERM", "EACCES", "ENOTSUP"].includes(error.code)) {
        t.skip("This host does not permit the managed sessions projection fixture");
        return null;
      }
      throw error;
    }
  } else {
    await fs.mkdir(projectedSessions, { recursive: true });
  }
  const sessionsRoot = path.join(codexHome, "sessions");
  const rolloutDirectory = path.join(sessionsRoot, "2026", "10", "25");
  await fs.mkdir(rolloutDirectory, { recursive: true });
  const rolloutPath = path.join(rolloutDirectory, `rollout-${nativeSession}.jsonl`);
  const workspaceHeader = { type: "session_meta", payload: { id: nativeSession, cwd: workspace } };
  const initialBytes = Buffer.from(JSON.stringify(workspaceHeader) + "\n", "utf8");
  await fs.writeFile(rolloutPath, initialBytes, { flag: "wx" });
  return {
    ...owned, managedProfile: owned.codexHome, workspace, nativeSession, codexHome,
    sessionsRoot, rolloutPath, initialBytes,
  };
}

async function providerLogSource(fixtureValue, {
  file = fixtureValue.rolloutPath,
  bytes = fixtureValue.initialBytes,
  status = "complete",
  identity = null,
  anchorHash = null,
} = {}) {
  const stat = await fs.stat(file, { bigint: true });
  const committedOffset = BigInt(bytes.length);
  const start = committedOffset > 4096n ? committedOffset - 4096n : 0n;
  const anchorBytes = bytes.subarray(Number(start), Number(committedOffset));
  return {
    provider_source_key: `codex:session:${fixtureValue.nativeSession}`,
    path: file,
    native_identity: identity ?? {
      platform: process.platform === "win32" ? "windows" : "unix",
      primary: stat.dev.toString(),
      secondary: stat.ino.toString(),
    },
    committed_offset: committedOffset.toString(),
    continuity_anchor: {
      start: start.toString(),
      len: BigInt(anchorBytes.length).toString(),
      sha256: anchorHash ?? createHashForTest(anchorBytes),
    },
    normalizer: {},
    status,
  };
}

function createHashForTest(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

async function writeProviderCaptureDocument(fixtureValue, document) {
  const capturePath = path.join(
    fixtureValue.wardianHome, "agents", fixtureValue.agentId, "conversation-capture.json",
  );
  const nextPath = capturePath + ".next";
  const serialized = JSON.stringify(document, null, 2)
    .replace(/("(?:primary|secondary|committed_offset|start|len)": )"([0-9]+)"/gu, "$1$2");
  await fs.writeFile(nextPath, serialized + "\n", { flag: "wx" });
  await fs.rename(nextPath, capturePath);
  return capturePath;
}

async function writeProviderCapture(fixtureValue, sources) {
  return writeProviderCaptureDocument(fixtureValue, { provider_log_sources: sources });
}

async function readProviderEvidence(fixtureValue, previousBinding = null) {
  return readAutomaticReplyProviderLogEvidence({
    wardianHome: fixtureValue.wardianHome,
    agentId: fixtureValue.agentId,
    workspace: fixtureValue.workspace,
    codexHome: fixtureValue.codexHome,
    nativeSession: fixtureValue.nativeSession,
    previousBinding,
  });
}

test("provider evidence waits for the app locator and never discovers rollout files by scanning", async (t) => {
  const owned = await providerLogFixture(t);
  if (!owned) return;
  assert.deepEqual(await readProviderEvidence(owned), { status: "pending", binding: null, evidence: null });
  await writeProviderCaptureDocument(owned, {});
  assert.deepEqual(await readProviderEvidence(owned), { status: "pending", binding: null, evidence: null },
    "Rust may omit the default-empty provider_log_sources field before the first locator");
  await writeProviderCaptureDocument(owned, { provider_log_sources: {} });
  await assert.rejects(readProviderEvidence(owned), /provider-log source list is malformed/u);

  const staleSource = await providerLogSource(owned);
  staleSource.provider_source_key = "codex:session:aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
  await writeProviderCapture(owned, [staleSource]);
  assert.deepEqual(await readProviderEvidence(owned), { status: "pending", binding: null, evidence: null });

  const currentSource = await providerLogSource(owned);
  await writeProviderCapture(owned, [currentSource, currentSource]);
  await assert.rejects(readProviderEvidence(owned), /duplicate entries for the current Codex source/u);
  await writeProviderCapture(owned, [currentSource]);
  const result = await readProviderEvidence(owned);
  assert.equal(result.status, "ready");
  assert.equal(result.binding.provider_source_key, `codex:session:${owned.nativeSession}`);
  assert.equal(result.evidence.source, "conversation_capture_provider_log");
  assert.equal(result.evidence.source_file, await fs.realpath(owned.rolloutPath));
  assert.equal(result.evidence.attempts.length, 0);
});

test("provider evidence accepts only the exact qualified sessions projection, including a namespaced rollout path", async (t) => {
  const matching = await providerLogFixture(t, { sessionsProjection: "matching" });
  if (!matching) return;
  const namespacedFile = process.platform === "win32"
    ? path.win32.toNamespacedPath(matching.rolloutPath)
    : matching.rolloutPath;
  await writeProviderCapture(matching, [await providerLogSource(matching, { file: namespacedFile })]);
  const accepted = await readProviderEvidence(matching);
  assert.equal(accepted.status, "ready");

  const replacementRoot = path.join(matching.wardianHome, "retargeted-codex-home", "sessions");
  await fs.mkdir(replacementRoot, { recursive: true });
  await fs.rm(path.join(matching.managedProfile, "sessions"), { recursive: true });
  await fs.symlink(replacementRoot,
    path.join(matching.managedProfile, "sessions"),
    process.platform === "win32" ? "junction" : "dir");
  await assert.rejects(readProviderEvidence(matching, accepted.binding), /exact qualified CODEX_HOME root/u);

  const foreign = await providerLogFixture(t, { sessionsProjection: "foreign" });
  if (!foreign) return;
  await writeProviderCapture(foreign, [await providerLogSource(foreign)]);
  await assert.rejects(readProviderEvidence(foreign), /exact qualified CODEX_HOME root/u);
});

test("provider evidence pins one rollout identity and permits only same-file committed-anchor advance", async (t) => {
  const owned = await providerLogFixture(t);
  if (!owned) return;
  const pendingSource = await providerLogSource(owned, { status: "pending" });
  await writeProviderCapture(owned, [pendingSource]);
  const first = await readProviderEvidence(owned);
  assert.equal(first.status, "pending");
  assert.ok(first.binding);
  const capturePath = path.join(owned.wardianHome, "agents", owned.agentId, "conversation-capture.json");
  await writeProviderCaptureDocument(owned, {});
  await assert.rejects(readProviderEvidence(owned, first.binding), /bound Codex provider-log locator disappeared/u);
  await fs.unlink(capturePath);
  await assert.rejects(readProviderEvidence(owned, first.binding), /bound Codex provider-log locator disappeared/u);
  await writeProviderCapture(owned, [pendingSource]);
  const rebound = await readProviderEvidence(owned, first.binding);
  assert.equal(rebound.status, "pending");
  assert.deepEqual(rebound.binding, first.binding,
    "The same app locator may resume without changing the pinned source identity");

  const committedRow = Buffer.from(JSON.stringify({
    type: "response_item",
    timestamp: "2026-10-25T00:00:00.000Z",
    payload: { type: "function_call", name: "exec_command", call_id: "committed-call", arguments: "{}" },
  }) + "\n", "utf8");
  const uncommittedRow = Buffer.from(JSON.stringify({
    type: "response_item",
    timestamp: "2026-10-25T00:00:01.000Z",
    payload: { type: "function_call", name: "exec_command", call_id: "uncommitted-call", arguments: "{}" },
  }) + "\n", "utf8");
  await fs.appendFile(owned.rolloutPath, Buffer.concat([committedRow, uncommittedRow]));
  const committedBytes = Buffer.concat([owned.initialBytes, committedRow]);
  await writeProviderCapture(owned, [await providerLogSource(owned, { bytes: committedBytes })]);
  const advanced = await readProviderEvidence(owned, rebound.binding);
  assert.equal(advanced.status, "ready");
  assert.equal(advanced.evidence.attempts.length, 1,
    "Rows beyond Wardian's committed cursor must not become execution evidence");
  assert.equal(advanced.evidence.attempts[0].call_id, "committed-call");
  assert.equal(advanced.evidence.source_bytes, committedBytes.length);
  assert.equal(advanced.evidence.observed_file_bytes,
    owned.initialBytes.length + committedRow.length + uncommittedRow.length);

  const replacementDirectory = path.join(owned.sessionsRoot, "2026", "10", "26");
  await fs.mkdir(replacementDirectory, { recursive: true });
  const replacementPath = path.join(replacementDirectory, path.basename(owned.rolloutPath));
  await fs.copyFile(owned.rolloutPath, replacementPath);
  const replacement = await providerLogSource(owned, { file: replacementPath, bytes: committedBytes });
  await writeProviderCapture(owned, [replacement]);
  await assert.rejects(readProviderEvidence(owned, advanced.binding), /bound Codex provider-log path changed/u);
});

test("provider evidence rejects unsafe native identity numbers and broken continuity anchors", async (t) => {
  const owned = await providerLogFixture(t);
  if (!owned) return;
  const source = await providerLogSource(owned);
  source.native_identity.secondary = "18446744073709551615";
  await writeProviderCapture(owned, [source]);
  await assert.rejects(readProviderEvidence(owned), /native file identity changed/u);

  const badAnchor = await providerLogSource(owned);
  badAnchor.continuity_anchor.sha256 = "0".repeat(64);
  await writeProviderCapture(owned, [badAnchor]);
  await assert.rejects(readProviderEvidence(owned), /continuity anchor/u);
});

test("provider evidence binds rollout session metadata to the current session and worker workspace", async (t) => {
  const owned = await providerLogFixture(t);
  if (!owned) return;
  const foreignSession = Buffer.from(JSON.stringify({
    type: "session_meta", payload: { id: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa", cwd: owned.workspace },
  }) + "\n", "utf8");
  await fs.writeFile(owned.rolloutPath, foreignSession);
  await writeProviderCapture(owned, [await providerLogSource(owned, { bytes: foreignSession })]);
  await assert.rejects(readProviderEvidence(owned), /foreign native session/u);

  const foreignWorkspace = path.join(owned.wardianHome, "workspaces", "foreign");
  await fs.mkdir(foreignWorkspace, { recursive: true });
  const foreignCwd = Buffer.from(JSON.stringify({
    type: "session_meta", payload: { id: owned.nativeSession, cwd: foreignWorkspace },
  }) + "\n", "utf8");
  await fs.writeFile(owned.rolloutPath, foreignCwd);
  await writeProviderCapture(owned, [await providerLogSource(owned, { bytes: foreignCwd })]);
  await assert.rejects(readProviderEvidence(owned), /foreign worker workspace/u);
});
