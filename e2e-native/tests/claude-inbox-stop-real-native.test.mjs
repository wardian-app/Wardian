// @tier manual — One explicitly selected real Claude turn; coordinator must serialize provider runs.
// PowerShell:
// $env:WARDIAN_E2E_REAL_CLAUDE_INBOX = '1'
// $env:WARDIAN_E2E_CLAUDE_INBOX_MODEL = 'haiku'
// $env:WARDIAN_NATIVE_SKIP_BUILD = '1'
// $env:WARDIAN_NATIVE_APP = '<absolute-isolated-artifact-path>'
// node scripts/run-native-e2e-fast.mjs e2e-native/tests/claude-inbox-stop-real-native.test.mjs
import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
import { randomBytes, createHash } from "node:crypto";
import { cleanupConformanceSession, pauseConformanceWork } from "../lib/conformance-cleanup.mjs";
import { createNativeHarness, invokeTauri, prepareIsolatedHome, startNativeSession, waitForAppShell } from "../lib/harness.mjs";

const OPT_IN = "WARDIAN_E2E_REAL_CLAUDE_INBOX";
const MODEL_ENV = "WARDIAN_E2E_CLAUDE_INBOX_MODEL";

function configuration(env) {
  assert.equal(env[OPT_IN], "1", `${OPT_IN}=1 is required`);
  const model = env[MODEL_ENV]?.trim();
  assert.ok(model, `Set ${MODEL_ENV} to an explicitly preflighted catalog model`);
  assert.ok(path.isAbsolute(env.WARDIAN_NATIVE_APP ?? ""),
    "WARDIAN_NATIVE_APP must be an already-built isolated app artifact");
  return { model, app: env.WARDIAN_NATIVE_APP };
}

function userHookCommand(scriptPath) {
  if (process.platform === "win32") {
    return `powershell -NoProfile -ExecutionPolicy Bypass -File "${scriptPath}"`;
  }
  const quote = (value) => `'${value.replaceAll("'", `'"'"'`)}'`;
  return `${quote(process.execPath)} ${quote(scriptPath)}`;
}

async function waitIdle(driver, sessionId, timeoutMs = 180_000) {
  return driver.wait(async () => {
    const rows = await invokeTauri(driver, "list_agent_metrics");
    const row = rows.find((entry) => entry.session_id === sessionId);
    return row?.current_status?.toLowerCase() === "idle" ? row : false;
  }, timeoutMs, `Claude session ${sessionId} did not reach idle`);
}

test("Claude creates no unverifiable Inbox completion after a hook continuation and preserves a workspace hook",
  { timeout: 360_000 }, async (t) => {
    if (process.env[OPT_IN] !== "1") {
      t.skip(`Set ${OPT_IN}=1; no provider turn was submitted`);
      return;
    }

    const config = configuration(process.env);
    const harness = await createNativeHarness();
    harness.appPath = config.app;
    const root = path.join(harness.repoRoot, ".tmp", "e2e-native", "claude-inbox-homes");
    await fs.mkdir(root, { recursive: true });
    harness.isolatedHome = await fs.mkdtemp(path.join(root, "claude-"));

    const owned = {
      session: undefined,
      agent: undefined,
      spawnAttempted: false,
      startupAttempted: false,
      cleanup: async () => {},
    };
    t.after(() => cleanupConformanceSession({
      harness,
      session: owned.session,
      startupAttempted: owned.startupAttempted,
      pause: () => pauseConformanceWork(
        (command, args) => invokeTauri(owned.session.driver, command, args),
        { spawnAttempted: owned.spawnAttempted, sessionId: owned.agent?.session_id },
      ),
      save: (result) => owned.cleanup(result),
    }));

    prepareIsolatedHome(harness);
    const workspace = path.join(harness.isolatedHome, "scratch-workspace");
    const projectClaude = path.join(workspace, ".claude");
    await fs.mkdir(projectClaude, { recursive: true });
    await fs.mkdir(path.join(harness.isolatedHome, "settings"), { recursive: true });
    await fs.writeFile(path.join(harness.isolatedHome, "settings", "shell.json"), JSON.stringify({
      schema_version: 2,
      overrides: { conversation_logging: "enabled" },
    }));

    const hookOutput = path.join(projectClaude, "workspace-stop-events.jsonl");
    const token = randomBytes(8).toString("hex").toUpperCase();
    const first = `CLAUDE_INBOX_FIRST_${token}`;
    const expected = `CLAUDE_INBOX_FINAL_${token}`;
    // The workspace hook blocks the first answer once, so Claude continues the
    // same prompt; only the second answer is final.
    const block = JSON.stringify({
      decision: "block",
      reason: `Now reply with exactly ${expected} and nothing else.`,
    });
    const hookScript = path.join(projectClaude,
      process.platform === "win32" ? "workspace-stop-hook.ps1" : "workspace-stop-hook.cjs");
    const hookScriptBody = process.platform === "win32"
      ? [
        "$payload = [Console]::In.ReadToEnd()",
        "if (-not [String]::IsNullOrWhiteSpace($payload)) {",
        `  [System.IO.File]::AppendAllText('${hookOutput.replaceAll("'", "''")}', ($payload -replace "[\`r\`n]+", ' ') + [Environment]::NewLine, [System.Text.UTF8Encoding]::new($false))`,
        "  if (-not ($payload | ConvertFrom-Json).stop_hook_active) {",
        `    Write-Output '${block.replaceAll("'", "''")}'`,
        "  }",
        "}",
        "",
      ].join("\n")
      : [
        "const fs = require('node:fs');",
        "const chunks = [];",
        "process.stdin.on('data', chunk => chunks.push(chunk));",
        "process.stdin.on('end', () => {",
        "  const payload = Buffer.concat(chunks).toString();",
        `  fs.appendFileSync(${JSON.stringify(hookOutput)}, payload.replace(/[\\r\\n]+/g, ' ') + '\\n');`,
        `  if (!JSON.parse(payload).stop_hook_active) process.stdout.write(${JSON.stringify(block)});`,
        "});",
        "",
      ].join("\n");
    await fs.writeFile(hookScript, hookScriptBody, "utf8");
    await fs.writeFile(path.join(projectClaude, "settings.local.json"), JSON.stringify({
      hooks: {
        Stop: [{ hooks: [{ type: "command", command: userHookCommand(hookScript) }] }],
      },
    }, null, 2));

    const report = {
      provider: "claude",
      model: config.model,
      artifact_sha256: createHash("sha256").update(await fs.readFile(config.app)).digest("hex"),
      harness_sha256: createHash("sha256").update(await fs.readFile(new URL(import.meta.url))).digest("hex"),
      started_at: new Date().toISOString(),
      provider_version: null,
      cases: { catalog: "not_run", user_hook_coexistence: "not_run", continued_turn_without_verdict: "not_run" },
    };
    const reportPath = path.join(harness.isolatedHome, "claude-inbox-stop-report.json");
    const save = async () => fs.writeFile(reportPath, `${JSON.stringify(report, null, 2)}\n`, "utf8");
    owned.cleanup = async (result) => {
      report.cleanup = result;
      report.completed_at = new Date().toISOString();
      await save();
      t.diagnostic(`Private evidence retained under isolated run home (${path.basename(reportPath)})`);
    };
    await save();

    owned.startupAttempted = true;
    owned.session = await startNativeSession(harness);
    await owned.session.driver.manage().setTimeouts({ script: 180_000 });
    await waitForAppShell(owned.session.driver, 30_000);

    const catalog = await invokeTauri(owned.session.driver, "list_provider_model_catalog", {
      provider: "claude",
      forceRefresh: true,
    });
    assert.equal(catalog.refresh_error, null, "Claude catalog preflight failed");
    assert.ok(catalog.models.some((model) => model.id === config.model),
      `Selected model ${config.model} is missing from the refreshed Claude catalog`);
    report.provider_version = catalog.version ?? null;
    report.cases.catalog = { status: "pass", source: catalog.source, selected_model_present: true };
    await save();

    owned.spawnAttempted = true;
    owned.agent = await invokeTauri(owned.session.driver, "spawn_agent", { req: {
      sessionName: `Claude-Inbox-${Date.now()}`,
      agentClass: "TestClass",
      folder: workspace,
      isOff: false,
      resumeSession: null,
      configOverride: {
        provider: "claude",
        model: config.model,
        session_persistence: "resume",
        conversation_logging: "enabled",
      },
    } });
    assert.equal(owned.agent.provider, "claude");
    assert.equal(owned.agent.model, config.model);
    await waitIdle(owned.session.driver, owned.agent.session_id);

    const launchRoot = path.join(harness.isolatedHome, "agents", owned.agent.session_id, "claude", "turn-verdicts");
    const launches = (await fs.readdir(launchRoot)).sort();
    const launch = JSON.parse(await fs.readFile(path.join(launchRoot, launches.at(-1), "launch.json"), "utf8"));
    // Claude Code exposes no supported interactive turn-end verdict (#1492).
    assert.equal(launch.verification, "unavailable");
    assert.match(launch.unavailable_reason, /turn-end verdict/);

    const prompt = `No tools. Reply with exactly ${first} and nothing else.`;
    const receipt = await invokeTauri(owned.session.driver, "submit_prompt_to_agent", {
      sessionId: owned.agent.session_id,
      prompt,
      inputMode: "message",
    });
    assert.equal(receipt.provider, "claude");
    assert.ok(["provider_accepted", "queued"].includes(receipt.delivery_state),
      `Claude prompt was not accepted: ${JSON.stringify(receipt)}`);

    const transcript = await owned.session.driver.wait(async () => {
      const rows = await invokeTauri(owned.session.driver, "load_agent_chat_transcript", { sessionId: owned.agent.session_id });
      const answer = rows.find((row) => row.provider === "claude" && row.kind === "message"
        && row.role === "assistant" && row.metadata?.provider_log === true && row.text?.trim() === expected);
      return answer ? { rows, answer } : false;
    }, 180_000, "Claude provider-authored final answer was not observed");
    await waitIdle(owned.session.driver, owned.agent.session_id);

    // The finished turn still produces no card: nothing proves it final.
    await new Promise((resolve) => setTimeout(resolve, 10_000));
    const items = await invokeTauri(owned.session.driver, "load_queue_items");
    assert.ok(!items.some((item) => item.type === "agent_completed"
      && item.agent_session_id === owned.agent.session_id),
    "an interactive Claude turn without a provider verdict must not create a completion");

    const userEvents = (await fs.readFile(hookOutput, "utf8"))
      .trim().split(/\r?\n/).filter(Boolean).map((line) => JSON.parse(line));
    const finalRound = userEvents.find((event) => event.hook_event_name === "Stop"
      && event.last_assistant_message?.trim() === expected);
    assert.ok(finalRound, "the workspace hook must see the final round");
    const rounds = userEvents.filter((event) => event.hook_event_name === "Stop"
      && event.prompt_id === finalRound.prompt_id);
    assert.equal(rounds.length, 2, "the workspace hook must see the blocked and the final round");
    assert.equal(rounds[0].stop_hook_active, false);
    assert.equal(rounds[0].last_assistant_message.trim(), first);
    assert.equal(rounds[1].stop_hook_active, true);
    assert.equal(rounds[1].last_assistant_message.trim(), expected);
    report.cases.user_hook_coexistence = { status: "pass", stop_events: userEvents.length };
    report.cases.continued_turn_without_verdict = {
      status: "pass",
      provider_authored_answer: transcript.answer.text.trim() === expected,
      verification: launch.verification,
      completion_items: 0,
    };
    report.cleanup = { status: "pending" };
    await save();
  });
