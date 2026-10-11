// @tier ci — Pure reply-correlation checks; no native app or provider.
import test from "node:test";
import assert from "node:assert/strict";
import {
  correlatedReply,
  offAgentNativeBinding,
  assertDetachedTerminal,
  assertNativeSession,
  assertOpenCodeHttpSession,
  assertOpenCodeCompletedAnswer,
  assertBusyTaskDeferred,
} from "../lib/canonical-messaging.mjs";

test("Off mock capability observation preserves the exact unsupported contract before and after information", () => {
  const errorPayload = { schema: 1, error: {
    code: "not_supported", message: "mock has no Wardian native transport", hint: null,
  } };
  let calls = 0;
  const executeSync = (cli, args, options) => {
    calls++;
    assert.equal(cli, "fixture-cli");
    assert.deepEqual(args, ["delivery", "capabilities", "peer"]);
    assert.equal(options.cwd, "fixture-cwd");
    assert.equal(options.env.WARDIAN_HOME, "fixture-home");
    assert.equal(Object.hasOwn(options.env, "WARDIAN_SESSION_ID"), false);
    throw Object.assign(new Error("CLI capability rejection"), {
      status: 1, signal: null, stdout: "", stderr: `${JSON.stringify(errorPayload)}\n`,
    });
  };
  const before = offAgentNativeBinding("fixture-cli", "fixture-home", "fixture-cwd", "peer", "mock", executeSync);
  const after = offAgentNativeBinding("fixture-cli", "fixture-home", "fixture-cwd", "peer", "mock", executeSync);
  assert.deepEqual(before, { provider: "mock", native_supported: false, binding: null,
    capability_error: errorPayload.error });
  assert.deepEqual(after, before);
  assert.equal(calls, 2, "Each observation must execute the capability command exactly once");
});

test("Off mock capability observation fails closed on unrelated errors and unexpected success", () => {
  const known = { schema: 1, error: {
    code: "not_supported", message: "mock has no Wardian native transport", hint: null,
  } };
  const failure = (patch = {}) => Object.assign(new Error("CLI failure"), {
    status: 1, signal: null, stdout: "", stderr: JSON.stringify(known), ...patch,
  });
  const payloads = [
    { ...known, schema: 2 },
    { ...known, binding: null },
    { ...known, error: { ...known.error, code: "permission_denied" } },
    { ...known, error: { ...known.error, code: "app_not_running" } },
    { ...known, error: { ...known.error, message: "codex has no Wardian native transport" } },
    { ...known, error: { ...known.error, hint: "unexpected recovery" } },
  ];
  const failures = [
    ...payloads.map((payload) => failure({ stderr: JSON.stringify(payload) })),
    failure({ status: 2 }), failure({ status: null, code: "ETIMEDOUT" }),
    failure({ signal: "SIGTERM" }), failure({ stdout: "unexpected output" }),
    failure({ stderr: "not JSON" }), failure({ stderr: "x".repeat(64 * 1024 + 1) }),
    failure({ stderr: undefined }), failure({ status: undefined, code: "ECONNREFUSED" }),
  ];
  for (const error of failures) {
    assert.throws(() => offAgentNativeBinding("fixture-cli", "fixture-home", "fixture-cwd", "peer", "mock",
      () => { throw error; }), `Unexpected failure must not become a null binding: ${error.code ?? error.status}`);
  }
  assert.throws(() => offAgentNativeBinding("fixture-cli", "fixture-home", "fixture-cwd", "peer", "mock",
    () => JSON.stringify({ schema: 1, binding: null })), /unexpectedly returned/);
  let calls = 0;
  assert.throws(() => offAgentNativeBinding("fixture-cli", "fixture-home", "fixture-cwd", "peer", "codex",
    () => { calls++; }), /retain its mock provider/);
  assert.equal(calls, 0, "A changed provider is rejected before querying its runtime");
});

test("canonical reply requires task correlation, peer and completed status", () => {
  const reply = { interaction_id: "reply-1", kind: "reply", parent_interaction_id: "task-1", sender: "peer", reply_status: "done", message: "marker" };
  const page = (row) => ({ messages: [row] });
  assert.equal(correlatedReply(page(reply), "task-1", "peer", "marker"), reply);
  assert.equal(correlatedReply(page({ ...reply, parent_interaction_id: "other" }), "task-1", "peer", "marker"), null);
  assert.equal(correlatedReply(page({ ...reply, kind: "message" }), "task-1", "peer", "marker"), null);
  for (const patch of [{ sender: "other" }, { reply_status: "failed" }, { message: "wrong" }]) {
    assert.throws(() => correlatedReply(page({ ...reply, ...patch }), "task-1", "peer", "marker"));
  }
  assert.throws(() => correlatedReply({ messages: [reply, reply] }, "task-1", "peer", "marker"));
});

test("detached terminal proof rejects replacement and later PTY activity", () => {
  const before = { session_id: "peer", runtime_generation: 4, sequence_barrier: 20 };
  assertDetachedTerminal(before, { ...before });
  for (const patch of [{ session_id: "other" }, { runtime_generation: 5 }, { sequence_barrier: 21 }]) {
    assert.throws(() => assertDetachedTerminal(before, { ...before, ...patch }));
  }
});

test("native qualification is per route and rejects identity changes or unnegotiated candidates", () => {
  for (const provider of ["codex", "claude", "opencode", "pi", "antigravity"]) {
    const capability = { native_negotiated: true, binding: { target_agent_id: "peer", provider,
      provider_session_id: "existing-session", generation: 7, transport: "native-protocol",
      capabilities: { persistent_session: true, positive_turn_start: true } } };
    const before = assertNativeSession(capability, "peer", provider);
    assert.throws(() => assertNativeSession({ ...capability, native_negotiated: false }, "peer", provider));
    for (const patch of [{ provider_session_id: "new" }, { generation: 8 }, { transport: "pty" }]) {
      assert.throws(() => assertNativeSession({ ...capability, binding: { ...capability.binding, ...patch } }, "peer", provider, before));
    }
  }
});

test("OpenCode HTTP keeps admission separate from provider-authored completion", () => {
  const capability = { native_negotiated: true, binding: { target_agent_id: "peer", provider: "opencode",
    provider_session_id: "existing-session", generation: 7, transport: "opencode_http",
    capabilities: { persistent_session: true, positive_turn_start: false, late_reconciliation: true } } };
  const identity = assertOpenCodeHttpSession(capability, "peer");
  const claim = { request_id: "task-1", recipient: "peer", sender: "origin", generation: 7,
    owner: "provider_accepted", status: "completed" };
  const reply = { interaction_id: "reply-1", kind: "reply", parent_interaction_id: "task-1",
    sender: "peer", reply_status: "done", message: "provider answer" };
  assert.deepEqual(
    assertOpenCodeCompletedAnswer({ capability, agentId: "peer", expected: identity,
      requestId: "task-1", reply, claim, expectedMessage: "provider answer" }),
    { ...identity, request_id: "task-1", reply_interaction_id: "reply-1", turn_started: false, completed: true },
  );
  assert.throws(() => assertOpenCodeHttpSession({
    ...capability,
    binding: { ...capability.binding, capabilities: { ...capability.binding.capabilities, positive_turn_start: true } },
  }, "peer"));
  assert.throws(() => assertOpenCodeCompletedAnswer({ capability, agentId: "peer", requestId: "other",
    reply, claim, expectedMessage: "provider answer" }));
  assert.throws(() => assertOpenCodeCompletedAnswer({ capability, agentId: "peer", requestId: "task-1",
    reply, claim: { ...claim, status: "provider_accepted" }, expectedMessage: "provider answer" }));
});

test("busy task evidence rejects overlap, duplicate delivery and steering into the active turn", () => {
  const active = { turn_id: "active", status: "inProgress" };
  const delivery = { frame_type: "canonical_task", message_id: "queued", turn_id: "later" };
  assert.equal(assertBusyTaskDeferred({ turns: [active], host_deliveries: [] }, "active", "queued"), undefined);
  assert.throws(() => assertBusyTaskDeferred({ turns: [active], host_deliveries: [delivery] }, "active", "queued"));
  const finished = { turns: [{ ...active, status: "completed" }], host_deliveries: [delivery] };
  assert.equal(assertBusyTaskDeferred(finished, "active", "queued"), "later");
  assert.throws(() => assertBusyTaskDeferred({ ...finished, host_deliveries: [delivery, delivery] }, "active", "queued"));
  assert.throws(() => assertBusyTaskDeferred({ ...finished, host_deliveries: [{ ...delivery, turn_id: "active" }] }, "active", "queued"));
});
