import { FrameDecoder, encodeFrame, exact, sameSecret, validateDelivery, customMessage } from './protocol.mjs';

/** One connection per extension runtime. Disconnection never reconnects or replays. */
export function createBridge({ config, runtimeNonce, pid, pi, ctx, socket, handshakeMs = 5000 }) {
  const binding = Object.freeze({ target_id: config.target_id, generation: config.generation,
    session_id: config.session_id, runtime_nonce: runtimeNonce });
  const decoder = new FrameDecoder();
  let stopped = false, authenticated = false, rx = 0, tx = 0, pending = null;
  const seen = new Set();
  const identityValid = () => ctx.mode === 'tui' && ctx.sessionManager.getSessionId() === config.session_id
    && ctx.sessionManager.getSessionFile() === config.session_file;
  const stop = () => { if (stopped) return; stopped = true; clearTimeout(timer); socket.destroy(); };
  const send = payload => {
    if (stopped) return false;
    // A false write result has already accepted bytes: close, never retry them.
    try {
      if (!socket.write(encodeFrame({ version: 1, ...binding, seq: ++tx, ...payload }))) { stop(); return false; }
    } catch { stop(); return false; }
    return true;
  };
  const timer = setTimeout(stop, handshakeMs);
  timer.unref?.();
  function receive(frame) {
    if (stopped) return;
    const keys = ['version', ...Object.keys(binding), 'seq', 'type'];
    exact(frame, [...keys, ...(authenticated ? ['delivery'] : ['token'])]);
    if (!identityValid() || frame.version !== 1 || !Number.isSafeInteger(frame.seq) || frame.seq !== rx + 1
        || Object.entries(binding).some(([k, v]) => frame[k] !== v)) throw new Error('binding');
    rx = frame.seq;
    if (!authenticated) {
      if (frame.type !== 'welcome' || !sameSecret(frame.token, config.token)) throw new Error('auth');
      authenticated = true;
      clearTimeout(timer);
      send({ type: 'ready', capabilities: { task: true, information: false, cancel: false, completion: false } });
      return;
    }
    if (frame.type !== 'deliver') throw new Error('type');
    const d = validateDelivery(frame.delivery);
    if (seen.has(d.message_id)) { send({ type: 'rejected', message_id: d.message_id, reason: 'duplicate_no_replay' }); return; }
    if (seen.size >= 1024) { stop(); return; }
    seen.add(d.message_id);
    if (d.kind === 'information' || pending) {
      send({ type: 'rejected', message_id: d.message_id, reason: d.kind === 'information' ? 'information_unsupported' : 'busy' });
      return;
    }
    pending = { message: customMessage(d, binding), observed: false };
    if (!send({ type: 'received', message_id: d.message_id })) return;
    // Persist uncertainty before calling the void Pi API. Receipt is not provider acceptance.
    if (!send({ type: 'submitted_unconfirmed', message_id: d.message_id })) return;
    try { pi.sendMessage(pending.message, { triggerTurn: true, deliverAs: 'followUp' }); }
    catch { stop(); } // No retry, no false rejection after possible handoff.
  }
  socket.on('connect', () => {
    if (!identityValid()) { stop(); return; }
    send({ type: 'hello', token: config.token, pid, session_file: config.session_file });
  });
  socket.on('data', data => { try { decoder.feed(data, receive); } catch { stop(); } });
  socket.on('error', stop);
  socket.on('close', stop);
  return {
    stop,
    messageStart(event) {
      if (stopped || !authenticated || !pending || pending.observed) return;
      if (!identityValid()) { stop(); return; }
      const m = event.message, expected = pending.message;
      if (m?.role !== 'custom' || m.customType !== expected.customType || m.content !== expected.content) return;
      try {
        exact(m.details, Object.keys(expected.details));
        if (Object.entries(expected.details).some(([k, v]) => m.details[k] !== v)) return;
      } catch { return; }
      pending.observed = true;
      send({ type: 'observed_consumption', message_id: expected.details.message_id });
    },
    settled() {
      if (stopped || !pending?.observed) return;
      if (!identityValid()) { stop(); return; }
      if (!ctx.isIdle()) return;
      // A settled run does not prove a correlated completed reply or provider success.
      if (send({ type: 'observed_settled', message_id: pending.message.details.message_id })) pending = null;
    },
  };
}
