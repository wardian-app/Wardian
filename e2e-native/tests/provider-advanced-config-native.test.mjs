// @tier nightly — Runs on the nightly schedule; too slow or too broad for every pull request.
import test from "node:test";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import {
  existsSync,
  mkdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import path from "node:path";

import {
  createNativeHarness,
  ensureNativeAppBuilt,
  freezeBuiltCliForRun,
  invokeTauri,
  prepareIsolatedHome,
  startNativeSession,
  waitForAppShell,
} from "../lib/harness.mjs";

const skipNativeBuild = process.env.WARDIAN_NATIVE_SKIP_BUILD === "1";
const runId = `${process.pid}-${Date.now()}`;
const providerCommands = {
  claude: "claude",
  gemini: "gemini",
  codex: "codex",
  antigravity: "agy",
  opencode: "opencode",
};

function buildCli(harness) {
  const result = spawnSync("cargo", ["build", "-p", "wardian-cli", "--bin", "wardian-cli"], {
    cwd: harness.repoRoot,
    encoding: "utf8",
  });
  assert.equal(
    result.status,
    0,
    `cargo build -p wardian-cli failed\nstdout:\n${result.stdout}\nstderr:\n${result.stderr}`,
  );

  return freezeBuiltCliForRun(harness);
}

function runCli(cliPath, harness, args) {
  const env = { ...process.env, WARDIAN_HOME: harness.isolatedHome };
  delete env.WARDIAN_SESSION_ID;
  return spawnSync(cliPath, args, {
    cwd: harness.repoRoot,
    env,
    encoding: "utf8",
    timeout: 60000,
  });
}

function recorderSource(provider) {
  return `
const fs = require("node:fs");
const path = require("node:path");

const provider = ${JSON.stringify(provider)};
const argv = process.argv.slice(2);
const sessionId = process.env.WARDIAN_SESSION_ID || "missing-session";
const headless =
  (provider === "claude" && argv.includes("--print")) ||
  (provider === "gemini" && argv.includes("-p")) ||
  (provider === "codex" && argv.includes("exec")) ||
  (provider === "antigravity" && argv.includes("--print")) ||
  (provider === "opencode" && argv[0] === "run");
const phase = headless ? "headless" : "interactive";
const captureDir = process.env.WARDIAN_PROVIDER_ARGV_DIR;
fs.mkdirSync(captureDir, { recursive: true });
const safeSessionId = sessionId.replace(/[^a-zA-Z0-9_.-]/g, "_");
const capturePath = path.join(captureDir, provider + "-" + phase + "-" + safeSessionId + ".json");
const temporaryPath = capturePath + ".tmp";
fs.writeFileSync(temporaryPath, JSON.stringify({ provider, phase, session_id: sessionId, argv }), "utf8");
fs.renameSync(temporaryPath, capturePath);

if (headless) {
  let output;
  if (provider === "codex") {
    output =
      JSON.stringify({ type: "thread.started", thread_id: "019c0000-0000-7000-8000-000000000001" }) + "\\n" +
      JSON.stringify({ type: "item.completed", item: { type: "agent_message", text: "native argv captured" } }) + "\\n";
  } else if (provider === "opencode") {
    output = JSON.stringify({
      type: "text",
      sessionID: "ses_native_argv_capture",
      part: { text: "native argv captured" },
    }) + "\\n";
  } else if (provider === "claude") {
    output = JSON.stringify({
      type: "result",
      session_id: "019c0000-0000-7000-8000-000000000002",
      result: "native argv captured",
    }) + "\\n";
  } else if (provider === "gemini") {
    output = JSON.stringify({ response: "native argv captured" }) + "\\n";
  } else {
    output = "native argv captured\\n";
  }
  process.stdout.write(output, () => process.exit(0));
} else {
  process.stdout.write("native argv recorder ready\\n");
  setInterval(() => {}, 1000);
}
`;
}

function seedProviderShims(harness) {
  const binDir = path.join(harness.isolatedHome, "provider-argv-bin");
  const captureDir = path.join(harness.isolatedHome, "provider-argv-captures");
  mkdirSync(binDir, { recursive: true });
  mkdirSync(captureDir, { recursive: true });

  for (const [provider, command] of Object.entries(providerCommands)) {
    const scriptName = `${command}-argv-recorder.cjs`;
    writeFileSync(path.join(binDir, scriptName), recorderSource(provider), "utf8");
    if (process.platform === "win32") {
      writeFileSync(
        path.join(binDir, `${command}.cmd`),
        `@ECHO off\r\n"${process.execPath}" "%~dp0${scriptName}" %*\r\n`,
        "utf8",
      );
    } else {
      const executable = path.join(binDir, command);
      writeFileSync(
        executable,
        `#!/bin/sh\nexec node "$(dirname "$0")/${scriptName}" "$@"\n`,
        { encoding: "utf8", mode: 0o755 },
      );
    }
  }

  return { binDir, captureDir };
}

function capturePath(captureDir, provider, phase, sessionId) {
  return path.join(captureDir, `${provider}-${phase}-${sessionId}.json`);
}

async function waitForCapture(captureDir, provider, phase, sessionId, timeoutMs = 20000) {
  const target = capturePath(captureDir, provider, phase, sessionId);
  const startedAt = Date.now();
  let lastError = null;
  while (Date.now() - startedAt < timeoutMs) {
    if (existsSync(target)) {
      try {
        const capture = JSON.parse(readFileSync(target, "utf8"));
        if (capture.session_id === sessionId) {
          return capture;
        }
      } catch (error) {
        lastError = error;
      }
    }
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  throw new Error(`Timed out waiting for ${target}${lastError ? `: ${lastError}` : ""}`);
}

function assertFlag(argv, flag, value) {
  const index = argv.indexOf(flag);
  assert.notEqual(index, -1, `missing ${flag} in ${JSON.stringify(argv)}`);
  if (value !== undefined) {
    assert.equal(argv[index + 1], value, `${flag} value mismatch in ${JSON.stringify(argv)}`);
  }
}

function assertNoFlag(argv, flag) {
  assert.equal(argv.includes(flag), false, `unexpected ${flag} in ${JSON.stringify(argv)}`);
}

function assertProviderArgs(testCase, phase, argv) {
  switch (testCase.id) {
    case "claude":
      assertFlag(argv, "--permission-mode", "acceptEdits");
      assertFlag(argv, "--tools", "Read,Edit");
      assertFlag(argv, "--allowedTools", "Bash");
      assertFlag(argv, "--disallowedTools", "WebFetch");
      assertFlag(argv, "--append-system-prompt", "native claude prompt");
      assertFlag(argv, "--mcp-config", testCase.providerConfig.mcp_config);
      assertFlag(argv, "--strict-mcp-config");
      if (phase === "headless") {
        assertFlag(argv, "--max-turns", "7");
      } else {
        assertNoFlag(argv, "--max-turns");
      }
      break;
    case "gemini":
      assertFlag(argv, "--sandbox");
      assertFlag(argv, "--approval-mode", "plan");
      assertFlag(argv, "--policy", "policy-a,policy-b");
      assertFlag(argv, "--admin-policy", "admin-a");
      assertFlag(argv, "--allowed-mcp-server-names", "sqlite");
      assertFlag(argv, "--extensions", "ext-a,ext-b");
      assertFlag(argv, "--screen-reader");
      assertNoFlag(argv, "--experimental-acp");
      break;
    case "codex-policy":
      assertFlag(argv, "--sandbox", "read-only");
      assertFlag(argv, "--ask-for-approval", "never");
      assertFlag(argv, "--profile", "native-profile");
      assertFlag(argv, "--search");
      assertNoFlag(argv, "--dangerously-bypass-approvals-and-sandbox");
      if (phase === "headless") {
        assertFlag(argv, "--skip-git-repo-check");
        assertFlag(argv, "--ephemeral");
      } else {
        assertNoFlag(argv, "--ephemeral");
      }
      break;
    case "codex-bypass":
      assertFlag(argv, "--dangerously-bypass-approvals-and-sandbox");
      assertNoFlag(argv, "--sandbox");
      assertNoFlag(argv, "--ask-for-approval");
      break;
    case "antigravity":
      assertFlag(argv, "--sandbox");
      assertFlag(argv, "--dangerously-skip-permissions");
      assertFlag(argv, "--mode", "plan");
      assertFlag(argv, "--agent", "reviewer");
      if (phase === "headless") {
        assertFlag(argv, "--print-timeout", "90s");
      } else {
        assertNoFlag(argv, "--print-timeout");
      }
      break;
    case "opencode":
      assertFlag(argv, "--agent", "build");
      assertFlag(argv, "--auto");
      break;
    default:
      assert.fail(`unknown provider argv case: ${testCase.id}`);
  }
}

function assertFreshClaudeResumeArgs(argv, staleProviderSession) {
  assertNoFlag(argv, "--resume");
  assertFlag(argv, "--session-id");
  const freshSessionId = argv[argv.indexOf("--session-id") + 1];
  assert.ok(freshSessionId, `missing fresh Claude session id in ${JSON.stringify(argv)}`);
  assert.notEqual(
    freshSessionId,
    staleProviderSession,
    `fresh resume reused the paused Claude session in ${JSON.stringify(argv)}`,
  );
  return freshSessionId;
}

function providerCases(harness) {
  const mcpConfig = path.join(harness.isolatedHome, "native-mcp.json");
  writeFileSync(mcpConfig, '{"mcpServers":{}}\n', "utf8");
  return [
    {
      id: "claude",
      provider: "claude",
      providerConfig: {
        type: "claude",
        permission_mode: "acceptEdits",
        max_turns: 7,
        tools: ["Read", "Edit"],
        allowed_tools: ["Bash"],
        disallowed_tools: ["WebFetch"],
        append_system_prompt: "native claude prompt",
        mcp_config: mcpConfig,
        strict_mcp_config: true,
      },
    },
    {
      id: "gemini",
      provider: "gemini",
      providerConfig: {
        type: "gemini",
        sandbox: true,
        approval_mode: "plan",
        policy: ["policy-a", "policy-b"],
        admin_policy: ["admin-a"],
        allowed_mcp_server_names: ["sqlite"],
        extensions: ["ext-a", "ext-b"],
        screen_reader: true,
      },
    },
    {
      id: "codex-policy",
      provider: "codex",
      providerConfig: {
        type: "codex",
        sandbox_mode: "read-only",
        approval_policy: "never",
        profile: "native-profile",
        full_auto: false,
        search: true,
        skip_git_repo_check: true,
        ephemeral: true,
      },
    },
    {
      id: "codex-bypass",
      provider: "codex",
      providerConfig: {
        type: "codex",
        full_auto: true,
      },
    },
    {
      id: "antigravity",
      provider: "antigravity",
      providerConfig: {
        type: "antigravity",
        sandbox: true,
        dangerously_skip_permissions: true,
        mode: "plan",
        agent: "reviewer",
        print_timeout: "90s",
      },
    },
    {
      id: "opencode",
      provider: "opencode",
      debug: true,
      providerConfig: {
        type: "opencode",
        agent: "build",
        auto: true,
      },
    },
  ];
}

async function spawnOffAgent(driver, harness, testCase) {
  return await invokeTauri(driver, "spawn_agent", {
    req: {
      sessionName: `NativeArgv-${testCase.id}-${runId}`,
      agentClass: "TestClass",
      folder: harness.repoRoot,
      resumeSession: null,
      isOff: true,
      configOverride: {
        provider: testCase.provider,
        provider_config: { type: testCase.provider },
      },
    },
  });
}

test("per-agent advanced config survives persistence and reaches native provider argv", { timeout: 420000 }, async (t) => {
  const harness = await createNativeHarness();

  try {
    if (!skipNativeBuild) {
      ensureNativeAppBuilt(harness);
    }
    assert.ok(harness.appPath);
  } catch (error) {
    t.skip(String(error));
    return;
  }

  prepareIsolatedHome(harness);
  const { binDir, captureDir } = seedProviderShims(harness);
  const cliPath = buildCli(harness);
  const previousPath = process.env.PATH;
  const previousPathExt = process.env.PATHEXT;
  const previousCaptureDir = process.env.WARDIAN_PROVIDER_ARGV_DIR;
  process.env.PATH = [binDir, previousPath].filter(Boolean).join(path.delimiter);
  process.env.WARDIAN_PROVIDER_ARGV_DIR = captureDir;
  if (process.platform === "win32") {
    process.env.PATHEXT = ".CMD;.EXE;.BAT";
  }

  let session;
  try {
    session = await startNativeSession(harness);
  } catch (error) {
    t.skip(String(error));
    return;
  } finally {
    if (previousPath === undefined) delete process.env.PATH;
    else process.env.PATH = previousPath;
    if (previousPathExt === undefined) delete process.env.PATHEXT;
    else process.env.PATHEXT = previousPathExt;
    if (previousCaptureDir === undefined) delete process.env.WARDIAN_PROVIDER_ARGV_DIR;
    else process.env.WARDIAN_PROVIDER_ARGV_DIR = previousCaptureDir;
  }

  const spawnedSessionIds = [];
  t.after(async () => {
    for (const sessionId of spawnedSessionIds) {
      try {
        await invokeTauri(session.driver, "kill_agent", { sessionId });
      } catch {
        // The recorder may have already exited after a failed assertion.
      }
    }
    await session.close();
  });

  await waitForAppShell(session.driver, 20000);

  for (const testCase of providerCases(harness)) {
    const agent = await spawnOffAgent(session.driver, harness, testCase);
    spawnedSessionIds.push(agent.session_id);

    const listed = await invokeTauri(session.driver, "list_agents");
    const persisted = listed.find((entry) => entry.session_id === agent.session_id);
    assert.ok(persisted, `${testCase.id} agent missing after spawn`);
    const updatedConfig = {
      ...persisted,
      debug: testCase.debug ?? persisted.debug,
      provider_config: testCase.providerConfig,
    };
    await invokeTauri(session.driver, "update_agent_config", { newConfig: updatedConfig });

    const reloaded = (await invokeTauri(session.driver, "list_agents"))
      .find((entry) => entry.session_id === agent.session_id);
    assert.deepEqual(reloaded.provider_config, testCase.providerConfig);

    rmSync(capturePath(captureDir, testCase.provider, "headless", agent.session_id), { force: true });
    const send = runCli(cliPath, harness, [
      "send",
      `NATIVE_ARGV_${testCase.id}`,
      "--to",
      agent.session_name,
      "--wait-until",
      "idle",
      "--timeout",
      "30s",
    ]);
    assert.equal(
      send.status,
      0,
      `${testCase.id} headless delivery failed\nstdout:\n${send.stdout}\nstderr:\n${send.stderr}`,
    );
    const headlessCapture = await waitForCapture(
      captureDir,
      testCase.provider,
      "headless",
      agent.session_id,
    );
    assertProviderArgs(testCase, "headless", headlessCapture.argv);

    rmSync(capturePath(captureDir, testCase.provider, "interactive", agent.session_id), { force: true });
    await invokeTauri(session.driver, "resume_agent", { sessionId: agent.session_id });
    const interactiveCapture = await waitForCapture(
      captureDir,
      testCase.provider,
      "interactive",
      agent.session_id,
    );
    assertProviderArgs(testCase, "interactive", interactiveCapture.argv);
    await invokeTauri(session.driver, "pause_agent", { sessionId: agent.session_id });
  }

  const freshClaude = await spawnOffAgent(session.driver, harness, {
    id: "claude-fresh-resume",
    provider: "claude",
  });
  spawnedSessionIds.push(freshClaude.session_id);

  const staleProviderSession = "stale-claude-provider-session";
  const freshClaudeConfig = (await invokeTauri(session.driver, "list_agents"))
    .find((entry) => entry.session_id === freshClaude.session_id);
  assert.ok(freshClaudeConfig, "fresh Claude agent missing after spawn");
  await invokeTauri(session.driver, "update_agent_config", {
    newConfig: {
      ...freshClaudeConfig,
      session_persistence: "fresh",
      resume_session: staleProviderSession,
    },
  });

  rmSync(capturePath(captureDir, "claude", "interactive", freshClaude.session_id), { force: true });
  await invokeTauri(session.driver, "resume_agent", { sessionId: freshClaude.session_id });
  const freshClaudeCapture = await waitForCapture(
    captureDir,
    "claude",
    "interactive",
    freshClaude.session_id,
  );
  const freshProviderSession = assertFreshClaudeResumeArgs(
    freshClaudeCapture.argv,
    staleProviderSession,
  );

  const resumedFreshClaude = (await invokeTauri(session.driver, "list_agents"))
    .find((entry) => entry.session_id === freshClaude.session_id);
  assert.equal(resumedFreshClaude.resume_session, freshProviderSession);
  assert.notEqual(resumedFreshClaude.resume_session, staleProviderSession);
  await invokeTauri(session.driver, "pause_agent", { sessionId: freshClaude.session_id });
});
