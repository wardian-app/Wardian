import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";

import {
  createNativeHarness,
  ensureNativeAppBuilt,
  prepareIsolatedHome,
  startNativeSession,
  waitForAppShell,
} from "../lib/harness.mjs";

const skipNativeBuild = process.env.WARDIAN_NATIVE_SKIP_BUILD === "1";

function writeSingleTurnMockScript(harness, { markerPath }) {
  const mockScript = path.join(harness.isolatedHome, "single-turn-live-mock.cjs");
  fs.writeFileSync(
    mockScript,
    `
const fs = require("node:fs");
const { spawnSync } = require("node:child_process");
const providerSessionId = process.env.WARDIAN_MOCK_SESSION_ID;
const wardianSessionId = process.env.WARDIAN_SESSION_ID;
if (!providerSessionId) throw new Error("WARDIAN_MOCK_SESSION_ID is required");
if (!wardianSessionId) throw new Error("WARDIAN_SESSION_ID is required");
const markerPath = process.env.WARDIAN_MOCK_MARKER;
const cli = process.env.WARDIAN_CLI || "wardian-cli";
const emit = (obj) => process.stdout.write(JSON.stringify(obj) + "\\n");
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

emit({ type: "init", session_id: providerSessionId, timestamp: new Date().toISOString() });

let buffer = "";
let completed = false;
process.stdin.setEncoding("utf8");
process.stdin.on("data", async (chunk) => {
  if (completed) return;
  buffer += chunk;
  if (buffer.includes("\\r") || buffer.includes("\\n")) {
    completed = true;
    const requestId = buffer.match(/wardian reply\\s+([^\\s]+)/)?.[1] || null;
    emit({ type: "model", content: "partial model response, not a turn completion" });
    await sleep(1200);
    emit({ type: "result", status: "success" });
    const reply = requestId
      ? spawnSync(cli, ["reply", requestId, "--status", "done", "--stdin"], {
          input: "workflow output complete",
          encoding: "utf8",
          env: { ...process.env, WARDIAN_SESSION_ID: wardianSessionId },
        })
      : { status: 1, stdout: "", stderr: "request id not found" };
    if (markerPath) {
      fs.writeFileSync(markerPath, JSON.stringify({
        completed: true,
        input: buffer,
        requestId,
        replyStatus: reply.status,
        replyStdout: reply.stdout,
        replyStderr: reply.stderr,
        at: new Date().toISOString(),
      }));
    }
    setInterval(() => {}, 1000);
  }
});
process.stdin.resume();
`,
    "utf8",
  );
  return mockScript;
}

function seedWorkflow(harness, { workflowId }) {
  const workflowsDir = path.join(harness.isolatedHome, "library", "workflows");
  fs.mkdirSync(workflowsDir, { recursive: true });
  const workflowPath = path.join(workflowsDir, `${workflowId}.md`);
  fs.writeFileSync(
    workflowPath,
    `---
schema: 2
id: ${workflowId}
name: Agent Completion Repro
nodes:
  - id: trigger
    type: manual_trigger
  - id: agent-node-1
    type: task
    fields:
      agent: role:worker
      prompt: Complete this workflow node.
edges:
  - from: trigger
    to: agent-node-1
---

# Agent Completion Repro
`,
    "utf8",
  );
  return workflowPath;
}

async function spawnMockAgent(driver, { sessionId, sessionName, folder }) {
  const result = await driver.executeAsyncScript((sessionId, sessionName, folder, done) => {
    window.__TAURI_INTERNALS__.invoke("spawn_agent", {
      req: {
        sessionName,
        agentClass: "TestClass",
        folder,
        resumeSession: sessionId,
        isOff: false,
        configOverride: { provider: "mock" },
      },
    }).then(
      (agent) => done({ ok: true, agent }),
      (error) => done({ ok: false, error: String(error) }),
    );
  }, sessionId, sessionName, folder);

  assert.equal(result.ok, true, `spawn_agent failed: ${result.error}`);
  return result.agent;
}

async function invokeWorkflowRun(driver, { workflowPath, sessionId, workspace }) {
  const result = await driver.executeAsyncScript((payload, done) => {
    window.__TAURI_INTERNALS__.invoke("workflow_run", payload).then(
      (value) => done({ ok: true, value }),
      (error) => done({ ok: false, error: String(error) }),
    );
  }, {
    path: workflowPath,
    provider: "mock",
    workspace,
    input: {},
    assignments: {
      worker: {
        target_type: "agent",
        agent_id: sessionId,
        conversation: "current",
        busy_policy: "wait",
      },
    },
  });

  assert.equal(result.ok, true, `workflow_run failed: ${result.error}`);
  assert.equal(result.value?.ok, true, `workflow_run did not start: ${JSON.stringify(result.value)}`);
  return result.value;
}

async function readLatestWorkflowState(harness, workflowId, timeoutMs = 10000) {
  const logDir = path.join(harness.isolatedHome, "logs", "workflows", workflowId);
  const startedAt = Date.now();

  while (Date.now() - startedAt < timeoutMs) {
    if (fs.existsSync(logDir)) {
      const runDirs = fs
        .readdirSync(logDir)
        .map((name) => path.join(logDir, name))
        .filter((candidate) => fs.statSync(candidate).isDirectory())
        .sort();

      const newest = runDirs.at(-1);
      const statePath = newest ? path.join(newest, "state.json") : null;
      const eventsPath = newest ? path.join(newest, "events.jsonl") : null;
      if (statePath && eventsPath && fs.existsSync(statePath) && fs.existsSync(eventsPath)) {
        try {
          const state = JSON.parse(fs.readFileSync(statePath, "utf8"));
          const events = fs
            .readFileSync(eventsPath, "utf8")
            .trim()
            .split(/\r?\n/)
            .filter(Boolean)
            .map((line) => JSON.parse(line));
          if (state.status === "completed" && events.length > 0) {
            return { dir: newest, state, events };
          }
        } catch {
          // The workflow may still be writing the trace files.
        }
      }
    }
    await new Promise((resolve) => setTimeout(resolve, 250));
  }

  assert.fail(`Timed out waiting for completed workflow state in ${logDir}`);
}

async function readAgentMetric(driver, sessionId) {
  const result = await driver.executeAsyncScript((done) => {
    window.__TAURI_INTERNALS__.invoke("list_agent_metrics").then(
      (metrics) => done({ ok: true, metrics }),
      (error) => done({ ok: false, error: String(error) }),
    );
  });

  assert.equal(result.ok, true, `list_agent_metrics failed: ${result.error}`);
  return result.metrics.find((entry) => entry.session_id === sessionId);
}

async function setAgentStatus(driver, sessionId, status) {
  const result = await driver.executeAsyncScript((sessionId, status, done) => {
    window.__TAURI_INTERNALS__.invoke("debug_set_agent_status", {
      sessionId,
      status,
    }).then(
      () => done({ ok: true }),
      (error) => done({ ok: false, error: String(error) }),
    );
  }, sessionId, status);

  assert.equal(result.ok, true, `debug_set_agent_status failed: ${result.error}`);
}

test("workflow detects live agent turn completion instead of timing out", { timeout: 180000 }, async (t) => {
  const harness = await createNativeHarness();
  assert.ok(harness.appPath);

  try {
    if (!skipNativeBuild) {
      ensureNativeAppBuilt(harness);
    }
  } catch (error) {
    t.skip(String(error));
    return;
  }

  prepareIsolatedHome(harness);

  const runId = `${process.pid}-${Date.now()}`;
  const workflowId = `wf-agent-completion-${runId}`;
  const providerSessionId = `workflow-completion-agent-${runId}`;
  const sessionName = `Workflow-Completion-Agent-${runId}`;
  const markerPath = path.join(harness.isolatedHome, "agent-turn-completed-marker.json");

  const previousMockScript = process.env.WARDIAN_MOCK_SCRIPT;
  const previousMockSessionId = process.env.WARDIAN_MOCK_SESSION_ID;
  const previousMockMarker = process.env.WARDIAN_MOCK_MARKER;
  let session;

  t.after(async () => {
    if (session) {
      await session.close();
    }
    if (previousMockScript === undefined) {
      delete process.env.WARDIAN_MOCK_SCRIPT;
    } else {
      process.env.WARDIAN_MOCK_SCRIPT = previousMockScript;
    }
    if (previousMockSessionId === undefined) {
      delete process.env.WARDIAN_MOCK_SESSION_ID;
    } else {
      process.env.WARDIAN_MOCK_SESSION_ID = previousMockSessionId;
    }
    if (previousMockMarker === undefined) {
      delete process.env.WARDIAN_MOCK_MARKER;
    } else {
      process.env.WARDIAN_MOCK_MARKER = previousMockMarker;
    }
  });

  process.env.WARDIAN_MOCK_SCRIPT = writeSingleTurnMockScript(harness, {
    markerPath,
  });
  process.env.WARDIAN_MOCK_SESSION_ID = providerSessionId;
  process.env.WARDIAN_MOCK_MARKER = markerPath;

  const workflowPath = seedWorkflow(harness, { workflowId });

  try {
    session = await startNativeSession(harness);
  } catch (error) {
    t.skip(String(error));
    return;
  }

  await waitForAppShell(session.driver, 20000);
  const agent = await spawnMockAgent(session.driver, {
    sessionId: providerSessionId,
    sessionName,
    folder: path.join(harness.repoRoot, "e2e-native"),
  });
  const sessionId = agent.session_id;
  assert.notEqual(sessionId, providerSessionId);
  await setAgentStatus(session.driver, sessionId, "idle");

  await new Promise((resolve) => setTimeout(resolve, 750));
  await invokeWorkflowRun(session.driver, {
    workflowPath,
    sessionId,
    workspace: path.join(harness.repoRoot, "e2e-native"),
  });

  const trace = await readLatestWorkflowState(harness, workflowId);
  const marker = fs.existsSync(markerPath)
    ? JSON.parse(fs.readFileSync(markerPath, "utf8"))
    : null;
  const metric = await readAgentMetric(session.driver, sessionId);

  assert.equal(marker?.completed, true, "mock provider did not complete its turn");
  assert.equal(metric?.current_status, "Idle");
  assert.equal(trace.state.status, "completed");
  assert.equal(trace.state.nodes?.["agent-node-1"], "completed");
  assert.ok(
    trace.events.some((event) => event.kind === "node_completed" && event.node === "agent-node-1"),
    `expected node_completed event, got ${JSON.stringify(trace.events)}`,
  );
  assert.equal(
    trace.state.registry?.nodes?.["agent-node-1"]?.output?.text,
    "workflow output complete",
  );
});
