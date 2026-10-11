import assert from 'node:assert/strict';
import { test } from 'node:test';
import { mkdtempSync, writeFileSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createNativeControllerClient, nativePrincipalToGrant } from './native-client.mjs';

const id = '10000000-0000-4000-8000-000000000000';
const agent = '20000000-0000-4000-8000-000000000000';
const resource = 'https://bridge.example/mcp';
const principal = { id, name: 'fixture controller', agent_ids: [agent], scopes: ['agents:read'], expires_at: 2000000000, revoked_at: null, resource };

function fixture(behavior = 'normal', timeout_ms = 2000) {
  const directory = mkdtempSync(join(tmpdir(), 'wardian-native-client-'));
  const script = join(directory, 'fixture.cjs');
  const log = join(directory, 'transcript.jsonl');
  writeFileSync(script, `const fs=require('node:fs'); const readline=require('node:readline');
const [log,behavior,...args]=process.argv.slice(2);
const record=value=>fs.appendFileSync(log,JSON.stringify(value)+'\\n');
record({args,token:process.env.WARDIAN_CONTROLLER_TOKEN,resource:process.env.WARDIAN_CONTROLLER_RESOURCE,home:process.env.WARDIAN_HOME,key:process.env.WARDIAN_CONTROLLER_REQUEST_KEY,sender:process.env.WARDIAN_SESSION_ID,pid:process.pid});
process.stderr.write('private-token private-path');
if(behavior==='hang'){process.stdin.resume();setInterval(()=>{},1000);}
else if(behavior==='oversized'){process.stdout.write('x'.repeat(1024*1024+1));}
else if(behavior==='invalid'){process.stdout.write(Buffer.from([255]));}
else if(args[0]==='controller'){process.stdout.write(JSON.stringify(${JSON.stringify(principal)}));}
else readline.createInterface({input:process.stdin}).on('line',line=>{
 const req=JSON.parse(line);record(req); if(req.id===undefined)return;
 if(behavior==='hang-tool' && req.method==='tools/call'){setInterval(()=>{},1000);return;}
 if(behavior==='wrong-id'){process.stdout.write(JSON.stringify({jsonrpc:'2.0',id:999,result:{}})+'\\n');return;}
 if(behavior==='lost' && req.method==='tools/call'){process.exit(3);return;}
 const result=req.method==='initialize'?{protocolVersion:req.params.protocolVersion,capabilities:{tools:{}}}:{content:[{type:'text',text:'fixture receipt'}],structuredContent:{operation:req.params.name,request_id:'ask_fixture'}};
 process.stdout.write(JSON.stringify({jsonrpc:'2.0',id:req.id,result})+'\\n');
});`);
  const client = createNativeControllerClient({ cli_path: process.execPath, wardian_home: directory, resource, argv_prefix: [script, log, behavior], timeout_ms });
  return { directory, client, records: () => readFileSync(log, 'utf8').trim().split('\n').map(line => JSON.parse(line)),
    close: () => rmSync(directory, { recursive: true, force: true }) };
}

test('principal projection keeps native attribution and expiry/revocation semantics', () => {
  const grant = nativePrincipalToGrant(principal, resource);
  assert.equal(grant.controller_id, id); assert.equal(grant.grant_id, id);
  assert.equal(grant.expires_at_ms, 2000000000000); assert.equal(grant.revoked, false);
  assert.equal(nativePrincipalToGrant({ ...principal, expires_at: null }, resource).expires_at_ms, Number.MAX_SAFE_INTEGER);
  assert.equal(nativePrincipalToGrant({ ...principal, revoked_at: 1 }, resource).revoked, true);
  for (const changed of [{ resource: 'https://wrong.example/mcp' }, { agent_ids: ['name'] }, { expires_at: 1.5 }, { expires_at: Number.MAX_SAFE_INTEGER }, { name: 'bad\nname' }, { scopes: ['unknown'] }]) {
    assert.throws(() => nativePrincipalToGrant({ ...principal, ...changed }, resource));
  }
});

test('verify subprocess uses fixed args, redacted native principal and private env token', async () => {
  const f = fixture();
  try {
    const grant = await f.client.verify_access_token('fixture-secret', { resource });
    assert.equal(grant.controller_id, id);
    const [record] = f.records();
    assert.deepEqual(record.args, ['controller', 'verify', '--resource', resource]);
    assert.equal(record.resource, resource);
    assert.equal(record.token, 'fixture-secret'); assert.equal(record.home, f.directory);
    assert.equal(record.key, undefined); assert.equal(record.sender, undefined);
    assert.equal(JSON.stringify(grant).includes('fixture-secret'), false);
  } finally { f.close(); }
});

test('MCP subprocess handshakes, forwards literal call/key/token and ignores supplied principal', async () => {
  const f = fixture();
  try {
    const args = { target: agent, message: 'literal\n$not-shell' };
    const receipt = await f.client.invoke({ operation: 'followup_task', arguments: args, access_token: 'fixture-secret', idempotency_key: 'controller:fixture:abc', principal: { controller_id: 'spoof', audience: resource } });
    assert.equal(receipt.structuredContent.request_id, 'ask_fixture');
    const records = f.records();
    assert.deepEqual(records[0].args, ['mcp', 'serve', '--controller']);
    assert.equal(records[0].key, 'controller:fixture:abc'); assert.equal(records[0].token, 'fixture-secret');
    assert.deepEqual(records.slice(1).map(record => record.method), ['initialize', 'notifications/initialized', 'tools/call']);
    assert.deepEqual(records[3].params, { name: 'followup_task', arguments: args });
    assert.equal(JSON.stringify(records.slice(1)).includes('spoof'), false);
    assert.throws(() => process.kill(records[0].pid, 0), 'owned child must be gone before resolution');
  } finally { f.close(); }
});

test('malformed, oversized and invalid UTF-8 verification fail without diagnostics leakage', async () => {
  for (const behavior of ['oversized', 'invalid']) {
    const f = fixture(behavior);
    try {
      await assert.rejects(f.client.verify_access_token('fixture-secret', { resource }), error => !/private-token|private-path|fixture-secret/.test(error.message));
      assert.equal(f.records().length, 1);
    } finally { f.close(); }
  }
});

test('wrong receipt IDs and lost receipts fail without retries or surviving child', async () => {
  for (const behavior of ['wrong-id', 'lost']) {
    const f = fixture(behavior);
    try {
      await assert.rejects(f.client.invoke({ operation: 'list_agents', arguments: {}, access_token: 'fixture-secret', idempotency_key: 'fixed-key', principal: { audience: resource } }));
      const records = f.records(); assert.equal(records.filter(record => record.args).length, 1);
      assert.throws(() => process.kill(records[0].pid, 0));
    } finally { f.close(); }
  }
});

test('deadline kills its child and never restarts', async () => {
  const f = fixture('hang', 500);
  try {
    await assert.rejects(f.client.verify_access_token('fixture-secret', { resource }));
    const records = f.records(); assert.equal(records.length, 1);
    assert.throws(() => process.kill(records[0].pid, 0));
  } finally { f.close(); }
});

test('MCP tool deadline kills an initialized child without replaying the admitted call', async () => {
  const f = fixture('hang-tool', 500);
  try {
    await assert.rejects(f.client.invoke({ operation: 'followup_task', arguments: { target: agent, message: 'fixture' },
      access_token: 'fixture-secret', idempotency_key: 'fixed-key', principal: { audience: resource } }));
    const records = f.records();
    assert.equal(records.filter(record => record.method === 'tools/call').length, 1);
    assert.throws(() => process.kill(records[0].pid, 0));
  } finally { f.close(); }
});

test('missing executable rejects with sanitized failure rather than hanging', async () => {
  const client = createNativeControllerClient({ cli_path: join(tmpdir(), 'nonexistent-wardian-fixture'), wardian_home: tmpdir(), resource, timeout_ms: 500 });
  await assert.rejects(client.verify_access_token('fixture-secret', { resource }), error => !error.message.includes(tmpdir()));
});

test('operator configuration and caller request bounds fail before subprocess', async () => {
  assert.throws(() => createNativeControllerClient({ cli_path: 'wardian', wardian_home: tmpdir() }));
  const f = fixture();
  try {
    for (const changed of [{ operation: 'reply' }, { arguments: null }, { idempotency_key: 'bad\nkey' }, { access_token: 'bad\0token' }, { principal: { audience: 'https://wrong.example/mcp' } }]) {
      await assert.rejects(f.client.invoke({ operation: 'list_agents', arguments: {}, access_token: 'fixture-secret', idempotency_key: 'key', principal: { audience: resource }, ...changed }));
    }
    assert.throws(() => f.records(), /ENOENT/);
  } finally { f.close(); }
});
