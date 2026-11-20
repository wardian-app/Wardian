import { test } from 'node:test';
import assert from 'node:assert/strict';
import { resolve } from 'node:path';
import { createGateway, validateServerConfig } from './server.mjs';

const config = { cli_path: process.execPath, wardian_home: resolve('fixture-home'), resource: 'https://bridge.example/mcp',
  allowed_origins: [], port: 47691, audit_path: resolve('fixture-audit.jsonl') };
test('gateway requires explicit loopback operator policy and creates no listener on import/factory', () => {
  for (const patch of [{port:0}, {port:65536}, {host:'0.0.0.0'}, {cli_path:'wardian'}, {resource:'http://bridge.example/mcp'},
    {resource:'https://user:password@bridge.example/mcp'}, {resource:'https://bridge.example/mcp?token=x'}, {allowed_origins:['https://client.example/path']}]) {
    assert.throws(() => validateServerConfig({...config,...patch}));
  }
  const policy = validateServerConfig(config);
  assert.ok(Object.isFrozen(policy.allowed_origins));
  const server = createGateway(config, {client:{verify_access_token:async()=>{throw new Error('not activated');},invoke:async()=>{throw new Error('not activated');}},audit:async()=>{}});
  assert.equal(server.listening, false);
  assert.equal(server.maxConnections, 32);
  assert.equal(server.requestTimeout, 10000);
});
