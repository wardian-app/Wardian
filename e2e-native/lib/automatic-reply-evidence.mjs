import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { createReadStream } from "node:fs";
import fs from "node:fs/promises";
import path from "node:path";

const CONVERSATION_LEASE_MODES = new Set([
  "background_resume", "background_fresh", "lifecycle_transition", "uncertain_previous_provider",
]);
const MAX_REPORTED_LEASE_CONFLICTS = 4;

function samePath(left, right, platform = process.platform) {
  if (platform === "win32") {
    const normalizedLeft = path.win32.toNamespacedPath(path.win32.resolve(left)).toLowerCase();
    const normalizedRight = path.win32.toNamespacedPath(path.win32.resolve(right)).toLowerCase();
    return normalizedLeft === normalizedRight;
  }
  return path.resolve(left) === path.resolve(right);
}

function generatedTomlSection(config, name) {
  const lines = config.split(/\r?\n/u);
  const header = new RegExp(`^\\s*\\[${name.replaceAll(".", "\\.")}\\]\\s*(?:#.*)?$`, "u");
  const matches = lines.flatMap((line, index) => header.test(line) ? [index] : []);
  assert.equal(matches.length, 1, `Expected exactly one generated TOML section ${name}`);
  const [start] = matches;
  let end = lines.findIndex((line, index) => index > start && /^\s*\[/u.test(line));
  if (end === -1) end = lines.length;
  return lines.slice(start + 1, end).join("\n");
}

function generatedTomlValue(section, key) {
  const matches = section.split("\n").filter((entry) =>
    new RegExp(`^\\s*${key}\\s*=`, "u").test(entry));
  assert.equal(matches.length, 1, `Expected exactly one generated TOML field ${key}`);
  const [line] = matches;
  const value = line.slice(line.indexOf("=") + 1).trim();
  if (/^'[^'\r\n]*'$/u.test(value)) return value.slice(1, -1);
  return JSON.parse(value);
}

function hasReplyToolPolicy(config) {
  let section = "";
  for (const line of config.split(/\r?\n/u)) {
    const trimmed = line.trim();
    if (!trimmed || trimmed.startsWith("#")) continue;
    if (trimmed.startsWith("[")) {
      const header = trimmed.split("#", 1)[0].trim()
        .replace(/^\[+|\]+$/gu, "").replace(/[\s"']/gu, "");
      section = header;
      if (header === "mcp_servers.wardian.tools" ||
        header.startsWith("mcp_servers.wardian.tools.reply") ||
        (header.startsWith("mcp_servers.wardian.tools.") && header.slice("mcp_servers.wardian.tools.".length).includes("."))) {
        return true;
      }
      continue;
    }

    const normalized = trimmed.replace(/[\s"']/gu, "");
    const assignment = normalized.split("=", 1)[0];
    if ((section === "mcp_servers.wardian" && assignment.startsWith("tools")) ||
      normalized.startsWith("mcp_servers.wardian.tools.reply") ||
      normalized.startsWith("mcp_servers.wardian.tools=") ||
      (section === "mcp_servers" && assignment.startsWith("wardian.tools"))) {
      return true;
    }
  }
  return false;
}

function assertPlainPath(stat, kind, label) {
  assert.equal(stat.isSymbolicLink(), false, `${label} must not be a link`);
  assert.equal(kind === "directory" ? stat.isDirectory() : stat.isFile(), true,
    `${label} must be a regular ${kind}`);
}

async function lstatPlain(pathname, kind, label) {
  const stat = await fs.lstat(pathname);
  assertPlainPath(stat, kind, label);
  assert.ok(samePath(await fs.realpath(pathname), pathname), `${label} resolved through a link`);
  return stat;
}

function assertSameFileIdentity(expected, actual, label) {
  assert.equal(actual.isSymbolicLink(), false, `${label} became a link`);
  assert.equal(actual.dev, expected.dev, `${label} device changed during fixture preparation`);
  assert.equal(actual.ino, expected.ino, `${label} identity changed during fixture preparation`);
}

async function readHandleUtf8(handle) {
  const { size } = await handle.stat();
  const bytes = Buffer.alloc(size);
  let offset = 0;
  while (offset < bytes.length) {
    const { bytesRead } = await handle.read(bytes, offset, bytes.length - offset, offset);
    assert.ok(bytesRead > 0, "Managed Codex config changed while it was being read");
    offset += bytesRead;
  }
  return bytes.toString("utf8");
}

async function writeHandleUtf8(handle, text) {
  const bytes = Buffer.from(text, "utf8");
  let offset = 0;
  while (offset < bytes.length) {
    const { bytesWritten } = await handle.write(bytes, offset, bytes.length - offset, offset);
    assert.ok(bytesWritten > 0, "Managed Codex config write made no progress");
    offset += bytesWritten;
  }
  await handle.truncate(bytes.length);
  await handle.sync();
}

function windowsEnvironmentValue(env, name) {
  const matches = Object.entries(env ?? {}).filter(([key]) => key.toLowerCase() === name.toLowerCase());
  assert.ok(matches.length <= 1, "The child environment has ambiguous " + name + " keys");
  return matches[0]?.[1];
}

function normalizedWindowsPathSegment(value) {
  return value.trim().replace(/^"+|"+$/gu, "").replaceAll("/", "\\").replace(/\\+$/u, "").toLowerCase();
}

function windowsChildPath(wardianHome, inheritedPath) {
  const bin = path.win32.join(wardianHome, "bin");
  const normalizedBin = normalizedWindowsPathSegment(bin);
  const inherited = (inheritedPath ?? "").split(";")
    .map((segment) => segment.trim())
    .filter(Boolean)
    .filter((segment) => normalizedWindowsPathSegment(segment) !== normalizedBin);
  const segments = [bin, ...inherited];
  for (const segment of segments) {
    assert.ok(!segment.includes('"'), "The effective Windows child PATH contains a quoted segment");
    assert.ok(path.win32.isAbsolute(segment), "The effective Windows child PATH contains a relative segment");
  }
  return { value: segments.join(";"), segments };
}

function windowsPathExtensions(value) {
  assert.ok(typeof value === "string" && value.trim(), "The child PATHEXT is required for Windows qualification");
  const extensions = value.split(";").map((extension) => extension.trim().toLowerCase()).filter(Boolean);
  assert.ok(extensions.length > 0 && extensions.every((extension) => /^\.[a-z0-9]+$/u.test(extension)),
    "The child PATHEXT contains an unsupported extension");
  assert.equal(new Set(extensions).size, extensions.length, "The child PATHEXT contains duplicate extensions");
  return extensions;
}

async function hashStableRegularFile(filePath, label, { allowParentLinks = false } = {}) {
  const before = await fs.lstat(filePath);
  assertPlainPath(before, "file", label);
  const realPath = await fs.realpath(filePath);
  if (!allowParentLinks) {
    assert.ok(samePath(realPath, filePath, "win32"), label + " does not resolve to its exact path");
  }
  const hash = createHash("sha256");
  for await (const chunk of createReadStream(realPath)) hash.update(chunk);
  const after = await fs.lstat(filePath);
  assertSameFileIdentity(before, after, label);
  assert.equal(after.size, before.size, label + " size changed during qualification");
  assert.equal(after.mtimeMs, before.mtimeMs, label + " changed during qualification");
  return { path: filePath, realpath: realPath, sha256: hash.digest("hex") };
}

async function firstWindowsCommand(pathEntries, command, extensions) {
  for (const directory of pathEntries) {
    for (const suffix of ["", ...extensions]) {
      const candidate = path.win32.join(directory, command + suffix);
      try {
        const stat = await fs.lstat(candidate);
        assertPlainPath(stat, "file", "Resolved " + command + " command");
        const realPath = await fs.realpath(candidate);
        return realPath;
      } catch (error) {
        if (error.code === "ENOENT" || error.code === "ENOTDIR") continue;
        throw error;
      }
    }
  }
  return null;
}

async function assertNoWorkspaceCommandShadow(workspace, command, extensions) {
  for (const suffix of ["", ...extensions]) {
    const candidate = path.win32.join(workspace, command + suffix);
    try {
      const stat = await fs.lstat(candidate);
      assertPlainPath(stat, "file", "Workspace " + command + " command candidate");
      assert.fail("The worker workspace contains a " + command + " command shadow");
    } catch (error) {
      if (error.code === "ENOENT" || error.code === "ENOTDIR") continue;
      throw error;
    }
  }
}

/**
 * Qualify the effective Windows Codex child PATH without changing it or exposing its entries.
 * Non-Windows fixtures keep their existing portable execution path.
 */
export async function qualifyAutomaticReplyExecEnvironment({
  platform = process.platform,
  wardianHome,
  workerWorkspace,
  env = process.env,
  nodeExecutable = process.execPath,
} = {}) {
  if (platform !== "win32") return null;

  assert.ok(typeof wardianHome === "string" && path.win32.isAbsolute(wardianHome),
    "The isolated Wardian home must be absolute on Windows");
  assert.ok(typeof workerWorkspace === "string" && path.win32.isAbsolute(workerWorkspace),
    "The worker workspace must be absolute on Windows");
  const configuredHome = windowsEnvironmentValue(env, "WARDIAN_HOME");
  assert.ok(typeof configuredHome === "string" && samePath(configuredHome, wardianHome, "win32"),
    "The child WARDIAN_HOME must match the isolated fixture home");
  const systemRoot = windowsEnvironmentValue(env, "SystemRoot");
  assert.ok(typeof systemRoot === "string" && path.win32.isAbsolute(systemRoot),
    "The child SystemRoot must be absolute");
  assert.ok(typeof nodeExecutable === "string" && path.win32.isAbsolute(nodeExecutable),
    "The exact Node executable path must be absolute");

  const effectivePath = windowsChildPath(wardianHome, windowsEnvironmentValue(env, "Path"));
  const pathext = windowsEnvironmentValue(env, "PATHEXT");
  const extensions = windowsPathExtensions(pathext);
  const systemRootStat = await fs.lstat(systemRoot);
  assertPlainPath(systemRootStat, "directory", "SystemRoot");
  const systemRootReal = await fs.realpath(systemRoot);
  assert.ok(samePath(systemRootReal, systemRoot, "win32"), "SystemRoot must resolve to its configured directory");
  const workspaceStat = await fs.lstat(workerWorkspace);
  assertPlainPath(workspaceStat, "directory", "Worker workspace");
  const workspaceReal = await fs.realpath(workerWorkspace);
  assert.ok(samePath(workspaceReal, workerWorkspace, "win32"), "The worker workspace must be canonical");
  const shellPath = path.win32.join(systemRoot, "System32", "cmd.exe");
  const [shell, node] = await Promise.all([
    hashStableRegularFile(shellPath, "System32 cmd.exe"),
    hashStableRegularFile(nodeExecutable, "Node executable", { allowParentLinks: true }),
  ]);
  await Promise.all([
    assertNoWorkspaceCommandShadow(workspaceReal, "cmd", extensions),
    assertNoWorkspaceCommandShadow(workspaceReal, "node", extensions),
  ]);
  const [resolvedCmd, resolvedNode] = await Promise.all([
    firstWindowsCommand(effectivePath.segments, "cmd", extensions),
    firstWindowsCommand(effectivePath.segments, "node", extensions),
  ]);
  assert.ok(samePath(shell.realpath, shell.path, "win32")
    && resolvedCmd && samePath(resolvedCmd, shell.realpath, "win32"),
    "The effective child PATH resolves cmd to the exact System32 executable");
  assert.ok(resolvedNode && samePath(resolvedNode, node.realpath, "win32"),
    "The effective child PATH resolves Node to the exact fixture executable");

  return {
    shell: shell.path,
    node: node.path,
    node_realpath: node.realpath,
    shell_sha256: shell.sha256,
    node_sha256: node.sha256,
    child_path: {
      sha256: createHash("sha256").update(effectivePath.value, "utf8").digest("hex"),
      pathext_sha256: createHash("sha256").update(pathext, "utf8").digest("hex"),
      entry_count: effectivePath.segments.length,
      cmd_resolution: "system32",
      node_resolution: "exact",
      workspace_command_shadows: 0,
      wardian_bin_precedence: true,
    },
  };
}

/** Resolve only the real System32 Cmd location for Windows Codex shell calls. */
export function automaticReplyExecShell(platform = process.platform, systemRoot = process.env.SystemRoot) {
  if (platform !== "win32") return null;
  assert.ok(typeof systemRoot === "string" && path.win32.isAbsolute(systemRoot),
    "Windows SystemRoot must be absolute before selecting the fixture shell");
  const shell = path.win32.join(systemRoot, "System32", "cmd.exe");
  assert.ok(!/\\WindowsApps(?:\\|$)/iu.test(shell), "The fixture shell must not resolve through WindowsApps");
  return shell;
}

/** Generate the only JavaScript wrapper accepted for the reply gate. */
export function automaticReplyExecScript(gateCommand, shell) {
  assert.ok(typeof gateCommand === "string" && gateCommand.trim(), "The worker gate command is required");
  if (shell !== null) {
    assert.ok(typeof shell === "string" && path.win32.isAbsolute(shell)
      && /\\System32\\cmd\.exe$/iu.test(shell), "Only an absolute System32 Cmd shell is supported");
    assert.ok(!/\\WindowsApps(?:\\|$)/iu.test(shell), "The fixture shell must not resolve through WindowsApps");
  }
  const shellArgument = shell === null ? "" : `,shell:${JSON.stringify(shell)}`;
  return `let result=await tools.exec_command({cmd:${JSON.stringify(gateCommand)}${shellArgument},yield_time_ms:1000});` +
    "text(JSON.stringify(result));while(result.session_id){" +
    "result=await tools.write_stdin({session_id:result.session_id,chars:'',yield_time_ms:1000});" +
    "text(JSON.stringify(result));}";
}

/** Keep the qualified shell and literal gate wrapper explicit in the sole initial worker task. */
export function automaticReplyTaskInstructions(gateCommand, shell = null, systemRoot = process.env.SystemRoot) {
  assert.ok(typeof gateCommand === "string" && gateCommand.trim(), "The worker gate command is required");
  if (shell !== null) {
    const expectedShell = automaticReplyExecShell("win32", systemRoot);
    assert.ok(typeof shell === "string" && samePath(shell, expectedShell, "win32"),
      "Only the verified absolute System32 Cmd shell is supported");
  }
  const execution = `Use exactly one functions.exec request with this exact JavaScript input (outer whitespace only may vary):\n` +
    automaticReplyExecScript(gateCommand, shell) + "\nDo not alter or replace it.";
  return `${execution} Wait for it to print one WORKER_ONLY_ UUID after the external test gate opens. ` +
    "Then call the Wardian reply tool once with status done and exactly that printed token. Do no other work.";
}

/** Build the one gate command with Cmd-safe quoting for an exact absolute Node executable. */
export function automaticReplyGateCommand(deadlineAt, {
  nodeExecutable = process.execPath,
  platform = process.platform,
} = {}) {
  assert.ok(Number.isSafeInteger(deadlineAt), "The worker gate deadline must be an absolute millisecond timestamp");
  const deadlineDate = new Date(deadlineAt);
  assert.ok(Number.isFinite(deadlineDate.getTime()), "The worker gate deadline is outside the supported date range");
  const deadline = deadlineDate.toISOString();
  const script = ".wardian-e2e-gate/reply-gate.mjs";
  if (platform !== "win32") {
    return "node " + script + " --deadline " + JSON.stringify(deadline);
  }
  assert.ok(typeof nodeExecutable === "string" && path.win32.isAbsolute(nodeExecutable),
    "The exact Windows Node executable path must be absolute");
  assert.ok(!/[%!^&|<>()"\r\n]/u.test(nodeExecutable),
    "The Windows Node executable path contains a Cmd metacharacter");
  return "\"" + nodeExecutable + "\" " + script + " --deadline " + deadline;
}

/** Assert the exact gate request from its native event representation without evaluating code. */
export function assertAutomaticReplyGateInvocation(attempt, { gateCommand, shell } = {}) {
  assert.ok(attempt && typeof attempt === "object", "A captured native exec attempt is required");
  if (attempt.type === "custom_tool_call" && attempt.name === "exec") {
    assert.equal(typeof attempt.raw_input, "string", "The functions.exec input must be preserved as raw text");
    assert.equal(attempt.raw_input.trim(), automaticReplyExecScript(gateCommand, shell),
      "The captured functions.exec script differs from the exact reply gate wrapper");
    return true;
  }
  assert.equal(attempt.type, "function_call", "The captured tool call type is unsupported");
  assert.equal(attempt.name, "exec_command", "The captured tool is not exec_command");
  assert.equal(typeof attempt.raw_input, "string", "The exec_command arguments must be preserved as raw JSON");
  let args;
  try {
    args = JSON.parse(attempt.raw_input);
  } catch {
    assert.fail("The captured exec_command arguments are not valid JSON");
  }
  assert.ok(args && typeof args === "object" && !Array.isArray(args),
    "The captured exec_command arguments must be a JSON object");
  assert.equal(args.cmd, gateCommand, "The captured exec_command command differs from the reply gate");
  if (shell === null) {
    assert.equal(Object.hasOwn(args, "shell"), false, "The portable exec_command must omit the shell override");
  } else {
    assert.equal(args.shell, shell, "The captured exec_command shell differs from the qualified shell");
  }
  assert.equal(Object.hasOwn(args, "workdir"), false, "The captured exec_command must not override its workspace");
  assert.equal(Object.hasOwn(args, "cwd"), false, "The captured exec_command must not override its workspace");
  return true;
}

const NATIVE_TOOL_CALLS = new Map([
  ["function_call", { inputKey: "arguments", outputType: "function_call_output" }],
  ["custom_tool_call", { inputKey: "input", outputType: "custom_tool_call_output" }],
]);
const NATIVE_TOOL_OUTPUT_TYPES = new Set(["function_call_output", "custom_tool_call_output"]);

function waitCellId(rawInput) {
  if (typeof rawInput !== "string") return null;
  let args;
  try {
    args = JSON.parse(rawInput);
  } catch {
    return null;
  }
  if (!args || typeof args !== "object" || Array.isArray(args)) return null;
  const value = args.cell_id;
  if (typeof value === "string" && value.trim() === value && /^[A-Za-z0-9_-]{1,128}$/u.test(value)) return value;
  if (Number.isSafeInteger(value) && value >= 0) return String(value);
  return null;
}

function nativeTextOutput(value) {
  if (typeof value === "string") return value;
  if (!Array.isArray(value)) return null;
  const flattened = [];
  for (const block of value) {
    if (!block || typeof block !== "object" || Array.isArray(block)
      || !["text", "input_text"].includes(block.type) || typeof block.text !== "string") return null;
    flattened.push(block.text);
  }
  return flattened.join("\n");
}

function observedCellId(output) {
  const text = nativeTextOutput(output);
  if (text === null) return null;
  const markers = [...text.matchAll(/Script running with cell ID\b/gu)];
  assert.ok(markers.length <= 1, "An exec attempt reported duplicate functions.exec cell markers");
  if (markers.length === 0) return null;
  const suffix = text.slice(markers[0].index + markers[0][0].length);
  const id = suffix.match(/^[ \t]+([^\s\r\n]+)/u)?.[1];
  assert.ok(id && /^[A-Za-z0-9_-]{1,128}$/u.test(id), "The functions.exec cell ID is malformed");
  return id;
}

function nativeOutput(row, payload, rowOrdinal) {
  return {
    row_ordinal: rowOrdinal,
    timestamp: row.timestamp ?? null,
    raw: Object.hasOwn(payload, "output") ? payload.output : payload,
  };
}

function correlateNativeToolOutput(call, outputsById, label) {
  const matching = outputsById.get(call.call_id) ?? [];
  assert.ok(matching.length <= 1, `Duplicate ${label} outputs are ambiguous`);
  if (matching.length === 0) return null;
  const [entry] = matching;
  assert.equal(entry.payload.type, call.output_type, `${label} output type does not match its call`);
  assert.ok(entry.rowOrdinal > call.row_ordinal, `${label} output precedes its native call`);
  return nativeOutput(entry.row, entry.payload, entry.rowOrdinal);
}

function knownFunctionsExecBody(text, expectedCellId) {
  const lines = text.split(/\r?\n/u);
  const first = lines[0] ?? "";
  const running = first.match(/^Script running with cell ID ([A-Za-z0-9_-]{1,128})$/u);
  if (running) {
    const cellId = running[1];
    if (expectedCellId !== null) assert.equal(cellId, expectedCellId, "functions.wait returned a foreign cell");
    return { pending: true, body: functionsExecEnvelopeBody(lines) };
  }

  const completed = first === "Script completed";
  const failed = first === "Script failed";
  if (completed || failed) {
    assert.equal(failed, false, "The outer functions.exec wrapper failed");
    return { pending: false, body: functionsExecEnvelopeBody(lines) };
  }
  if (/^Script (?:running|completed|failed|error)\b/u.test(first)) {
    assert.fail("The functions.exec output contains an unsupported script envelope");
  }
  return { pending: false, body: text };
}

function functionsExecEnvelopeBody(lines) {
  const wallTime = lines[1] ?? "";
  assert.match(wallTime, /^Wall time \d+(?:\.\d+)? seconds$/u, "The functions.exec envelope is malformed");
  assert.equal(lines[2], "Output:", "The functions.exec envelope is missing Output");
  return lines.slice(3).join("\n");
}

function parseFunctionsExecResults(text) {
  const results = [];
  for (const line of text.split(/\r?\n/u)) {
    if (!line.trim()) continue;
    let result;
    try {
      result = JSON.parse(line);
    } catch {
      assert.fail("The functions.exec output contains a non-JSON result line");
    }
    assert.ok(result && typeof result === "object" && !Array.isArray(result),
      "A functions.exec result line must be a JSON object");
    assert.ok(typeof result.output === "string", "A functions.exec result must contain a text output chunk");
    const knownKeys = new Set([
      "output", "exit_code", "session_id", "wall_time_seconds", "original_token_count", "chunk_id", "error",
    ]);
    assert.ok(Object.keys(result).every((key) => knownKeys.has(key)),
      "A functions.exec result contains an unknown field");
    if (Object.hasOwn(result, "exit_code") && result.exit_code !== null) {
      assert.ok(Number.isInteger(result.exit_code), "A functions.exec exit code must be an integer");
    }
    if (Object.hasOwn(result, "session_id") && result.session_id !== null) {
      assert.ok(typeof result.session_id === "string" || Number.isSafeInteger(result.session_id),
        "A functions.exec session id must be a string or integer");
    }
    if (Object.hasOwn(result, "error") && result.error !== null) {
      assert.ok(typeof result.error === "string", "A functions.exec error must be text");
    }
    results.push(result);
  }
  return results;
}

function functionsExecOutputPending(output, cellId) {
  const text = nativeTextOutput(output.raw);
  assert.notEqual(text, null, "The captured functions.wait output has an unsupported text representation");
  const envelope = knownFunctionsExecBody(text, cellId);
  if (envelope.pending) return true;
  const results = envelope.body.trim() ? parseFunctionsExecResults(envelope.body) : [];
  return !results.some((result) => result.exit_code !== null && result.exit_code !== undefined)
    && results.some((result) => result.session_id !== null && result.session_id !== undefined);
}

/**
 * Preserve native exec input and correlate its output and ordered waits for an observed functions.exec cell.
 * Wait arguments are parsed only for cell_id; per-kind ordinals and row_ordinal retain event order.
 */
export function collectAutomaticReplyExecAttempts(rows) {
  assert.ok(Array.isArray(rows), "Native response rows must be an array");
  const attempts = [];
  const waitCalls = [];
  const callsById = new Map();
  const foreignCallIds = new Set();
  const replyCallIds = new Set();
  const outputsById = new Map();

  for (let rowOrdinal = 0; rowOrdinal < rows.length; rowOrdinal += 1) {
    const row = rows[rowOrdinal];
    if (row?.type !== "response_item" || !row.payload || typeof row.payload !== "object") continue;
    const payload = row.payload;
    if (NATIVE_TOOL_OUTPUT_TYPES.has(payload.type)) {
      if (typeof payload.call_id !== "string" || !payload.call_id) continue;
      const outputRows = outputsById.get(payload.call_id) ?? [];
      outputRows.push({ row, payload, rowOrdinal });
      outputsById.set(payload.call_id, outputRows);
      continue;
    }

    const spec = NATIVE_TOOL_CALLS.get(payload.type);
    const isExec = (payload.type === "function_call" && payload.name === "exec_command")
      || (payload.type === "custom_tool_call" && payload.name === "exec");
    const isWait = spec && payload.name === "wait";
    if (!spec || (!isExec && !isWait)) {
      if (spec && payload.name === "reply" && typeof payload.call_id === "string" && payload.call_id.trim()) {
        assert.ok(!callsById.has(payload.call_id) && !foreignCallIds.has(payload.call_id),
          "A reply tool call_id makes an exec attempt ambiguous");
        foreignCallIds.add(payload.call_id);
        replyCallIds.add(payload.call_id);
      } else if (spec && typeof payload.call_id === "string" && payload.call_id.trim()) {
        assert.ok(!callsById.has(payload.call_id), "A foreign tool call_id makes an exec attempt ambiguous");
        foreignCallIds.add(payload.call_id);
      }
      continue;
    }
    assert.ok(typeof payload.call_id === "string" && payload.call_id.trim(),
      "A captured exec or wait call must have its native call_id");
    assert.ok(!callsById.has(payload.call_id), isExec
      ? "Duplicate exec attempt call_id is ambiguous"
      : "Duplicate functions.wait call_id is ambiguous");
    assert.ok(!foreignCallIds.has(payload.call_id), "A foreign tool call_id makes an exec attempt ambiguous");
    const call = {
      call_id: payload.call_id,
      row_ordinal: rowOrdinal,
      timestamp: row.timestamp ?? null,
      type: payload.type,
      namespace: payload.namespace ?? null,
      name: payload.name,
      raw_input: payload[spec.inputKey] ?? null,
      output_type: spec.outputType,
    };
    callsById.set(payload.call_id, call);
    if (isExec) {
      assert.ok(typeof call.raw_input === "string", "An exec attempt must preserve its raw input string");
      const attempt = {
        ...call,
        ordinal: attempts.length + 1,
        output: null,
        cell_id: null,
        continuations: [],
      };
      attempts.push(attempt);
      call.attempt = attempt;
    } else {
      call.ordinal = waitCalls.length + 1;
      call.cell_id = waitCellId(call.raw_input);
      waitCalls.push(call);
    }
  }

  for (const call of callsById.values()) {
    call.output = correlateNativeToolOutput(call, outputsById, call.attempt ? "exec attempt" : "functions.wait");
    if (call.attempt) call.attempt.output = call.output;
  }

  const attemptsByCell = new Map();
  for (const attempt of attempts) {
    attempt.cell_id = observedCellId(attempt.output?.raw);
    if (attempt.cell_id === null) continue;
    assert.ok(!attemptsByCell.has(attempt.cell_id), "Duplicate functions.exec cell IDs are ambiguous");
    attemptsByCell.set(attempt.cell_id, attempt);
  }

  const waitsByCell = new Map();
  for (const call of waitCalls) {
    if (call.cell_id === null) continue;
    const matching = waitsByCell.get(call.cell_id) ?? [];
    matching.push(call);
    waitsByCell.set(call.cell_id, matching);
  }
  for (const [cellId, attempt] of attemptsByCell) {
    const matching = waitsByCell.get(cellId) ?? [];
    if (matching.length === 0) continue;
    matching.sort((left, right) => left.row_ordinal - right.row_ordinal);
    let previousOutput = attempt.output;
    for (let index = 0; index < matching.length; index += 1) {
      const wait = matching[index];
      assert.ok(previousOutput && wait.row_ordinal > previousOutput.row_ordinal,
        "functions.wait must follow the previous output for its cell");
      if (index > 0) {
        assert.equal(functionsExecOutputPending(previousOutput, cellId), true,
          "A repeated functions.wait requires the same cell to remain pending");
      }
      attempt.continuations.push({
        exec_call_id: attempt.call_id,
        cell_id: cellId,
        call_id: wait.call_id,
        ordinal: wait.ordinal,
        row_ordinal: wait.row_ordinal,
        timestamp: wait.timestamp,
        type: wait.type,
        namespace: wait.namespace,
        name: wait.name,
        raw_input: wait.raw_input,
        output: wait.output,
      });
      previousOutput = wait.output;
    }
  }
  const tokenSourceCallIds = [];
  for (const attempt of attempts) {
    const initialText = nativeTextOutput(attempt.output?.raw);
    const continuedText = attempt.continuations
      .map(({ output }) => nativeTextOutput(output?.raw) ?? "").join("\n");
    if (initialText?.includes("WORKER_ONLY_") || continuedText.includes("WORKER_ONLY_")) {
      tokenSourceCallIds.push(attempt.call_id);
    }
  }
  assert.ok(tokenSourceCallIds.length <= 1, "Multiple native exec outputs contained worker tokens");
  for (const wait of waitCalls) {
    if (wait.cell_id !== null && attemptsByCell.has(wait.cell_id)) continue;
    const text = nativeTextOutput(wait.output?.raw);
    assert.ok(!text?.includes("WORKER_ONLY_"),
      "A foreign functions.wait output contained an uncorrelated worker token");
  }
  for (const [callId, outputs] of outputsById) {
    if (callsById.has(callId)) continue;
    if (replyCallIds.has(callId)) continue;
    assert.ok(!outputs.some(({ payload }) => nativeTextOutput(payload.output)?.includes("WORKER_ONLY_")),
      "A foreign native tool output contained an uncorrelated worker token");
  }
  return attempts;
}

/** Return one worker token only after correlated result chunks report terminal exit code zero. */
export function automaticReplyGateToken(attempt) {
  assert.ok(attempt && typeof attempt === "object", "A captured native exec attempt is required");
  const hasCell = attempt.cell_id !== null && attempt.cell_id !== undefined;
  const outputs = hasCell
    ? [attempt.output, ...(attempt.continuations ?? []).map(({ output }) => output)]
    : [attempt.output];
  if (outputs.some((output) => output === null || output === undefined)) return null;

  const parsedOutputs = outputs.map((output) => {
    const text = nativeTextOutput(output.raw);
    assert.notEqual(text, null, "The captured gate output has an unsupported text representation");
    const envelope = knownFunctionsExecBody(text, hasCell ? attempt.cell_id : null);
    return { ...envelope, results: envelope.body.trim() ? parseFunctionsExecResults(envelope.body) : [] };
  });
  if (parsedOutputs.at(-1).pending) return null;
  const results = parsedOutputs.flatMap(({ results: parsed }) => parsed);
  const terminal = results.filter((result) => Object.hasOwn(result, "exit_code") && result.exit_code !== null);
  if (terminal.length === 0) {
    if (parsedOutputs.at(-1).pending || results.some((result) => result.session_id !== null
      && result.session_id !== undefined)) return null;
    assert.fail("The completed functions.exec output is missing a terminal result");
  }
  assert.equal(terminal.length, 1, "The functions.exec output contains multiple terminal results");
  assert.equal(results.at(-1), terminal[0], "The terminal functions.exec result must be last");
  assert.equal(terminal[0].exit_code, 0, "The reply gate command did not exit successfully");
  const stdout = results.map((result) => result.output).join("").trim();
  assert.match(stdout,
    /^WORKER_ONLY_[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/u,
    "The successful gate stdout must be exactly one strict worker UUID");
  return stdout;
}

/**
 * Append the one fixture-owned reply-tool grant only to the exact, prepared
 * agent Codex home whose registration record and config still match Wardian.
 */
export async function appendOwnedReplyToolApproval({ wardianHome, agentId, expectedCommand }) {
  assert.ok(path.isAbsolute(wardianHome), "The isolated Wardian home must be absolute");
  assert.match(agentId ?? "", /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/iu,
    "A canonical UUID agent ID is required");
  assert.ok(path.isAbsolute(expectedCommand), "The registered MCP command must be absolute");

  const home = path.resolve(wardianHome);
  const agentRoot = path.join(home, "agents", agentId);
  const codexHome = path.join(agentRoot, "habitat", ".codex");
  const configPath = path.join(codexHome, "config.toml");
  const ownerPath = path.join(codexHome, ".wardian-messaging.json");
  const commandPath = path.resolve(expectedCommand);
  const directories = [home, path.join(home, "agents"), agentRoot,
    path.join(agentRoot, "habitat"), codexHome, path.dirname(commandPath)];
  const directoryStats = new Map();
  for (const directory of directories) {
    directoryStats.set(directory, await lstatPlain(directory, "directory", "Owned fixture directory"));
  }
  const homeReal = await fs.realpath(home);
  assert.ok(samePath(homeReal, home), "The isolated Wardian home must not be redirected");
  const codexReal = await fs.realpath(codexHome);
  assert.ok(samePath(codexReal, path.join(homeReal, "agents", agentId, "habitat", ".codex")),
    "The worker Codex profile must remain in its exact managed habitat");
  const commandReal = await fs.realpath(commandPath);
  const relativeCommand = path.relative(homeReal, commandReal);
  assert.ok(relativeCommand && !relativeCommand.startsWith("..") && !path.isAbsolute(relativeCommand),
    "The managed MCP command must remain inside the isolated Wardian home");
  const commandStat = await lstatPlain(commandReal, "file", "Managed MCP command");

  const ownerStat = await lstatPlain(ownerPath, "file", "Managed messaging ownership record");
  const configStat = await lstatPlain(configPath, "file", "Managed Codex config");
  const [ownerBytes, original] = await Promise.all([
    fs.readFile(ownerPath), fs.readFile(configPath, "utf8"),
  ]);
  const owner = JSON.parse(ownerBytes.toString("utf8"));
  assert.deepEqual(Object.keys(owner).sort(), ["agent_id", "command", "version", "wardian_home"],
    "The ownership record must retain Wardian's exact schema");
  assert.equal(owner.version, 1);
  assert.equal(owner.agent_id, agentId, "The messaging record belongs to a different agent");
  assert.ok(samePath(owner.wardian_home, home), "The messaging record belongs to a different Wardian home");
  assert.ok(samePath(owner.command, commandReal), "The messaging record names a different executable");

  const server = generatedTomlSection(original, "mcp_servers.wardian");
  const env = generatedTomlSection(original, "mcp_servers.wardian.env");
  assert.equal(generatedTomlValue(server, "command"), owner.command,
    "The Codex server command differs from its ownership record");
  assert.deepEqual(generatedTomlValue(server, "args"), ["mcp", "serve"],
    "The Codex server args differ from Wardian's managed registration");
  assert.ok(samePath(generatedTomlValue(env, "WARDIAN_HOME"), owner.wardian_home),
    "The Codex server environment names a different Wardian home");
  assert.equal(generatedTomlValue(env, "WARDIAN_SESSION_ID"), agentId,
    "The Codex server environment names a different agent");
  assert.equal(hasReplyToolPolicy(original), false,
    "A preexisting or ambiguous Wardian reply-tool policy must be reviewed, never overwritten");
  const newline = original.includes("\r\n") ? "\r\n" : "\n";
  const updated = original + (original.endsWith("\n") ? "" : newline)
    + newline + "[mcp_servers.wardian.tools.reply]" + newline + "approval_mode = \"approve\"" + newline;

  const handle = await fs.open(configPath, "r+");
  try {
    const openedStat = await handle.stat();
    assertSameFileIdentity(configStat, openedStat, "Managed Codex config");
    assert.equal(await readHandleUtf8(handle), original,
      "Managed Codex config changed between validation and open");
    for (const [directory, before] of directoryStats) {
      assertSameFileIdentity(before, await fs.lstat(directory), "Owned fixture directory");
    }
    assertSameFileIdentity(ownerStat, await fs.lstat(ownerPath), "Managed messaging ownership record");
    assert.deepEqual(await fs.readFile(ownerPath), ownerBytes,
      "Managed messaging ownership changed during fixture preparation");
    assertSameFileIdentity(commandStat, await fs.lstat(commandReal), "Managed MCP command");
    assertSameFileIdentity(openedStat, await fs.lstat(configPath), "Managed Codex config");
    assert.equal(await readHandleUtf8(handle), original,
      "Managed Codex config changed before the fixture grant was written");
    await writeHandleUtf8(handle, updated);
    assertSameFileIdentity(openedStat, await fs.lstat(configPath), "Managed Codex config");
    for (const [directory, before] of directoryStats) {
      assertSameFileIdentity(before, await fs.lstat(directory), "Owned fixture directory");
    }
    assert.equal(await readHandleUtf8(handle), updated, "The exact fixture grant was not persisted");
  } finally {
    await handle.close();
  }

  return {
    agent_id: agentId,
    config_before_sha256: createHash("sha256").update(original).digest("hex"),
    config_after_sha256: createHash("sha256").update(updated).digest("hex"),
    fixture_permission_delta: "[mcp_servers.wardian.tools.reply] approval_mode=approve",
    command_file: path.basename(commandReal),
  };
}

/** Persist evidence only after this run acquired its isolated home. */
export async function writeOwnedReplyReport(harness, report) {
  if (typeof harness.runId !== "string" || !harness.runId ||
    harness.homeLock?.runId !== harness.runId) return false;
  await fs.writeFile(path.join(harness.isolatedHome, "automatic-inbound-reply-report.json"),
    JSON.stringify(report, null, 2) + "\n");
  return true;
}

/** Record the first causal failure without allowing report I/O to replace it. */
export async function persistAutomaticReplyFailure(report, caseName, error, save) {
  const message = error instanceof Error ? error.message : String(error);
  report.cases[caseName] = { ...report.cases[caseName], status: "fail", error: message };
  report.error = message;
  try {
    await save();
  } catch (saveError) {
    report.report_persistence_error = saveError instanceof Error ? saveError.message : String(saveError);
  }
  return error;
}

/** Preserve the caller's single absolute run deadline, including cleanup. */
export function automaticReplyDeadlines(deadlineUtc, cleanupReserveMs, now = Date.now()) {
  assert.match(deadlineUtc ?? "", /(Z|[+-]\d{2}:\d{2})$/u, "An explicit granted UTC deadline is required");
  const deadlineAt = Date.parse(deadlineUtc);
  assert.ok(Number.isFinite(deadlineAt), "The granted deadline must be valid");
  assert.ok(Number.isSafeInteger(cleanupReserveMs) && cleanupReserveMs > 0, "Cleanup reserve must be positive");
  assert.ok(deadlineAt - now > cleanupReserveMs, "The original run budget must include cleanup");
  return { deadlineAt, workDeadlineAt: deadlineAt - cleanupReserveMs, cleanupReserveMs };
}

/** Select only the exact, directly owned native Antigravity conversation. */
export function exactCurrentCascade(snapshot, expectedSession) {
  assert.equal(snapshot.provider, "antigravity");
  assert.equal(snapshot.sessions.length, 1, "Current mode must resolve one owned native cascade");
  const cascade = snapshot.sessions[0];
  assert.equal(cascade.id, expectedSession);
  assert.equal(cascade.ownership_basis, "workspace_uri",
    "Current-mode evidence requires direct workspace ownership");
  assert.ok(Array.isArray(cascade.records));
  return cascade;
}

/** Read the persisted readiness for one exact requester without touching delivery state. */
export async function readProviderInputState(home, sessionId) {
  const databasePath = path.join(home, "state.db");
  try {
    await fs.access(databasePath);
  } catch (error) {
    if (error.code === "ENOENT") return null;
    throw error;
  }
  const { DatabaseSync } = await import("node:sqlite");
  const db = new DatabaseSync(databasePath, { readOnly: true });
  try {
    db.exec("BEGIN");
    return db.prepare(
      "SELECT session_id,state,generation FROM provider_input_state WHERE session_id=?",
    ).get(sessionId) ?? null;
  } finally {
    db.close();
  }
}

export function providerInputIsReady(state, expectedSessionId) {
  return state?.session_id === expectedSessionId && state.state === "ready"
    && Number.isSafeInteger(state.generation) && state.generation > 0;
}

function parseLeaseTimestamp(value) {
  if (typeof value !== "string") return null;
  const match = /^(\d{4})-(\d{2})-(\d{2})[Tt](\d{2}):(\d{2}):(\d{2})(?:\.(\d+))?([Zz]|([+-])(\d{2}):(\d{2}))$/u.exec(value);
  if (!match) return null;
  const [, yearText, monthText, dayText, hourText, minuteText, secondText, , zone, , offsetHourText, offsetMinuteText] = match;
  const year = Number(yearText);
  const month = Number(monthText);
  const day = Number(dayText);
  const hour = Number(hourText);
  const minute = Number(minuteText);
  const second = Number(secondText);
  const leapYear = year % 4 === 0 && (year % 100 !== 0 || year % 400 === 0);
  const daysInMonth = [31, leapYear ? 29 : 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31][month - 1];
  if (!daysInMonth || day < 1 || day > daysInMonth || hour > 23 || minute > 59 || second > 59) return null;
  if (zone !== "Z" && zone !== "z" && (Number(offsetHourText) > 23 || Number(offsetMinuteText) > 59)) return null;
  const calendar = new Date(0);
  calendar.setUTCFullYear(year, month - 1, day);
  calendar.setUTCHours(hour, minute, second, 0);
  const fractionMs = Number(((match[7] ?? "") + "000").slice(0, 3));
  let timestamp = calendar.getTime() + fractionMs;
  if (zone !== "Z" && zone !== "z") {
    const offset = (Number(offsetHourText) * 60 + Number(offsetMinuteText)) * 60_000;
    timestamp += match[9] === "+" ? -offset : offset;
  }
  return Number.isFinite(timestamp) ? timestamp : null;
}

function validateConversationLease(lease, index) {
  assert.ok(lease && typeof lease === "object" && !Array.isArray(lease),
    `conversation lease ${index} must be an object`);
  for (const field of ["agent_id", "provider", "resume_session", "owner_kind", "owner_id", "mode",
    "started_at", "heartbeat_at", "expires_at"]) {
    assert.equal(typeof lease[field], "string", `conversation lease ${index} has invalid ${field}`);
  }
  for (const field of ["agent_id", "provider", "owner_kind", "owner_id"]) {
    assert.ok(lease[field].trim(), `conversation lease ${index} has empty ${field}`);
  }
  assert.ok(CONVERSATION_LEASE_MODES.has(lease.mode), `conversation lease ${index} has unknown mode`);
  if (lease.mode === "background_resume") {
    assert.ok(lease.resume_session.trim(), `conversation lease ${index} has empty resume_session`);
  }
  if (lease.mode === "uncertain_previous_provider") {
    assert.equal(lease.owner_kind, "prior_provider_hold",
      `conversation lease ${index} has invalid previous provider hold owner`);
    assert.ok(lease.resume_session.trim(), `conversation lease ${index} has empty previous provider session`);
  }
  for (const field of ["started_at", "heartbeat_at", "expires_at"]) {
    assert.notEqual(parseLeaseTimestamp(lease[field]), null,
      `conversation lease ${index} has invalid ${field}`);
  }
  if (Object.hasOwn(lease, "acquisition_id")) {
    assert.equal(typeof lease.acquisition_id, "string", `conversation lease ${index} has invalid acquisition_id`);
  }
  if (Object.hasOwn(lease, "owner_node_id")) {
    assert.ok(lease.owner_node_id === null || typeof lease.owner_node_id === "string",
      `conversation lease ${index} has invalid owner_node_id`);
  }
}

function safeLeaseOwner(lease) {
  return {
    owner_kind: lease.owner_kind,
    owner_id: lease.owner_id,
    mode: lease.mode,
    agent_id: lease.agent_id,
    provider: lease.provider,
    resume_session: lease.resume_session,
    started_at: lease.started_at,
    heartbeat_at: lease.heartbeat_at,
    expires_at: lease.expires_at,
  };
}

/** Apply the same requester conflict and expiry rules as the canonical lease store. */
export function activeConversationLeaseConflicts(leases, requesterId, resumeSession = "", now = Date.now()) {
  assert.ok(typeof requesterId === "string" && requesterId.trim(), "A nonempty requester id is required");
  assert.ok(resumeSession === null || typeof resumeSession === "string", "resume_session must be a string or null");
  assert.ok(Array.isArray(leases), "Conversation leases must be an array");
  const nowMs = now instanceof Date ? now.getTime()
    : typeof now === "string" ? parseLeaseTimestamp(now) : Number(now);
  assert.ok(Number.isFinite(nowMs), "The lease observation time must be valid");
  const resume = resumeSession ?? "";
  return leases.flatMap((lease, index) => {
    validateConversationLease(lease, index);
    const matchesResume = resume.trim() !== "" && lease.resume_session === resume;
    const conflicts = lease.mode === "uncertain_previous_provider"
      ? matchesResume
      : parseLeaseTimestamp(lease.expires_at) > nowMs && (lease.agent_id === requesterId || matchesResume);
    return conflicts ? [safeLeaseOwner(lease)] : [];
  });
}

function observationTime(now) {
  const timestamp = now instanceof Date ? now.getTime()
    : typeof now === "string" ? parseLeaseTimestamp(now) : Number(now);
  assert.ok(Number.isFinite(timestamp), "The lease observation time must be valid");
  return { timestamp, observedAt: new Date(timestamp).toISOString() };
}

/** Read the owned runtime lease file without creating, changing, or releasing any lease. */
export async function readConversationLeaseSnapshot(home, requesterId, resumeSession = "", now = Date.now()) {
  assert.ok(typeof requesterId === "string" && requesterId.trim(), "A nonempty requester id is required");
  assert.ok(resumeSession === null || typeof resumeSession === "string", "resume_session must be a string or null");
  const { timestamp, observedAt } = observationTime(now);
  const exactResumeSession = resumeSession ?? "";
  const leasePath = path.join(home, "runtime", "conversation-leases.json");
  let bytes;
  let exists = true;
  try {
    bytes = await fs.readFile(leasePath);
  } catch (error) {
    if (error.code !== "ENOENT") {
      error.lease_snapshot = {
        observed_at: observedAt, exists: null, snapshot_sha256: null, valid: false,
        read_error: error instanceof Error ? error.message : String(error),
      };
      throw error;
    }
    bytes = Buffer.alloc(0);
    exists = false;
  }
  const base = {
    observed_at: observedAt,
    exists,
    snapshot_sha256: createHash("sha256").update(bytes).digest("hex"),
  };
  if (!exists) {
    return {
      ...base, valid: true, lease_count: 0, active_conflict_count: 0,
      has_active_conflict: false, active_conflicts: [], omitted_active_conflicts: 0,
    };
  }

  try {
    const document = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(bytes));
    assert.ok(document && typeof document === "object" && !Array.isArray(document),
      "conversation lease file must contain an object");
    const schema = Object.hasOwn(document, "schema") ? document.schema : 1;
    assert.equal(schema, 1, `unsupported conversation lease schema ${String(schema)}`);
    const leases = Object.hasOwn(document, "leases") ? document.leases : [];
    assert.ok(Array.isArray(leases), "conversation lease file leases must be an array");
    leases.forEach(validateConversationLease);
    const conflicts = activeConversationLeaseConflicts(leases, requesterId, exactResumeSession, timestamp);
    return {
      ...base,
      valid: true,
      lease_count: leases.length,
      active_conflict_count: conflicts.length,
      has_active_conflict: conflicts.length > 0,
      active_conflicts: conflicts.slice(0, MAX_REPORTED_LEASE_CONFLICTS),
      omitted_active_conflicts: Math.max(0, conflicts.length - MAX_REPORTED_LEASE_CONFLICTS),
    };
  } catch (error) {
    error.lease_snapshot = {
      ...base, valid: false,
      validation_error: error instanceof Error ? error.message : String(error),
    };
    throw error;
  }
}

/** Retain only first/last blockers and the latest bounded snapshot, never poll history. */
export function recordConversationLeaseObservation(evidence, snapshot) {
  evidence.final_snapshot = snapshot;
  if (snapshot.has_active_conflict) {
    evidence.first_blocking ??= snapshot;
    evidence.last_blocking = snapshot;
  }
  return snapshot.has_active_conflict;
}

/** A complete record requires both the reader's completion and native status. */
export function completedAssistantRows(records) {
  return records.filter((row) => row.role === "assistant" && row.complete === true && row.native_status === 3);
}

/** Verify the admitted requester origin against its live provider-input generation. */
export function assertRequesterOriginMatchesProviderInput(
  proof, expectedProvider, expectedConversationId,
) {
  const origin = proof?.requester_origin;
  assert.ok(origin, "The admitted task must have a persisted requester origin");
  assert.equal(origin.provider, expectedProvider, "The task requester provider changed");
  assert.equal(origin.conversation_id, expectedConversationId,
    "The task requester conversation differs from the captured provider conversation");
  assert.ok(Number.isSafeInteger(origin.generation) && origin.generation > 0,
    "The task requester origin must have a positive provider-input generation");
  assert.ok(Number.isSafeInteger(proof.provider_input_generation) && proof.provider_input_generation > 0,
    "The requester must expose its current provider-input generation");
  assert.equal(origin.generation, proof.provider_input_generation,
    "The task requester generation must match the current persisted provider-input generation, not terminal runtime generation");
}

export function assertPromptDeliveryAccepted(receipt, expectedSessionId, expectedProvider) {
  assert.ok(receipt && typeof receipt === "object", "The initial prompt must return one delivery detail");
  assert.equal(receipt.uuid, expectedSessionId, "The initial prompt receipt targeted a different agent");
  assert.equal(receipt.provider, expectedProvider, "The initial prompt receipt reported a different provider");
  assert.ok(["provider_accepted", "queued"].includes(receipt.delivery_state),
    `The initial prompt was not accepted or queued: ${JSON.stringify(receipt)}`);
}

function inlineBody(reference) {
  const value = JSON.parse(reference);
  assert.equal(value.storage, "inline", "The bounded acceptance text must remain inline");
  assert.equal(typeof value.body, "string");
  return value.body;
}

/**
 * Read the one task admitted by the requester and its canonical reply without
 * claiming, acknowledging, or polling the delivery mailbox.
 */
export async function readCanonicalTaskReply(home, sender, recipient, expectedRequestId = null) {
  const databasePath = path.join(home, "state.db");
  try {
    await fs.access(databasePath);
  } catch (error) {
    if (error.code === "ENOENT") return null;
    throw error;
  }
  const { DatabaseSync } = await import("node:sqlite");
  const db = new DatabaseSync(databasePath, { readOnly: true });
  try {
    db.exec("BEGIN");
    const tasks = db.prepare(
      "SELECT id,target_session_ids,body_ref,status FROM interactions WHERE kind='task' AND sender_session_id=?",
    ).all(sender);
    if (tasks.length === 0) return null;
    assert.equal(tasks.length, 1, "The requester must admit exactly one task");
    const task = tasks[0];
    if (expectedRequestId !== null) assert.equal(task.id, expectedRequestId, "The owned request changed");
    assert.deepEqual(JSON.parse(task.target_session_ids), [recipient]);

    const origins = db.prepare(
      "SELECT requester_provider,requester_conversation_id,requester_generation FROM agent_message_delivery " +
      "WHERE interaction_id=? AND operation='followup_task'",
    ).all(task.id);
    assert.equal(origins.length, 1, "The admitted task must have one canonical requester-origin row");
    const origin = origins[0];
    assert.ok(typeof origin.requester_provider === "string" && origin.requester_provider.trim(),
      "The admitted task requester provider is missing");
    assert.ok(typeof origin.requester_conversation_id === "string" && origin.requester_conversation_id.trim(),
      "The admitted task requester conversation is missing");
    assert.ok(Number.isSafeInteger(origin.requester_generation) && origin.requester_generation > 0,
      "The admitted task requester generation is missing or invalid");
    const providerInputState = db.prepare(
      "SELECT generation FROM provider_input_state WHERE session_id=?",
    ).get(sender);

    const replies = db.prepare(
      "SELECT id,sender_session_id,target_session_ids,parent_interaction_id,body_ref FROM interactions WHERE kind='reply' AND parent_interaction_id=?",
    ).all(task.id);
    assert.ok(replies.length <= 1, "The task received duplicate canonical replies");

    const requesterReplies = db.prepare(
      "SELECT id,parent_interaction_id FROM interactions WHERE kind='reply' AND sender_session_id=?",
    ).all(sender);
    let reply = null;
    let replyToReplyCount = 0;
    if (replies.length === 1) {
      const row = replies[0];
      assert.equal(row.sender_session_id, recipient);
      assert.deepEqual(JSON.parse(row.target_session_ids), [sender]);
      assert.equal(row.parent_interaction_id, task.id);
      const statuses = db.prepare(
        "SELECT status FROM structured_replies WHERE request_id=?",
      ).all(task.id);
      assert.equal(statuses.length, 1, "The canonical request must have one structured reply status");
      replyToReplyCount = db.prepare(
        "SELECT id FROM interactions WHERE kind='reply' AND parent_interaction_id=?",
      ).all(row.id).length;
      reply = {
        interaction_id: row.id,
        request_id: task.id,
        parent_interaction_id: row.parent_interaction_id,
        sender: row.sender_session_id,
        recipients: JSON.parse(row.target_session_ids),
        status: statuses[0].status,
        body: inlineBody(row.body_ref),
      };
    }

    return {
      request_id: task.id,
      task_status: task.status,
      task_body: inlineBody(task.body_ref),
      requester_origin: {
        provider: origin.requester_provider,
        conversation_id: origin.requester_conversation_id,
        generation: origin.requester_generation,
      },
      provider_input_generation: providerInputState?.generation ?? null,
      reply_count: replies.length,
      reply,
      requester_reply_count: requesterReplies.length,
      reply_to_reply_count: replyToReplyCount,
    };
  } finally {
    db.close();
  }
}
