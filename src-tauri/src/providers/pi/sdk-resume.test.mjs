import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { randomUUID, createHash } from 'node:crypto';
import { pathToFileURL } from 'node:url';
import test from 'node:test';
import { validateArgs, validateHistory, sessionOptions, resolveTrust, createExactRuntime, runExactTui } from './sdk-resume.mjs';

const hash = bytes => createHash('sha256').update(bytes).digest('hex');
function fixture() {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'wardian-pi-sdk-test-'));
  const cwd = path.join(root, 'destination');
  const sessionDir = path.join(root, 'sessions');
  fs.mkdirSync(cwd); fs.mkdirSync(sessionDir);
  const sessionFile = path.join(sessionDir, 'exact.jsonl');
  const prefix = Buffer.from('{"type":"session","version":3,"id":"exact","cwd":"source"}\n'
    + '{"type":"message","message":{"role":"assistant"}}\n');
  fs.writeFileSync(sessionFile, prefix);
  return { root, prefix, launch: { cwd, sessionDir, sessionFile, sessionId: 'exact',
    prefixBytes: prefix.length, prefixSha256: hash(prefix) } };
}

test('unsupported session modes, metadata commands, overrides and attachments reject', () => {
  const { launch } = fixture();
  const base = ['--session', launch.sessionFile, '--session-dir', launch.sessionDir];
  for (const flag of ['--fork', '--session-id', '--resume', '--continue', '--no-session',
    '--session=other', '--api-key', '--print', '--mode', '--help', '--list-models', '@file', '--extension-flag']) {
    assert.throws(() => validateArgs([...base, flag], launch));
  }
  assert.throws(() => validateArgs([...base, '--session', launch.sessionFile], launch));
  assert.throws(() => validateArgs([...base, '--session-dir', launch.sessionDir], launch));
  assert.throws(() => validateArgs([...base, '--thinking'], launch));
  validateArgs([...base, '--tools', 'read,bash', '--exclude-tools', 'bash', '--no-approve',
    '--extension', '/bridge.mjs', '--offline', '--tui-mode', 'regular', 'prompt text'], launch);
});

test('history receipt rejects missing, changed, wrong-ID, incomplete and legacy before open', () => {
  const { launch, prefix } = fixture();
  validateHistory(launch, 3);
  const before = fs.readFileSync(launch.sessionFile);
  for (const altered of [{ ...launch, sessionId: 'wrong' }, { ...launch, prefixBytes: prefix.length + 1 },
    { ...launch, prefixSha256: '0'.repeat(64) }, { ...launch, sessionFile: path.join(launch.sessionDir, 'missing') }]) {
    assert.throws(() => validateHistory(altered, 3));
    assert.deepEqual(fs.readFileSync(launch.sessionFile), before);
  }
  assert.throws(() => validateHistory(launch, 2));
  for (const content of ['', 'bad\n', '{"type":"session","version":3,"id":"exact"}\n',
    prefix.toString().replace('"version":3', '"version":2')]) {
    fs.writeFileSync(launch.sessionFile, content);
    const guarded = { ...launch, prefixBytes: Buffer.byteLength(content), prefixSha256: hash(content) };
    assert.throws(() => validateHistory(guarded, 3));
    assert.equal(fs.readFileSync(launch.sessionFile, 'utf8'), content);
  }
});

test('model suffix, explicit thinking, scoped cycling, and tool exclusion preserve precedence', async () => {
  const selected = { provider: 'provider', id: 'chosen' };
  const scoped = { provider: 'provider', id: 'scoped' };
  const sdk = { resolveModelScopeWithDiagnostics: async () => ({ scopedModels: [{ model: scoped }], diagnostics: [] }),
    resolveCliModel: () => ({ model: selected, thinkingLevel: 'high' }) };
  const services = { settingsManager: { getEnabledModels: () => ['scoped'] }, modelRuntime: {} };
  const manager = { buildSessionContext: () => ({ messages: ['retained'] }) };
  const resolved = await sessionOptions(sdk, { model: 'chosen:high', tools: ['read', 'bash'],
    excludeTools: ['bash'], noBuiltinTools: true }, services, manager);
  assert.equal(resolved.options.model, selected);
  assert.equal(resolved.options.thinkingLevel, 'high');
  assert.equal(resolved.thinkingOverride, true);
  assert.deepEqual(resolved.options.scopedModels, [{ model: scoped }]);
  assert.equal(resolved.options.noTools, 'builtin');
  assert.deepEqual(resolved.options.tools, ['read', 'bash']);
  assert.deepEqual(resolved.options.excludeTools, ['bash']);
  const explicit = await sessionOptions(sdk, { model: 'chosen:high', thinking: 'off', noTools: true }, services, manager);
  assert.equal(explicit.options.thinkingLevel, 'off');
  assert.equal(explicit.options.noTools, 'all');
});

test('trust overrides, extension decisions, remembered/default and session-only prompt preserve policy', async () => {
  const { launch } = fixture();
  const sdk = { hasTrustRequiringProjectResources: () => true };
  const writes = [];
  const store = { get: () => null, set: (...args) => writes.push(args), setMany: (...args) => writes.push(args) };
  const settings = { getDefaultProjectTrust: () => 'ask' };
  const context = { hasUI: true, ui: { select: async () => 'Trust for this session only', notify: () => {} } };
  const empty = { extensions: [] };
  assert.equal(await resolveTrust(sdk, { projectTrustOverride: false }, store, settings, launch.cwd, empty, context), false);
  assert.equal(await resolveTrust(sdk, {}, store, settings, launch.cwd, empty, context), true);
  assert.equal(writes.length, 0);
  const extensions = { extensions: [{ handlers: new Map([['project_trust', [
    async () => ({ trusted: 'undecided' }), async () => ({ trusted: 'no', remember: true })]]]) }] };
  assert.equal(await resolveTrust(sdk, {}, store, settings, launch.cwd, extensions, context), false);
  assert.deepEqual(writes, [[launch.cwd, false]]);
  assert.equal(await resolveTrust(sdk, {}, { ...store, get: () => true }, settings, launch.cwd, empty, context), true);
  assert.equal(await resolveTrust(sdk, {}, store, { getDefaultProjectTrust: () => 'never' }, launch.cwd, empty, context), false);
});

test('TUI theme/init/run failures stop rendering and await SDK runtime drain', async () => {
  for (const failure of ['theme', 'constructor', 'run']) {
    const events = [];
    const runtime = { services: { settingsManager: { getTheme: () => 'dark' } },
      dispose: async () => { await Promise.resolve(); events.push('disposed'); } };
    const sdk = { initTheme: () => { if (failure === 'theme') throw new Error(failure); },
      InteractiveMode: class {
        constructor() { if (failure === 'constructor') throw new Error(failure); }
        async run() { throw new Error(failure); }
        stop() { events.push('stopped'); }
      } };
    await assert.rejects(runExactTui(sdk, runtime, { messages: [] }), new RegExp(failure));
    assert.deepEqual(events, failure === 'run' ? ['stopped', 'disposed'] : ['disposed']);
  }
});

test('real installed SDK restores exact history, destination resources/tools and extension without TUI',
  { skip: !process.env.PI_SDK_PACKAGE_ROOT }, async () => {
    const { root, launch } = fixture();
    const agentDir = path.join(root, 'agent');
    fs.mkdirSync(agentDir);
    process.env.PI_CODING_AGENT_DIR = agentDir;
    const sdk = await import(pathToFileURL(path.join(process.env.PI_SDK_PACKAGE_ROOT, 'dist/index.js')));
    const id = randomUUID();
    const source = path.join(root, 'source'); fs.mkdirSync(source);
    const manager = sdk.SessionManager.create(source, launch.sessionDir, { id });
    manager.appendThinkingLevelChange('off');
    manager.appendMessage({ role: 'user', content: 'synthetic fixture retained context', timestamp: Date.now() });
    manager.appendMessage({ role: 'assistant', content: [{ type: 'text', text: 'synthetic fixture retained reply' }],
      api: 'openai-responses', provider: 'fixture', model: 'fixture', stopReason: 'stop', timestamp: Date.now(),
      usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, totalTokens: 0,
        cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 } } });
    const file = manager.getSessionFile(); const bytes = fs.readFileSync(file);
    const exact = { ...launch, sessionId: id, sessionFile: file, prefixBytes: bytes.length, prefixSha256: hash(bytes) };
    fs.writeFileSync(path.join(source, 'where.txt'), 'source-only');
    fs.writeFileSync(path.join(launch.cwd, 'where.txt'), 'destination-only');
    fs.writeFileSync(path.join(launch.cwd, 'AGENTS.md'), 'Destination context instructions');
    const append = path.join(root, 'append.md'); fs.writeFileSync(append, 'Explicit appended context');
    const extension = path.join(root, 'extension.mjs');
    const trustMarker = path.join(root, 'trust-mode.txt');
    fs.mkdirSync(path.join(launch.cwd, '.pi'));
    fs.writeFileSync(path.join(launch.cwd, '.pi', 'settings.json'), '{}');
    fs.writeFileSync(extension, `import fs from 'node:fs'; export default pi => {
      pi.registerFlag('loaded-proof', { type: 'boolean', default: true });
      pi.on('project_trust', (event, ctx) => { fs.writeFileSync(${JSON.stringify(trustMarker)}, ctx.mode); return { trusted: 'no' }; });
    };\n`);
    const modelRuntime = await sdk.ModelRuntime.create({ authPath: path.join(agentDir, 'auth.json'),
      modelsPath: null, modelsStorePath: path.join(agentDir, 'model-store.json'), refreshOnCreate: false,
      allowModelNetwork: false });
    const args = ['--session', file, '--session-dir', launch.sessionDir, '--tui-mode', 'regular',
      '--offline', '--thinking', 'off', '--tools', 'read,bash', '--exclude-tools', 'bash',
      '--append-system-prompt', append, '-e', extension];
    const ui = { select: async () => undefined, notify: () => {} };
    const { runtime } = await createExactRuntime(sdk, exact, args, { agentDir, modelRuntime, ui });
    try {
      assert.equal(runtime.cwd, launch.cwd);
      assert.equal(runtime.session.sessionId, id);
      assert.equal(runtime.session.sessionFile, file);
      assert.ok(JSON.stringify(runtime.session.messages).includes('synthetic fixture retained context'));
      assert.ok(runtime.session.agent.state.systemPrompt.includes('Destination context instructions'));
      assert.ok(runtime.session.agent.state.systemPrompt.includes('Explicit appended context'));
      assert.ok(runtime.services.resourceLoader.getExtensions().extensions.some(item => item.flags.has('loaded-proof')));
      assert.equal(fs.readFileSync(trustMarker, 'utf8'), 'tui');
      assert.equal(runtime.session.agent.state.tools.some(tool => tool.name === 'bash'), false);
      const read = runtime.session.agent.state.tools.find(tool => tool.name === 'read');
      const result = await read.execute(randomUUID(), { path: 'where.txt' });
      assert.equal(result.content[0].text, 'destination-only');
      assert.deepEqual(fs.readFileSync(file), bytes);
      // Same route used by a later ordinary Resume; saved cwd is still source.
      assert.equal(sdk.SessionManager.open(file, launch.sessionDir).getCwd(), source);
      assert.equal(sdk.SessionManager.open(file, launch.sessionDir, launch.cwd).getSessionId(), id);
    } finally { await runtime.dispose(); }
  });


test('actual SDK path normalization preserves exact binding while distinct authorities reject',
  { skip: !process.env.PI_SDK_PACKAGE_ROOT }, async () => {
    const { root, launch, prefix } = fixture();
    const { SessionManager } = await import(pathToFileURL(path.join(process.env.PI_SDK_PACKAGE_ROOT,
      'dist/core/session-manager.js')));
    const accepted = new Error('read-only binding guard accepted');
    let crossed = 0;
    const probe = open => ({ CURRENT_SESSION_VERSION: 3,
      parseArgs: () => ({ diagnostics: [], unknownFlags: new Map() }),
      SessionManager: { open }, getAgentDir: () => root,
      // Stop before trust, runtime, extension loading or any model/tool request.
      ProjectTrustStore: class { constructor() { crossed++; throw accepted; } },
    });
    const args = ['--session', launch.sessionFile, '--session-dir', launch.sessionDir];
    const spelling = process.platform === 'win32' ? launch.cwd.replaceAll('\\', '/')
      : `${launch.cwd}${path.sep}.${path.sep}`;
    assert.notEqual(spelling, launch.cwd);
    assert.equal(path.resolve(spelling), path.resolve(launch.cwd));
    for (const cwd of [launch.cwd, spelling]) {
      await assert.rejects(createExactRuntime(probe((file, directory, requestedCwd) =>
        SessionManager.open(file, directory, requestedCwd)), { ...launch, cwd }, args),
        error => error === accepted);
      assert.deepEqual(fs.readFileSync(launch.sessionFile), prefix);
    }
    assert.equal(crossed, 2);
    const wrongCwd = path.join(root, 'wrong-destination'); fs.mkdirSync(wrongCwd);
    await assert.rejects(createExactRuntime(probe((file, directory) =>
      SessionManager.open(file, directory, wrongCwd)), launch, args), /binding changed/);
    const otherDirectory = path.join(root, 'other-session-directory'); fs.mkdirSync(otherDirectory);
    const otherFile = path.join(otherDirectory, path.basename(launch.sessionFile));
    fs.writeFileSync(otherFile, prefix);
    await assert.rejects(createExactRuntime(probe(() => SessionManager.open(otherFile, otherDirectory,
      launch.cwd)), launch, args), /binding changed/);
    await assert.rejects(createExactRuntime(probe((file, directory, cwd) => {
      const manager = SessionManager.open(file, directory, cwd);
      return { getSessionId: () => 'different-session', getSessionFile: () => manager.getSessionFile(),
        getCwd: () => manager.getCwd() };
    }), launch, args), /binding changed/);
    await assert.rejects(createExactRuntime(probe((file, directory, cwd) => {
      const manager = SessionManager.open(file, directory, cwd);
      fs.renameSync(file, `${file}.previous`); fs.writeFileSync(file, prefix);
      return manager;
    }), launch, args), /binding changed/);
    assert.equal(crossed, 2);
    assert.deepEqual(fs.readFileSync(launch.sessionFile), prefix);
  });
