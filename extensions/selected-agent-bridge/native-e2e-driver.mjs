/** Opt-in native acceptance driver. Test-owned fake grant, ephemeral loopback and mock worker only. */
import assert from 'node:assert/strict';
import { request } from 'node:http';
import { once } from 'node:events';
import { readFile } from 'node:fs/promises';
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
  process.stdout.write('native_controller_http_roundtrip_passed\n');
} finally { server.closeAllConnections(); await new Promise(accept => server.close(accept)); }
