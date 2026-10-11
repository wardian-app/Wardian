import test from 'node:test';
import assert from 'node:assert/strict';
import { Readable, Writable } from 'node:stream';
import { createTunnelRequestHandler, serveTunnelStdio } from './tunnel-stdio.mjs';

const target = '20000000-0000-4000-8000-000000000000';
const controller = '10000000-0000-4000-8000-000000000000';
const prefix = 'io.modelcontextprotocol/';
const meta = { [prefix + 'protocolVersion']: '2026-07-28', [prefix + 'clientCapabilities']: {} };
const config = { cli_path: process.execPath, wardian_home: process.cwd(), audit_path: process.execPath, resource: 'https://mcp.example.test/mcp', port: 4444, allowed_origins: [] };
const rpc = (method, params = {}, id = 1) => ({ jsonrpc: '2.0', id, method, params });
const init = () => rpc('initialize', { protocolVersion: '2025-11-25', capabilities: {}, clientInfo: { name: 'test', version: '1' } });
function fixture() {
  const state = { calls: [], events: [], verifies: 0, grant: { controller_id: controller, controller_name: 'private owner', grant_id: 'fixture', audience: config.resource, expires_at_ms: Date.now() + 60000, revoked: false, agent_uuids: [target], scopes: ['agents:read', 'messages:send', 'tasks:submit', 'agents:wake', 'controller:receive'] } };
  const client = { verify_access_token: async token => { assert.equal(token, 'fake-token'); state.verifies++; return structuredClone(state.grant); }, invoke: async call => { state.calls.push(call); return { isError: false, content: [{ type: 'text', text: 'receipt' }], structuredContent: { request_id: 'canonical' } }; } };
  const handler = createTunnelRequestHandler(config, { client, audit: async event => { state.events.push(event); }, token: 'fake-token' });
  return { state, client, handler };
}
async function stream(chunks, handler, delay = 0) {
  const frames = [];
  const output = new Writable({ write(chunk, encoding, callback) { frames.push(...chunk.toString().trim().split('\n').map(JSON.parse)); setTimeout(callback, delay); } });
  await serveTunnelStdio(Readable.from(chunks), output, handler);
  return frames;
}

test('modern discovery and scoped catalog match normative complete/private schema', async () => {
  const { handler, state } = fixture();
  const discover = await handler(rpc('server/discover', { _meta: meta }));
  assert.deepEqual(discover.result.supportedVersions, ['2026-07-28', '2025-11-25']);
  assert.deepEqual(discover.result.capabilities, { tools: {} });
  assert.equal(discover.result.resultType, 'complete');
  assert.equal(discover.result.ttlMs, 0); assert.equal(discover.result.cacheScope, 'private');
  assert.equal(discover.result._meta[prefix + 'serverInfo'].name, 'wardian-external-controller');
  const catalog = await handler(rpc('tools/list', { _meta: meta }));
  assert.equal(catalog.result.resultType, 'complete'); assert.equal(catalog.result.cacheScope, 'private');
  assert.deepEqual(catalog.result.tools.map(t => t.name), ['list_agents', 'send_message', 'followup_task', 'receive_messages']);
  assert.deepEqual(catalog.result.tools[1].inputSchema.properties.target.enum, [target]);
  assert.equal(catalog.result.tools[3].inputSchema.properties.ack_cursor, undefined);
  assert.equal(state.events.length, 2);
});

test('modern required metadata and unsupported versions fail without dispatch', async () => {
  const { handler, state } = fixture();
  assert.equal((await handler(rpc('server/discover'))).error.code, -32602);
  assert.equal((await handler(rpc('ping', { _meta: { ...meta, [prefix + 'protocolVersion']: 'future' } }))).error.code, -32022);
  assert.equal((await handler(rpc('ping', { _meta: { [prefix + 'protocolVersion']: '2026-07-28' } }))).error.code, -32602);
  assert.equal((await handler(rpc('server/discover', { _meta: meta, extra: true }))).error.code, -32602);
  assert.equal(state.calls.length, 0);
});

test('legacy repeated initialize and completed IDs remain stateless across callers', async () => {
  const { handler, state } = fixture();
  assert.equal((await handler(init())).result.protocolVersion, '2025-11-25');
  assert.equal((await handler(init())).result.protocolVersion, '2025-11-25');
  assert.equal((await handler(rpc('ping'))).id, 1);
  assert.equal((await handler(rpc('ping', { _meta: meta }))).result.resultType, 'complete');
  assert.equal(state.verifies, 8);
});

test('only explicit replay keys persist; fixed owner and non-waking operation preserved', async () => {
  const { handler, state } = fixture();
  const args = { target, message: 'hello' };
  const call = key => rpc('tools/call', { name: 'send_message', arguments: args, _meta: { ...meta, ...(key ? { 'wardian/request_key': key } : {}) } });
  assert.equal((await handler(call('stable'))).result.resultType, 'complete');
  await handler(call('stable')); await handler(call()); await handler(call());
  assert.equal(state.calls.length, 4);
  assert.equal(state.calls[0].idempotency_key, state.calls[1].idempotency_key);
  assert.notEqual(state.calls[2].idempotency_key, state.calls[3].idempotency_key);
  assert.equal(state.calls[0].principal.mailbox_owner, 'controller:' + controller);
  assert.equal(state.calls[0].operation, 'send_message');
  assert.deepEqual(state.calls[0].arguments, args);
  assert.equal(state.verifies, 12);
});

test('revocation and unselected target reject without native dispatch or identity spoofing', async () => {
  const { handler, state } = fixture();
  await handler(init()); state.grant.revoked = true;
  assert.equal((await handler(rpc('tools/list', { _meta: meta }))).error.data.status, 401);
  state.grant.revoked = false;
  const rejected = await handler(rpc('tools/call', { name: 'send_message', arguments: { target: controller, message: 'no' }, _meta: { ...meta, [prefix + 'clientInfo']: { name: 'forged worker', version: '1' } } }));
  assert.ok(rejected.error); assert.equal(state.calls.length, 0);
});

test('audit failure prevents dispatch; native uncertainty causes no retry', async () => {
  const { client, state } = fixture();
  const call = rpc('tools/call', { name: 'send_message', arguments: { target, message: 'hi' } });
  const failing = createTunnelRequestHandler(config, { client, audit: async () => { throw new Error('audit failed'); }, token: 'fake-token' });
  assert.ok((await failing(call)).error); assert.equal(state.calls.length, 0);
  let attempts = 0;
  client.invoke = async () => { attempts++; throw new Error('secret should not escape'); };
  const handler = createTunnelRequestHandler(config, { client, audit: async () => {}, token: 'fake-token' });
  const result = await handler(call); assert.equal(result.result.isError, true); assert.equal(attempts, 1);
  assert.ok(!JSON.stringify(result).includes('secret should not escape'));
});

test('UTF8 split framing, malformed JSON, notifications and truncated tail', async () => {
  const { handler } = fixture();
  const encoded = Buffer.from(JSON.stringify(rpc('ping', { _meta: { ...meta, custom: '雪' } })) + '\n');
  const i = encoded.indexOf(Buffer.from('雪')) + 1;
  const frames = await stream([encoded.subarray(0, i), encoded.subarray(i), Buffer.from('{bad}\n'), Buffer.from([0xff, 10]), Buffer.from('{"jsonrpc":"2.0","method":"notifications/initialized"}\n'), Buffer.from('{}')], handler);
  assert.equal(frames.length, 4); assert.equal(frames.find(f => f.result).result.resultType, 'complete');
  assert.deepEqual(frames.filter(f => f.error).map(f => f.error.code), [-32700, -32700, -32700]);
});

test('oversized frame stops without dispatching its remainder', async () => {
  let calls = 0; const frames = [];
  const output = new Writable({ write(chunk, encoding, cb) { frames.push(JSON.parse(chunk)); cb(); } });
  await assert.rejects(serveTunnelStdio(Readable.from([Buffer.concat([Buffer.alloc(1048577, 32), Buffer.from('\n{}\n')])]), output, async () => { calls++; }));
  assert.equal(calls, 0); assert.equal(frames.length, 1); assert.equal(frames[0].error.code, -32700);
});

test('bounded eight concurrent exchanges and stdout backpressure', async () => {
  let active = 0, peak = 0, calls = 0;
  const lines = Array.from({ length: 25 }, (_, id) => JSON.stringify(rpc('ping', {}, id)) + '\n').join('');
  const frames = await stream([Buffer.from(lines)], async request => { active++; peak = Math.max(peak, active); await new Promise(r => setTimeout(r, 5)); active--; calls++; return { jsonrpc: '2.0', id: request.id, result: {} }; }, 2);
  assert.equal(calls, 25); assert.equal(frames.length, 25); assert.ok(peak <= 8);
});

test('outstanding duplicate IDs reject; completed reuse is allowed', async () => {
  let release; const blocked = new Promise(r => { release = r; }); let calls = 0;
  const running = stream([Buffer.from(JSON.stringify(rpc('ping')) + '\n' + JSON.stringify(rpc('ping')) + '\n')], async request => { calls++; await blocked; return { jsonrpc: '2.0', id: request.id, result: {} }; });
  setTimeout(release, 10); const frames = await running;
  assert.equal(calls, 1); assert.equal(frames.filter(f => f.error?.code === -32600).length, 1);
});

test('revocation during discovery audit blocks capability delivery', async () => {
  const { client, state } = fixture();
  const handler = createTunnelRequestHandler(config, { client, token: 'fake-token', audit: async () => { state.grant.revoked = true; } });
  const result = await handler(rpc('server/discover', { _meta: meta }));
  assert.equal(result.error.code, -32603); assert.equal(result.result, undefined);
});

test('handler admits at most eight boundary exchanges without an authentication queue', async () => {
  const { client } = fixture(); let release;
  const block = new Promise(r => { release = r; }); let verified = 0;
  const verify = client.verify_access_token;
  client.verify_access_token = async (...args) => { verified++; await block; return verify(...args); };
  const handler = createTunnelRequestHandler(config, { client, token: 'fake-token', audit: async () => {} });
  const pending = Array.from({ length: 8 }, (_, id) => handler(rpc('ping', {}, id)));
  assert.equal((await handler(rpc('ping', {}, 9))).error.code, -32603);
  const admitted = verified;
  release(); await Promise.all(pending);
  assert.equal(admitted, 8); assert.equal(verified, 16);
});

test('response framing is bounded and malformed envelope rejected', async () => {
  const frames = await stream([Buffer.from(JSON.stringify(rpc('ping')) + '\n')], async request => ({ jsonrpc: '2.0', id: request.id, result: { text: 'x'.repeat(1048576) } }));
  assert.equal(frames[0].error.code, -32603);
  const { handler } = fixture();
  assert.equal((await handler(rpc('ping', null))).error.code, -32602);
  assert.equal((await handler({ ...rpc('ping'), actor: target })).error.code, -32600);
});


test('notification audit records bounded operation metadata only', async () => {
  const {handler,state} = fixture();
  const privateMethod = 'private-payload-' + 'x'.repeat(200000);
  assert.equal(await handler({jsonrpc:'2.0',method:privateMethod,params:{message:'private-message'}}), undefined);
  assert.equal(state.events.length, 1); assert.equal(state.events[0].operation, 'unknown');
  assert.equal(JSON.stringify(state.events).includes('private-payload'), false);
  assert.equal(JSON.stringify(state.events).includes('private-message'), false);
  assert.equal(state.calls.length, 0);
});
