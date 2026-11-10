#!/usr/bin/env node
/**
 * Mock Agent Emitter — deterministic provider simulator for Wardian testing.
 *
 * Emits JSON events matching the Gemini/Wardian event format to stdout.
 *
 * Environment variables:
 *   WARDIAN_MOCK_SCENARIO  — scenario name (default: "basic")
 *   WARDIAN_MOCK_DELAY_MS  — delay between events in ms (default: 100)
 *   WARDIAN_MOCK_SESSION_ID — session ID for init event (default: "mock-session-001")
 *   WARDIAN_MOCK_LOG       — optional path to mirror the event stream to, so the
 *                            chat transcript can read it back like a real provider log
 *
 * Supported scenarios:
 *   basic         — init → user → generating → model_response → turn_completed
 *   file_changes  — init → user → read/edit/write/shell tool calls → model_response → turn_completed
 *   chat_acceptance — file_changes → paged history/full body/distinct repeated turns → completed assistant (waits for stdin)
 *   resume        — init(session_id) → generating → model_response → turn_completed
 *   action_needed — init → user → action_required (waits for stdin) → turn_completed
 *   delayed_ready — init → user → generating → MOCK_INPUT_READY → model_response → turn_completed
 *   action_required_stale — init → action_required(APPROVAL_PROMPT_A) → action_required(APPROVAL_PROMPT_B)
 *   failure       — init → user → generating → exit(1)
 *   long_output   — init → user → 200 lines of text → model_response → turn_completed
 *   headless      — single JSON response object, then exit
 *   headless_failure — exits unsuccessfully before a headless result
 *   headless_delayed — waits for the configured delay, then emits the headless response
 *   headless_structured_reply — completes a structured Wardian request through the CLI
 *   multi_turn    — init → [user → generating → model_response → turn_completed] × 3
 *   interactive_multi_turn — init → action_required → stdin-driven responses × 2
 *   interactive_echo_then_response — init → action_required → accepted input → prompt echo → response
 *   ansi_output   — init → ANSI terminal output → model_response → turn_completed
 */

"use strict";

const fs = require("node:fs");
const readline = require("node:readline");
const { spawnSync } = require("node:child_process");

const transcriptLog = process.env.WARDIAN_MOCK_LOG || "";

const scenario = process.env.WARDIAN_MOCK_SCENARIO || "basic";
const delay = parseInt(process.env.WARDIAN_MOCK_DELAY_MS || "100", 10);
const sessionId = process.env.WARDIAN_MOCK_SESSION_ID || "mock-session-001";

// Check for --print flag (headless mode)
const isPrint = process.argv.includes("--print");

function emit(obj) {
  const line = JSON.stringify(obj) + "\n";
  process.stdout.write(line);
  // Real providers are observed through a log they own, and the chat
  // transcript reads normalized events back from that log rather than from the
  // terminal. Mirroring here gives the mock provider the same surface.
  if (transcriptLog) {
    try {
      fs.appendFileSync(transcriptLog, line);
    } catch {
      // The log is test scaffolding; losing it must never stop the run.
    }
  }
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function waitForStdin() {
  return new Promise((resolve) => {
    let data = "";
    process.stdin.setEncoding("utf8");
    process.stdin.on("data", (chunk) => {
      data += chunk;
      if (/[\r\n]/.test(data)) {
        const [line] = data.replace(/\r\n/g, "\n").split(/[\r\n]/);
        resolve(line.trim());
      }
    });
    process.stdin.resume();
  });
}

let callSequence = 0;
const lastCallIdByTool = new Map();

function nextCallId(toolName) {
  callSequence += 1;
  const id = `mock-call-${callSequence}`;
  lastCallIdByTool.set(toolName, id);
  return id;
}

function currentCallId(toolName) {
  return lastCallIdByTool.get(toolName) || `mock-call-${callSequence}`;
}

// Event helpers matching Gemini JSON format
const events = {
  init: (sid) => ({
    type: "init",
    session_id: sid || sessionId,
    timestamp: new Date().toISOString(),
  }),
  // Distinct text matters when a scenario emits more than one: the chat
  // transcript collapses provider messages that share their text.
  user: (content) => ({ type: "user", content: content || "mock user query" }),
  generating: () => ({
    type: "message",
    role: "assistant",
    content: "Processing your request...",
  }),
  modelResponse: (content) => ({
    type: "model",
    content: content || "Mock response completed successfully.",
  }),
  turnCompleted: () => ({ type: "result", status: "success" }),
  actionRequired: (message) => ({
    type: "action_required",
    message: message || "Approve file write to output.txt?",
  }),
  // Tool calls carry their arguments the way real providers do, so the chat
  // transcript can exercise structured edits and per-turn change summaries
  // without a provider subscription. `input` mirrors Claude's shape because
  // that is the one the normalizer preserves verbatim.
  //
  // `call_id` is not decoration: real providers correlate a call with its
  // result through one, and the transcript both pairs and de-duplicates on it.
  // Without it two calls to the same tool collapse into a single row.
  toolCall: (name, input, command) => ({
    type: "tool_call",
    call_id: nextCallId(name),
    tool_name: name,
    input,
    ...(command ? { command } : {}),
  }),
  toolResult: (name, content, status) => ({
    type: "tool_result",
    call_id: currentCallId(name),
    tool_name: name,
    content,
    status: status || "success",
  }),
};

async function runBasic() {
  emit(events.init());
  await sleep(delay);
  emit(events.user());
  await sleep(delay);
  emit(events.generating());
  await sleep(delay * 2);
  emit(events.modelResponse());
  await sleep(delay);
  emit(events.turnCompleted());
}

/**
 * A turn that edits, creates, and inspects files.
 *
 * Exists so the chat transcript's file-change surface is reachable offline:
 * every other scenario emits messages and status only, which left structured
 * edits, work-log grouping, and the per-turn change card testable solely
 * against a real provider.
 */
async function runFileChanges() {
  emit(events.init());
  await sleep(delay);
  emit(events.user("Lower the work-log grouping threshold and record a spec."));
  await sleep(delay);
  emit(events.modelResponse("Reading the transcript presentation module first."));
  await sleep(delay);

  emit(events.toolCall("Read", { file_path: "src/features/chat/chatPresentation.ts" }));
  await sleep(delay);
  emit(events.toolResult("Read", "read 337 lines"));
  await sleep(delay);

  emit(
    events.toolCall("Edit", {
      file_path: "src/features/chat/chatPresentation.ts",
      old_string: "const WORK_GROUP_MIN_ENTRIES = 4;",
      new_string: "const WORK_GROUP_MIN_ENTRIES = 3;",
    }),
  );
  await sleep(delay);
  emit(events.toolResult("Edit", "applied"));
  await sleep(delay);

  emit(
    events.toolCall("Write", {
      file_path: "docs/specs/mock-change-surface.md",
      content: "# Mock spec\n\nWritten by the mock provider.\n",
    }),
  );
  await sleep(delay);
  emit(events.toolResult("Write", "written"));
  await sleep(delay);

  emit(events.toolCall("Bash", {}, "npm run test -- --run"));
  await sleep(delay);
  emit(events.toolResult("Bash", "2830 passed"));
  await sleep(delay);

  emit(events.modelResponse("Lowered the grouping threshold and recorded the spec."));
  await sleep(delay);
  emit(events.turnCompleted());
  await sleep(delay);

  // A second, smaller turn. Three or more adjacent tool calls collapse into a
  // work-log group whose entries are one-liners, so a lone edit is the only
  // way the structured edit panel is reachable. Both shapes matter.
  emit(events.user("Now widen the change kinds."));
  await sleep(delay);
  emit(
    events.toolCall("Edit", {
      file_path: "src/features/chat/chatTurns.ts",
      old_string: "kind: \"edited\" | \"created\";",
      new_string: "kind: \"edited\" | \"created\" | \"deleted\" | \"written\";",
    }),
  );
  await sleep(delay);
  emit(events.toolResult("Edit", "applied"));
  await sleep(delay);
  emit(events.modelResponse("Widened the change kinds."));
  await sleep(delay);
  emit(events.turnCompleted());
}

/** Independent raw-provider declaration; importing it never starts a provider. */
function chatAcceptanceFixture() {
  return {
    historyTurns: 96,
    longBody: "bounded mock body ".repeat(3334).slice(0, 59999) + ".",
    repeatBody: "These are two distinct completed assistant turns with equal text.",
    latestBody: "Native acceptance completed assistant: the amber otter has finished.",
    latestTurn: "chat-acceptance-completed",
    // Each added turn has user/model/result frames. The unchanged file_changes
    // prefix has fewer than 32 frames. Status frames are included conservatively.
    maxRawFrames: 32 + (96 + 4) * 3,
  };
}

/** Real Mock grammar, consumed by the normal producer, never prebuilt Chat DTOs. */
function* chatAcceptanceEvents() {
  const fixture = chatAcceptanceFixture();
  const turns = Array.from({ length: fixture.historyTurns }, (_, index) => ({
    id: `chat-acceptance-history-${index}`,
    prompt: `Please retain controlled history turn ${index}.`,
    body: `Controlled assistant history ${index} is complete.`,
  }));
  turns.push(
    { id: "chat-acceptance-long", prompt: "Retain the full bounded body.", body: fixture.longBody },
    { id: "chat-acceptance-repeat-a", prompt: "Complete repeated turn A.", body: fixture.repeatBody },
    { id: "chat-acceptance-repeat-b", prompt: "Complete repeated turn B.", body: fixture.repeatBody },
    { id: fixture.latestTurn, prompt: "Complete the independent native timing target.", body: fixture.latestBody },
  );
  for (const turn of turns) {
    yield { type: "user", turn_id: turn.id, content: turn.prompt };
    yield { type: "model", turn_id: turn.id, content: turn.body };
    yield { type: "result", turn_id: turn.id, status: "success" };
  }
}

async function runChatAcceptance() {
  await runFileChanges();
  for (const event of chatAcceptanceEvents()) emit(event);
  // Keep live and saved-Off measurements distinct. Only the owned test's normal
  // pause ends this process; no submitted Chat prompt is needed for the fixture.
  await waitForStdin();
}

async function runResume() {
  emit(events.init(sessionId));
  await sleep(delay);
  emit(events.generating());
  await sleep(delay * 2);
  emit(events.modelResponse("Resumed session — mock response."));
  await sleep(delay);
  emit(events.turnCompleted());
}

async function runActionNeeded() {
  emit(events.init());
  await sleep(delay);
  emit(events.user());
  await sleep(delay);
  emit(events.actionRequired());
  const input = await waitForStdin();
  await sleep(delay);
  if (input.toLowerCase().startsWith("y")) {
    emit(events.modelResponse("Action approved, continuing."));
  } else {
    emit(events.modelResponse("Action denied by user."));
  }
  await sleep(delay);
  emit(events.turnCompleted());
}

async function runDelayedReady() {
  emit(events.init());
  await sleep(delay);
  emit(events.user());
  await sleep(delay);
  emit(events.generating());
  await sleep(delay * 20);
  process.stdout.write("MOCK_INPUT_READY\n");
  await sleep(delay);
  emit(events.modelResponse("Mock delayed-ready response completed."));
  await sleep(delay);
  emit(events.turnCompleted());
}

async function runActionRequiredStale() {
  emit(events.init());
  await sleep(delay);
  emit(events.actionRequired("APPROVAL_PROMPT_A"));
  process.stdout.write("APPROVAL_PROMPT_A\n");
  await sleep(delay * 20);
  emit(events.actionRequired("APPROVAL_PROMPT_B"));
  process.stdout.write("APPROVAL_PROMPT_B\n");
  await new Promise(() => {});
}

async function runFailure() {
  emit(events.init());
  await sleep(delay);
  emit(events.user());
  await sleep(delay);
  emit(events.generating());
  await sleep(delay * 3);
  process.stderr.write("Error: Mock failure scenario triggered\n");
  process.exit(1);
}

async function runLongOutput() {
  emit(events.init());
  await sleep(delay);
  emit(events.user());
  await sleep(delay);
  emit(events.generating());
  for (let i = 1; i <= 200; i++) {
    process.stdout.write(
      `[mock-output] Line ${i}: Lorem ipsum dolor sit amet, consectetur adipiscing elit.\n`
    );
    if (i % 50 === 0) await sleep(delay);
  }
  await sleep(delay);
  emit(events.modelResponse("Long output completed."));
  await sleep(delay);
  emit(events.turnCompleted());
}

async function runHeadless() {
  emit({
    response: "Mock headless execution completed successfully.",
    status: "ok",
    result: "All tasks completed.",
  });
}

async function runHeadlessDelayed() {
  await sleep(delay);
  await runHeadless();
}

async function runHeadlessStructuredReply() {
  await sleep(delay);
  const prompt = process.argv.at(-1) || "";
  const requestId = prompt.match(/Wardian request id:\s*(ask_[a-f0-9]+)/i)?.[1];
  const cliPath = process.env.WARDIAN_E2E_CLI_PATH;
  if (!requestId || !cliPath) {
    throw new Error("headless structured-reply mock requires a request id and WARDIAN_E2E_CLI_PATH");
  }

  const reply = spawnSync(
    cliPath,
    ["reply", requestId, "--status", "done", "--stdin"],
    {
      cwd: process.cwd(),
      env: process.env,
      encoding: "utf8",
      input: "Mock structured headless reply.",
    },
  );
  if (reply.status !== 0) {
    throw new Error(
      `mock structured reply failed: ${reply.stderr || reply.stdout || `exit ${reply.status}`}`,
    );
  }

  emit({
    response: "Mock structured headless reply completed.",
    status: "ok",
    result: "Structured reply recorded.",
  });
}

async function runMultiTurn() {
  emit(events.init());
  await sleep(delay);
  for (let turn = 1; turn <= 3; turn++) {
    emit(events.user());
    await sleep(delay);
    emit(events.generating());
    await sleep(delay * 2);
    emit(events.modelResponse(`Turn ${turn} response.`));
    await sleep(delay);
    emit(events.turnCompleted());
    await sleep(delay);
  }
}

async function runInteractiveMultiTurn() {
  emit(events.init());
  await sleep(delay);

  const lines = readline.createInterface({
    input: process.stdin,
    crlfDelay: Infinity,
  });
  const iterator = lines[Symbol.asyncIterator]();

  try {
    for (let turn = 1; turn <= 2; turn++) {
      emit(events.actionRequired(`Interactive turn ${turn}: waiting for input`));
      const next = await iterator.next();
      const input = next.done ? "" : String(next.value).trim();
      await sleep(delay);
      emit(events.modelResponse(`Interactive turn ${turn}: ${input}`));
      await sleep(delay);
      emit(events.turnCompleted());
      await sleep(delay);
    }
  } finally {
    lines.close();
  }
}

async function runInteractiveEchoThenResponse() {
  emit(events.init());
  await sleep(delay);

  emit(events.actionRequired("Interactive echo test: waiting for input"));
  const input = await waitForStdin();
  const marker = input.match(/[A-Z0-9_]{4,}/)?.[0] || input;
  // This mirrors the provider event that follows an accepted interactive
  // submission. Emitting only the echoed response would make the fixture look
  // like a composer repaint rather than a started provider turn.
  emit(events.user());
  await sleep(delay);
  emit(events.modelResponse(input));
  await sleep(delay);
  emit(events.modelResponse(`Actual response after echo: ${marker}`));
  await sleep(delay);
  emit(events.turnCompleted());
}

async function runAnsiOutput() {
  emit(events.init());
  await sleep(delay);
  emit(events.user());
  await sleep(delay);
  process.stdout.write("\x1b[31mANSI_TERMINAL_LINE\x1b[0m\n");
  await sleep(delay);
  emit(events.modelResponse("ANSI readable answer."));
  await sleep(delay);
  emit(events.turnCompleted());
}

async function main() {
  // Headless mode: --print normally returns the standard response, while the
  // delayed variant lets native tests observe the active headless interval.
  if (isPrint) {
    if (scenario === "headless_failure") {
      process.stderr.write("Error: Mock headless failure before result\n");
      process.exit(1);
    } else if (scenario === "headless_delayed") {
      await runHeadlessDelayed();
    } else if (scenario === "headless_structured_reply") {
      await runHeadlessStructuredReply();
    } else {
      await runHeadless();
    }
    process.exit(0);
  }

  const scenarios = {
    basic: runBasic,
    file_changes: runFileChanges,
    chat_acceptance: runChatAcceptance,
    resume: runResume,
    action_needed: runActionNeeded,
    delayed_ready: runDelayedReady,
    action_required_stale: runActionRequiredStale,
    failure: runFailure,
    long_output: runLongOutput,
    headless: runHeadless,
    headless_delayed: runHeadlessDelayed,
    headless_structured_reply: runHeadlessStructuredReply,
    multi_turn: runMultiTurn,
    interactive_multi_turn: runInteractiveMultiTurn,
    interactive_echo_then_response: runInteractiveEchoThenResponse,
    ansi_output: runAnsiOutput,
  };

  const runner = scenarios[scenario];
  if (!runner) {
    process.stderr.write(
      `Unknown scenario: "${scenario}". Available: ${Object.keys(scenarios).join(", ")}\n`
    );
    process.exit(2);
  }

  await runner();
  process.exit(0);
}

module.exports = { chatAcceptanceFixture, chatAcceptanceEvents };

if (require.main === module) {
  main().catch((err) => {
    process.stderr.write(`Mock agent error: ${err.message}\n`);
    process.exit(1);
  });
}
