import { createConnection } from 'node:net';
import { randomUUID } from 'node:crypto';
import { parseConfig } from './protocol.mjs';
import { createBridge } from './bridge.mjs';

function reportSessionStartRejection(code) {
  // Keep extension diagnostics credential-free and independent of session data.
  console.error(`[Wardian] Pi bridge rejection stage=session_start code=${code}`);
}

/** Explicitly load with Pi -e; never installs itself or controls another session. */
export default function wardianPiMessaging(pi) {
  let bridge;
  let started = false;
  pi.on('session_start', (_event, ctx) => {
    if (started) return; // Reload requires a fresh factory and broker-approved nonce.
    started = true;
    const raw = process.env.WARDIAN_PI_BRIDGE_CONFIG;
    // Avoid forwarding the bearer credential to later tool children.
    delete process.env.WARDIAN_PI_BRIDGE_CONFIG;
    if (!raw) {
      reportSessionStartRejection('config_missing');
      return;
    }
    let config;
    try {
      config = parseConfig(raw);
    } catch {
      reportSessionStartRejection('config_parse_failed');
      return;
    }
    if (ctx.mode !== 'tui') {
      reportSessionStartRejection('mode_not_tui');
      return;
    }
    if (ctx.sessionManager.getSessionId() !== config.session_id) {
      reportSessionStartRejection('session_id_mismatch');
      return;
    }
    if (ctx.sessionManager.getSessionFile() !== config.session_file) {
      reportSessionStartRejection('session_file_mismatch');
      return;
    }
    const socket = createConnection({ host: config.host, port: config.port });
    bridge = createBridge({ config, runtimeNonce: randomUUID(), pid: process.pid, pi, ctx, socket });
  });
  pi.on('message_start', event => bridge?.messageStart(event));
  pi.on('agent_settled', () => bridge?.settled());
  pi.on('session_shutdown', () => { started = true; bridge?.stop(); bridge = undefined; });
}
