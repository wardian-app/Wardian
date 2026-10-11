/** Safe native protocol check: initialize/list only, no home access or agent operation. */
import assert from 'node:assert/strict';
import { resolve } from 'node:path';
import { Bridge } from './bridge.mjs';

const bridge = new Bridge({
  cli_path: resolve(process.argv[2]), wardian_home: resolve('.'),
  sender_uuid: '10000000-0000-4000-8000-000000000000',
  agent_uuids: ['20000000-0000-4000-8000-000000000000'],
  tools: ['list_agents', 'send_message', 'followup_task', 'receive_messages', 'wait_agent', 'reply', 'interrupt_agent'],
});
try {
  const response = await bridge.handle({ jsonrpc: '2.0', id: 0, method: 'initialize', params: {
    protocolVersion: '2025-11-25', capabilities: {}, clientInfo: { name: 'bridge-discovery-smoke', version: '0.1.0' },
  } });
  assert.ok(response.result, JSON.stringify(response));
  await bridge.handle({ jsonrpc: '2.0', method: 'notifications/initialized' });
  const listed = await bridge.handle({ jsonrpc: '2.0', id: 1, method: 'tools/list' });
  assert.equal(listed.result.tools.length, 7);
  console.log(JSON.stringify({ protocol_version: response.result.protocolVersion, tools: listed.result.tools.map(tool => tool.name), agent_operations_dispatched: 0 }));
} finally { bridge.close(); }
