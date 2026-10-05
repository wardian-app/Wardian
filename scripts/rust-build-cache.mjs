import { spawnSync } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { existsSync, lstatSync, mkdirSync, readdirSync, readFileSync, realpathSync, renameSync, rmSync, writeFileSync } from 'node:fs';
import { homedir } from 'node:os';
import path from 'node:path';
import { pathToFileURL } from 'node:url';
import { assertCompilerAdmission, canonicalPath, readProtectedInputs } from './compiler-input-guard.mjs';

const MARKER = '.wardian-rust-target.json';
const SCHEMA = 1;
const PINNED_VERSION = '0.18.0';

function normalized(file) {
  const value = realpathSync.native(file);
  return process.platform === 'win32' ? value.replaceAll('\\', '/').toLowerCase() : value;
}

/** Stable identity shared by the launcher and managed-worktree configuration. */
export function cacheKey(file) {
  return createHash('sha256').update(normalized(file)).digest('hex').slice(0, 16);
}

function git(cwd, args) {
  const result = spawnSync('git', args, { cwd, encoding: 'utf8', windowsHide: true });
  if (result.error || result.status !== 0) throw new Error('Cannot resolve Git workspace for Rust cache');
  return result.stdout.trim();
}

/** Resolve central writable outputs without Cargo, rustc, metadata or filesystem writes. */
export function cacheLayout(cwd = process.cwd(), env = process.env, sourceRoot) {
  const workspace = realpathSync.native(cwd);
  const source = sourceRoot ?? env.WARDIAN_RUST_CACHE_SOURCE_ROOT ?? path.dirname(path.resolve(workspace, git(workspace, ['rev-parse', '--git-common-dir'])));
  const root = path.resolve(env.WARDIAN_RUST_CACHE_ROOT ?? `${realpathSync.native(source)}.cargo-cache`);
  const repoKey = cacheKey(source);
  const worktreeKey = cacheKey(workspace);
  return { root, source, workspace, repoKey, worktreeKey,
    target: path.join(root, 'launcher-targets', repoKey, worktreeKey),
    directTarget: path.join(root, 'direct-targets', repoKey, worktreeKey),
    claim: path.join(root, 'claims', repoKey, worktreeKey),
    store: path.join(root, 'sccache') };
}

function executable(name, env) {
  if (path.isAbsolute(name)) return existsSync(name) ? name : null;
  for (const directory of (env.PATH ?? env.Path ?? '').split(path.delimiter)) {
    if (!directory) continue;
    for (const suffix of process.platform === 'win32' ? ['', '.exe', '.cmd'] : ['']) {
      const candidate = path.join(directory, name + suffix);
      if (existsSync(candidate) && lstatSync(candidate).isFile()) return candidate;
    }
  }
  return null;
}

function configuredWrapper(cwd, env) {
  const directories = [env.CARGO_HOME ?? path.join(homedir(), '.cargo')];
  for (let current = cwd; ; current = path.dirname(current)) {
    directories.push(path.join(current, '.cargo'));
    if (path.dirname(current) === current) break;
  }
  return directories.some((directory) => {
    const file = ['config', 'config.toml'].map((name) => path.join(directory, name)).find(existsSync);
    return file && wrapperOrUncertainConfig(readFileSync(file, 'utf8'));
  });
}

// Prove absence only in a conservative, single-line TOML subset. Quoted/dotted
// keys are decoded; includes, inline tables and unrecognized syntax decline caching.
function wrapperOrUncertainConfig(text) {
  const part = String.raw`(?:[A-Za-z0-9_-]+|"(?:[^"\\]|\\.)*"|'[^']*')`;
  const key = `${part}(?:\\s*\\.\\s*${part})*`;
  for (const original of text.split(/\r?\n/)) {
    const line = original.trim();
    if (!line || line.startsWith('#')) continue;
    const header = line.match(new RegExp(`^\\[(${key})\\]\\s*(?:#.*)?$`));
    const entry = line.match(new RegExp(`^(${key})\\s*=\\s*(.*)$`));
    if (!header && !entry) return true;
    for (const token of (header ?? entry)[1].match(new RegExp(part, 'g'))) {
      let name = token;
      try {
        if (token.startsWith('"')) name = JSON.parse(token);
        else if (token.startsWith("'")) name = token.slice(1, -1);
      } catch { return true; }
      if (['rustc-wrapper', 'rustc-workspace-wrapper', 'include'].includes(name)) return true;
    }
    // Complex values can carry additional keys or span lines. Leave them to Cargo.
    if (entry && !/^(?:"(?:[^"\\]|\\.)*"|'[^']*'|true|false|[-+]?[\d._eE]+|\[[^\r\n{}]*\])\s*(?:#.*)?$/.test(entry[2])) return true;
  }
  return false;
}

/** Preserve explicit wrappers and incremental overrides; cache only the managed lane. */
export function cacheEnvironment(layout, input = process.env, lookup = executable) {
  const env = { ...input };
  const explicit = ['RUSTC_WRAPPER', 'CARGO_BUILD_RUSTC_WRAPPER', 'RUSTC_WORKSPACE_WRAPPER', 'CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER'];
  const custom = explicit.some((key) => Object.hasOwn(input, key)) || configuredWrapper(layout.workspace, input);
  const identityFile = input.WARDIAN_SCCACHE_IDENTITY_MANIFEST ?? path.join(layout.root, 'sccache-identity.json');
  const protectedLane = Object.hasOwn(input, 'WARDIAN_PROTECTED_INPUT_MANIFESTS');
  // sccache rejects the numeric opt-in; Cargo also accepts a boolean generic
  // carrier. Preserve either affirmative setting without injecting a wrapper.
  const incrementalOverride = ['CARGO_INCREMENTAL', 'CARGO_BUILD_INCREMENTAL'].some((key) => ['1', 'true'].includes(input[key]));
  const identity = !custom && !protectedLane && !incrementalOverride && !input.WARDIAN_SCCACHE && existsSync(identityFile) ? JSON.parse(readFileSync(identityFile, 'utf8')) : null;
  if (identity && (identity.schema !== 1 || identity.version !== PINNED_VERSION || !path.isAbsolute(identity.wrapper_path)
    || createHash('sha256').update(readFileSync(identity.wrapper_path)).digest('hex') !== identity.wrapper_sha256)) throw new Error('sccache identity changed; rerun explicit setup');
  const wrapper = !custom && input.WARDIAN_RUST_CACHE_DISABLE !== '1' && !protectedLane && !incrementalOverride
    ? lookup(identity?.wrapper_path ?? input.WARDIAN_SCCACHE ?? 'sccache', input) : null;
  if (wrapper) {
    env.RUSTC_WRAPPER = wrapper;
    env.SCCACHE_DIR = input.SCCACHE_DIR ?? identity?.cache_dir ?? layout.store;
    env.SCCACHE_CACHE_SIZE = input.SCCACHE_CACHE_SIZE ?? identity?.cache_size ?? '10G';
    // One endpoint for this managed store; never one server per worktree.
    env.SCCACHE_SERVER_PORT = input.SCCACHE_SERVER_PORT ?? identity?.server_port ?? '4227';
    if (identity) env.WARDIAN_SCCACHE_IDENTITY_MANIFEST = identityFile;
    if (input.CARGO_INCREMENTAL === undefined && input.CARGO_BUILD_INCREMENTAL === undefined) env.CARGO_INCREMENTAL = '0';
  }
  return { env, enabled: Boolean(wrapper), reason: wrapper ? 'sccache' : custom ? 'custom wrapper preserved or configuration uncertain'
    : protectedLane ? 'uncached: protected-input lane cannot qualify a shared sccache daemon'
    : incrementalOverride ? 'uncached: explicit incremental compilation' : 'uncached: sccache unavailable or disabled' };
}

function assertPlainTree(directory) {
  for (let current = path.resolve(directory); ; current = path.dirname(current)) {
    if (existsSync(current) || (() => { try { lstatSync(current); return true; } catch { return false; } })()) {
      if (lstatSync(current).isSymbolicLink()) throw new Error('Rust cache refuses linked output or claim directories');
      if (!lstatSync(current).isDirectory()) throw new Error('Rust cache path is not a directory');
    }
    if (path.dirname(current) === current) break;
  }
}

function overlaps(first, second) {
  const rel = path.relative(first, second);
  return rel === '' || (!path.isAbsolute(rel) && rel !== '..' && !rel.startsWith(`..${path.sep}`));
}

function assertUnprotected(target, env) {
  const actual = canonicalPath(target);
  for (const input of readProtectedInputs(env) ?? []) {
    if (overlaps(actual, input) || overlaps(input, actual)) throw new Error('Rust cache target overlaps a protected input');
  }
}

function marker(layout) {
  return { schema: SCHEMA, kind: 'compiler-writable', repo_key: layout.repoKey, worktree_key: layout.worktreeKey,
    routing: 'exclusive-launcher', target: path.resolve(layout.target), workspace: normalized(layout.workspace) };
}

function readMarker(target) {
  const file = path.join(target, MARKER);
  if (!existsSync(file) || lstatSync(file).isSymbolicLink()) throw new Error('Rust cache refuses unowned target');
  return JSON.parse(readFileSync(file, 'utf8'));
}

function assertMarker(layout) {
  const saved = readMarker(layout.target);
  if (JSON.stringify(saved) !== JSON.stringify(marker(layout))) throw new Error('Rust cache ownership marker mismatch');
}

/** Hold a per-target exclusive claim; crashed/foreign claims are never reclaimed automatically. */
export function claimTarget(layout, env = process.env) {
  assertPlainTree(layout.target);
  assertPlainTree(layout.claim);
  assertUnprotected(layout.target, env);
  mkdirSync(path.dirname(layout.claim), { recursive: true });
  try { mkdirSync(layout.claim); } catch (error) {
    if (error.code === 'EEXIST') throw new Error('Rust cache target is claimed; use inspect, never automatic stale-claim removal', { cause: error });
    throw error;
  }
  const token = randomUUID();
  try {
    writeFileSync(path.join(layout.claim, 'owner.json'), JSON.stringify({ token, pid: process.pid, started: new Date().toISOString() }));
    if (existsSync(layout.target)) {
      if (!existsSync(path.join(layout.target, MARKER)) && readdirSync(layout.target).length) throw new Error('Rust cache refuses existing unowned target');
    } else mkdirSync(layout.target, { recursive: true });
    if (!existsSync(path.join(layout.target, MARKER))) writeFileSync(path.join(layout.target, MARKER), JSON.stringify(marker(layout)));
    assertMarker(layout);
  } catch (error) {
    rmSync(layout.claim, { recursive: true });
    throw error;
  }
  return { token, release() {
    assertPlainTree(layout.claim);
    const owner = JSON.parse(readFileSync(path.join(layout.claim, 'owner.json'), 'utf8'));
    if (owner.token !== token) throw new Error('Rust cache claim changed; refusing release');
    rmSync(layout.claim, { recursive: true });
  } };
}

/** Execute a synchronous verification phase under one claim, also shared with nested scripts. */
export function withRustCache(run, { cwd = process.cwd(), env = process.env, sourceRoot, lookup } = {}) {
  if (env.WARDIAN_RUST_CACHE_CLAIM) {
    const owner = JSON.parse(readFileSync(path.join(env.WARDIAN_RUST_CACHE_CLAIM, 'owner.json'), 'utf8'));
    if (owner.token !== env.WARDIAN_RUST_CACHE_TOKEN) throw new Error('Invalid nested Rust cache claim');
    return run();
  }
  const layout = cacheLayout(cwd, env, sourceRoot);
  for (const key of ['CARGO_TARGET_DIR', 'CARGO_BUILD_TARGET_DIR', 'CARGO_BUILD_BUILD_DIR']) {
    if (env[key] && ![layout.target, layout.directTarget].some((target) => canonicalPath(env[key], cwd) === canonicalPath(target))) {
      throw new Error(`Rust cache preserves custom ${key}; use direct Cargo or align the managed target`);
    }
  }
  const lease = claimTarget(layout, env);
  const previous = { ...process.env };
  try {
    const selected = cacheEnvironment(layout, env, lookup);
    console.error(`Rust cache: enabled=${selected.enabled}; ${selected.reason}; target=${layout.target}`);
    const next = { ...selected.env, WARDIAN_RUST_CACHE_TARGET: layout.target,
      WARDIAN_RUST_CACHE_CLAIM: layout.claim, WARDIAN_RUST_CACHE_TOKEN: lease.token };
    // Unique CARGO_* routing values are hashed by sccache even for registry crates.
    delete next.CARGO_TARGET_DIR;
    delete next.CARGO_BUILD_TARGET_DIR;
    delete next.CARGO_BUILD_BUILD_DIR;
    for (const key of Object.keys(process.env)) if (!(key in next)) delete process.env[key];
    Object.assign(process.env, next);
    return run(layout, selected);
  } finally {
    for (const key of Object.keys(process.env)) if (!(key in previous)) delete process.env[key];
    Object.assign(process.env, previous);
    lease.release();
  }
}

// Cargo accepts these global flags before its command. Reject unknown forms and
// external subcommands instead of forwarding a command whose write effects are unknown.
function cargoCommand(args) {
  let index = args[0]?.startsWith('+') ? 1 : 0;
  for (; index < args.length; index += 1) {
    const argument = args[index];
    if (['--offline', '--locked', '--frozen', '--verbose', '--quiet', '-q'].includes(argument) || /^-v+$/.test(argument)) continue;
    if (argument === '--config' || argument === '--color') {
      if (!args[index + 1] || args[index + 1].startsWith('-')) throw new Error(`Missing Cargo global option value: ${argument}`);
      index += 1;
      continue;
    }
    if (/^--(?:config|color)=.+$/.test(argument)) continue;
    if (!['build', 'check', 'test', 'clippy', 'doc', 'metadata', 'fmt'].includes(argument)) {
      throw new Error(`Unsupported Cargo command or global option: ${argument}`);
    }
    return argument;
  }
  throw new Error('Missing supported Cargo command');
}

/** Route Cargo through arguments so registry dependencies keep reusable environment keys. */
export function cargoInvocation(args, { cwd = process.cwd(), env = process.env } = {}) {
  const target = env.WARDIAN_RUST_CACHE_TARGET;
  const forwarded = [...args];
  const subcommand = cargoCommand(args);
  const routable = subcommand !== 'fmt';
  let explicitTarget = false;
  for (let index = 0; target && index < args.length && args[index] !== '--'; index += 1) {
    if (args[index] === '--target-dir' || args[index].startsWith('--target-dir=')) {
      explicitTarget = true;
      const value = args[index] === '--target-dir' ? args[++index] : args[index].slice('--target-dir='.length);
      if (!value || canonicalPath(value, cwd) !== canonicalPath(target)) throw new Error('Launcher target override escapes its exclusive claim');
    }
  }
  if (target && routable && subcommand !== 'metadata' && !explicitTarget) {
    const separator = forwarded.indexOf('--');
    forwarded.splice(separator < 0 ? forwarded.length : separator, 0, '--target-dir', target);
  }
  // Keep build-dir equal to target-dir, including when an inherited config separates them.
  // Last CLI configuration wins; keep rustup's +toolchain first and test args after --.
  const separator = forwarded.indexOf('--');
  const configOffset = separator < 0 ? forwarded.length : separator;
  if (target && subcommand === 'metadata') forwarded.splice(configOffset, 0, '--config', `build.target-dir=${JSON.stringify(target)}`);
  if (target && routable) forwarded.splice(configOffset, 0, '--config', `build.build-dir=${JSON.stringify(target)}`);
  assertCompilerAdmission({ program: 'cargo', args: forwarded, cwd, env });
  return { program: 'cargo', args: forwarded, env, cwd };
}

function sizeOf(directory) {
  let bytes = 0;
  let modified = 0;
  for (const entry of readdirSync(directory, { withFileTypes: true })) {
    const file = path.join(directory, entry.name);
    const stat = lstatSync(file);
    if (stat.isSymbolicLink()) throw new Error('Rust cache target contains a link; pruning refused');
    modified = Math.max(modified, stat.mtimeMs);
    if (stat.isDirectory()) {
      const nested = sizeOf(file);
      bytes += nested.bytes;
      modified = Math.max(modified, nested.modified);
    } else bytes += stat.size;
  }
  return { bytes, modified };
}

/** Inventory only positively owned targets. Claims include both active and unreconciled crashes. */
export function inspectTargets(layout, env = process.env) {
  const base = path.join(layout.root, 'launcher-targets', layout.repoKey);
  assertPlainTree(base);
  if (!existsSync(base)) return [];
  return readdirSync(base).map((key) => {
    const target = path.join(base, key);
    try {
      if (!/^[0-9a-f]{16}$/.test(key)) throw new Error('unowned name');
      assertPlainTree(target);
      const record = readMarker(target);
      if (record.schema !== SCHEMA || record.kind !== 'compiler-writable' || record.routing !== 'exclusive-launcher' || record.repo_key !== layout.repoKey
        || record.worktree_key !== key || record.target !== target) throw new Error('unowned marker');
      assertUnprotected(target, env);
      return { key, target, claim: path.join(layout.root, 'claims', layout.repoKey, key),
        claimed: existsSync(path.join(layout.root, 'claims', layout.repoKey, key)), ...sizeOf(target) };
    } catch (error) { return { key, target, refused: error.message }; }
  });
}

/** Explicit operator retention; active/protected/unowned targets never enter the deletion set. */
export function pruneTargets(layout, { keep = 2, maxBytes = 10 * 1024 ** 3, env = process.env } = {}) {
  if (!Number.isSafeInteger(keep) || keep < 0 || !Number.isSafeInteger(maxBytes) || maxBytes < 0) throw new Error('Invalid retention limit');
  const entries = inspectTargets(layout, env);
  const inactive = entries.filter((entry) => !entry.refused && !entry.claimed).sort((a, b) => b.modified - a.modified);
  let kept = 0;
  let bytes = 0;
  const removed = [];
  for (const entry of inactive) {
    if (kept < keep && bytes + entry.bytes <= maxBytes) { kept += 1; bytes += entry.bytes; continue; }
    assertPlainTree(entry.claim);
    mkdirSync(path.dirname(entry.claim), { recursive: true });
    try { mkdirSync(entry.claim); } catch (error) { if (error.code === 'EEXIST') continue; throw error; }
    try {
      // Revalidate after exclusive acquisition, including new links/protection/markers.
      const fresh = inspectTargets(layout, env).find((candidate) => candidate.key === entry.key);
      if (!fresh || fresh.refused) throw new Error('Target changed during prune; refused');
      assertPlainTree(entry.target);
      const quarantine = path.join(layout.root, 'pruning', `${entry.key}-${randomUUID()}`);
      assertPlainTree(path.dirname(quarantine));
      mkdirSync(path.dirname(quarantine), { recursive: true });
      // This namespace is exclusively launcher-routed; direct Cargo uses direct-targets/.
      // Remove only the renamed generation, never a new target created by another command.
      renameSync(entry.target, quarantine);
      rmSync(quarantine, { recursive: true });
      removed.push(entry.key);
    } finally { rmSync(entry.claim, { recursive: true }); }
  }
  return { removed, retained_inactive_bytes: bytes, skipped: entries.filter((entry) => entry.claimed || entry.refused) };
}

/** Public launcher dispatch; Cargo commands are validated before claims or spawning. */
export function main(argv = process.argv.slice(2), options = {}) {
  const [command, ...args] = argv;
  if (command === 'inspect' || command === 'prune' || command === 'setup') {
    if (command !== 'prune' && args.length) throw new Error('Use npm run rust:cache:setup -- --install for installation; setup/inspect accept no extra arguments');
    const layout = cacheLayout();
    if (command === 'inspect') {
      const selected = cacheEnvironment(layout);
      const directRoot = path.join(layout.root, 'direct-targets', layout.repoKey);
      assertPlainTree(directRoot);
      const directTargets = existsSync(directRoot) ? readdirSync(directRoot).map((key) => {
        const target = path.join(directRoot, key);
        try { assertPlainTree(target); assertUnprotected(target, process.env); return { target, prunable: false, ...sizeOf(target) }; }
        catch (error) { return { target, prunable: false, refused: error.message }; }
      }) : [];
      console.log(JSON.stringify({ layout, cache: { enabled: selected.enabled, reason: selected.reason,
        store: selected.env.SCCACHE_DIR ?? layout.store, size_limit: selected.env.SCCACHE_CACHE_SIZE ?? '10G' },
        targets: inspectTargets(layout), direct_targets: directTargets }, null, 2));
    }
    else if (command === 'prune') {
      const options = { keep: 2, maxBytes: 10 * 1024 ** 3 };
      for (let index = 0; index < args.length; index += 2) {
        if (args[index] === '--keep') options.keep = Number(args[index + 1]);
        else if (args[index] === '--max-bytes') options.maxBytes = Number(args[index + 1]);
        else throw new Error('prune supports --keep and --max-bytes');
      }
      console.log(JSON.stringify(pruneTargets(layout, options), null, 2));
    } else {
      const selected = cacheEnvironment(layout);
      if (!selected.enabled) throw new Error(`sccache unavailable (${selected.reason}); see docs/developer/rust-build-cache.md for pinned dependency setup`);
      const version = spawnSync(selected.env.RUSTC_WRAPPER, ['--version'], { encoding: 'utf8', windowsHide: true });
      if (version.error || version.status !== 0 || version.stdout.trim() !== `sccache ${PINNED_VERSION}`) throw new Error(`Install official sccache ${PINNED_VERSION}; setup does not install or start servers`);
      const identity = { schema: 1, version: PINNED_VERSION, wrapper_path: realpathSync.native(selected.env.RUSTC_WRAPPER),
        wrapper_sha256: createHash('sha256').update(readFileSync(selected.env.RUSTC_WRAPPER)).digest('hex'),
        cache_dir: path.resolve(selected.env.SCCACHE_DIR), cache_size: selected.env.SCCACHE_CACHE_SIZE,
        server_port: selected.env.SCCACHE_SERVER_PORT };
      assertPlainTree(layout.root);
      assertUnprotected(layout.root, process.env);
      mkdirSync(layout.root, { recursive: true });
      writeFileSync(path.join(layout.root, 'sccache-identity.json'), JSON.stringify(identity, null, 2));
      console.log(JSON.stringify({ ...identity, manifest: path.join(layout.root, 'sccache-identity.json'), target: layout.target }, null, 2));
    }
    return 0;
  }
  if (command !== 'cargo' || args.length === 0) throw new Error('Use rust:cache setup|inspect|prune or cargo <arguments>');
  cargoCommand(args);
  const separator = args.indexOf('--');
  const cargoArgs = separator < 0 ? args : args.slice(0, separator);
  // Caller config can name a wrapper or include another file. Leave its precedence
  // intact rather than injecting an environment wrapper over uninspected CLI config.
  const env = options.env ?? process.env;
  const lane = cargoArgs.some((arg) => arg === '--config' || arg.startsWith('--config='))
    ? { ...options, env: { ...env, WARDIAN_RUST_CACHE_DISABLE: '1' } } : options;
  return withRustCache(() => {
    const invocation = cargoInvocation(args);
    const result = (options.spawn ?? spawnSync)(invocation.program, invocation.args, { cwd: invocation.cwd, env: invocation.env,
      stdio: 'inherit', windowsHide: true });
    if (result.error) throw result.error;
    return result.status ?? 1;
  }, lane);
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  try { process.exitCode = main(); } catch (error) { console.error(error.message); process.exitCode = 1; }
}
