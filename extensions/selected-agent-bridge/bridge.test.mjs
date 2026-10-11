import assert from 'node:assert/strict';
import { test } from 'node:test';
import { Readable } from 'node:stream';
import { mkdtempSync, writeFileSync, rmSync, readFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { Bridge, StdioUpstream, frames, validateConfig } from './bridge.mjs';

const sender = '10000000-0000-4000-8000-000000000000';
const selected = '20000000-0000-4000-8000-000000000000';
const other = '30000000-0000-4000-8000-000000000000';
const names = ['list_agents', 'send_message', 'followup_task', 'receive_messages', 'wait_agent', 'reply', 'interrupt_agent'];
const policy = { cli_path: process.execPath, wardian_home: resolve('.'), sender_uuid: sender, agent_uuids: [selected], tools: names };
const initialize = { jsonrpc: '2.0', id: 0, method: 'initialize', params: { protocolVersion: '2025-11-25', capabilities: {}, clientInfo: { name: 'test', version: '1' } } };
const call = (id, name, args = {}) => ({ jsonrpc: '2.0', id, method: 'tools/call', params: { name, arguments: args } });
const result = value => ({ structuredContent: value, content: [{ type: 'text', text: JSON.stringify(value) }], isError: false });

class Mock {
  calls = [];
  closed = false;
  async request(method, params) {
    this.calls.push({ method, params });
    if (method === 'initialize') return { protocolVersion: params.protocolVersion, capabilities: { tools: {} } };
    if (method === 'tools/list') return { tools: names.map(name => ({ name, inputSchema: { type: 'object', properties: ['send_message', 'followup_task', 'interrupt_agent'].includes(name) ? { target: { type: 'string' } } : {} } })) };
    if (params.name === 'list_agents') return result({ operation: 'list_agents', agents: [
      { uuid: selected, name: 'chosen', provider: 'codex', status: 'idle', status_source: 'live', workspace: 'private-path' },
      { uuid: other, name: 'hidden', provider: 'claude' },
    ] });
    return result({ operation: params.name, request_id: 'ask_1', interaction_id: 'interaction_1', delivery_state: 'admitted', duplicate: false });
  }
  async notify(method) { this.calls.push({ method }); }
  close() { this.closed = true; }
}
async function fixture(tools = names) {
  const mock = new Mock();
  const previous = process.env.WARDIAN_SESSION_ID;
  process.env.WARDIAN_SESSION_ID = sender;
  const bridge = new Bridge({ ...policy, tools }, () => mock);
  if (previous === undefined) delete process.env.WARDIAN_SESSION_ID;
  else process.env.WARDIAN_SESSION_ID = previous;
  assert.ok((await bridge.handle(initialize)).result);
  await bridge.handle({ jsonrpc: '2.0', method: 'notifications/initialized' });
  mock.calls.length = 0;
  return { bridge, mock };
}

test('policy fails closed and is an immutable snapshot', () => {
  for (const invalid of [{ agent_uuids: [] }, { agent_uuids: ['chosen'] }, { sender_uuid: '' }, { tools: ['spawn_agent'] }, { tools: ['reply', 'reply'] }, { cli_path: 'wardian' }, { env: {} }]) {
    assert.throws(() => validateConfig({ ...policy, ...invalid }));
  }
  const input = { ...policy, agent_uuids: [selected] };
  const fixed = validateConfig(input);
  input.agent_uuids.push(other);
  assert.deepEqual(fixed.agent_uuids, [selected]);
});

test('discovery limits tools, target enum and both roster representations', async () => {
  const { bridge } = await fixture(['list_agents', 'send_message']);
  const listed = await bridge.handle({ jsonrpc: '2.0', id: 1, method: 'tools/list' });
  assert.deepEqual(listed.result.tools.map(tool => tool.name), ['list_agents', 'send_message']);
  assert.deepEqual(listed.result.tools[1].inputSchema.properties.target.enum, [selected]);
  const roster = await bridge.handle(call(2, 'list_agents'));
  assert.equal(roster.result.structuredContent.agents.length, 1);
  assert.equal(roster.result.structuredContent.agents[0].uuid, selected);
  assert.equal(JSON.stringify(roster).includes('hidden'), false);
  assert.equal(JSON.stringify(roster).includes('private-path'), false);
  assert.deepEqual(JSON.parse(roster.result.content[0].text), roster.result.structuredContent);
});

test('invalid routes, tool grants, arguments and notifications never dispatch', async () => {
  const { bridge, mock } = await fixture(['send_message', 'receive_messages']);
  const invalid = [call(1, 'send_message', { target: other, message: 'hello' }), call(2, 'send_message', { target: 'chosen', message: 'hello' }),
    call(3, 'interrupt_agent', { target: selected }), call(4, 'send_message', { target: selected, message: 'hello', sender }),
    call(5, 'receive_messages', { timeout_ms: 60001 }), call(6, 'receive_messages', { limit: 0 }),
    call(7, 'receive_messages', null), call(8, 'send_message', { target: selected, message: ' ' }),
    call(9, 'send_message', { target: selected, message: '🙂'.repeat(17000) })];
  for (const request of invalid) assert.equal((await bridge.handle(request)).result.isError, true);
  await bridge.handle({ jsonrpc: '2.0', method: 'tools/call', params: { name: 'send_message', arguments: { target: selected, message: 'ignored' } } });
  assert.equal(mock.calls.length, 0);
});

test('external hosts cannot manufacture managed attribution from a policy', async () => {
  const { bridge, mock } = await fixture();
  for (const origin of [undefined, other]) {
    bridge.managed_sender_uuid = origin;
    const response = await bridge.handle(call(String(origin), 'followup_task', { target: selected, message: 'work' }));
    assert.equal(response.result.structuredContent.error.code, 'missing_managed_sender');
  }
  assert.equal(mock.calls.length, 0);
});

test('information, tasks, receive, wake, reply and interrupt retain exact canonical operations and receipts', async () => {
  const { bridge, mock } = await fixture();
  const payloads = [
    ['send_message', { target: selected, message: 'line one\nUnicode 🙂 $literal' }],
    ['followup_task', { target: selected, message: 'work' }],
    ['receive_messages', { cursor: 'opaque', ack_cursor: 'prior-page', limit: 5, timeout_ms: 0 }],
    ['wait_agent', { timeout_ms: 0 }],
    ['reply', { request_id: 'ask_1', status: 'blocked', message: 'needs input' }],
    ['interrupt_agent', { target: selected }],
  ];
  for (const [index, [name, args]] of payloads.entries()) {
    const receipt = await bridge.handle(call(index + 1, name, args));
    assert.equal(receipt.result.structuredContent.operation, name);
    assert.deepEqual(mock.calls.at(-1), { method: 'tools/call', params: { name, arguments: args } });
  }
});

test('typed call IDs reject replay; cancellation never dispatches an interruption', async () => {
  const { bridge, mock } = await fixture();
  await bridge.handle(call(1, 'followup_task', { target: selected, message: 'work' }));
  assert.equal((await bridge.handle(call(1, 'followup_task', { target: selected, message: 'work' }))).error.code, -32600);
  await bridge.handle(call('1', 'send_message', { target: selected, message: 'information' }));
  await bridge.handle({ jsonrpc: '2.0', method: 'notifications/cancelled', params: { requestId: 1 } });
  assert.equal(mock.calls.length, 2);
});

test('lost receipts close upstream without replay', async () => {
  const { bridge, mock } = await fixture();
  mock.request = async () => { mock.calls.push({ dispatched: true }); throw new Error('lost receipt'); };
  const response = await bridge.handle(call(1, 'followup_task', { target: selected, message: 'work' }));
  assert.equal(response.result.structuredContent.error.code, 'bridge_transport_error');
  assert.equal(mock.closed, true);
  assert.equal(mock.calls.length, 1);
});

test('malformed roster is replaced by failure rather than leaked', async () => {
  const { bridge, mock } = await fixture();
  mock.request = async () => ({ content: [{ type: 'text', text: 'private roster' }], structuredContent: { agents: [] } });
  const response = await bridge.handle(call(1, 'list_agents'));
  assert.equal(response.result.isError, true);
  assert.equal(JSON.stringify(response).includes('private roster'), false);
});

test('framing preserves split UTF-8; oversized, truncated and invalid UTF-8 are rejected', async () => {
  const bytes = Buffer.from('{"message":"🙂"}\n');
  const collected = [];
  for await (const line of frames(Readable.from([bytes.subarray(0, 14), bytes.subarray(14)]))) collected.push(line);
  assert.deepEqual(collected, ['{"message":"🙂"}']);
  for (const bytes of [Buffer.from('{}'), Buffer.alloc(1024 * 1024 + 1, 32), Buffer.from([0xff, 10])]) {
    await assert.rejects(async () => { for await (const line of frames(Readable.from([bytes]))) void line; });
  }
});

test('stdio subprocess exchanges real frames with a loopback mock and fixed child identity', async () => {
  const directory = mkdtempSync(join(tmpdir(), 'wardian-bridge-test-'));
  const bridgePath = join(dirname(fileURLToPath(import.meta.url)), 'bridge.mjs');
  // Node's executable treats the fixed `mcp serve` args as `node mcp serve`.
  // The temp fixture supplies `mcp`; production instead runs the native Wardian CLI.
  const mockSource = `const readline = require('node:readline');
const fs = require('node:fs');
const names = ${JSON.stringify(names)};
fs.writeFileSync('identity.json', JSON.stringify({home:process.env.WARDIAN_HOME,sender:process.env.WARDIAN_SESSION_ID}));
readline.createInterface({input:process.stdin}).on('line', line => {
  const req = JSON.parse(line);
  if (req.id === undefined) return;
  let result;
  if(req.method==='initialize') result={protocolVersion:req.params.protocolVersion,capabilities:{tools:{}}};
  else if(req.method==='tools/list') result={tools:names.map(name=>({name,inputSchema:{type:'object',properties:{}}}))};
  else { const v={operation:req.params.name,request_id:'ask_loopback',delivery_state:'admitted'}; result={structuredContent:v,content:[{type:'text',text:JSON.stringify(v)}],isError:false}; }
  process.stdout.write(JSON.stringify({jsonrpc:'2.0',id:req.id,result})+'\\n');
});`;
  writeFileSync(join(directory, 'mcp'), mockSource);
  writeFileSync(join(directory, 'policy.json'), JSON.stringify(policy));
  const child = spawn(process.execPath, [bridgePath, join(directory, 'policy.json')], { cwd: directory, windowsHide: true, stdio: ['pipe', 'pipe', 'pipe'], env: { ...process.env, WARDIAN_SESSION_ID: sender } });
  let diagnostics = '';
  child.stderr.on('data', chunk => { diagnostics += chunk; });
  const output = frames(child.stdout)[Symbol.asyncIterator]();
  const send = req => child.stdin.write(JSON.stringify(req) + '\n');
  try {
    send(initialize);
    assert.equal(JSON.parse((await output.next()).value).result.serverInfo.name, 'wardian-selected-agent-bridge');
    send({ jsonrpc: '2.0', method: 'notifications/initialized' });
    send(call(1, 'followup_task', { target: selected, message: 'loopback' }));
    assert.equal(JSON.parse((await output.next()).value).result.structuredContent.request_id, 'ask_loopback');
    assert.deepEqual(JSON.parse(readFileSync(join(directory, 'identity.json'), 'utf8')), { home: policy.wardian_home, sender });
    const exited = once(child, 'exit');
    child.stdin.end();
    assert.equal((await exited)[0], 0, diagnostics);
  } finally {
    child.kill();
    rmSync(directory, { recursive: true, force: true });
  }
});

test('upstream deadline fails closed and does not reconnect', async () => {
  const directory = mkdtempSync(join(tmpdir(), 'wardian-bridge-timeout-'));
  writeFileSync(join(directory, 'mcp'), 'process.stdin.resume();');
  const previous = process.cwd();
  let upstream;
  try {
    process.chdir(directory);
    upstream = new StdioUpstream(policy, { timeout_ms: 100 });
  } finally { process.chdir(previous); }
  const started = Date.now();
  try {
    await assert.rejects(upstream.request('initialize', {}));
    assert.ok(Date.now() - started >= 80, 'must exercise the deadline, not early process exit');
    await assert.rejects(upstream.request('initialize', {}));
    assert.equal(upstream.sequence, 1);
  } finally {
    const exited = upstream.child.exitCode === null ? once(upstream.child, 'exit') : Promise.resolve();
    upstream.close();
    await exited;
    rmSync(directory, { recursive: true, force: true });
  }
});
