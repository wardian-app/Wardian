import { spawnSync } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { existsSync, lstatSync, mkdirSync, readdirSync, readFileSync, realpathSync, renameSync, rmSync, rmdirSync, unlinkSync, writeFileSync } from 'node:fs';
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
    recoveryBase: path.join(root, 'claim-recovery', repoKey, worktreeKey),
    recoveryReservation: path.join(root, 'claim-recovery', repoKey, worktreeKey, '.active'),
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

function assertUnprotected(target, env, kind = 'target') {
  const actual = canonicalPath(target);
  for (const input of readProtectedInputs(env) ?? []) {
    if (overlaps(actual, input) || overlaps(input, actual)) throw new Error(`Rust cache ${kind} overlaps a protected input`);
  }
}

function assertPlainFile(file) {
  assertPlainTree(path.dirname(file));
  const stat = lstatSync(file);
  if (stat.isSymbolicLink() || !stat.isFile() || stat.nlink !== 1) {
    throw new Error('Rust cache refuses linked, multiply-linked or non-file metadata');
  }
  return readFileSync(file);
}

function sha256(bytes) {
  return createHash('sha256').update(bytes).digest('hex');
}

function assertNoRecoveryReservation(layout) {
  assertPlainTree(layout.recoveryReservation);
  if (existsSync(layout.recoveryReservation)) throw new Error('Rust cache recovery is reserved; ordinary claim acquisition is blocked');
}

function parseOwner(bytes) {
  let owner;
  try { owner = JSON.parse(bytes.toString('utf8')); } catch { throw new Error('Rust cache claim owner metadata is unreadable'); }
  const keys = owner && typeof owner === 'object' && !Array.isArray(owner) ? Object.keys(owner).sort() : [];
  if (keys.join(',') !== 'pid,started,token' || typeof owner.token !== 'string' || !owner.token
    || !Number.isSafeInteger(owner.pid) || owner.pid < 1 || typeof owner.started !== 'string' || !Number.isFinite(Date.parse(owner.started))) {
    throw new Error('Rust cache claim owner format is unknown');
  }
  if (!Buffer.from(JSON.stringify(owner)).equals(bytes)) throw new Error('Rust cache claim owner encoding is unknown');
  return owner;
}

function readClaim(layout) {
  assertPlainTree(layout.claim);
  if (!existsSync(layout.claim)) throw new Error('Rust cache claim is absent');
  const entries = readdirSync(layout.claim);
  if (entries.length !== 1 || entries[0] !== 'owner.json') throw new Error('Rust cache claim contains unexpected metadata');
  const bytes = assertPlainFile(path.join(layout.claim, 'owner.json'));
  return { bytes, owner: parseOwner(bytes), sha256: sha256(bytes) };
}

function readOwnedMarker(layout) {
  assertPlainTree(layout.target);
  const bytes = assertPlainFile(path.join(layout.target, MARKER));
  let saved;
  try { saved = JSON.parse(bytes.toString('utf8')); } catch { throw new Error('Rust cache target marker is unreadable'); }
  if (!Buffer.from(JSON.stringify(saved)).equals(bytes) || JSON.stringify(saved) !== JSON.stringify(marker(layout))) {
    throw new Error('Rust cache ownership marker is unknown or changed');
  }
  return { bytes, sha256: sha256(bytes) };
}

function validIdentity(identity, allowCapturedHandle) {
  if (!identity || typeof identity !== 'object' || Array.isArray(identity)) return false;
  if (identity.basis === 'os-birth') return typeof identity.birth_id === 'string' && identity.birth_id.length > 0;
  return allowCapturedHandle && identity.basis === 'captured-process-handle';
}

function validateClosureReceipt(receipt, layout, owner, ownerHash, markerHash) {
  if (!receipt || typeof receipt !== 'object' || Array.isArray(receipt) || receipt.schema !== 1
    || receipt.kind !== 'rust-cache-ended-claim-recovery' || receipt.repo_key !== layout.repoKey
    || receipt.worktree_key !== layout.worktreeKey || receipt.owner_sha256 !== ownerHash
    || receipt.marker_sha256 !== markerHash || receipt.disposition !== 'ended'
    || typeof receipt.evidence_producer !== 'string' || !receipt.evidence_producer.trim()
    || typeof receipt.observed_at !== 'string' || !/^\d{4}-\d\d-\d\dT.+(?:Z|[+-]\d\d:\d\d)$/.test(receipt.observed_at)
    || !Number.isFinite(Date.parse(receipt.observed_at))) {
    throw new Error('Closure receipt does not bind the selected ended claim generation');
  }
  if (!receipt.owner || Object.keys(receipt.owner).sort().join(',') !== 'pid,started,token'
    || receipt.owner.token !== owner.token || receipt.owner.pid !== owner.pid || receipt.owner.started !== owner.started) {
    throw new Error('Closure receipt owner identity does not match the claim');
  }
  const closure = receipt.closure;
  const root = closure?.root;
  const streamsClosed = (streams) => streams && streams.stdin === 'closed' && streams.stdout === 'eof' && streams.stderr === 'eof';
  if (!closure || !['owned-job-zero', 'owned-process-handles-joined'].includes(closure.basis)
    || closure.complete !== true || !Array.isArray(closure.descendants)
    || !root || root.pid !== owner.pid || root.joined !== true || !streamsClosed(root.streams)
    || !validIdentity(root.identity, true)) {
    throw new Error('Closure receipt lacks complete joined-process and stream-closure evidence');
  }
  const pids = new Set([root.pid]);
  for (const process of closure.descendants) {
    if (!process || !Number.isSafeInteger(process.pid) || process.pid < 1 || pids.has(process.pid)
      || process.joined !== true || !streamsClosed(process.streams) || !validIdentity(process.identity, true)) {
      throw new Error('Closure receipt contains an unknown or unjoined descendant');
    }
    pids.add(process.pid);
  }
  if (closure.basis === 'owned-job-zero') {
    if (!closure.job || Object.keys(closure.job).sort().join(',') !== 'active_process_count,observed'
      || closure.job.observed !== true || closure.job.active_process_count !== 0
      || root.identity.basis !== 'os-birth' || closure.descendants.some((process) => process.identity.basis !== 'os-birth')) {
      throw new Error('Closure receipt lacks an observed zero-process owned Job');
    }
  } else if (Object.hasOwn(closure, 'job')) {
    throw new Error('Closure receipt claims Job evidence under a process-handle basis');
  }
}

function marker(layout) {
  return { schema: SCHEMA, kind: 'compiler-writable', repo_key: layout.repoKey, worktree_key: layout.worktreeKey,
    routing: 'exclusive-launcher', target: path.resolve(layout.target), workspace: normalized(layout.workspace) };
}

function readMarkerFile(target) {
  const file = path.join(target, MARKER);
  if (!existsSync(file)) throw new Error('Rust cache refuses unowned target');
  const bytes = assertPlainFile(file);
  try { return { bytes, record: JSON.parse(bytes.toString('utf8')) }; }
  catch { throw new Error('Rust cache target marker is unreadable'); }
}

function readMarker(target) {
  return readMarkerFile(target).record;
}

function assertMarker(layout) {
  const saved = readMarker(layout.target);
  if (JSON.stringify(saved) !== JSON.stringify(marker(layout))) throw new Error('Rust cache ownership marker mismatch');
}

/**
 * Hold a per-target exclusive claim; crashed/foreign claims are never reclaimed
 * automatically. Both the target and its sibling claim must be unprotected;
 * cleanup and release recheck manifests that another owner may have updated.
 */
export function claimTarget(layout, env = process.env) {
  assertPlainTree(layout.target);
  assertPlainTree(layout.claim);
  assertPlainTree(layout.recoveryBase);
  assertUnprotected(layout.target, env);
  // Claims are siblings of compiler output, so the target guard cannot protect them.
  assertUnprotected(layout.claim, env, 'claim');
  assertNoRecoveryReservation(layout);
  mkdirSync(path.dirname(layout.claim), { recursive: true });
  try { mkdirSync(layout.claim); } catch (error) {
    if (error.code === 'EEXIST') throw new Error('Rust cache target is claimed; use inspect, never automatic stale-claim removal', { cause: error });
    throw error;
  }
  // A launcher that passed the first gate check may have paused while recovery
  // moved the old claim. It owns only this empty directory until this recheck.
  try {
    assertNoRecoveryReservation(layout);
  } catch (error) { rmdirSync(layout.claim); throw error; }
  const token = randomUUID();
  try {
    writeFileSync(path.join(layout.claim, 'owner.json'), JSON.stringify({ token, pid: process.pid, started: new Date().toISOString() }));
    if (existsSync(layout.target)) {
      if (!existsSync(path.join(layout.target, MARKER)) && readdirSync(layout.target).length) throw new Error('Rust cache refuses existing unowned target');
    } else mkdirSync(layout.target, { recursive: true });
    if (!existsSync(path.join(layout.target, MARKER))) writeFileSync(path.join(layout.target, MARKER), JSON.stringify(marker(layout)));
    assertMarker(layout);
  } catch (error) {
    assertUnprotected(layout.claim, env, 'claim');
    rmSync(layout.claim, { recursive: true });
    throw error;
  }
  return { token, release() {
    assertPlainTree(layout.claim);
    const owner = JSON.parse(readFileSync(path.join(layout.claim, 'owner.json'), 'utf8'));
    if (owner.token !== token) throw new Error('Rust cache claim changed; refusing release');
    assertUnprotected(layout.claim, env, 'claim');
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
      const { bytes: markerBytes, record } = readMarkerFile(target);
      if (record.schema !== SCHEMA || record.kind !== 'compiler-writable' || record.routing !== 'exclusive-launcher' || record.repo_key !== layout.repoKey
        || record.worktree_key !== key || record.target !== target || typeof record.workspace !== 'string') throw new Error('unowned marker');
      assertUnprotected(target, env);
      const claim = path.join(layout.root, 'claims', layout.repoKey, key);
      const selected = { ...layout, worktreeKey: key, claim };
      let claimInfo = null;
      if (existsSync(claim)) {
        assertPlainTree(claim);
        claimInfo = readdirSync(claim).length === 0 ? { empty: true } : readClaim(selected);
      }
      const recoveryReservation = path.join(layout.root, 'claim-recovery', layout.repoKey, key, '.active');
      assertPlainTree(recoveryReservation);
      return { key, target, claim, claimed: Boolean(claimInfo), owner: claimInfo?.owner ?? null,
        owner_sha256: claimInfo?.sha256 ?? null, marker_sha256: sha256(markerBytes),
        recovery_reserved: existsSync(recoveryReservation), ...sizeOf(target) };
    } catch (error) { return { key, target, refused: error.message }; }
  });
}

function readClosureReceipt(file) {
  const bytes = assertPlainFile(path.resolve(file));
  if (bytes.length > 1024 * 1024) throw new Error('Closure receipt exceeds the 1 MiB limit');
  let receipt;
  try { receipt = JSON.parse(bytes.toString('utf8')); } catch { throw new Error('Closure receipt is not valid JSON'); }
  return { bytes, receipt, sha256: sha256(bytes) };
}

function recoveryArchive(layout, ownerHash) {
  return path.join(layout.recoveryBase, ownerHash);
}

function readRecoveryOutcome(archive, receiptHash, ownerHash, markerHash, reservationExists) {
  assertPlainTree(archive);
  const receiptPath = path.join(archive, 'receipt.json');
  const claimPath = path.join(archive, 'claim');
  const outcomePath = path.join(archive, 'outcome.json');
  if (!existsSync(receiptPath) || !existsSync(claimPath) || !existsSync(outcomePath)) {
    throw new Error('A prior recovery attempt is incomplete; preserving its evidence and refusing another mutation');
  }
  const entries = readdirSync(archive).sort();
  if (entries.join(',') !== 'claim,outcome.json,receipt.json') throw new Error('Prior recovery archive contains unexpected metadata');
  const savedReceipt = assertPlainFile(receiptPath);
  if (sha256(savedReceipt) !== receiptHash) throw new Error('A prior recovery archive is bound to different closure evidence');
  assertPlainTree(claimPath);
  const archivedClaim = readClaim({ claim: claimPath });
  if (archivedClaim.sha256 !== ownerHash) throw new Error('A prior recovery archive contains different owner metadata');
  const savedOutcomeBytes = assertPlainFile(outcomePath);
  let archivedReceipt;
  let outcome;
  try {
    archivedReceipt = JSON.parse(savedReceipt.toString('utf8'));
    outcome = JSON.parse(savedOutcomeBytes.toString('utf8'));
  } catch { throw new Error('Prior recovery receipt or outcome is unreadable; preserving its evidence'); }
  const closureBasis = archivedReceipt?.closure?.basis;
  if (!['owned-job-zero', 'owned-process-handles-joined'].includes(closureBasis)
    || outcome.closure_basis !== closureBasis) {
    throw new Error('Prior recovery outcome does not match the archived closure basis');
  }
  if (outcome.schema !== 1 || outcome.status !== 'recovered' || outcome.owner_sha256 !== ownerHash
    || outcome.marker_sha256 !== markerHash || outcome.receipt_sha256 !== receiptHash) {
    throw new Error('Prior recovery outcome does not match the selected generation');
  }
  return { ...outcome, status: reservationExists ? 'partial' : 'already_recovered', archive, reservation_retained: reservationExists };
}

function createRecoveryReservation(layout, reservation, env) {
  assertPlainTree(layout.recoveryBase);
  assertUnprotected(layout.recoveryBase, env, 'recovery quarantine');
  assertUnprotected(layout.recoveryReservation, env, 'recovery reservation');
  mkdirSync(layout.recoveryBase, { recursive: true });
  assertPlainTree(layout.recoveryBase);
  const token = randomUUID();
  try { mkdirSync(layout.recoveryReservation); }
  catch (error) {
    if (error.code === 'EEXIST') throw new Error('Another recovery transition already owns this worktree reservation', { cause: error });
    throw error;
  }
  try {
    writeFileSync(path.join(layout.recoveryReservation, 'reservation.json'), JSON.stringify({ schema: 1, token, ...reservation }), { flag: 'wx' });
  } catch (error) {
    try { rmdirSync(layout.recoveryReservation); } catch { /* retain an uncertain reservation fail-closed */ }
    throw error;
  }
  return token;
}

function assertRecoveryReservationOwner(layout, token, ownerHash, markerHash, receiptHash) {
  assertPlainTree(layout.recoveryReservation);
  const entries = readdirSync(layout.recoveryReservation);
  if (entries.length !== 1 || entries[0] !== 'reservation.json') throw new Error('Recovery reservation contains unexpected metadata');
  const bytes = assertPlainFile(path.join(layout.recoveryReservation, 'reservation.json'));
  let record;
  try { record = JSON.parse(bytes.toString('utf8')); } catch { throw new Error('Recovery reservation metadata is unreadable'); }
  const keys = record && typeof record === 'object' && !Array.isArray(record) ? Object.keys(record).sort() : [];
  if (keys.join(',') !== 'marker_sha256,owner_sha256,phase,receipt_sha256,schema,token'
    || record.schema !== 1 || record.token !== token || record.owner_sha256 !== ownerHash
    || record.marker_sha256 !== markerHash || record.receipt_sha256 !== receiptHash || record.phase !== 'reserved') {
    throw new Error('Recovery reservation ownership changed; preserving it');
  }
}

function releaseRecoveryReservation(layout, token, ownerHash, markerHash, receiptHash, env) {
  if (!existsSync(layout.recoveryReservation)) throw new Error('Recovery reservation disappeared before release');
  const file = path.join(layout.recoveryReservation, 'reservation.json');
  const bytes = assertPlainFile(file);
  try { JSON.parse(bytes.toString('utf8')); } catch { throw new Error('Recovery reservation metadata is unreadable'); }
  assertRecoveryReservationOwner(layout, token, ownerHash, markerHash, receiptHash);
  assertUnprotected(layout.recoveryReservation, env, 'recovery reservation');
  unlinkSync(file);
  rmdirSync(layout.recoveryReservation);
}

function validateRecoveryCandidate(layout, ownerHash, markerHash, receipt) {
  assertPlainTree(layout.target);
  assertPlainTree(layout.claim);
  assertPlainTree(layout.recoveryBase);
  assertUnprotected(layout.target, receipt.env, 'target');
  assertUnprotected(layout.claim, receipt.env, 'claim');
  assertUnprotected(layout.recoveryBase, receipt.env, 'recovery quarantine');
  const claim = readClaim(layout);
  if (claim.sha256 !== ownerHash) throw new Error('Rust cache claim owner bytes changed');
  const markerFile = readOwnedMarker(layout);
  if (markerFile.sha256 !== markerHash) throw new Error('Rust cache target marker bytes changed');
  validateClosureReceipt(receipt.value, layout, claim.owner, ownerHash, markerHash);
  return { claim, marker: markerFile };
}

/** Explicitly archive one caller-qualified ended claim; never touches compiler outputs. */
export function recoverEndedClaim(layout, { ownerHash, markerHash, receiptPath, env = process.env, hooks = {} } = {}) {
  if (!/^[0-9a-f]{64}$/.test(ownerHash ?? '') || !/^[0-9a-f]{64}$/.test(markerHash ?? '') || typeof receiptPath !== 'string' || !receiptPath) {
    throw new Error('recover-ended-claim requires exact owner/marker SHA-256 values and a closure receipt');
  }
  const receipt = readClosureReceipt(path.resolve(layout.workspace, receiptPath));
  const archive = recoveryArchive(layout, ownerHash);
  assertPlainTree(layout.recoveryReservation);
  const reservationExists = existsSync(layout.recoveryReservation);
  if (existsSync(archive)) return readRecoveryOutcome(archive, receipt.sha256, ownerHash, markerHash, reservationExists);
  assertPlainTree(layout.recoveryReservation);
  if (reservationExists) throw new Error('A recovery transition is already reserved; refusing automatic retry');

  const candidate = validateRecoveryCandidate(layout, ownerHash, markerHash, { value: receipt.receipt, env });
  const token = createRecoveryReservation(layout, { owner_sha256: ownerHash, marker_sha256: markerHash,
    receipt_sha256: receipt.sha256, phase: 'reserved' }, env);
  let moved = false;
  try {
    hooks.afterReservation?.();
    assertRecoveryReservationOwner(layout, token, ownerHash, markerHash, receipt.sha256);
    validateRecoveryCandidate(layout, ownerHash, markerHash, { value: receipt.receipt, env });
    assertUnprotected(archive, env, 'recovery quarantine');
    mkdirSync(archive);
    writeFileSync(path.join(archive, 'receipt.json'), receipt.bytes, { flag: 'wx' });

    hooks.beforeMove?.();
    const currentReceipt = readClosureReceipt(path.resolve(layout.workspace, receiptPath));
    if (currentReceipt.sha256 !== receipt.sha256) throw new Error('Closure receipt bytes changed before recovery');
    assertPlainTree(archive);
    const archiveEntries = readdirSync(archive);
    if (archiveEntries.length !== 1 || archiveEntries[0] !== 'receipt.json'
      || existsSync(path.join(archive, 'claim')) || existsSync(path.join(archive, 'outcome.json'))) {
      throw new Error('Recovery quarantine destination is not empty except for its receipt');
    }
    if (!assertPlainFile(path.join(archive, 'receipt.json')).equals(receipt.bytes)) {
      throw new Error('Recovery receipt changed in the quarantine destination');
    }
    assertUnprotected(layout.claim, env, 'claim');
    assertUnprotected(archive, env, 'recovery quarantine');
    assertRecoveryReservationOwner(layout, token, ownerHash, markerHash, receipt.sha256);
    const finalCandidate = validateRecoveryCandidate(layout, ownerHash, markerHash, { value: currentReceipt.receipt, env });
    if (!finalCandidate.claim.bytes.equals(candidate.claim.bytes) || !finalCandidate.marker.bytes.equals(candidate.marker.bytes)) {
      throw new Error('Selected claim generation changed before atomic move');
    }
    (hooks.renameClaim ?? renameSync)(layout.claim, path.join(archive, 'claim'));
    moved = true;

    hooks.afterMove?.();
    assertRecoveryReservationOwner(layout, token, ownerHash, markerHash, receipt.sha256);
    const archived = readClaim({ claim: path.join(archive, 'claim') });
    if (!archived.bytes.equals(candidate.claim.bytes)) throw new Error('Archived claim owner bytes changed after move');
    const currentMarker = readOwnedMarker(layout);
    if (!currentMarker.bytes.equals(candidate.marker.bytes)) throw new Error('Target marker changed during recovery; compiler outputs were left untouched');
    const outcome = { schema: 1, status: 'recovered', repo_key: layout.repoKey, worktree_key: layout.worktreeKey,
      owner_sha256: ownerHash, marker_sha256: markerHash, receipt_sha256: receipt.sha256,
      closure_basis: currentReceipt.receipt.closure.basis, archived_at: new Date().toISOString() };
    (hooks.writeOutcome ?? ((file, bytes) => writeFileSync(file, bytes, { flag: 'wx' })))(
      path.join(archive, 'outcome.json'), JSON.stringify(outcome),
    );
    releaseRecoveryReservation(layout, token, ownerHash, markerHash, receipt.sha256, env);
    return { ...outcome, archive, reservation_retained: false };
  } catch (error) {
    if (moved) {
      throw new Error(`Recovery partially completed; archived metadata and reservation are retained: ${error.message}`, { cause: error });
    }
    // Before the atomic move, this operation has not removed the launch exclusion.
    // Keep the reservation only if a claim move may already have happened.
    try {
      if (!existsSync(path.join(archive, 'claim'))) releaseRecoveryReservation(layout, token, ownerHash, markerHash, receipt.sha256, env);
    } catch { /* fail closed if ownership/protection is uncertain */ }
    throw error;
  }
}

/** Explicit operator retention; active/protected/unowned targets never enter the deletion set. */
export function pruneTargets(layout, { keep = 2, maxBytes = 10 * 1024 ** 3, env = process.env } = {}) {
  if (!Number.isSafeInteger(keep) || keep < 0 || !Number.isSafeInteger(maxBytes) || maxBytes < 0) throw new Error('Invalid retention limit');
  assertNoRecoveryReservation(layout);
  const entries = inspectTargets(layout, env);
  const inactive = entries.filter((entry) => !entry.refused && !entry.claimed && !entry.recovery_reserved).sort((a, b) => b.modified - a.modified);
  let kept = 0;
  let bytes = 0;
  const removed = [];
  for (const entry of inactive) {
    if (kept < keep && bytes + entry.bytes <= maxBytes) { kept += 1; bytes += entry.bytes; continue; }
    const selectedLayout = { ...layout, worktreeKey: entry.key, target: entry.target, claim: entry.claim,
      recoveryBase: path.join(layout.root, 'claim-recovery', layout.repoKey, entry.key),
      recoveryReservation: path.join(layout.root, 'claim-recovery', layout.repoKey, entry.key, '.active') };
    assertPlainTree(entry.claim);
    mkdirSync(path.dirname(entry.claim), { recursive: true });
    try { mkdirSync(entry.claim); } catch (error) { if (error.code === 'EEXIST') continue; throw error; }
    try {
      assertNoRecoveryReservation(selectedLayout);
      // Revalidate after exclusive acquisition, including new links/protection/markers.
      const fresh = inspectTargets(layout, env).find((candidate) => candidate.key === entry.key);
      if (!fresh || fresh.refused || fresh.recovery_reserved) throw new Error('Target changed during prune; refused');
      assertPlainTree(entry.target);
      const quarantine = path.join(layout.root, 'pruning', `${entry.key}-${randomUUID()}`);
      assertPlainTree(path.dirname(quarantine));
      assertUnprotected(path.dirname(quarantine), env, 'prune quarantine');
      mkdirSync(path.dirname(quarantine), { recursive: true });
      // This namespace is exclusively launcher-routed; direct Cargo uses direct-targets/.
      // Remove only the renamed generation, never a new target created by another command.
      assertNoRecoveryReservation(selectedLayout);
      renameSync(entry.target, quarantine);
      rmSync(quarantine, { recursive: true });
      removed.push(entry.key);
    } finally { rmdirSync(entry.claim); }
  }
  return { removed, retained_inactive_bytes: bytes, skipped: entries.filter((entry) => entry.claimed || entry.recovery_reserved || entry.refused) };
}

/** Public launcher dispatch; Cargo commands are validated before claims or spawning. */
export function main(argv = process.argv.slice(2), options = {}) {
  const [command, ...args] = argv;
  const env = options.env ?? process.env;
  const cwd = options.cwd ?? process.cwd();
  if (command === 'recover-ended-claim') {
    const values = {};
    const keys = new Map([['--owner-sha256', 'ownerHash'], ['--marker-sha256', 'markerHash'], ['--closure-receipt', 'receiptPath']]);
    for (let index = 0; index < args.length; index += 2) {
      const key = keys.get(args[index]);
      if (!key || Object.hasOwn(values, key) || !args[index + 1] || args[index + 1].startsWith('--')) {
        throw new Error('recover-ended-claim requires exactly --owner-sha256, --marker-sha256 and --closure-receipt');
      }
      values[key] = args[index + 1];
    }
    if (Object.keys(values).length !== keys.size) throw new Error('recover-ended-claim requires exactly --owner-sha256, --marker-sha256 and --closure-receipt');
    const layout = cacheLayout(cwd, env, options.sourceRoot);
    const outcome = recoverEndedClaim(layout, { ...values, env, hooks: options.recoveryHooks });
    console.log(JSON.stringify(outcome, null, 2));
    return 0;
  }
  if (command === 'inspect' || command === 'prune' || command === 'setup') {
    if (command !== 'prune' && args.length) throw new Error('Use npm run rust:cache:setup -- --install for installation; setup/inspect accept no extra arguments');
    const layout = cacheLayout(cwd, env, options.sourceRoot);
    if (command === 'inspect') {
      const selected = cacheEnvironment(layout, env);
      const directRoot = path.join(layout.root, 'direct-targets', layout.repoKey);
      assertPlainTree(directRoot);
      const directTargets = existsSync(directRoot) ? readdirSync(directRoot).map((key) => {
        const target = path.join(directRoot, key);
        try { assertPlainTree(target); assertUnprotected(target, env); return { target, prunable: false, ...sizeOf(target) }; }
        catch (error) { return { target, prunable: false, refused: error.message }; }
      }) : [];
      console.log(JSON.stringify({ layout, cache: { enabled: selected.enabled, reason: selected.reason,
        store: selected.env.SCCACHE_DIR ?? layout.store, size_limit: selected.env.SCCACHE_CACHE_SIZE ?? '10G' },
        targets: inspectTargets(layout, env), direct_targets: directTargets }, null, 2));
    }
    else if (command === 'prune') {
      const options = { keep: 2, maxBytes: 10 * 1024 ** 3 };
      for (let index = 0; index < args.length; index += 2) {
        if (args[index] === '--keep') options.keep = Number(args[index + 1]);
        else if (args[index] === '--max-bytes') options.maxBytes = Number(args[index + 1]);
        else throw new Error('prune supports --keep and --max-bytes');
      }
      console.log(JSON.stringify(pruneTargets(layout, { ...options, env }), null, 2));
    } else {
      const selected = cacheEnvironment(layout, env);
      if (!selected.enabled) throw new Error(`sccache unavailable (${selected.reason}); see docs/developer/rust-build-cache.md for pinned dependency setup`);
      const version = spawnSync(selected.env.RUSTC_WRAPPER, ['--version'], { encoding: 'utf8', windowsHide: true });
      if (version.error || version.status !== 0 || version.stdout.trim() !== `sccache ${PINNED_VERSION}`) throw new Error(`Install official sccache ${PINNED_VERSION}; setup does not install or start servers`);
      const identity = { schema: 1, version: PINNED_VERSION, wrapper_path: realpathSync.native(selected.env.RUSTC_WRAPPER),
        wrapper_sha256: createHash('sha256').update(readFileSync(selected.env.RUSTC_WRAPPER)).digest('hex'),
        cache_dir: path.resolve(selected.env.SCCACHE_DIR), cache_size: selected.env.SCCACHE_CACHE_SIZE,
        server_port: selected.env.SCCACHE_SERVER_PORT };
      assertPlainTree(layout.root);
      assertUnprotected(layout.root, env);
      mkdirSync(layout.root, { recursive: true });
      writeFileSync(path.join(layout.root, 'sccache-identity.json'), JSON.stringify(identity, null, 2));
      console.log(JSON.stringify({ ...identity, manifest: path.join(layout.root, 'sccache-identity.json'), target: layout.target }, null, 2));
    }
    return 0;
  }
  if (command !== 'cargo' || args.length === 0) throw new Error('Use rust:cache setup|inspect|prune|recover-ended-claim or cargo <arguments>');
  cargoCommand(args);
  const separator = args.indexOf('--');
  const cargoArgs = separator < 0 ? args : args.slice(0, separator);
  // Caller config can name a wrapper or include another file. Leave its precedence
  // intact rather than injecting an environment wrapper over uninspected CLI config.
  const lane = cargoArgs.some((arg) => arg === '--config' || arg.startsWith('--config='))
    ? { ...options, env: { ...env, WARDIAN_RUST_CACHE_DISABLE: '1' } } : options;
  return withRustCache(() => {
    const invocation = cargoInvocation(args, { cwd, env: process.env });
    const result = (options.spawn ?? spawnSync)(invocation.program, invocation.args, { cwd: invocation.cwd, env: invocation.env,
      stdio: 'inherit', windowsHide: true });
    if (result.error) throw result.error;
    return result.status ?? 1;
  }, lane);
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  try { process.exitCode = main(); } catch (error) { console.error(error.message); process.exitCode = 1; }
}
