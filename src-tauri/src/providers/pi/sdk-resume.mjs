/** Exact materialized Pi resume through its public SDK; never a fresh fallback. */
import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import { createHash } from 'node:crypto';
import { createInterface } from 'node:readline/promises';
import { createRequire } from 'node:module';
import { pathToFileURL, fileURLToPath } from 'node:url';

export const VALUE_FLAGS = new Set(['--provider', '--model', '--thinking', '--system-prompt',
  '--append-system-prompt', '--name', '-n', '--session', '--session-dir', '--models',
  '--tools', '-t', '--exclude-tools', '-xt', '--extension', '-e', '--skill',
  '--prompt-template', '--theme', '--use-theme', '--tui-mode']);
export const SWITCH_FLAGS = new Set(['--no-tools', '-nt', '--no-builtin-tools', '-nbt',
  '--no-extensions', '-ne', '--no-skills', '-ns', '--no-prompt-templates', '-np',
  '--no-themes', '--no-context-files', '-nc', '--verbose', '--approve', '-a',
  '--no-approve', '-na', '--offline']);

/** Match adapter preflight: unsupported command modes/extension flags fail closed. */
export function validateArgs(args, launch) {
  let session = 0;
  let directory = 0;
  for (let i = 0; i < args.length; i++) {
    const flag = args[i];
    if (VALUE_FLAGS.has(flag)) {
      const value = args[++i];
      if (value === undefined || !value.trim()) throw new Error(`Pi exact resume requires a value for ${flag}`);
      if (flag === '--thinking' && !['off', 'minimal', 'low', 'medium', 'high', 'xhigh', 'max'].includes(value))
        throw new Error('Unsupported Pi exact resume thinking level');
      if (flag === '--tui-mode' && !['regular', 'fullscreen'].includes(value))
        throw new Error('Unsupported Pi exact resume TUI mode');
      if (flag === '--session' && (++session !== 1 || path.resolve(value) !== launch.sessionFile))
        throw new Error('Pi exact resume selector changed');
      if (flag === '--session-dir' && (++directory !== 1 || path.resolve(value) !== launch.sessionDir))
        throw new Error('Pi exact resume directory changed');
    } else if (!SWITCH_FLAGS.has(flag) && (flag.startsWith('-') || flag.startsWith('@'))) {
      throw new Error(`Pi exact SDK resume does not support argument ${flag}`);
    }
  }
  if (session !== 1 || directory !== 1) throw new Error('Pi exact resume lost managed selectors');
}

/** Read the Rust-bound confirmation prefix without opening Pi's mutating API. */
export function validateHistory(launch, version) {
  if (!path.isAbsolute(launch.sessionFile) || !path.isAbsolute(launch.cwd)
      || !path.isAbsolute(launch.sessionDir) || !Number.isSafeInteger(launch.prefixBytes)
      || launch.prefixBytes < 1 || launch.prefixBytes > 64 * 1024 * 1024
      || !/^[0-9a-f]{64}$/.test(launch.prefixSha256)) throw new Error('Invalid Pi exact resume receipt');
  if (!fs.statSync(launch.cwd).isDirectory()) throw new Error('Pi destination is not a directory');
  const file = fs.lstatSync(launch.sessionFile);
  if (!file.isFile()) throw new Error('Pi exact resume history is not a plain file');
  const fd = fs.openSync(launch.sessionFile, 'r');
  try {
    const stat = fs.fstatSync(fd, { bigint: true });
    const prefix = Buffer.alloc(launch.prefixBytes);
    let offset = 0;
    while (offset < prefix.length) {
      const read = fs.readSync(fd, prefix, offset, prefix.length - offset, offset);
      if (read === 0) throw new Error('Pi history confirmation was truncated');
      offset += read;
    }
    if (createHash('sha256').update(prefix).digest('hex') !== launch.prefixSha256)
      throw new Error('Pi history confirmation changed before SDK open');
    const text = new TextDecoder('utf-8', { fatal: true }).decode(prefix);
    if (!text.endsWith('\n')) throw new Error('Pi history confirmation is incomplete');
    const entries = text.trimEnd().split('\n').map(line => JSON.parse(line));
    if (entries[0].type !== 'session' || entries[0].id !== launch.sessionId
        || entries[0].version !== version) throw new Error('Pi exact resume identity/version mismatch');
    if (!entries.some(entry => entry.type === 'message' && entry.message?.role === 'assistant'))
      throw new Error('Pi exact resume requires materialized assistant history');
    const current = fs.statSync(launch.sessionFile, { bigint: true });
    if (current.dev !== stat.dev || current.ino !== stat.ino)
      throw new Error('Pi exact resume physical file changed');
    return stat;
  } finally { fs.closeSync(fd); }
}

const resolveInput = (cwd, value) => {
  if (/^file:\/\//.test(value)) return fileURLToPath(value);
  if (value === '~') return os.homedir();
  if (process.platform === 'win32' && /^\/(?:mnt\/|cygdrive\/)?[a-z](?:\/|$)/i.test(value)
      && !value.includes('\\')) {
    const match = value.match(/^\/(?:mnt\/|cygdrive\/)?([a-z])(?:\/(.*))?$/i);
    value = `${match[1].toUpperCase()}:\\${(match[2] ?? '').replaceAll('/', '\\')}`;
  }
  return path.resolve(cwd, value.startsWith('~/') || value.startsWith('~\\')
    ? path.join(os.homedir(), value.slice(2)) : value);
};
const resourcePaths = (cwd, values) => values?.map(value =>
  /^(npm:|git:|github:|https?:|ssh:)/.test(value.trim()) ? value : resolveInput(cwd, value));
const promptInput = (cwd, value) => value !== undefined && fs.existsSync(resolveInput(cwd, value))
  ? resolveInput(cwd, value) : value;

/** Preserve CLI model precedence, including thinking suffix and model cycling. */
export async function sessionOptions(sdk, parsed, services, manager) {
  const { settingsManager, modelRuntime } = services;
  const patterns = parsed.models ?? settingsManager.getEnabledModels();
  const scoped = patterns?.length ? await sdk.resolveModelScopeWithDiagnostics(patterns,
    modelRuntime, { signal: AbortSignal.timeout(15000) }) : { scopedModels: [], diagnostics: [] };
  const options = {};
  const diagnostics = [...scoped.diagnostics];
  let thinkingOverride = parsed.thinking !== undefined;
  if (parsed.model) {
    const resolved = sdk.resolveCliModel({ cliProvider: parsed.provider, cliModel: parsed.model,
      cliThinking: parsed.thinking, modelRuntime });
    if (resolved.error) diagnostics.push({ type: 'error', message: resolved.error });
    if (resolved.warning) diagnostics.push({ type: 'warning', message: resolved.warning });
    if (resolved.model) {
      options.model = resolved.model;
      if (parsed.thinking === undefined && resolved.thinkingLevel !== undefined) {
        options.thinkingLevel = resolved.thinkingLevel;
        thinkingOverride = true;
      }
    }
  }
  if (!options.model && scoped.scopedModels.length && manager.buildSessionContext().messages.length === 0) {
    const saved = scoped.scopedModels.find(item => item.model.provider === settingsManager.getDefaultProvider()
      && item.model.id === settingsManager.getDefaultModel()) ?? scoped.scopedModels[0];
    options.model = saved.model;
    options.thinkingLevel = saved.thinkingLevel;
  }
  if (parsed.thinking !== undefined) options.thinkingLevel = parsed.thinking;
  if (scoped.scopedModels.length) options.scopedModels = scoped.scopedModels;
  if (parsed.noTools) options.noTools = 'all';
  else if (parsed.noBuiltinTools) options.noTools = 'builtin';
  if (parsed.tools) options.tools = [...parsed.tools];
  if (parsed.excludeTools) options.excludeTools = [...parsed.excludeTools];
  return { options, diagnostics, thinkingOverride };
}

async function startupUi() {
  const input = createInterface({ input: process.stdin, output: process.stdout });
  return {
    select: async (title, options) => {
      const answer = await input.question(`${title}\n${options.map((item, i) => `${i + 1}. ${item}`).join('\n')}\n> `);
      return options[Number(answer.trim()) - 1];
    },
    confirm: async (title, message) => /^y(es)?$/i.test((await input.question(`${title}\n${message}\n[y/N] `)).trim()),
    input: async (title, placeholder) => input.question(`${title}${placeholder ? ` (${placeholder})` : ''}\n> `),
    notify: (message) => console.error(message),
    close: () => input.close(),
  };
}

/** Run documented extension trust handlers before stored/default trust decisions. */
export async function resolveTrust(sdk, parsed, store, settings, cwd, extensionsResult, context) {
  if (parsed.projectTrustOverride !== undefined) return parsed.projectTrustOverride;
  if (!sdk.hasTrustRequiringProjectResources(cwd)) return true;
  for (const extension of extensionsResult.extensions) {
    for (const handler of extension.handlers.get('project_trust') ?? []) {
      try {
        const result = await handler({ type: 'project_trust', cwd }, context);
        if (!result || result.trusted === 'undecided') continue;
        if (result.trusted !== 'yes' && result.trusted !== 'no') throw new Error('Invalid project trust decision');
        const trusted = result.trusted === 'yes';
        if (result.remember === true) store.set(cwd, trusted);
        return trusted;
      } catch (error) { context.ui.notify(`Extension project_trust error: ${error.message}`, 'warning'); }
    }
  }
  const remembered = store.get(cwd);
  if (remembered !== null) return remembered;
  const policy = settings.getDefaultProjectTrust();
  if (policy === 'always') return true;
  if (policy === 'never' || !context.hasUI) return false;
  const parent = path.dirname(fs.realpathSync(cwd));
  const labels = ['Trust this folder', ...(parent !== cwd ? ['Trust parent folder'] : []),
    'Trust for this session only', 'Do not trust this folder', 'Do not trust for this session'];
  const selected = await context.ui.select(`Trust project folder?\n${cwd}\nLoad project settings, resources and extensions?`, labels);
  if (selected === 'Trust for this session only') return true;
  if (selected === 'Trust this folder') { store.set(cwd, true); return true; }
  if (selected === 'Trust parent folder') { store.setMany([{ path: parent, decision: true }, { path: cwd, decision: null }]); return true; }
  if (selected === 'Do not trust this folder') store.set(cwd, false);
  return false;
}

function checkDiagnostics(diagnostics) {
  for (const entry of diagnostics) console.error(`${entry.type}: ${entry.message}`);
  if (diagnostics.some(entry => entry.type === 'error')) throw new Error('Pi SDK runtime configuration rejected');
}

/** Public runtime factory also rebuilds resources for subsequent /resume and /new. */
export async function createExactRuntime(sdk, launch, args, host = {}) {
  validateArgs(args, launch);
  const parsed = sdk.parseArgs(args);
  checkDiagnostics(parsed.diagnostics);
  if (parsed.unknownFlags.size) throw new Error('Unsupported Pi extension flags for exact SDK resume');
  if (parsed.offline || /^(1|true|yes)$/i.test(process.env.PI_OFFLINE ?? '')) {
    process.env.PI_OFFLINE = '1';
    process.env.PI_SKIP_VERSION_CHECK = '1';
  }
  const stat = validateHistory(launch, sdk.CURRENT_SESSION_VERSION);
  const manager = sdk.SessionManager.open(launch.sessionFile, launch.sessionDir, launch.cwd);
  const after = fs.statSync(launch.sessionFile, { bigint: true });
  if (after.dev !== stat.dev || after.ino !== stat.ino || manager.getSessionId() !== launch.sessionId
      || path.resolve(manager.getSessionFile()) !== launch.sessionFile || manager.getCwd() !== launch.cwd)
    throw new Error('Pi exact SDK resume binding changed');
  const agentDir = host.agentDir ?? sdk.getAgentDir();
  const trustStore = new sdk.ProjectTrustStore(agentDir);
  const bootstrap = sdk.SettingsManager.create(launch.cwd, agentDir, { projectTrusted: false });
  checkDiagnostics(bootstrap.drainErrors().map(entry => ({ type: 'warning', message: `Settings ${entry.scope}: ${entry.error.message}` })));
  if (host.configureHttp) await host.configureHttp(bootstrap);
  const trustByCwd = new Map();
  const initialUi = host.ui ?? await startupUi();
  const cliResources = {
    additionalExtensionPaths: resourcePaths(launch.cwd, parsed.extensions),
    additionalSkillPaths: resourcePaths(launch.cwd, parsed.skills),
    additionalPromptTemplatePaths: resourcePaths(launch.cwd, parsed.promptTemplates),
    additionalThemePaths: resourcePaths(launch.cwd, parsed.themes),
    noExtensions: parsed.noExtensions, noSkills: parsed.noSkills,
    noPromptTemplates: parsed.noPromptTemplates, noThemes: parsed.noThemes,
    noContextFiles: parsed.noContextFiles, systemPrompt: promptInput(launch.cwd, parsed.systemPrompt),
    appendSystemPrompt: parsed.appendSystemPrompt?.map(value => promptInput(launch.cwd, value)),
    extensionFactories: host.extensionFactories,
  };
  const createRuntime = async ({ cwd, sessionManager, sessionStartEvent, projectTrustContext }) => {
    const hasResources = sdk.hasTrustRequiringProjectResources(cwd);
    const needsTrust = parsed.projectTrustOverride === undefined && !trustByCwd.has(cwd) && hasResources;
    const trusted = trustByCwd.get(cwd) ?? parsed.projectTrustOverride ?? !hasResources;
    const settingsManager = sdk.SettingsManager.create(cwd, agentDir, { projectTrusted: trusted });
    if (parsed.useTheme !== undefined) settingsManager.applyOverrides({ theme: parsed.useTheme });
    const services = await sdk.createAgentSessionServices({ cwd, agentDir, settingsManager,
      modelRuntime: host.modelRuntime, modelRuntimeSignal: AbortSignal.timeout(15000),
      extensionFlagValues: parsed.unknownFlags,
      resourceLoaderOptions: cliResources,
      resourceLoaderReloadOptions: needsTrust ? { resolveProjectTrust: async ({ extensionsResult }) => {
        const decision = await resolveTrust(sdk, parsed, trustStore, bootstrap, cwd, extensionsResult,
          projectTrustContext ?? { cwd, mode: 'tui', hasUI: true, ui: initialUi });
        trustByCwd.set(cwd, decision);
        return decision;
      } } : undefined });
    const resolved = await sessionOptions(sdk, parsed, services, sessionManager);
    const diagnostics = [...services.diagnostics, ...resolved.diagnostics,
      ...settingsManager.drainErrors().map(entry => ({ type: 'warning', message: `Settings ${entry.scope}: ${entry.error.message}` })),
      ...services.resourceLoader.getExtensions().errors.map(error => ({ type: 'error',
        message: `Failed to load extension ${error.path}: ${error.error}` }))];
    checkDiagnostics(diagnostics);
    if (host.configureHttp) await host.configureHttp(settingsManager);
    const created = await sdk.createAgentSessionFromServices({ services, sessionManager,
      sessionStartEvent, ...resolved.options });
    if (created.session.model && resolved.thinkingOverride)
      created.session.setThinkingLevel(created.session.thinkingLevel);
    return { ...created, services, diagnostics };
  };
  try {
    const runtime = await sdk.createAgentSessionRuntime(createRuntime,
      { cwd: launch.cwd, agentDir, sessionManager: manager });
    try {
      if (parsed.name !== undefined) manager.appendSessionInfo(parsed.name.trim());
      return { runtime, parsed, autoTrustOnReloadCwd: parsed.projectTrustOverride === undefined
        && !sdk.hasTrustRequiringProjectResources(launch.cwd) ? launch.cwd : undefined };
    } catch (error) { await runtime.dispose(); throw error; }
  } finally { if (!host.ui) initialUi.close(); }
}

/** SDK remains the sole runtime owner; await its extension drain on TUI failure. */
export async function runExactTui(sdk, runtime, parsed, autoTrustOnReloadCwd) {
  let ui;
  try {
    sdk.initTheme(runtime.services.settingsManager.getTheme(), true);
    const messages = [...parsed.messages];
    ui = new sdk.InteractiveMode(runtime, { modelFallbackMessage: runtime.modelFallbackMessage,
      autoTrustOnReloadCwd, initialMessage: messages.shift(), initialMessages: messages,
      verbose: parsed.verbose, tuiMode: parsed.tuiMode, initialThemeSetting: parsed.useTheme });
    await ui.run();
  } finally {
    try { ui?.stop(); } finally { await runtime.dispose(); }
  }
}

export async function main(argv) {
  const [manifestPath, ...args] = argv;
  const launch = JSON.parse(fs.readFileSync(manifestPath, 'utf8'));
  const packageJson = JSON.parse(fs.readFileSync(path.join(launch.packageRoot, 'package.json'), 'utf8'));
  if (packageJson.name !== '@earendil-works/pi-coding-agent' || packageJson.version !== '0.84.2')
    throw new Error('Unsupported Pi SDK installation for exact resume');
  validateArgs(args, launch);
  validateHistory(launch, 3);
  const sdk = await import(pathToFileURL(path.join(launch.packageRoot, 'dist/index.js')));
  const require = createRequire(path.join(launch.packageRoot, 'package.json'));
  const undici = await import(pathToFileURL(require.resolve('undici')));
  // Load Pi's bundled extension through the same supported extension-factory
  // input as user SDK integrations; no provider methods are intercepted.
  const builtin = await import(pathToFileURL(path.join(launch.packageRoot, 'dist/extensions/llama/index.js')));
  const configureHttp = async settings => {
    const proxy = settings.getGlobalSettings().httpProxy?.trim();
    if (proxy) { process.env.HTTP_PROXY ??= proxy; process.env.HTTPS_PROXY ??= proxy; }
    undici.setGlobalDispatcher(new undici.EnvHttpProxyAgent({ allowH2: false,
      bodyTimeout: settings.getHttpIdleTimeoutMs(), headersTimeout: settings.getHttpIdleTimeoutMs(),
      connect: { autoSelectFamilyAttemptTimeout: 2000 } }));
    undici.install?.();
  };
  const { runtime, parsed, autoTrustOnReloadCwd } = await createExactRuntime(sdk, launch, args,
    { extensionFactories: [{ name: 'llama.cpp', factory: builtin.default, hidden: true }], configureHttp });
  await runExactTui(sdk, runtime, parsed, autoTrustOnReloadCwd);
}

if (process.argv[1] && pathToFileURL(path.resolve(process.argv[1])).href === import.meta.url) {
  main(process.argv.slice(2)).catch(error => {
    console.error(`Pi exact resume failed: ${error.message}`);
    process.exit(1);
  });
}
