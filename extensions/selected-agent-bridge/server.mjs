#!/usr/bin/env node
/** Explicit optional loopback gateway. Importing this file never starts a listener. */
import { createServer } from 'node:http';
import { appendFile, readFile } from 'node:fs/promises';
import { isAbsolute, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { createControllerHandler } from './controller.mjs';
import { createNativeControllerClient } from './native-client.mjs';

export function validateServerConfig(value) {
  const keys = ['cli_path', 'wardian_home', 'resource', 'allowed_origins', 'port', 'audit_path'];
  if (!value || typeof value !== 'object' || Array.isArray(value) || Object.keys(value).some(key => !keys.includes(key)) ||
      !['cli_path', 'wardian_home', 'audit_path'].every(key => typeof value[key] === 'string' && isAbsolute(value[key])) ||
      !Number.isInteger(value.port) || value.port < 1 || value.port > 65535 || !Array.isArray(value.allowed_origins)) throw new Error('Invalid controller gateway configuration');
  const resource = new URL(value.resource);
  if (resource.protocol !== 'https:' || resource.username || resource.password || resource.search || resource.hash || resource.pathname === '/') throw new Error('Exact HTTPS resource required');
  if (!value.allowed_origins.every(origin => typeof origin === 'string' && new URL(origin).origin === origin)) throw new Error('Exact browser origins required');
  return Object.freeze({...value, allowed_origins: Object.freeze([...value.allowed_origins])});
}

export function createGateway(config, { client = createNativeControllerClient(config), audit } = {}) {
  const policy = validateServerConfig(config);
  const auditEvent = audit ?? (event => appendFile(policy.audit_path, JSON.stringify({ at: new Date().toISOString(), ...event }) + '\n', { mode: 0o600 }));
  const handler = createControllerHandler({ resource: policy.resource, authorization_servers: [], allowed_origins: policy.allowed_origins,
    verify_access_token: client.verify_access_token, invoke: client.invoke, audit: auditEvent });
  const server = createServer(handler);
  server.maxConnections = 32;
  server.requestTimeout = 10000;
  server.headersTimeout = 10000;
  server.keepAliveTimeout = 5000;
  return server;
}

export async function launchGateway(config) {
  const policy = validateServerConfig(config);
  const server = createGateway(policy);
  await new Promise((accept, reject) => { server.once('error', reject); server.listen(policy.port, '127.0.0.1', accept); });
  return server;
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  try {
    if (process.argv.length !== 5 || process.argv[2] !== '--config' || process.argv[4] !== '--listen') throw new Error('Explicit --config <file> --listen required');
    const config = validateServerConfig(JSON.parse(await readFile(process.argv[3], 'utf8')));
    const server = await launchGateway(config);
    const stop = () => { server.close(); server.closeAllConnections(); };
    process.once('SIGINT', stop); process.once('SIGTERM', stop);
    process.stderr.write('Wardian controller gateway listening on configured loopback port. TLS routing must be configured separately.\n');
  } catch {
    process.stderr.write('Controller gateway startup failed. Check explicit configuration; no credentials are printed.\n');
    process.exitCode = 1;
  }
}
