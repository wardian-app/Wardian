import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { promisify } from "node:util";

const execute = promisify(execFile);

/** One managed-origin operation. A lost receipt is never retried. */
export async function messageCli(cli, home, cwd, sender, args) {
  assert.ok(sender, "Canonical messaging requires a managed sender");
  const { stdout } = await execute(cli, ["message", ...args], {
    cwd, env: { ...process.env, WARDIAN_HOME: home, WARDIAN_SESSION_ID: sender },
    timeout: 65_000, windowsHide: true, maxBuffer: 1024 * 1024,
  });
  return JSON.parse(stdout);
}

/** Text alone is not a reply: require the admitted task and exact peer identity. */
export function correlatedReply(page, requestId, sender, expected) {
  const replies = page.messages.filter((row) => row.kind === "reply" && row.parent_interaction_id === requestId);
  assert.ok(replies.length <= 1, "Duplicate replies to the same task");
  if (!replies.length) return null;
  const reply = replies[0];
  assert.equal(reply.sender, sender);
  assert.equal(reply.reply_status, "done");
  assert.equal(reply.message, expected);
  assert.ok(reply.interaction_id);
  return reply;
}

/** Call only after the native debug hook has removed the owned runtime. */
export function assertDetachedTerminal(before, after) {
  assert.equal(after.session_id, before.session_id);
  assert.equal(after.runtime_generation, before.runtime_generation, "Messaging recreated a PTY runtime");
  assert.equal(after.sequence_barrier, before.sequence_barrier, "Detached PTY received new activity");
}

/** Qualification needs an existing identity, not merely candidate capabilities. */
export function assertNativeSession(capability, agentId, provider, expected = null) {
  assert.equal(capability.native_negotiated, true, "Existing-session native route is not negotiated; no composer fallback is permitted in this case");
  const binding = capability.binding;
  assert.equal(binding?.target_agent_id, agentId);
  assert.equal(binding.provider, provider);
  assert.ok(binding.provider_session_id);
  assert.ok(Number.isSafeInteger(binding.generation));
  assert.equal(binding.capabilities.persistent_session, true);
  assert.equal(binding.capabilities.positive_turn_start, true);
  assert.ok(binding.transport && !/pty|composer/i.test(binding.transport));
  const identity = { provider, provider_session_id: binding.provider_session_id, generation: binding.generation, transport: binding.transport };
  if (expected) assert.deepEqual(identity, expected, "Messaging replaced the existing native provider session/generation");
  return identity;
}

/**
 * OpenCode HTTP proves a persistent owner and exact session, but its 204
 * response is admission only. Turn start and completion are established by
 * the later provider-authored canonical answer.
 */
export function assertOpenCodeHttpSession(capability, agentId, expected = null) {
  assert.equal(capability.native_negotiated, true, "OpenCode existing-session HTTP owner was not negotiated");
  const binding = capability.binding;
  assert.equal(binding?.target_agent_id, agentId);
  assert.equal(binding.provider, "opencode");
  assert.ok(binding.provider_session_id);
  assert.ok(Number.isSafeInteger(binding.generation));
  assert.equal(binding.capabilities.persistent_session, true);
  assert.equal(binding.capabilities.positive_turn_start, false, "HTTP admission must not claim turn start");
  assert.equal(binding.capabilities.late_reconciliation, true);
  assert.equal(binding.transport, "opencode_http");
  const identity = {
    provider: "opencode",
    provider_session_id: binding.provider_session_id,
    generation: binding.generation,
    transport: binding.transport,
  };
  if (expected) assert.deepEqual(identity, expected, "OpenCode messaging replaced the existing HTTP owner session/generation");
  return identity;
}

/**
 * Admission is not completion. Call this only after the real OpenCode
 * provider has emitted the exact canonical reply for the admitted request.
 */
export function assertOpenCodeCompletedAnswer({
  capability,
  agentId,
  expected = null,
  requestId,
  reply,
  claim,
  expectedMessage,
}) {
  const identity = assertOpenCodeHttpSession(capability, agentId, expected);
  assert.equal(claim?.request_id, requestId, "Native claim is for a different canonical request");
  assert.equal(claim.recipient, agentId);
  assert.equal(claim.generation, identity.generation);
  assert.ok(["provider_accepted", "provider_visible", "provider_completed"].includes(claim.owner));
  assert.equal(claim.status, "completed", "HTTP admission did not reach completed interaction state");
  assert.equal(reply?.kind, "reply");
  assert.equal(reply.parent_interaction_id, requestId);
  assert.equal(reply.sender, agentId);
  assert.equal(reply.reply_status, "done");
  assert.ok(reply.interaction_id);
  if (expectedMessage !== undefined) assert.equal(reply.message, expectedMessage);
  return {
    ...identity,
    request_id: requestId,
    reply_interaction_id: reply.interaction_id,
    turn_started: false,
    completed: true,
  };
}

/** A queued canonical task cannot appear in provider history during the active turn. */
export function assertBusyTaskDeferred(trace, activeTurnId, queuedRequestId) {
  const deliveries = trace.host_deliveries.filter((row) => row.frame_type === "canonical_task" && row.message_id === queuedRequestId);
  assert.ok(deliveries.length <= 1, "Busy task was delivered more than once");
  const queuedTurn = deliveries[0]?.turn_id;
  if (trace.turns.some((turn) => turn.turn_id === activeTurnId && turn.status === "inProgress")) {
    assert.equal(queuedTurn, undefined, "Busy task started before the active task finished");
  }
  if (queuedTurn) assert.notEqual(queuedTurn, activeTurnId, "Busy task was steered into the earlier turn");
  return queuedTurn;
}
