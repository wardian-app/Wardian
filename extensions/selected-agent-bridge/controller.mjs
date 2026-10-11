/** Dormant MCP resource handler: no listener, credentials, grants, CLI, or provider startup. */
import { createHash, randomUUID } from 'node:crypto';

const VERSION = '2025-11-25';
const MAX_BYTES = 1024 * 1024;
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;
const object = value => value !== null && typeof value === 'object' && !Array.isArray(value);
const only = (value, keys) => object(value) && Object.keys(value).every(key => keys.includes(key));
const nonempty = value => typeof value === 'string' && value.trim().length > 0;
const SCOPES = Object.freeze({
  list_agents: ['agents:read'], send_message: ['messages:send'],
  followup_task: ['tasks:submit', 'agents:wake'],
  receive_messages: ['controller:receive'], wait_agent: ['controller:wait'],
  interrupt_agent: ['agents:interrupt'],
});
const FIELDS = {
  list_agents: [], send_message: ['target', 'message'], followup_task: ['target', 'message'],
  interrupt_agent: ['target'], receive_messages: ['cursor', 'ack_cursor', 'limit', 'timeout_ms'], wait_agent: ['timeout_ms'],
};
const target = { type: 'string', description: 'Exact selected Wardian agent UUID.' };
const message = { type: 'string', minLength: 1, description: 'Literal text; maximum 64 KiB UTF-8.' };
const timeout = { type: 'integer', minimum: 0, maximum: 60000 };
const PROPERTIES = {
  list_agents: {}, send_message: { target, message }, followup_task: { target, message }, interrupt_agent: { target },
  receive_messages: { cursor: { type: 'string', minLength: 1 }, ack_cursor: { type: 'string', minLength: 1 }, limit: { type: 'integer', minimum: 1, maximum: 100 }, timeout_ms: timeout },
  wait_agent: { timeout_ms: timeout },
};
const DESCRIPTIONS = {
  list_agents: 'Read selected agents with status provenance.',
  send_message: 'Send non-waking information; receipt is admission, not model consumption.',
  followup_task: 'Submit work, potentially waking its recipient. Returns a canonical request receipt; poll the controller mailbox for its result.',
  interrupt_agent: 'Interrupt the selected agent current turn; not request-specific cancellation or process termination.',
  receive_messages: 'Read only this controller mailbox. Explicit ack_cursor is a separate granted mutation. Opaque cursors are controller-bound.',
  wait_agent: 'Wait for controller mailbox activity without reading or acknowledgement. Timeout does not cancel work.',
};

const failure = (code, text) => {
  const value = { error: { code, message: text } };
  return { isError: true, structuredContent: value, content: [{ type: 'text', text: JSON.stringify(value) }] };
};
const rpcError = (id, code, message) => ({ jsonrpc: '2.0', id, error: { code, message } });
const grantKey = grant => JSON.stringify({ controller_id: grant.controller_id, controller_name: grant.controller_name,
  grant_id: grant.grant_id, audience: grant.audience, expires_at_ms: grant.expires_at_ms, revoked: grant.revoked,
  agent_uuids: [...grant.agent_uuids].sort(), scopes: [...grant.scopes].sort() });
async function bounded(callback, milliseconds) {
  let timer;
  try { return await Promise.race([Promise.resolve().then(callback), new Promise((_, reject) => { timer = setTimeout(() => reject(new Error('deadline')), milliseconds); })]); }
  finally { clearTimeout(timer); }
}

function validGrant(grant, audience, now) {
  return only(grant, ['controller_id', 'controller_name', 'grant_id', 'audience', 'expires_at_ms', 'revoked', 'agent_uuids', 'scopes']) &&
    typeof grant.controller_id === 'string' && UUID.test(grant.controller_id) && nonempty(grant.controller_name) &&
    grant.controller_name.length <= 100 && !/[\r\n]/.test(grant.controller_name) && nonempty(grant.grant_id) &&
    grant.grant_id.length <= 100 && grant.audience === audience && Number.isSafeInteger(grant.expires_at_ms) &&
    grant.expires_at_ms > now && grant.revoked === false && Array.isArray(grant.agent_uuids) &&
    grant.agent_uuids.length > 0 && grant.agent_uuids.length <= 100 &&
    grant.agent_uuids.every(id => typeof id === 'string' && UUID.test(id)) &&
    new Set(grant.agent_uuids).size === grant.agent_uuids.length && Array.isArray(grant.scopes) &&
    grant.scopes.every(scope => [...new Set(Object.values(SCOPES).flat()), 'controller:ack'].includes(scope));
}

function validateArguments(name, args, grant) {
  if (!only(args, FIELDS[name]) || Object.values(args).some(value => value === null)) return false;
  if (FIELDS[name].includes('target') && !grant.agent_uuids.includes(args.target)) return false;
  if (FIELDS[name].includes('message') && (!nonempty(args.message) || Buffer.byteLength(args.message) > 65536)) return false;
  if (args.limit !== undefined && (!Number.isInteger(args.limit) || args.limit < 1 || args.limit > 100)) return false;
  if (args.timeout_ms !== undefined && (!Number.isInteger(args.timeout_ms) || args.timeout_ms < 0 || args.timeout_ms > 60000)) return false;
  return ['cursor', 'ack_cursor'].every(key => args[key] === undefined || nonempty(args[key]));
}

function definitions(grant) {
  return Object.keys(SCOPES).filter(name => SCOPES[name].every(scope => grant.scopes.includes(scope))).map(name => {
    const properties = structuredClone(PROPERTIES[name]);
    if (properties.target) properties.target.enum = [...grant.agent_uuids];
    if (name === 'receive_messages' && !grant.scopes.includes('controller:ack')) delete properties.ack_cursor;
    return { name, description: DESCRIPTIONS[name], inputSchema: { type: 'object', properties,
      required: FIELDS[name].includes('target') ? (FIELDS[name].includes('message') ? ['target', 'message'] : ['target']) : [], additionalProperties: false },
    annotations: { readOnlyHint: name === 'list_agents' || name === 'wait_agent', destructiveHint: name === 'interrupt_agent', idempotentHint: name === 'list_agents', openWorldHint: name !== 'list_agents' } };
  });
}

/** Read one bounded JSON body, with a deadline. No frame remainder is dispatched. */
async function readBody(request) {
  let size = 0;
  const chunks = [];
  const timer = setTimeout(() => request.destroy(), 10000);
  try {
    for await (const chunk of request) {
      size += chunk.length;
      if (size > MAX_BYTES) throw new Error('body_too_large');
      chunks.push(chunk);
    }
    return JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(Buffer.concat(chunks)));
  } finally { clearTimeout(timer); }
}

function respond(response, status, body, headers = {}) {
  let encoded = body === undefined ? undefined : JSON.stringify(body);
  if (encoded && Buffer.byteLength(encoded) > MAX_BYTES) {
    status = 500;
    encoded = JSON.stringify({ error: 'response_too_large', delivery_uncertain: true });
  }
  response.writeHead(status, { 'Cache-Control': 'no-store', ...headers, ...(encoded ? { 'Content-Type': 'application/json' } : {}) });
  response.end(encoded);
}

/**
 * Construct a stateless JSON-response Streamable HTTP handler; never binds a socket.
 * verify_access_token must validate the credential, resource audience and return a fresh authoritative
 * grant on EACH request. It must not cache revocation or accept caller-supplied attribution.
 * invoke must implement the distinct controller mailbox/admission port, never agent impersonation.
 * The optional native client authenticates owner-issued bearer grants. OAuth authorization
 * servers may be supplied by an integration; manual bearer provisioning is not OAuth login.
 */
export function createControllerHandler({ resource, authorization_servers, allowed_origins, verify_access_token, invoke, audit, now = Date.now, exchange_timeout_ms = 75000 }) {
  const parsed = new URL(resource);
  if (parsed.protocol !== 'https:' || parsed.username || parsed.password || parsed.search || parsed.hash || parsed.pathname === '/') throw new Error('HTTPS MCP resource URL required');
  if (!Array.isArray(authorization_servers) || !authorization_servers.every(value => {
    const url = new URL(value); return url.protocol === 'https:' && !url.username && !url.password && !url.search && !url.hash;
  }) || !Array.isArray(allowed_origins) || !allowed_origins.every(value => new URL(value).origin === value) ||
      ![verify_access_token, invoke, audit, now].every(value => typeof value === 'function') || !Number.isInteger(exchange_timeout_ms) || exchange_timeout_ms < 1 || exchange_timeout_ms > 75000) throw new Error('Explicit authority, origins, verifier, broker and audit required');
  const metadataPath = '/.well-known/oauth-protected-resource' + parsed.pathname;
  const challenge = { 'WWW-Authenticate': `Bearer resource_metadata="${parsed.origin}${metadataPath}"` };
  let activeRequests = 0;

  return async (request, response) => {
    // TCP connection limits alone do not bound pipelined authentication subprocesses.
    if (activeRequests >= 8) return respond(response, 503, { error: 'controller_busy' }, { Connection: 'close' });
    activeRequests++;
    let grant;
    let dispatched = false;
    try {
      if (request.headers.host !== parsed.host || (request.headers.origin !== undefined && !allowed_origins.includes(request.headers.origin))) return respond(response, 403, { error: 'origin_rejected' });
      if (request.url === metadataPath && request.method === 'GET') return respond(response, 200, {
        resource, authorization_servers, scopes_supported: [...new Set(Object.values(SCOPES).flat()), 'controller:ack'], bearer_methods_supported: ['header'],
      });
      if (request.url !== parsed.pathname) return respond(response, 404, { error: 'not_found' });
      const authorization = request.headers.authorization;
      if (typeof authorization !== 'string' || !/^Bearer [A-Za-z0-9._~+/-]+={0,2}$/.test(authorization) || authorization.length > 8192) return respond(response, 401, { error: 'invalid_token' }, challenge);
      try { grant = await bounded(() => verify_access_token(authorization.slice(7), { resource }), exchange_timeout_ms); }
      catch { return respond(response, 401, { error: 'invalid_token' }, challenge); }
      if (!validGrant(grant, resource, now())) return respond(response, 401, { error: 'invalid_token' }, challenge);
      grant = structuredClone(grant);
      if (request.method === 'GET' || request.method === 'DELETE') return respond(response, 405, undefined, { Allow: 'POST' });
      if (request.method !== 'POST') return respond(response, 405, undefined, { Allow: 'POST' });
      const accept = request.headers.accept ?? '';
      if (!accept.includes('application/json') || !accept.includes('text/event-stream')) return respond(response, 406, { error: 'accept_required' });
      if (!/^application\/json(?:\s*;.*)?$/i.test(request.headers['content-type'] ?? '')) return respond(response, 415, { error: 'json_required' });
      let input;
      try { input = await readBody(request); } catch { return respond(response, 400, rpcError(null, -32700, 'Invalid or oversized JSON')); }
      const hasId = object(input) && Object.hasOwn(input, 'id');
      const id = hasId ? input.id : null;
      if (!only(input, ['jsonrpc', 'id', 'method', 'params']) || input.jsonrpc !== '2.0' || typeof input.method !== 'string' ||
          (hasId && !(typeof id === 'string' || Number.isSafeInteger(id)))) return respond(response, 400, rpcError(null, -32600, 'Invalid request'));
      if (JSON.stringify(id).length > 200) return respond(response, 400, rpcError(null, -32600, 'Request ID too long'));
      if (input.method !== 'initialize' && request.headers['mcp-protocol-version'] !== VERSION) return respond(response, 400, { error: 'protocol_version_required' });
      const params = input.params === undefined ? {} : input.params;
      if (!object(params)) return respond(response, 400, rpcError(id, -32602, 'Invalid params'));
      if (!hasId) return respond(response, 202); // Cancellation never interrupts or retracts work.
      let result;
      if (input.method === 'initialize') {
        if (params.protocolVersion !== VERSION || !object(params.capabilities) || !nonempty(params.clientInfo?.name) || !nonempty(params.clientInfo?.version)) return respond(response, 400, rpcError(id, -32602, 'Unsupported initialization'));
        result = { protocolVersion: VERSION, capabilities: { tools: {} }, serverInfo: { name: 'wardian-external-controller', version: '0.1.0' } };
      } else if (input.method === 'ping') result = {};
      else if (input.method === 'tools/list') {
        if (!only(params, ['_meta'])) return respond(response, 400, rpcError(id, -32602, 'No pagination'));
        result = { tools: definitions(grant) };
      } else if (input.method === 'tools/call') {
        const name = params.name;
        const args = params.arguments === undefined ? {} : params.arguments;
        const requestKey = params._meta?.['wardian/request_key'];
        const validMeta = params._meta === undefined || (object(params._meta) &&
          (requestKey === undefined || (nonempty(requestKey) && Buffer.byteLength(requestKey) <= 200)));
        const scopes = Object.hasOwn(SCOPES, name) ? SCOPES[name] : null;
        const permitted = only(params, ['name', 'arguments', '_meta']) && validMeta && scopes && scopes.every(scope => grant.scopes.includes(scope)) &&
          validateArguments(name, args, grant) && (args.ack_cursor === undefined || grant.scopes.includes('controller:ack'));
        const event = { controller_id: grant.controller_id, controller_name: grant.controller_name, grant_id: grant.grant_id,
          operation: scopes ? name : 'unknown', target_uuid: permitted && args.target ? args.target : null, request_id: id };
        await bounded(() => audit({ ...event, outcome: permitted ? 'authorized' : 'rejected' }), exchange_timeout_ms);
        if (!permitted) return respond(response, 403, rpcError(id, -32003, 'Insufficient capability or invalid arguments'), { 'WWW-Authenticate': `Bearer error="insufficient_scope", resource_metadata="${parsed.origin}${metadataPath}"` });
        // Revalidate after asynchronous authorization/audit and body parsing. Revoked grants never dispatch.
        let fresh;
        try { fresh = await bounded(() => verify_access_token(authorization.slice(7), { resource }), exchange_timeout_ms); } catch { fresh = null; }
        if (!validGrant(fresh, resource, now()) || grantKey(fresh) !== grantKey(grant)) return respond(response, 401, { error: 'invalid_token' }, challenge);
        const principal = Object.freeze({ kind: 'external_controller', controller_id: grant.controller_id, controller_name: grant.controller_name,
          grant_id: grant.grant_id, audience: resource, mailbox_owner: `controller:${grant.controller_id}` });
        // JSON-RPC IDs correlate responses and may restart with a new SDK instance.
        // Durable replay is an explicit caller choice; new calls otherwise get fresh keys.
        const idempotency_key = `controller:${grant.controller_id}:${createHash('sha256').update(requestKey ?? randomUUID()).digest('hex')}`;
        dispatched = true;
        try { result = await bounded(() => invoke({ principal, operation: name, arguments: structuredClone(args), idempotency_key, access_token: authorization.slice(7) }), exchange_timeout_ms); }
        catch { result = failure('controller_backend_unavailable', 'Controller receipt unavailable; delivery may be uncertain. Do not replay automatically.'); }
        if (!object(result) || !Array.isArray(result.content)) result = failure('invalid_receipt', 'Invalid controller receipt; delivery may be uncertain. Do not replay automatically.');
        if (name === 'list_agents' && !result.isError) {
          if (result.structuredContent?.operation !== name || !Array.isArray(result.structuredContent.agents)) result = failure('invalid_receipt', 'Invalid roster');
          else {
            const agents = result.structuredContent.agents.filter(agent => grant.agent_uuids.includes(agent.uuid)).map(({ uuid, name, provider, status, status_source }) => ({ uuid, name, provider, status, status_source }));
            const value = { operation: name, agents };
            result = { content: [{ type: 'text', text: JSON.stringify(value) }], structuredContent: value, isError: false };
          }
        }
        await bounded(() => audit({ ...event, outcome: result.isError ? 'uncertain_or_failed' : 'receipt_returned' }), exchange_timeout_ms);
        try { fresh = await bounded(() => verify_access_token(authorization.slice(7), { resource }), exchange_timeout_ms); } catch { fresh = null; }
        if (!validGrant(fresh, resource, now()) || grantKey(fresh) !== grantKey(grant)) return respond(response, 401, { error: 'invalid_token', delivery_uncertain: dispatched }, challenge);
      } else return respond(response, 400, rpcError(id, -32601, 'Method not found'));
      return respond(response, 200, { jsonrpc: '2.0', id, result });
    } catch {
      if (!response.headersSent && !response.destroyed) respond(response, 503, { error: 'controller_boundary_unavailable', delivery_uncertain: dispatched });
    } finally {
      activeRequests--;
    }
  };
}
