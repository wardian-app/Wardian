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

function writeSingleTurnMockScript(harness, { sessionId, markerPath }) {
  const mockScript = path.join(harness.isolatedHome, "single-turn-live-mock.cjs");
  fs.writeFileSync(
    mockScript,
    `
const fs = require("node:fs");
const sessionId = process.env.WARDIAN_MOCK_SESSION_ID || "mock-session";
const markerPath = process.env.WARDIAN_MOCK_MARKER;
const emit = (obj) => process.stdout.write(JSON.stringify(obj) + "\\n");
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

emit({ type: "init", session_id: sessionId, timestamp: new Date().toISOString() });
emit({ type: "action_required", message: "waiting for workflow prompt" });

let buffer = "";
let completed = false;
process.stdin.setEncoding("utf8");
process.stdin.on("data", async (chunk) => {
  if (completed) return;
  buffer += chunk;
  if (buffer.includes("\\r") || buffer.includes("\\n")) {
    completed = true;
    emit({ type: "model", content: "partial model response, not a turn completion" });
    await sleep(1200);
    emit({ type: "result", status: "success" });
    if (markerPath) {
      fs.writeFileSync(markerPath, JSON.stringify({
        completed: true,
        input: buffer,
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

function seedWorkflow(harness, { workflowId, sessionId }) {
  const workflowsDir = path.join(harness.isolatedHome, "workflows");
  fs.mkdirSync(workflowsDir, { recursive: true });
  fs.writeFileSync(
    path.join(workflowsDir, `${workflowId}.json`),
    JSON.stringify(
      {
        id: workflowId,
        name: "Agent Completion Repro",
        settings: { max_iterations: 10, on_limit_reached: "stop" },
        nodes: [
          {
            id: "agent-node-1",
            type: "agent",
            name: "Live mock agent node",
            config: {
              agent_id: sessionId,
              mode: "inherit_resume",
              prompt: "Complete this workflow node.",
              output_format: "text",
              timeout_ms: 1500,
            },
            dependencies: null,
          },
        ],
        role_mappings: {},
      },
      null,
      2,
    ),
    "utf8",
  );
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

async function invokeWorkflowRun(driver, workflowId) {
  const result = await driver.executeAsyncScript((workflowId, done) => {
    window.__TAURI_INTERNALS__.invoke("run_workflow", { id: workflowId, payload: null }).then(
      () => done({ ok: true }),
      (error) => done({ ok: false, error: String(error) }),
    );
  }, workflowId);

  assert.equal(result.ok, true, `run_workflow failed: ${result.error}`);
}

async function readLatestWorkflowTrace(harness, workflowId, timeoutMs = 10000) {
  const logDir = path.join(harness.isolatedHome, "logs", "workflows", workflowId);
  const startedAt = Date.now();

  while (Date.now() - startedAt < timeoutMs) {
    if (fs.existsSync(logDir)) {
      const jsonFiles = fs
        .readdirSync(logDir)
        .filter((name) => name.endsWith(".json"))
        .map((name) => path.join(logDir, name))
        .sort();

      const newest = jsonFiles.at(-1);
      if (newest) {
        try {
          const events = JSON.parse(fs.readFileSync(newest, "utf8"));
          if (Array.isArray(events) && events.length > 0) {
            return { file: newest, events };
          }
        } catch {
          // The workflow may still be writing the trace file.
        }
      }
    }
    await new Promise((resolve) => setTimeout(resolve, 250));
  }

  assert.fail(`Timed out waiting for workflow trace in ${logDir}`);
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
  const sessionId = `workflow-completion-agent-${runId}`;
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
    sessionId,
    markerPath,
  });
  process.env.WARDIAN_MOCK_SESSION_ID = sessionId;
  process.env.WARDIAN_MOCK_MARKER = markerPath;

  seedWorkflow(harness, { workflowId, sessionId });

  try {
    session = await startNativeSession(harness);
  } catch (error) {
    t.skip(String(error));
    return;
  }

  await waitForAppShell(session.driver, 20000);
  await spawnMockAgent(session.driver, {
    sessionId,
    sessionName,
    folder: path.join(harness.repoRoot, "e2e-native"),
  });

  await new Promise((resolve) => setTimeout(resolve, 750));
  await invokeWorkflowRun(session.driver, workflowId);

  const trace = await readLatestWorkflowTrace(harness, workflowId);
  const marker = fs.existsSync(markerPath)
    ? JSON.parse(fs.readFileSync(markerPath, "utf8"))
    : null;
  const metric = await readAgentMetric(session.driver, sessionId);

  assert.equal(marker?.completed, true, "mock provider did not complete its turn");
  assert.equal(metric?.current_status, "Idle");
  assert.match(trace.events[0]?.output?.text ?? "", /"result","status":"success"|type":"result"/);
  assert.deepEqual(
    trace.events.map((event) => ({ node_id: event.node_id, status: event.status, error: event.error })),
    [{ node_id: "agent-node-1", status: "completed", error: null }],
  );
});
