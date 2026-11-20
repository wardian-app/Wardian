/** Safe native protocol check: initialize/list only, no home access or agent operation. */
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { Bridge } from './bridge.mjs';

assert.ok(process.argv[2], 'pass the absolute native Wardian CLI path');
const policy = JSON.parse(readFileSync(new URL('./policy.example.json', import.meta.url), 'utf8'));
policy.cli_path = resolve(process.argv[2]);
policy.wardian_home = resolve('.');
const bridge = new Bridge(policy);
try {
  const response = await bridge.handle({ jsonrpc: '2.0', id: 0, method: 'initialize', params: {
    protocolVersion: '2025-11-25', capabilities: {}, clientInfo: { name: 'bridge-discovery-smoke', version: '0.1.0' },
  } });
  assert.ok(response.result, JSON.stringify(response));
  await bridge.handle({ jsonrpc: '2.0', method: 'notifications/initialized' });
  const listed = await bridge.handle({ jsonrpc: '2.0', id: 1, method: 'tools/list' });
  assert.deepEqual(listed.result.tools.map(tool => tool.name), policy.tools);
  console.log(JSON.stringify({ protocol_version: response.result.protocolVersion, tools: listed.result.tools.map(tool => tool.name), agent_operations_dispatched: 0 }));
} finally { bridge.close(); }
