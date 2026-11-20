/* global wardianBridge */
// Loaded only from the launch-scoped plugin prepared by Wardian. No Node APIs,
// shell commands, transcript rereads, prompt rewrites, or provider setting writes.
const safeId = (id) => typeof id === "string" && /^[A-Za-z0-9_-]{1,128}$/.test(id);
const own = (next) => next.origin?.plugin === "engine" && next.origin?.tier === "core";
const enqueue = (state, task) => {
  state.serial = state.serial.then(task, task);
  return state.serial;
};
// Observation failures must leave the provider's own middleware result intact.
const observe = async (task) => { try { await task(); } catch { /* Unavailable, never inferred. */ } };

// Claude's capability scanner follows $ only into module-scope declarations.
// Keep API helpers here, with explicit per-registration state rather than a
// module-global outbox that could mix separate registrations.
async function flush($, state) {
  const { key, binding, captured } = state;
  const pending = (await $.store.get(key)) ?? [];
  if (!Array.isArray(pending)) return;
  for (const receipt of captured.values()) {
    if (!pending.some((item) => item.turn_id === receipt.turn_id)) pending.push(receipt);
  }
  // Keep the in-memory copy if the durable store rejects, so a later timer
  // can retry without re-running a prompt or reconstructing an answer.
  await $.store.set(key, pending);
  captured.clear();
  const remaining = [];
  for (const receipt of pending) {
    if (receipt.launch_id !== binding.launch_id || !safeId(receipt.turn_id)) continue;
    const path = `${binding.directory}/notice-${receipt.turn_id}`;
    if (await $.fs.exists(`${path}.ack`)) continue;
    remaining.push(receipt);
    const text = JSON.stringify(receipt);
    // $.fs.write is not atomic. The backend requires a complete validated
    // JSON record; retries repair torn files without changing the receipt.
    if (!(await $.fs.exists(`${path}.json`)) || (await $.fs.read(`${path}.json`)) !== text) {
      await $.fs.write(`${path}.json`, text);
    }
  }
  await $.store.set(key, remaining);
}

async function initialize($, state) {
  if (state.initialized) return;
  const { binding } = state;
  const provider_session_id = await $.session.id();
  if (!safeId(provider_session_id)) return;
  await $.fs.write(`${binding.directory}/ready.json`, JSON.stringify({
    wardian_session_id: binding.wardian_session_id,
    launch_id: binding.launch_id,
    provider_session_id,
  }));
  $.clock.every(2000, () => observe(() => enqueue(state, () => flush($, state))));
  state.initialized = true;
  await flush($, state);
}

export function register(on) {
  const binding = wardianBridge;
  const key = `outbox:${binding.launch_id}`;
  const state = {
    binding, key, prompts: new Set(), turns: new Map(), captured: new Map(),
    serial: Promise.resolve(), initialized: false,
  };
  const { prompts, turns, captured } = state;

  on("session.start", async ($, e, next) => {
    const result = await next(e);
    if (own(next)) await observe(() => enqueue(state, () => initialize($, state)));
    return result;
  });

  on("prompt.submit", async ($, e, next) => {
    const candidate = {
      text: e.text,
      origin: e.origin?.kind,
      accepted: false,
      used: false,
    };
    if (own(next) && ["composer", "bridge"].includes(candidate.origin)
      && typeof candidate.text === "string" && candidate.text.trim()) prompts.add(candidate);
    try {
      const result = await next(e);
      if (result?.drop !== undefined) prompts.delete(candidate);
      else {
        candidate.text = typeof result?.text === "string" ? result.text : candidate.text;
        candidate.accepted = true;
      }
      return result;
    } catch (error) {
      prompts.delete(candidate);
      throw error;
    }
  });

  on("turn.start", async ($, e, next) => {
    const result = await next(e);
    if (!own(next) || e.agentId !== undefined || !safeId(e.turnId) || turns.has(e.turnId)) return result;
    await observe(async () => {
      const matches = [...prompts].filter((prompt) => !prompt.used && prompt.text === e.text);
      if (matches.length !== 1) return;
      const prompt = matches[0];
      prompt.used = true;
      turns.set(e.turnId, {
        prompt, text: e.text, started: await $.clock.now(),
        provider_session_id: await $.session.id(),
      });
    });
    return result;
  });

  on("turn.complete", async ($, e, next) => {
    // Capture the immutable provider answer now. Never look up the latest
    // transcript after a subsequent prompt has started.
    const turn = turns.get(e.turnId);
    const snapshot = { answer: e.answer, is_aborted: e.isAborted, reason: e.reason ?? null };
    const result = await next(e);
    if (!own(next) || e.agentId !== undefined || !safeId(e.turnId) || !turn?.prompt.accepted
      || turn.text !== turn.prompt.text || typeof snapshot.answer !== "string"
      || typeof snapshot.is_aborted !== "boolean") return result;
    turns.delete(e.turnId);
    prompts.delete(turn.prompt);
    await observe(() => enqueue(state, async () => {
      await initialize($, state);
      const provider_session_id = await $.session.id();
      if (!safeId(provider_session_id) || provider_session_id !== turn.provider_session_id) return;
      const receipt = {
        schema: 1,
        wardian_session_id: binding.wardian_session_id,
        launch_id: binding.launch_id,
        provider_session_id,
        turn_id: e.turnId,
        agent_id: null,
        prompt_origin: turn.prompt.origin,
        prompt_text: turn.prompt.text,
        start_text: turn.text,
        ...snapshot,
        started_at_ms: turn.started,
        captured_at_ms: await $.clock.now(),
      };
      captured.set(receipt.turn_id, receipt);
      await flush($, state);
    }));
    return result;
  });
}
