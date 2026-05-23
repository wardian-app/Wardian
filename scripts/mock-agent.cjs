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
 *
 * Supported scenarios:
 *   basic         — init → user → generating → model_response → turn_completed
 *   resume        — init(session_id) → generating → model_response → turn_completed
 *   action_needed — init → user → action_required (waits for stdin) → turn_completed
 *   delayed_ready — init → user → generating → MOCK_INPUT_READY → model_response → turn_completed
 *   action_required_stale — init → action_required(APPROVAL_PROMPT_A) → action_required(APPROVAL_PROMPT_B)
 *   failure       — init → user → generating → exit(1)
 *   long_output   — init → user → 200 lines of text → model_response → turn_completed
 *   headless      — single JSON response object, then exit
 *   multi_turn    — init → [user → generating → model_response → turn_completed] × 3
 *   interactive_multi_turn — init → action_required → stdin-driven responses × 2
 *   interactive_echo_then_response — init → action_required → prompt echo → response
 *   ansi_output   — init → ANSI terminal output → model_response → turn_completed
 */

"use strict";

const readline = require("node:readline");

const scenario = process.env.WARDIAN_MOCK_SCENARIO || "basic";
const delay = parseInt(process.env.WARDIAN_MOCK_DELAY_MS || "100", 10);
const sessionId = process.env.WARDIAN_MOCK_SESSION_ID || "mock-session-001";

// Check for --print flag (headless mode)
const isPrint = process.argv.includes("--print");

function emit(obj) {
  process.stdout.write(JSON.stringify(obj) + "\n");
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

// Event helpers matching Gemini JSON format
const events = {
  init: (sid) => ({
    type: "init",
    session_id: sid || sessionId,
    timestamp: new Date().toISOString(),
  }),
  user: () => ({ type: "user", content: "mock user query" }),
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
  // Headless mode: --print flag overrides scenario
  if (isPrint) {
    await runHeadless();
    process.exit(0);
  }

  const scenarios = {
    basic: runBasic,
    resume: runResume,
    action_needed: runActionNeeded,
    delayed_ready: runDelayedReady,
    action_required_stale: runActionRequiredStale,
    failure: runFailure,
    long_output: runLongOutput,
    headless: runHeadless,
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

main().catch((err) => {
  process.stderr.write(`Mock agent error: ${err.message}\n`);
  process.exit(1);
});
