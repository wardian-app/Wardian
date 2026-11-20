/** Opt-in native acceptance driver. Test-owned fake grant, ephemeral loopback and mock worker only. */
import assert from 'node:assert/strict';
import { request } from 'node:http';
import { once } from 'node:events';
import { readFile, writeFile } from 'node:fs/promises';
import { spawn } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { createInterface } from 'node:readline';
import { createGateway } from './server.mjs';

const resource = process.env.WARDIAN_BRIDGE_E2E_RESOURCE;
const token = process.env.WARDIAN_BRIDGE_E2E_TOKEN;
const worker = process.env.WARDIAN_BRIDGE_E2E_AGENT;
const server = createGateway({ cli_path: process.env.WARDIAN_BRIDGE_E2E_CLI, wardian_home: process.env.WARDIAN_HOME,
  resource, allowed_origins: [], port: 47691, audit_path: process.env.WARDIAN_HOME + '/test-audit.jsonl' });
server.listen(0, '127.0.0.1');
await once(server, 'listening');
let sequence = 10;
function call(name, args, id = ++sequence, { request_key, expect_error = false } = {}) {
  return new Promise((accept, reject) => {
    const pending = request({ hostname: '127.0.0.1', port: server.address().port, path: new URL(resource).pathname, method: 'POST',
      headers: { Host: new URL(resource).host, Authorization: `Bearer ${token}`, Accept: 'application/json, text/event-stream',
        'Content-Type': 'application/json', 'MCP-Protocol-Version': '2025-11-25' } }, response => {
      const chunks = []; response.on('data', chunk => chunks.push(chunk));
      response.on('end', () => { try { assert.equal(response.statusCode, 200); const reply = JSON.parse(Buffer.concat(chunks).toString());
        assert.equal(reply.result.isError, expect_error, JSON.stringify(reply)); accept(reply.result.structuredContent); } catch(error) { reject(error); } });
    });
    pending.on('error', reject); pending.end(JSON.stringify({jsonrpc:'2.0',id,method:'tools/call',params:{name,arguments:args,
      ...(request_key === undefined ? {} : {_meta:{'wardian/request_key':request_key}})}}));
  });
}

async function runControllerStdioClient() {
  const env = { ...process.env };
  delete env.WARDIAN_SESSION_ID;
  delete env.WARDIAN_CONTROLLER_REQUEST_KEY;
  env.WARDIAN_CONTROLLER_TOKEN = token;
  env.WARDIAN_CONTROLLER_RESOURCE = resource;
  const child = spawn(process.env.WARDIAN_BRIDGE_E2E_CLI, ['mcp', 'serve', '--controller'], {
    windowsHide: true,
    stdio: ['pipe', 'pipe', 'pipe'],
    env,
  });
  let stderr = '';
  child.stderr.on('data', chunk => { stderr += chunk.toString(); });
  const lines = createInterface({ input: child.stdout, crlfDelay: Infinity })[Symbol.asyncIterator]();
  const read = async expectedId => {
    let timer;
    try {
      const next = await Promise.race([
        lines.next(),
        new Promise((_, reject) => { timer = setTimeout(() => reject(new Error('stdio MCP response deadline')), 10000); }),
      ]);
      assert.equal(next.done, false, 'stdio MCP exited before returning a response');
      const response = JSON.parse(next.value);
      assert.equal(response.id, expectedId);
      return response;
    } finally {
      clearTimeout(timer);
    }
  };
  const send = value => child.stdin.write(JSON.stringify(value) + '\n');
  const request = async (id, method, params = {}) => {
    send({ jsonrpc: '2.0', id, method, params });
    return read(id);
  };
  try {
    const initialized = await request(1, 'initialize', {
      protocolVersion: '2025-11-25',
      capabilities: {},
      clientInfo: { name: 'codex-stdio-fixture', version: '1' },
    });
    assert.equal(initialized.result.serverInfo.name, 'wardian');
    send({ jsonrpc: '2.0', method: 'notifications/initialized' });

    const listed = await request(2, 'tools/list');
    const tools = listed.result.tools;
    const names = tools.map(tool => tool.name);
    assert.deepEqual(names, ['list_agents', 'send_message', 'followup_task', 'receive_messages', 'wait_agent']);
    assert.ok(!names.includes('reply'), 'a controller cannot reply as a managed worker');
    assert.deepEqual(
      tools.find(tool => tool.name === 'followup_task').inputSchema.properties.target.enum,
      [worker],
    );
    const roster = await request(3, 'tools/call', { name: 'list_agents', arguments: {} });
    assert.deepEqual(roster.result.structuredContent.agents.map(agent => agent.uuid), [worker]);

    const info = await request(4, 'tools/call', {
      name: 'send_message',
      arguments: { target: worker, message: 'stdio-controller-information' },
    });
    assert.equal(info.result.isError, false);
    assert.ok(info.result.structuredContent.interaction_id);

    const submitted = await request(5, 'tools/call', {
      name: 'followup_task',
      arguments: { target: worker, message: 'stdio-controller-task' },
    });
    assert.equal(submitted.result.isError, false);
    const requestId = submitted.result.structuredContent.request_id;
    assert.ok(requestId);

    const page = await request(6, 'tools/call', {
      name: 'receive_messages',
      arguments: { timeout_ms: 0 },
    });
    assert.equal(page.result.isError, false);
    const messages = page.result.structuredContent.messages;
    assert.equal(messages.length, 1);
    assert.equal(messages[0].parent_interaction_id, requestId);
    assert.equal(messages[0].sender, worker);
    assert.equal(messages[0].message, 'external-controller-complete');

    const acknowledged = await request(7, 'tools/call', {
      name: 'receive_messages',
      arguments: { ack_cursor: page.result.structuredContent.next_cursor, timeout_ms: 0 },
    });
    assert.deepEqual(acknowledged.result.structuredContent.messages, []);

    for (const [id, name] of [[8, 'send_message'], [9, 'followup_task']]) {
      const denied = await request(id, 'tools/call', {
        name,
        arguments: { target: process.env.WARDIAN_BRIDGE_E2E_UNSELECTED_AGENT, message: 'denied' },
      });
      assert.equal(denied.result.isError, true);
      assert.equal(denied.result.structuredContent.error.code, 'forbidden');
    }
  } finally {
    child.stdin.end();
    if (child.exitCode === null && child.signalCode === null) {
      await new Promise(resolve => child.once('close', resolve));
    }
    if (child.exitCode !== 0) {
      assert.fail('controller stdio process failed: ' + stderr.replaceAll(token, '[REDACTED]'));
    }
  }
}

try {
  const roster = await call('list_agents', {});
  assert.deepEqual(roster.agents.map(agent => agent.uuid), [worker]);
  const info = await call('send_message', { target: worker, message: 'external-controller-information' });
  assert.ok(info.interaction_id);
  const replay = {request_key:'native-e2e-task'};
  const task = await call('followup_task', { target: worker, message: 'external-controller-work' }, 'native-e2e-task', replay);
  const duplicate = await call('followup_task', { target: worker, message: 'external-controller-work' }, 'different-rpc-id', replay);
  assert.equal(duplicate.request_id, task.request_id);
  const collision = await call('followup_task', { target: worker, message: 'changed payload' }, 'collision-rpc-id', {...replay,expect_error:true});
  assert.ok(collision.error);
  let page;
  for (let count = 0; count < 10; count++) {
    const receipt = await call('receive_messages', { timeout_ms: 1000 });
    page = receipt;
    if (page.messages.length) break;
  }
  assert.equal(page.messages.length, 1);
  assert.equal(page.messages[0].parent_interaction_id, task.request_id);
  assert.equal(page.messages[0].sender, worker);
  assert.equal(page.messages[0].message, 'external-controller-complete');
  const ack = await call('receive_messages', { ack_cursor: page.next_cursor, timeout_ms: 0 });
  assert.deepEqual(ack.messages, []);
  const audit = await readFile(process.env.WARDIAN_HOME + '/test-audit.jsonl', 'utf8');
  assert.ok(audit.includes('receipt_returned'));
  assert.equal(audit.includes(token), false);
  assert.equal(audit.includes('external-controller-work'), false);
  await runControllerStdioClient();
  // The released private tunnel keeps one child across logical reconnects.
  const relayConfig = process.env.WARDIAN_HOME + '/test-tunnel.json';
  await writeFile(relayConfig, JSON.stringify({ cli_path: process.env.WARDIAN_BRIDGE_E2E_CLI,
    wardian_home: process.env.WARDIAN_HOME, resource, allowed_origins: [], port: 47691,
    audit_path: process.env.WARDIAN_HOME + '/test-tunnel-audit.jsonl' }));
  const relay = spawn(process.execPath, [fileURLToPath(new URL('./tunnel-stdio.mjs', import.meta.url)),
    '--config', relayConfig, '--stdio'], { windowsHide: true, stdio: ['pipe', 'pipe', 'pipe'],
    env: {...process.env, WARDIAN_CONTROLLER_TOKEN: token} });
  relay.stderr.resume();
  let buffered = '';
  const responses = [];
  let waiter;
  relay.stdout.on('data', chunk => {
    buffered += chunk.toString();
    while (buffered.includes('\n')) {
      const at = buffered.indexOf('\n');
      responses.push(JSON.parse(buffered.slice(0, at))); buffered = buffered.slice(at + 1);
    }
    if (waiter && responses.length) { const accept = waiter; waiter = undefined; accept(responses.shift()); }
  });
  async function relayCall(method, params, id = 1) {
    relay.stdin.write(JSON.stringify({jsonrpc:'2.0', id, method, params}) + '\n');
    let timer;
    try {
      return await Promise.race([responses.length ? Promise.resolve(responses.shift()) : new Promise(accept => { waiter = accept; }),
        new Promise((_, reject) => { timer = setTimeout(() => reject(new Error('relay deadline')), 75000); })]);
    } finally { clearTimeout(timer); }
  }
  try {
    const init = { protocolVersion:'2025-11-25', capabilities:{}, clientInfo:{name:'fixture',version:'1'} };
    assert.ok((await relayCall('initialize', init)).result);
    assert.ok((await relayCall('initialize', init)).result); // reused ID, fresh logical session
    const meta = {'io.modelcontextprotocol/protocolVersion':'2026-07-28', 'io.modelcontextprotocol/clientCapabilities':{}};
    const discovery = await relayCall('server/discover', {_meta:meta});
    assert.equal(discovery.result.resultType, 'complete');
    assert.ok(discovery.result.supportedVersions.includes('2026-07-28'));
    const tools = await relayCall('tools/list', {_meta:meta});
    assert.ok(tools.result.tools.every(tool => tool.name !== 'reply'));
    const taskTool = tools.result.tools.find(tool => tool.name === 'followup_task');
    assert.deepEqual(taskTool.inputSchema.properties.target.enum, [worker]);
    const replayed = await relayCall('tools/call', {name:'followup_task', arguments:{target:worker,message:'external-controller-work'},
      _meta:{...meta,'wardian/request_key':'native-e2e-task'}});
    assert.equal(replayed.result.structuredContent.request_id, task.request_id);
    const denied = await relayCall('tools/call', {name:'send_message', arguments:{target:'ffffffff-ffff-4fff-8fff-ffffffffffff',message:'denied'},_meta:meta});
    assert.ok(denied.error); assert.equal(denied.result, undefined);
    const empty = await relayCall('tools/call', {name:'receive_messages',arguments:{timeout_ms:0},_meta:meta});
    assert.deepEqual(empty.result.structuredContent.messages, []);
    // Owner revocation must also close a persistent relay's next logical session.
    const revokeEnv = {...process.env}; delete revokeEnv.WARDIAN_SESSION_ID;
    const revoke = spawn(process.env.WARDIAN_BRIDGE_E2E_CLI, ['controller','revoke',token.split('.')[1]],
      {windowsHide:true,stdio:['ignore','ignore','ignore'],env:revokeEnv});
    const [revokeCode] = await once(revoke, 'exit'); assert.equal(revokeCode, 0);
    const revoked = await relayCall('initialize', init);
    assert.ok(revoked.error); assert.equal(revoked.result, undefined);
  } finally { relay.kill(); }
  process.stdout.write('native_controller_http_roundtrip_passed\nnative_controller_stdio_roundtrip_passed\n');
} finally { server.closeAllConnections(); await new Promise(accept => server.close(accept)); }
