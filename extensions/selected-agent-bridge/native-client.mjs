/** Explicit native controller port. Importing this module starts no process. */
import { spawn } from 'node:child_process';
import { isAbsolute } from 'node:path';
import { TextDecoder } from 'node:util';

const MAX_BYTES = 1024 * 1024;
const VERSION = '2025-11-25';
const object = value => value !== null && typeof value === 'object' && !Array.isArray(value);
const text = value => typeof value === 'string' && value.trim().length > 0;
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;
const TOOLS = ['list_agents', 'send_message', 'followup_task', 'receive_messages', 'wait_agent', 'interrupt_agent'];
const SCOPES = ['agents:read', 'messages:send', 'tasks:submit', 'agents:wake', 'controller:receive', 'controller:wait', 'controller:ack', 'agents:interrupt'];
const failure = () => new Error('Native controller exchange failed; delivery may be uncertain. Do not retry automatically.');

/** Map only the verified, redacted native principal; token and caller principal never supply attribution. */
export function nativePrincipalToGrant(principal, resource) {
  if (!object(principal) || !UUID.test(principal.id) || !text(principal.name) || principal.name.length > 100 ||
      /[\r\n]/.test(principal.name) || principal.resource !== resource ||
      !Array.isArray(principal.agent_ids) || principal.agent_ids.length < 1 || principal.agent_ids.length > 100 ||
      !principal.agent_ids.every(id => typeof id === 'string' && UUID.test(id)) || new Set(principal.agent_ids).size !== principal.agent_ids.length ||
      !Array.isArray(principal.scopes) || !principal.scopes.every(scope => SCOPES.includes(scope)) ||
      (principal.expires_at != null && (!Number.isSafeInteger(principal.expires_at) || principal.expires_at < 0 || !Number.isSafeInteger(principal.expires_at * 1000))) ||
      (principal.revoked_at != null && (!Number.isSafeInteger(principal.revoked_at) || principal.revoked_at < 0))) throw failure();
  return { controller_id: principal.id, controller_name: principal.name, grant_id: principal.id, audience: principal.resource,
    expires_at_ms: principal.expires_at == null ? Number.MAX_SAFE_INTEGER : principal.expires_at * 1000,
    revoked: principal.revoked_at != null, agent_uuids: [...principal.agent_ids], scopes: [...principal.scopes] };
}

/** Operator-only fixed executable/arguments. Each exchange owns one child, kills it on deadline, and never retries. */
export function createNativeControllerClient({ cli_path, wardian_home, resource, argv_prefix = [], timeout_ms = 75000 }) {
  let url;
  try { url = new URL(resource); } catch { throw new Error('Invalid native client resource'); }
  if (typeof cli_path !== 'string' || !isAbsolute(cli_path) || typeof wardian_home !== 'string' || !isAbsolute(wardian_home) ||
      url.protocol !== 'https:' || url.username || url.password || url.search || url.hash || url.pathname === '/' ||
      !Array.isArray(argv_prefix) || !argv_prefix.every(arg => typeof arg === 'string') ||
      !Number.isInteger(timeout_ms) || timeout_ms < 1 || timeout_ms > 75000) throw new Error('Invalid native client operator configuration');
  const prefix = [...argv_prefix];

  function exchange(token, args, key, mcpCall) {
    if (!text(token) || token.length > 8192 || /[\r\n\0]/.test(token)) return Promise.reject(failure());
    return new Promise((resolve, reject) => {
      const env = { ...process.env, WARDIAN_HOME: wardian_home, WARDIAN_CONTROLLER_TOKEN: token, WARDIAN_CONTROLLER_RESOURCE: resource };
      delete env.WARDIAN_CONTROLLER_REQUEST_KEY;
      // Controller mode must never inherit managed-agent attribution as a fallback.
      delete env.WARDIAN_SESSION_ID;
      // Platform tunnel credentials are not Wardian native credentials.
      delete env.CONTROL_PLANE_API_KEY;
      delete env.OPENAI_API_KEY;
      if (key !== undefined) env.WARDIAN_CONTROLLER_REQUEST_KEY = key;
      let child;
      try { child = spawn(cli_path, [...prefix, ...args], { shell: false, windowsHide: true, stdio: ['pipe', 'pipe', 'pipe'], env }); }
      catch { reject(failure()); return; }
      child.stderr.resume();
      let settled = false;
      let result;
      let failed = false;
      let bytes = 0;
      let buffer = Buffer.alloc(0);
      let stage = 0;
      const decoder = new TextDecoder('utf-8', { fatal: true });
      const stop = (value, bad = false) => {
        if (settled) return;
        settled = true; result = value; failed = bad; clearTimeout(timer);
        child.stdin.destroy();
        if (child.exitCode === null && child.signalCode === null) child.kill('SIGKILL');
      };
      const timer = setTimeout(() => stop(undefined, true), timeout_ms);
      const send = value => {
        const frame = JSON.stringify(value) + '\n';
        if (Buffer.byteLength(frame) > MAX_BYTES) throw failure();
        child.stdin.write(frame, err => { if (err) stop(undefined, true); });
      };
      child.stdin.on('error', () => stop(undefined, true));
      child.on('error', () => stop(undefined, true));
      child.stdout.on('data', chunk => {
        if (settled) return;
        try {
          bytes += chunk.length;
          if (bytes > MAX_BYTES) throw failure();
          buffer = Buffer.concat([buffer, chunk]);
          if (!mcpCall) return;
          let newline;
          while ((newline = buffer.indexOf(10)) !== -1) {
            const frame = JSON.parse(decoder.decode(buffer.subarray(0, newline)));
            buffer = buffer.subarray(newline + 1);
            if (!object(frame) || frame.jsonrpc !== '2.0' || frame.id !== (stage === 0 ? 1 : 2) ||
                !Object.hasOwn(frame, 'result') || Object.hasOwn(frame, 'error')) throw failure();
            if (stage === 0) {
              if (frame.result?.protocolVersion !== VERSION || !object(frame.result.capabilities?.tools)) throw failure();
              stage = 1;
              send({ jsonrpc: '2.0', method: 'notifications/initialized' });
              send({ jsonrpc: '2.0', id: 2, method: 'tools/call', params: { name: mcpCall.operation, arguments: mcpCall.arguments } });
            } else {
              if (!object(frame.result) || !Array.isArray(frame.result.content) || buffer.length) throw failure();
              stop(frame.result); return;
            }
          }
        } catch { stop(undefined, true); }
      });
      child.on('close', code => {
        clearTimeout(timer);
        if (settled) { if (failed) reject(failure()); else resolve(result); return; }
        settled = true;
        if (mcpCall || code !== 0) { reject(failure()); return; }
        try { resolve(JSON.parse(decoder.decode(buffer))); } catch { reject(failure()); }
      });
      if (mcpCall) {
        try { send({ jsonrpc: '2.0', id: 1, method: 'initialize', params: { protocolVersion: VERSION,
          capabilities: {}, clientInfo: { name: 'wardian-controller-native-client', version: '0.1.0' } } }); }
        catch { stop(undefined, true); }
      } else child.stdin.end();
    });
  }

  return {
    async verify_access_token(token, context) {
      if (context?.resource !== resource) throw failure();
      const principal = await exchange(token, ['controller', 'verify', '--resource', resource]);
      return nativePrincipalToGrant(principal, resource);
    },
    async invoke(call) {
      if (!object(call) || call.principal?.audience !== resource || !TOOLS.includes(call.operation) || !object(call.arguments) || !text(call.idempotency_key) ||
          Buffer.byteLength(call.idempotency_key) > 256 || /[\r\n\0]/.test(call.idempotency_key)) throw failure();
      return exchange(call.access_token, ['mcp', 'serve', '--controller'], call.idempotency_key, call);
    },
  };
}
