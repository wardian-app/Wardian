#!/usr/bin/env node
/** Optional owner-private tunnel adapter. Importing this module opens no listener. */
import { Readable } from 'node:stream';
import { appendFile, readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { createControllerHandler } from './controller.mjs';
import { createNativeControllerClient } from './native-client.mjs';
import { validateServerConfig } from './server.mjs';

const MAX_BYTES = 1024 * 1024;
const LEGACY = '2025-11-25';
const MODERN = '2026-07-28';
const VERSIONS = [MODERN, LEGACY];
const INFO = { name: 'wardian-external-controller', version: '0.1.0' };
const AUDIT_METHODS = new Set(['initialize', 'ping', 'tools/list', 'server/discover', 'notifications/initialized', 'notifications/cancelled']);
const PREFIX = 'io.modelcontextprotocol/';
const object = value => value !== null && typeof value === 'object' && !Array.isArray(value);
const error = (id, code, message, data) => ({ jsonrpc: '2.0', id, error: { code, message, ...(data === undefined ? {} : { data }) } });
async function bounded(callback) {
  let timer;
  try { return await Promise.race([Promise.resolve().then(callback), new Promise((_, reject) => { timer = setTimeout(() => reject(new Error('deadline')), 75000); })]); }
  finally { clearTimeout(timer); }
}
const validId = id => (typeof id === 'string' || Number.isSafeInteger(id)) && JSON.stringify(id).length <= 200;

/** All callers share this fixed owner principal. Wire metadata never selects credentials or identity. */
export function createTunnelRequestHandler(config, { client, audit, token = process.env.WARDIAN_CONTROLLER_TOKEN } = {}) {
  const policy = validateServerConfig(config);
  if (typeof token !== 'string' || !/^[A-Za-z0-9._~+/-]+={0,2}$/.test(token) || token.length > 8185) throw new Error('Controller credential environment required');
  client ??= createNativeControllerClient(policy);
  const auditEvent = audit ?? (event => appendFile(policy.audit_path, JSON.stringify({ at: new Date().toISOString(), ...event }) + '\n', { mode: 0o600 }));
  const resource = new URL(policy.resource);
  let active = 0;
  return async input => {
    const hasId = object(input) && Object.hasOwn(input, 'id');
    const id = hasId && validId(input.id) ? input.id : null;
    if (!object(input) || Object.keys(input).some(key => !['jsonrpc', 'id', 'method', 'params'].includes(key)) || input.jsonrpc !== '2.0' || typeof input.method !== 'string' || (hasId && !validId(input.id))) return error(null, -32600, 'Invalid request');
    if (active >= 8) return hasId ? error(id, -32603, 'Controller busy; do not replay automatically') : undefined;
    const params = input.params === undefined ? {} : input.params;
    if (!object(params) || (params._meta !== undefined && !object(params._meta))) return hasId ? error(id, -32602, 'Invalid params') : undefined;
    const meta = params._meta ?? {};
    const requested = meta[PREFIX + 'protocolVersion'];
    const modern = requested !== undefined || input.method === 'server/discover';
    if (modern && requested === undefined) return hasId ? error(id, -32602, 'Required protocol metadata missing') : undefined;
    if (modern && requested !== MODERN) return hasId ? error(id, -32022, 'Unsupported protocol version', { supported: VERSIONS, requested: requested ?? null }) : undefined;
    if (modern && (!object(meta[PREFIX + 'clientCapabilities']) || (meta[PREFIX + 'clientInfo'] !== undefined && (!object(meta[PREFIX + 'clientInfo']) || !['name', 'version'].every(key => typeof meta[PREFIX + 'clientInfo'][key] === 'string' && meta[PREFIX + 'clientInfo'][key].trim()))))) return hasId ? error(id, -32602, 'Required client metadata missing or invalid') : undefined;
    if (input.method === 'server/discover' && Object.keys(params).some(key => key !== '_meta')) return hasId ? error(id, -32602, 'Invalid discovery params') : undefined;
    if (modern && input.method === 'initialize') return hasId ? error(id, -32601, 'Use server/discover for this protocol version') : undefined;
    active++;
    try {
      let grant;
      const handler = createControllerHandler({ resource: policy.resource, authorization_servers: [], allowed_origins: policy.allowed_origins,
        verify_access_token: async (...args) => { grant = await client.verify_access_token(...args); return grant; }, invoke: client.invoke, audit: auditEvent });
      const mapped = input.method === 'server/discover' ? { ...input, method: 'ping', params: {} } : input;
      const request = Readable.from([Buffer.from(JSON.stringify(mapped))]);
      Object.assign(request, { method: 'POST', url: resource.pathname, headers: { host: resource.host, authorization: 'Bearer ' + token, accept: 'application/json, text/event-stream', 'content-type': 'application/json', 'mcp-protocol-version': LEGACY } });
      let status, body;
      const response = { headersSent: false, destroyed: false, writeHead(value) { status = value; this.headersSent = true; }, end(value) { body = value === undefined ? undefined : JSON.parse(value); } };
      await handler(request, response);
      if (status !== 200 && status !== 202) return hasId ? (body?.jsonrpc ? body : error(id, -32603, 'Controller boundary rejected request', { status, delivery_uncertain: body?.delivery_uncertain === true })) : undefined;
      if (input.method !== 'tools/call') {
        const admitted = JSON.stringify(grant);
        await bounded(() => auditEvent({ controller_id: grant.controller_id, controller_name: grant.controller_name, grant_id: grant.grant_id, operation: AUDIT_METHODS.has(input.method) ? input.method : 'unknown', request_id: id, outcome: 'boundary_accepted' }));
        const fresh = await bounded(() => client.verify_access_token(token, { resource: policy.resource }));
        if (JSON.stringify(fresh) !== admitted || fresh.revoked || fresh.expires_at_ms <= Date.now()) return hasId ? error(id, -32603, 'Controller authorization changed') : undefined;
      }
      if (!hasId) return undefined;
      if (input.method === 'server/discover') body.result = { resultType: 'complete', supportedVersions: VERSIONS, capabilities: { tools: {} }, ttlMs: 0, cacheScope: 'private' };
      if (modern && body.result) {
        body.result = { ...body.result, resultType: 'complete', _meta: { ...(body.result._meta ?? {}), [PREFIX + 'serverInfo']: INFO } };
        if (input.method === 'tools/list') Object.assign(body.result, { ttlMs: 0, cacheScope: 'private' });
      }
      return body;
    } catch { return hasId ? error(id, -32603, 'Controller receipt unavailable; do not replay automatically') : undefined; }
    finally { active--; }
  };
}

/** Bounded newline framing; at most eight requests, no replay, no queued input frames. */
export async function serveTunnelStdio(input, output, handler) {
  const pending = new Set();
  const outstandingIds = new Set();
  let tail = Buffer.alloc(0);
  let writes = Promise.resolve();
  const write = value => {
    if (value === undefined) return Promise.resolve();
    let frame = JSON.stringify(value);
    if (Buffer.byteLength(frame) > MAX_BYTES) frame = JSON.stringify(error(value.id ?? null, -32603, 'Response too large; delivery may be uncertain. Do not replay automatically'));
    const next = writes.then(() => new Promise((accept, reject) => output.write(frame + '\n', err => err ? reject(err) : accept())));
    writes = next;
    return next;
  };
  const dispatch = async line => {
    let message;
    try { message = JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(line)); }
    catch { await write(error(null, -32700, 'Invalid UTF-8 JSON frame')); return; }
    const key = object(message) && Object.hasOwn(message, 'id') && validId(message.id) ? JSON.stringify(message.id) : undefined;
    if (key !== undefined && outstandingIds.has(key)) { await write(error(message.id, -32600, 'Request ID already outstanding')); return; }
    while (pending.size >= 8) await Promise.race(pending);
    if (key !== undefined) outstandingIds.add(key);
    const task = Promise.resolve().then(() => handler(message)).then(write).finally(() => { pending.delete(task); if (key !== undefined) outstandingIds.delete(key); });
    pending.add(task);
  };
  try {
    for await (const value of input) {
      const chunk = Buffer.isBuffer(value) ? value : Buffer.from(value);
      let start = 0;
      for (;;) {
        const end = chunk.indexOf(10, start);
        const part = chunk.subarray(start, end < 0 ? chunk.length : end);
        if (tail.length + part.length > MAX_BYTES) { await write(error(null, -32700, 'Frame too large; stream stopped')); throw new Error('Oversized tunnel frame'); }
        tail = Buffer.concat([tail, part]);
        if (end < 0) break;
        const line = tail; tail = Buffer.alloc(0);
        await dispatch(line);
        start = end + 1;
      }
    }
    if (tail.length) await write(error(null, -32700, 'Truncated newline frame'));
  } finally { await Promise.allSettled(pending); await writes; }
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  try {
    if (process.argv.length !== 5 || process.argv[2] !== '--config' || process.argv[4] !== '--stdio') throw new Error('Explicit --config <file> --stdio required');
    const config = JSON.parse(await readFile(process.argv[3], 'utf8'));
    await serveTunnelStdio(process.stdin, process.stdout, createTunnelRequestHandler(config));
  } catch {
    process.stderr.write('Wardian tunnel adapter stopped. Check private configuration; no credentials are printed.\n');
    process.exitCode = 1;
  }
}
