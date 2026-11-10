#!/usr/bin/env node
/** Optional local MCP adapter. Canonical identity, admission and inbox state remain in Wardian. */
import { spawn } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { isAbsolute, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { TextDecoder } from 'node:util';

const MAX_FRAME = 1024 * 1024;
const MAX_CALLS = 10000;
const TOOLS = ['list_agents', 'send_message', 'followup_task', 'receive_messages', 'wait_agent', 'reply', 'interrupt_agent'];
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;
const object = value => value !== null && typeof value === 'object' && !Array.isArray(value);
const only = (value, keys) => object(value) && Object.keys(value).every(key => keys.includes(key));

/** Load a fixed operator-owned policy; no environment, command or identity supplied by tool calls. */
export function validateConfig(value) {
  if (!only(value, ['cli_path', 'wardian_home', 'sender_uuid', 'agent_uuids', 'tools']) ||
      !['cli_path', 'wardian_home'].every(key => typeof value[key] === 'string' && isAbsolute(value[key])) ||
      typeof value.sender_uuid !== 'string' || !UUID.test(value.sender_uuid) || !Array.isArray(value.agent_uuids) || value.agent_uuids.length === 0 ||
      value.agent_uuids.length > 100 || !value.agent_uuids.every(id => typeof id === 'string' && UUID.test(id)) ||
      new Set(value.agent_uuids).size !== value.agent_uuids.length || !Array.isArray(value.tools) ||
      value.tools.length === 0 || !value.tools.every(tool => TOOLS.includes(tool)) ||
      new Set(value.tools).size !== value.tools.length) {
    throw new Error('Invalid bridge policy: absolute paths, canonical UUIDs and explicit known tools required');
  }
  return Object.freeze({ ...value, agent_uuids: Object.freeze([...value.agent_uuids]), tools: Object.freeze([...value.tools]) });
}

/** Bounded UTF-8 newline framing shared by both sides. Truncated/oversized frames fail closed. */
export async function* frames(stream) {
  let buffer = Buffer.alloc(0);
  const decoder = new TextDecoder('utf-8', { fatal: true });
  for await (const chunk of stream) {
    buffer = Buffer.concat([buffer, Buffer.from(chunk)]);
    let newline;
    while ((newline = buffer.indexOf(10)) !== -1) {
      if (newline + 1 > MAX_FRAME) throw new Error('Oversized frame');
      const line = buffer.subarray(0, newline);
      buffer = buffer.subarray(newline + 1);
      yield decoder.decode(line);
    }
    if (buffer.length >= MAX_FRAME) throw new Error('Oversized frame');
  }
  if (buffer.length) throw new Error('Truncated frame');
}

function write(stream, value) {
  const frame = JSON.stringify(value) + '\n';
  if (Buffer.byteLength(frame) > MAX_FRAME) throw new Error('Oversized response');
  return new Promise((accept, reject) => stream.write(frame, error => error ? reject(error) : accept()));
}

/** One persistent upstream session, one in-flight call, no automatic retries or reconnects. */
export class StdioUpstream {
  constructor(config, { timeout_ms = 75000 } = {}) {
    this.sequence = 0;
    this.pending = null;
    this.failed = false;
    this.timeout_ms = timeout_ms;
    this.child = spawn(config.cli_path, ['mcp', 'serve'], {
      shell: false, windowsHide: true, stdio: ['pipe', 'pipe', 'pipe'],
      env: { ...process.env, WARDIAN_HOME: config.wardian_home },
    });
    // Never leak backend diagnostics, prompts, paths or credentials into model-facing stdout.
    this.child.stderr.resume();
    this.child.stdin.on('error', () => this.fail());
    this.child.on('error', () => this.fail());
    this.child.on('exit', () => this.fail());
    this.reader = this.read();
  }
  fail() {
    this.failed = true;
    if (this.pending) {
      clearTimeout(this.pending.timer);
      this.pending.reject(new Error('Upstream unavailable; delivery may be uncertain. Do not replay automatically.'));
      this.pending = null;
    }
    this.close();
  }
  async read() {
    try {
      for await (const line of frames(this.child.stdout)) {
        const response = JSON.parse(line);
        if (!object(response) || response.jsonrpc !== '2.0' || !this.pending ||
            response.id !== this.pending.id || (Object.hasOwn(response, 'result') === Object.hasOwn(response, 'error'))) {
          throw new Error('Invalid upstream response');
        }
        const pending = this.pending;
        clearTimeout(pending.timer);
        this.pending = null;
        if (response.error) pending.reject(new Error('Upstream rejected request; do not replay uncertain delivery.'));
        else pending.accept(response.result);
      }
      this.fail();
    } catch { this.fail(); }
  }
  async request(method, params) {
    if (this.failed || this.pending) throw new Error('Upstream unavailable or busy');
    const id = ++this.sequence;
    return new Promise((accept, reject) => {
      this.pending = { id, accept, reject, timer: setTimeout(() => this.fail(), this.timeout_ms) };
      write(this.child.stdin, { jsonrpc: '2.0', id, method, params }).catch(() => this.fail());
    });
  }
  async notify(method) {
    if (this.failed) throw new Error('Upstream unavailable');
    await write(this.child.stdin, { jsonrpc: '2.0', method });
  }
  close() {
    this.failed = true;
    if (this.pending) {
      clearTimeout(this.pending.timer);
      this.pending.reject(new Error('Upstream closed; delivery may be uncertain. Do not replay automatically.'));
      this.pending = null;
    }
    this.child.stdin.destroy();
    if (this.child.exitCode === null) this.child.kill();
  }
}

function validArguments(name, args, config) {
  const fields = {
    list_agents: [], send_message: ['target', 'message'], followup_task: ['target', 'message'],
    interrupt_agent: ['target'], receive_messages: ['cursor', 'ack_cursor', 'limit', 'timeout_ms'],
    wait_agent: ['timeout_ms'], reply: ['request_id', 'status', 'message'],
  };
  if (!only(args, fields[name]) || Object.values(args).some(value => value === null)) return false;
  if (fields[name].includes('target') && !config.agent_uuids.includes(args.target)) return false;
  if (fields[name].includes('message') && (typeof args.message !== 'string' || !args.message.trim() || Buffer.byteLength(args.message) > 65536)) return false;
  if (name === 'reply' && (typeof args.request_id !== 'string' || !args.request_id.trim() || !['done', 'blocked', 'failed'].includes(args.status))) return false;
  if (args.timeout_ms !== undefined && (!Number.isInteger(args.timeout_ms) || args.timeout_ms < 0 || args.timeout_ms > 60000)) return false;
  if (args.limit !== undefined && (!Number.isInteger(args.limit) || args.limit < 1 || args.limit > 100)) return false;
  return ['cursor', 'ack_cursor'].every(key => args[key] === undefined || (typeof args[key] === 'string' && args[key].trim()));
}

const toolFailure = (code, message) => {
  const value = { error: { code, message } };
  return { content: [{ type: 'text', text: JSON.stringify(value) }], structuredContent: value, isError: true };
};
const error = (id, code, message) => ({ jsonrpc: '2.0', id, error: { code, message } });

/** Tools-only MCP lens. Selection limits outgoing routes; mailbox operations belong to the configured sender. */
export class Bridge {
  constructor(config, upstreamFactory = policy => new StdioUpstream(policy)) {
    this.config = validateConfig(config);
    // Existing managed-host attribution only. A policy must never manufacture an origin.
    this.managed_sender_uuid = process.env.WARDIAN_SESSION_ID;
    this.upstreamFactory = upstreamFactory;
    this.upstream = null;
    this.ready = false;
    this.initialized = false;
    this.calls = new Set();
    this.definitions = [];
  }
  async handle(request) {
    const hasId = object(request) && Object.hasOwn(request, 'id');
    const id = hasId ? request.id : null;
    if (!object(request) || request.jsonrpc !== '2.0' ||
        (hasId && !(typeof id === 'string' || Number.isSafeInteger(id)))) return error(null, -32600, 'Invalid request');
    if (typeof request.method !== 'string') return hasId && (request.result || request.error) ? null : error(id, -32600, 'Invalid request');
    if (!hasId) {
      if (request.method === 'notifications/initialized' && this.initialized) this.ready = true;
      // Cancellation is advisory and never interrupts an agent or replays work.
      return null;
    }
    const params = request.params === undefined ? {} : request.params;
    if (!object(params)) return error(id, -32602, 'Invalid params');
    if (request.method === 'ping') return { jsonrpc: '2.0', id, result: {} };
    if (request.method === 'initialize') {
      if (this.initialized || this.upstream) return error(id, -32600, 'Already initialized');
      if (!['2025-11-25', '2025-06-18'].includes(params.protocolVersion) || !object(params.capabilities) ||
          typeof params.clientInfo?.name !== 'string' || typeof params.clientInfo?.version !== 'string') return error(id, -32602, 'Invalid initialization');
      try {
        this.upstream = this.upstreamFactory(this.config);
        const result = await this.upstream.request('initialize', params);
        if (result?.protocolVersion !== params.protocolVersion || !object(result.capabilities?.tools)) throw new Error('Unsupported upstream');
        await this.upstream.notify('notifications/initialized');
        const listed = await this.upstream.request('tools/list', {});
        if (!Array.isArray(listed?.tools)) throw new Error('Invalid tools');
        this.definitions = this.config.tools.map(name => {
          const definition = listed.tools.find(tool => tool.name === name);
          if (!object(definition?.inputSchema) || !object(definition.inputSchema.properties)) throw new Error('Missing tool');
          const selected = structuredClone(definition);
          if (selected.inputSchema.properties.target) selected.inputSchema.properties.target.enum = [...this.config.agent_uuids];
          return selected;
        });
        this.initialized = true;
        return { jsonrpc: '2.0', id, result: { protocolVersion: result.protocolVersion, capabilities: { tools: {} }, serverInfo: { name: 'wardian-selected-agent-bridge', version: '0.1.0' } } };
      } catch { this.close(); return error(id, -32000, 'Bridge upstream initialization failed'); }
    }
    if (!['tools/list', 'tools/call'].includes(request.method)) return error(id, -32601, 'Method not found');
    if (!this.ready) return error(id, -32000, 'Server not initialized');
    if (request.method === 'tools/list') {
      if (!only(params, ['_meta'])) return error(id, -32602, 'No pagination');
      return { jsonrpc: '2.0', id, result: { tools: this.definitions } };
    }
    const key = JSON.stringify(id);
    if (this.calls.has(key) || this.calls.size >= MAX_CALLS || key.length > 200) return error(id, -32600, 'Call ID reused or session call bound reached; no dispatch');
    this.calls.add(key);
    const args = params.arguments === undefined ? {} : params.arguments;
    if (!only(params, ['name', 'arguments', '_meta']) || !this.config.tools.includes(params.name) || !validArguments(params.name, args, this.config)) {
      return { jsonrpc: '2.0', id, result: toolFailure('bridge_policy_rejected', 'Tool or arguments outside the selected policy; no dispatch') };
    }
    if (this.managed_sender_uuid !== this.config.sender_uuid) {
      return { jsonrpc: '2.0', id, result: toolFailure('missing_managed_sender', 'An existing managed host with the matching authorized sender is required. Policy files do not create identities. No request was sent.') };
    }
    let result;
    try {
      result = await this.upstream.request('tools/call', { name: params.name, arguments: args });
      if (!object(result) || !Array.isArray(result.content)) throw new Error('Invalid receipt');
      if (params.name === 'list_agents' && !result.isError) {
        if (result.structuredContent?.operation !== 'list_agents' || !Array.isArray(result.structuredContent.agents)) throw new Error('Invalid roster');
        const agents = result.structuredContent.agents.filter(agent => this.config.agent_uuids.includes(agent.uuid))
          .map(({ uuid, name, provider, status, status_source }) => ({ uuid, name, provider, status, status_source }));
        const value = { operation: 'list_agents', agents };
        // Replace both representations, including any untrusted content blocks.
        result = { content: [{ type: 'text', text: JSON.stringify(value) }], structuredContent: value, isError: false };
      }
    } catch {
      this.close();
      result = toolFailure('bridge_transport_error', 'Receipt unavailable; delivery may be uncertain. Do not replay automatically. Restart requires operator reconciliation.');
    }
    return { jsonrpc: '2.0', id, result };
  }
  close() { this.upstream?.close(); }
}

/** Explicit launch only; importing or installing this directory starts no process or listener. */
export async function serve(config, input = process.stdin, output = process.stdout) {
  const bridge = new Bridge(config);
  const onExit = () => bridge.close();
  process.once('exit', onExit);
  try {
    for await (const line of frames(input)) {
      let request;
      try { request = JSON.parse(line); }
      catch { await write(output, error(null, -32700, 'Parse error')); continue; }
      const response = await bridge.handle(request);
      if (response) await write(output, response);
    }
  } finally { process.off('exit', onExit); bridge.close(); }
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  process.once('SIGINT', () => process.exit(130));
  process.once('SIGTERM', () => process.exit(143));
  try {
    if (process.argv.length !== 3) throw new Error('Expected policy file');
    const bytes = readFileSync(process.argv[2]);
    if (bytes.length > 65536) throw new Error('Policy too large');
    await serve(validateConfig(JSON.parse(bytes.toString('utf8'))));
  } catch {
    process.stderr.write('Wardian bridge stopped: invalid policy or stdio failure. Delivery may be uncertain; do not replay automatically.\n');
    process.exitCode = 1;
  }
}
