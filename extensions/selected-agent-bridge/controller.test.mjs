import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createServer, request as httpRequest } from 'node:http';
import { once } from 'node:events';
import { createControllerHandler } from './controller.mjs';

const agent = '20000000-0000-4000-8000-000000000000';
const other = '30000000-0000-4000-8000-000000000000';
const controller = '10000000-0000-4000-8000-000000000000';
const resource = 'https://mcp.example.test/mcp';
const scopes = ['agents:read','messages:send','tasks:submit','agents:wake','agents:interrupt','controller:receive','controller:ack','controller:wait'];
const rpc = (name, args = {}, id = 1) => ({ jsonrpc: '2.0', id, method: 'tools/call', params: { name, arguments: args } });
const receipt = value => ({ content: [{ type: 'text', text: JSON.stringify(value) }], structuredContent: value, isError: false });

async function fixture(callback, customize = {}) {
  const state = { grant: { controller_id: controller, controller_name: 'fixture-controller', grant_id: 'fake-grant', audience: resource,
    expires_at_ms: Date.now()+60000, revoked: false, agent_uuids: [agent], scopes }, calls: [], events: [], verified: 0 };
  const handler = createControllerHandler({ resource, authorization_servers: ['https://auth.example.test'], allowed_origins: ['https://client.example.test'],
    verify_access_token: async token => { state.verified++; if(token !== 'fake-token') throw new Error('invalid'); return state.grant; },
    invoke: async call => { state.calls.push(call); return receipt({ operation: call.operation, request_id: 'ask_fixture', interaction_id: 'interaction_fixture', delivery_state: 'admitted' }); },
    audit: async event => { state.events.push(event); }, ...customize });
  // Transient test-only loopback; exported production module never listens.
  const server = createServer(handler);
  server.listen(0, '127.0.0.1');
  await once(server, 'listening');
  const url = `http://127.0.0.1:${server.address().port}`;
  const request = async (body, options = {}) => {
    return await new Promise((resolve, reject) => {
    const pending = httpRequest(url + (options.path ?? '/mcp'), { method: options.method ?? 'POST',
      headers: { Host: 'mcp.example.test', Authorization: 'Bearer fake-token', Accept: 'application/json, text/event-stream', 'Content-Type': 'application/json', 'MCP-Protocol-Version': '2025-11-25', ...options.headers },
      }, response => {
        const chunks = [];
        response.on('data', chunk => chunks.push(chunk));
        response.on('end', () => { const text = Buffer.concat(chunks).toString(); resolve({status: response.statusCode, headers: new Headers(response.headers), body: text ? JSON.parse(text) : null}); });
      });
      pending.on('error', reject);
      pending.end(body === undefined ? undefined : typeof body === 'string' ? body : JSON.stringify(body));
    });
  };
  try { await callback({ state, request }); }
  finally { server.closeAllConnections(); await new Promise(resolve => server.close(resolve)); }
}

test('dormant factory requires an explicit authenticated native port', () => {
  assert.throws(() => createControllerHandler({ resource: 'http://unsafe.test/mcp' }));
  assert.throws(() => createControllerHandler({ resource }));
});
test('in-flight request cap fences authentication subprocesses and releases slots', async () => {
  let release, ready, grant, count = 0;
  const blocked = new Promise(resolve => { release = resolve; });
  const started = new Promise(resolve => { ready = resolve; });
  await fixture(async ({request,state}) => {
    grant = state.grant;
    const pending = Array.from({length:8}, (_,id) => request(rpc('list_agents',{},id)));
    await started;
    try { assert.equal((await request(rpc('list_agents',{},9))).status,503); assert.equal(count,8); }
    finally { release(); }
    assert.ok((await Promise.all(pending)).every(response => response.status === 200));
    assert.equal((await request(rpc('list_agents',{},10))).status,200);
  }, {verify_access_token:async()=>{count++; if(count===8) ready(); await blocked; return grant;},exchange_timeout_ms:3000});
});
test('resource discovery, stateless initialization and JSON-only transport', async () => fixture(async ({ request, state }) => {
  const metadata = await request(undefined, { method: 'GET', path: '/.well-known/oauth-protected-resource/mcp', headers: { Authorization: '' } });
  assert.equal(metadata.status, 200);
  assert.equal(metadata.body.resource, resource);
  const initialized = await request({ jsonrpc:'2.0',id:0,method:'initialize',params:{protocolVersion:'2025-11-25',capabilities:{},clientInfo:{name:'test',version:'1'}} });
  assert.equal(initialized.body.result.protocolVersion, '2025-11-25');
  assert.equal(initialized.headers.has('mcp-session-id'), false);
  assert.equal((await request(undefined, { method:'GET' })).status, 405);
  assert.equal((await request({jsonrpc:'2.0',method:'notifications/cancelled',params:{requestId:1}})).status, 202);
  assert.equal(state.calls.length, 0);
}));
test('authentication rejects unknown, revoked, expired and wrong-audience grants', async () => fixture(async ({ request, state }) => {
  assert.equal((await request(rpc('list_agents'), {headers:{Authorization:'Bearer unknown'}})).status,401);
  for(const patch of [{revoked:true},{expires_at_ms:0},{audience:'https://other.test/mcp'},{controller_id:'an-agent-name'}]) {
    const previous=state.grant; state.grant={...previous,...patch};
    const denied=await request(rpc('list_agents'));
    assert.equal(denied.status,401);
    assert.match(denied.headers.get('www-authenticate'),/resource_metadata/);
    state.grant=previous;
  }
  assert.equal(state.calls.length,0);
}));
test('host, origin, content type, accept, protocol and body validation fail before dispatch', async () => fixture(async ({request,state}) => {
  for(const [headers,status] of [[{Host:'evil.test'},403],[{Origin:'https://evil.test'},403],[{Accept:'application/json'},406],[{'Content-Type':'text/plain'},415],[{'MCP-Protocol-Version':'old'},400]]) {
    assert.equal((await request(rpc('list_agents'),{headers})).status,status);
  }
  assert.equal((await request('not-json')).status,400);
  assert.equal((await request(' '.repeat(1024*1024+1))).status,400);
  assert.equal((await request(rpc('list_agents'),{path:'/mcp?token=fake-token'})).status,404);
  assert.equal(state.calls.length,0);
}));
test('capabilities separate read, send, submit/wake, interrupt, receive and ack', async () => fixture(async ({request,state}) => {
  state.grant={...state.grant,scopes:['agents:read','tasks:submit','controller:receive']};
  const tools=await request({jsonrpc:'2.0',id:0,method:'tools/list'});
  assert.deepEqual(tools.body.result.tools.map(tool=>tool.name),['list_agents','receive_messages']);
  assert.equal(tools.body.result.tools[1].inputSchema.properties.ack_cursor,undefined);
  for(const [name,args] of [['send_message',{target:agent,message:'information'}],['followup_task',{target:agent,message:'work'}],['interrupt_agent',{target:agent}],['receive_messages',{ack_cursor:'opaque'}],['reply',{request_id:'ask_fixture',message:'spoof worker'}]]) {
    assert.equal((await request(rpc(name,args))).status,403);
  }
  assert.equal(state.calls.length,0);
}));
test('literal operations carry distinct controller attribution and mailbox namespace, never a managed sender', async () => fixture(async ({request,state}) => {
  const text='literal\n🙂 $message';
  const payloads=[['send_message',{target:agent,message:text}],['followup_task',{target:agent,message:text}],['interrupt_agent',{target:agent}],['receive_messages',{cursor:'opaque',ack_cursor:'prior',limit:3,timeout_ms:0}],['wait_agent',{timeout_ms:0}]];
  for(const [index,[name,args]] of payloads.entries()) {
    assert.equal((await request(rpc(name,args,index))).status,200);
    const call=state.calls.at(-1);
    assert.equal(call.principal.kind,'external_controller');
    assert.equal(call.principal.mailbox_owner,`controller:${controller}`);
    assert.equal(Object.hasOwn(call.principal,'session_id'),false);
    assert.deepEqual(call.arguments,args);
    assert.equal(call.operation,name);
    assert.equal(JSON.stringify(state.events).includes(text),false);
    assert.equal(JSON.stringify(state.events).includes('fake-token'),false);
  }
}));
test('unselected UUIDs, sender overrides, unknown fields and malformed bounds never invoke broker', async () => fixture(async ({request,state}) => {
  for(const [name,args] of [['send_message',{target:other,message:'x'}],['send_message',{target:agent,message:'x',sender:controller}],['send_message',{target:agent,message:'x',idempotency_key:'spoof'}],['receive_messages',{timeout_ms:60001}],['receive_messages',{limit:0}],['receive_messages',null]]) {
    assert.equal((await request(rpc(name,args))).status,403);
  }
  assert.equal(state.calls.length,0);
}));
test('revocation during audit fences dispatch and revoked waits disclose no result', async () => {
  let revoke;
  await fixture(async ({request,state})=>{revoke=()=>{state.grant={...state.grant,revoked:true};};
    assert.equal((await request(rpc('followup_task',{target:agent,message:'work'}))).status,401);
    assert.equal(state.calls.length,0);
  },{audit:async()=>revoke()});
  let revokeAfter;
  await fixture(async ({request,state})=>{revokeAfter=()=>{state.grant={...state.grant,revoked:true};};
    const response=await request(rpc('receive_messages'));
    assert.equal(response.status,401);
    assert.equal(JSON.stringify(response).includes('private-result'),false);
  },{invoke:async()=>{revokeAfter();return receipt({operation:'receive_messages',message:'private-result'});}});
});
test('audit failure prevents dispatch; lost receipts do not retry; backend deadlines are bounded', async () => {
  await fixture(async ({request,state})=>{assert.equal((await request(rpc('followup_task',{target:agent,message:'work'}))).status,503);assert.equal(state.calls.length,0);}, {audit:async()=>{throw new Error('audit offline');}});
  let calls=0;
  await fixture(async ({request})=>{const response=await request(rpc('followup_task',{target:agent,message:'work'}));assert.equal(response.body.result.isError,true);assert.equal(calls,1);}, {invoke:async()=>{calls++;throw new Error('lost receipt');}});
  await fixture(async ({request})=>{const response=await request(rpc('wait_agent'));assert.equal(response.body.result.isError,true);}, {invoke:async()=>new Promise(()=>{}),exchange_timeout_ms:30});
});
test('SDK counters can restart; explicit request keys are principal-bound and stable', async () => fixture(async ({request,state})=>{
  await request(rpc('send_message',{target:agent,message:'x'},1));
  const ordinary=state.calls.at(-1).idempotency_key;
  await request(rpc('send_message',{target:agent,message:'new task'},1));
  assert.notEqual(state.calls.at(-1).idempotency_key,ordinary);
  const keyed = id => { const value = rpc('send_message',{target:agent,message:'x'},id); value.params._meta={'wardian/request_key':'logical-request'}; return value; };
  await request(keyed(1));
  const first = state.calls.at(-1).idempotency_key;
  state.grant={...state.grant,grant_id:'rotated-fake-grant'};
  await request(keyed(2));
  assert.equal(state.calls.at(-1).idempotency_key,first);
  state.grant={...state.grant,controller_id:other};
  await request(keyed(1));
  assert.notEqual(state.calls.at(-1).idempotency_key,first);
  const before=state.calls.length;
  for(const key of [null,42,'',' ', 'é'.repeat(101)]) { const value=keyed(3); value.params._meta['wardian/request_key']=key; assert.equal((await request(value)).status,403); }
  assert.equal(state.calls.length,before);
}));
test('roster filtering replaces both representations and omits private fields', async()=> fixture(async({request})=>{
  const response=await request(rpc('list_agents'));
  assert.equal(response.body.result.structuredContent.agents.length,1);
  assert.equal(JSON.stringify(response).includes('private-path'),false);
  assert.equal(JSON.stringify(response).includes('hidden-agent'),false);
},{invoke:async()=>receipt({operation:'list_agents',agents:[{uuid:agent,name:'selected',workspace:'private-path'},{uuid:other,name:'hidden-agent'}]})}));
