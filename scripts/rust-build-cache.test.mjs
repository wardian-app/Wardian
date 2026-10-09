import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { existsSync, linkSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, symlinkSync, unlinkSync, writeFileSync } from 'node:fs';
import { once } from 'node:events';
import { homedir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';
import { cacheEnvironment, cacheKey, cacheLayout, cargoInvocation, claimTarget, inspectTargets, main, pruneTargets, withRustCache } from './rust-build-cache.mjs';
import { verifyDownload } from './setup-rust-build-cache.mjs';

const testBase = process.env.WARDIAN_RUST_CACHE_TEST_ROOT ?? path.join(process.env.WARDIAN_HOME ?? path.join(homedir(), '.wardian'),
  'agents', process.env.WARDIAN_SESSION_ID ?? 'rust-cache-tests', 'workspace', 'temp', 'rust-cache-node-tests');
mkdirSync(testBase, { recursive: true });

function fixture(t) {
  const root = mkdtempSync(path.join(testBase, 'owned-'));
  t.after(() => {
    assert.equal(path.dirname(realpathSync(root)), realpathSync(testBase));
    assert.ok(path.basename(root).startsWith('owned-'));
    rmSync(root, { recursive: true, force: true });
  });
  const source = path.join(root, 'repo');
  const workspace = path.join(root, 'worktree');
  mkdirSync(source);
  mkdirSync(workspace);
  writeFileSync(path.join(workspace, 'Cargo.toml'), '[workspace]\n');
  const env = { WARDIAN_RUST_CACHE_ROOT: path.join(root, 'cache'), CARGO_HOME: path.join(root, 'cargo-home'), PATH: '' };
  const layout = cacheLayout(workspace, env, source);
  return { root, source, workspace, env, layout };
}

function hash(bytes) {
  return createHash('sha256').update(bytes).digest('hex');
}

function physicalClosureReceipt(layout, ownerBytes, markerBytes, owner, pid) {
  return {
    schema: 1,
    kind: 'rust-cache-ended-claim-recovery',
    repo_key: layout.repoKey,
    worktree_key: layout.worktreeKey,
    owner_sha256: hash(ownerBytes),
    owner: { token: owner.token, pid: owner.pid, started: owner.started },
    marker_sha256: hash(markerBytes),
    disposition: 'ended',
    evidence_producer: 'owned-node-test-child-handle',
    observed_at: new Date().toISOString(),
    closure: {
      basis: 'owned-process-handles-joined',
      complete: true,
      root: {
        pid,
        identity: { basis: 'captured-process-handle' },
        joined: true,
        streams: { stdin: 'closed', stdout: 'eof', stderr: 'eof' },
      },
      descendants: [],
    },
  };
}

function runNode(args, { cwd, env }) {
  const child = spawn(process.execPath, args, { cwd, env, stdio: ['ignore', 'pipe', 'pipe'], windowsHide: true });
  let stdout = '';
  let stderr = '';
  child.stdout.setEncoding('utf8').on('data', (chunk) => { stdout += chunk; });
  child.stderr.setEncoding('utf8').on('data', (chunk) => { stderr += chunk; });
  return once(child, 'close').then(([code, signal]) => ({ child, code, signal, stdout, stderr }));
}

function fixtureProcessEnv(f) {
  return {
    PATH: process.env.PATH ?? '',
    ...(process.env.SystemRoot ? { SystemRoot: process.env.SystemRoot } : {}),
    WARDIAN_RUST_CACHE_ROOT: f.env.WARDIAN_RUST_CACHE_ROOT,
    WARDIAN_RUST_CACHE_SOURCE_ROOT: f.source,
    CARGO_HOME: f.env.CARGO_HOME,
  };
}

async function createEndedClaim(f) {
  const launcher = fileURLToPath(new URL('./rust-build-cache.mjs', import.meta.url));
  const childCode = `
    import { readFileSync } from 'node:fs';
    import { cacheLayout, claimTarget } from ${JSON.stringify(new URL('./rust-build-cache.mjs', import.meta.url).href)};
    const layout = cacheLayout(process.cwd(), process.env);
    claimTarget(layout, process.env);
    const owner = JSON.parse(readFileSync(layout.claim + '/owner.json', 'utf8'));
    process.stdout.write(JSON.stringify({ pid: process.pid, owner }) + '\\n', () => { process.exitCode = 23; });
  `;
  const env = fixtureProcessEnv(f);
  const processResult = await runNode(['--input-type=module', '-e', childCode], { cwd: f.workspace, env });
  assert.equal(processResult.code, 23, processResult.stderr);
  assert.equal(processResult.signal, null);
  const ownerFromChild = JSON.parse(processResult.stdout.trim());
  assert.equal(ownerFromChild.pid, processResult.child.pid);
  assert.equal(ownerFromChild.owner.pid, processResult.child.pid);
  const ownerBytes = readFileSync(path.join(f.layout.claim, 'owner.json'));
  const markerBytes = readFileSync(path.join(f.layout.target, '.wardian-rust-target.json'));
  const receiptPath = path.join(f.root, 'closure-receipt.json');
  const receiptBytes = Buffer.from(JSON.stringify(physicalClosureReceipt(
    f.layout, ownerBytes, markerBytes, ownerFromChild.owner, processResult.child.pid,
  )));
  writeFileSync(receiptPath, receiptBytes);
  return { launcher, env, processResult, owner: ownerFromChild.owner, ownerBytes, markerBytes, receiptPath, receiptBytes };
}

function recoveryArgs(ended) {
  return ['recover-ended-claim', '--owner-sha256', hash(ended.ownerBytes), '--marker-sha256', hash(ended.markerBytes),
    '--closure-receipt', ended.receiptPath];
}

function recoverThroughDispatcher(f, ended, recoveryHooks) {
  return main(recoveryArgs(ended), { cwd: f.workspace, env: f.env, sourceRoot: f.source, recoveryHooks });
}

test('central worktree keys are distinct while repository store is shared', (t) => {
  const f = fixture(t);
  const second = path.join(f.root, 'second');
  mkdirSync(second);
  const other = cacheLayout(second, f.env, f.source);
  assert.notEqual(other.target, f.layout.target);
  assert.equal(other.store, f.layout.store);
  assert.equal(other.repoKey, f.layout.repoKey);
  const actual = realpathSync.native(f.workspace);
  const expected = createHash('sha256').update(process.platform === 'win32' ? actual.replaceAll('\\', '/').toLowerCase() : actual).digest('hex').slice(0, 16);
  assert.equal(cacheKey(f.workspace), expected);
  if (process.platform === 'win32') assert.equal(cacheKey(f.workspace.toUpperCase().replaceAll('\\', '/')), expected);
});

test('explicit source supports copied fixture without Git and layout never creates targets', (t) => {
  const f = fixture(t);
  assert.equal(cacheLayout(f.workspace, { ...f.env, WARDIAN_RUST_CACHE_SOURCE_ROOT: f.source }).target, f.layout.target);
  assert.equal(existsSync(f.layout.root), false);
});

test('sccache lane caps store and disables incremental without changing Cargo home', (t) => {
  const f = fixture(t);
  const selected = cacheEnvironment(f.layout, f.env, () => '/tools/sccache');
  assert.equal(selected.enabled, true);
  assert.equal(selected.env.CARGO_INCREMENTAL, '0');
  assert.equal(selected.env.SCCACHE_CACHE_SIZE, '10G');
  assert.equal(selected.env.SCCACHE_SERVER_PORT, '4227');
  assert.equal(selected.env.CARGO_HOME, f.env.CARGO_HOME);
  assert.equal(f.env.CARGO_INCREMENTAL, undefined);
});

test('missing/disabled sccache leaves ordinary incremental compilation intact', (t) => {
  const f = fixture(t);
  assert.equal(cacheEnvironment(f.layout, f.env, () => null).env.CARGO_INCREMENTAL, undefined);
  assert.equal(cacheEnvironment(f.layout, { ...f.env, WARDIAN_RUST_CACHE_DISABLE: '1' }, () => '/tools/sccache').enabled, false);
});

test('all custom wrapper carriers including explicitly empty values are preserved', (t) => {
  const f = fixture(t);
  for (const key of ['RUSTC_WRAPPER', 'CARGO_BUILD_RUSTC_WRAPPER', 'RUSTC_WORKSPACE_WRAPPER', 'CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER']) {
    for (const value of ['', 'custom-wrapper']) {
      const selected = cacheEnvironment(f.layout, { ...f.env, [key]: value }, () => '/tools/sccache');
      assert.equal(selected.enabled, false);
      assert.equal(selected.env[key], value);
    }
  }
});

test('Cargo configuration custom wrapper is preserved', (t) => {
  const f = fixture(t);
  mkdirSync(path.join(f.workspace, '.cargo'));
  writeFileSync(path.join(f.workspace, '.cargo/config.toml'), '[build]\nrustc-wrapper="custom"\n');
  assert.equal(cacheEnvironment(f.layout, f.env, () => '/tools/sccache').enabled, false);
});

test('quoted, dotted and workspace wrapper keys decline injection without cache lookup', (t) => {
  const f = fixture(t);
  mkdirSync(path.join(f.workspace, '.cargo'));
  const config = path.join(f.workspace, '.cargo/config.toml');
  for (const text of [
    '[build]\n"rustc-wrapper" = "custom"\n',
    "[build]\n'rustc-workspace-wrapper' = 'custom'\n",
    'build."rustc-wrapper" = "custom"\n',
    '"build"."rustc-workspace-wrapper" = "custom"\n',
    '["build"]\n"rustc\\u002dwrapper" = "custom"\n',
    'build = { "rustc-wrapper" = "custom" }\n',
    'include = "other-config.toml"\n',
    '[build]\nrustflags = [\n  "--cfg=custom",\n]\n',
  ]) {
    writeFileSync(config, text);
    const selected = cacheEnvironment(f.layout, f.env, () => assert.fail('uncertain/wrapper config must not look up cache'));
    assert.equal(selected.enabled, false, text);
    assert.equal(selected.env.RUSTC_WRAPPER, undefined, text);
    assert.equal(readFileSync(config, 'utf8'), text);
  }
});

test('legacy config takes precedence and ancestor/Cargo-home wrappers are preserved', (t) => {
  const f = fixture(t);
  const directory = path.join(f.workspace, '.cargo');
  mkdirSync(directory);
  writeFileSync(path.join(directory, 'config.toml'), '[build]\n"rustc-wrapper"="ignored"\n');
  writeFileSync(path.join(directory, 'config'), '[build]\ntarget-dir="target"\n');
  assert.equal(cacheEnvironment(f.layout, f.env, () => '/tools/sccache').enabled, true);
  for (const inherited of [path.join(f.root, '.cargo'), f.env.CARGO_HOME]) {
    mkdirSync(inherited);
    writeFileSync(path.join(inherited, 'config'), 'build."rustc-workspace-wrapper"="custom"\n');
    assert.equal(cacheEnvironment(f.layout, f.env, () => '/tools/sccache').enabled, false);
  }
});

test('POSIX registered worktrees preserve literal backslash identities', { skip: process.platform === 'win32' }, (t) => {
  const f = fixture(t);
  function git(args) {
    const result = spawnSync('git', args, { cwd: f.source, encoding: 'utf8' });
    assert.equal(result.status, 0, result.stderr);
    return result.stdout;
  }
  git(['init']);
  writeFileSync(path.join(f.source, 'Cargo.toml'), '[workspace]\n');
  git(['add', 'Cargo.toml']);
  git(['-c', 'user.name=Cache Test', '-c', 'user.email=cache-test@example.invalid', '-c', 'commit.gpgsign=false', 'commit', '-m', 'Fixture']);
  const literal = path.join(f.root, 'trees', 'a\\b');
  const nested = path.join(f.root, 'trees', 'a', 'b');
  git(['worktree', 'add', '--detach', literal]);
  git(['worktree', 'add', '--detach', nested]);
  const registered = git(['worktree', 'list', '--porcelain', '-z']);
  assert.ok(registered.includes(`worktree ${literal}\0`));
  assert.ok(registered.includes(`worktree ${nested}\0`));
  const first = cacheLayout(literal, f.env);
  const second = cacheLayout(nested, f.env);
  assert.equal(first.repoKey, second.repoKey);
  for (const key of ['worktreeKey', 'directTarget', 'target', 'claim']) assert.notEqual(first[key], second[key], key);
  const one = claimTarget(first, f.env);
  const two = claimTarget(second, f.env);
  try {
    assert.notEqual(JSON.parse(readFileSync(path.join(first.target, '.wardian-rust-target.json'))).workspace,
      JSON.parse(readFileSync(path.join(second.target, '.wardian-rust-target.json'))).workspace);
  } finally { two.release(); one.release(); }
});

test('explicit diagnostic incremental/debug overrides disable injected caching', (t) => {
  const f = fixture(t);
  const selected = cacheEnvironment(f.layout, { ...f.env, CARGO_INCREMENTAL: '1', CARGO_PROFILE_DEV_DEBUG: '2' }, () => '/tools/sccache');
  assert.equal(selected.env.CARGO_INCREMENTAL, '1');
  assert.equal(selected.env.CARGO_PROFILE_DEV_DEBUG, '2');
  assert.equal(selected.enabled, false);
  assert.equal(selected.env.RUSTC_WRAPPER, undefined);
  assert.match(selected.reason, /explicit incremental/);
});

test('claims serialize one target while independent worktrees can both claim', (t) => {
  const f = fixture(t);
  const lease = claimTarget(f.layout, f.env);
  assert.throws(() => claimTarget(f.layout, f.env), /claimed/);
  const other = path.join(f.root, 'other');
  mkdirSync(other);
  const second = claimTarget(cacheLayout(other, f.env, f.source), f.env);
  second.release();
  lease.release();
});

test('public recovery archives an owned crash claim and the ordinary launcher reacquires the same target', async (t) => {
  const f = fixture(t);
  const ended = await createEndedClaim(f);
  const { launcher, env, ownerBytes, markerBytes, receiptBytes } = ended;
  const markerPath = path.join(f.layout.target, '.wardian-rust-target.json');
  const sentinel = path.join(f.layout.target, 'preserve-me.bin');
  writeFileSync(sentinel, 'compiler output stays byte-for-byte');
  const sentinelBytes = readFileSync(sentinel);
  const other = path.join(f.root, 'unrelated-worktree');
  mkdirSync(other);
  const unrelatedLayout = cacheLayout(other, f.env, f.source);
  const unrelatedLease = claimTarget(unrelatedLayout, f.env);
  const unrelatedOwner = readFileSync(path.join(unrelatedLayout.claim, 'owner.json'));

  const inspection = await runNode([launcher, 'inspect'], { cwd: f.workspace, env });
  assert.equal(inspection.code, 0, inspection.stderr);
  const targetRecord = JSON.parse(inspection.stdout).targets.find((entry) => entry.key === f.layout.worktreeKey);
  assert.equal(targetRecord.owner_sha256, hash(ownerBytes));
  assert.equal(targetRecord.marker_sha256, hash(markerBytes));

  const result = await runNode([launcher, ...recoveryArgs(ended).slice(0)], { cwd: f.workspace, env });

  assert.equal(result.code, 0, result.stderr);
  assert.equal(result.signal, null);
  const outcome = JSON.parse(result.stdout);
  assert.equal(outcome.status, 'recovered');
  assert.equal(outcome.closure_basis, 'owned-process-handles-joined');
  assert.equal(existsSync(f.layout.claim), false);
  assert.deepEqual(readFileSync(markerPath), markerBytes);
  assert.deepEqual(readFileSync(sentinel), sentinelBytes);
  assert.deepEqual(readFileSync(path.join(outcome.archive, 'claim', 'owner.json')), ownerBytes);
  assert.deepEqual(readFileSync(path.join(outcome.archive, 'receipt.json')), receiptBytes);
  assert.deepEqual(readFileSync(path.join(unrelatedLayout.claim, 'owner.json')), unrelatedOwner);

  let cargoCalls = 0;
  assert.equal(main(['cargo', 'check'], {
    cwd: f.workspace,
    env: f.env,
    sourceRoot: f.source,
    lookup: () => null,
    spawn(program, args, options) {
      cargoCalls += 1;
      assert.equal(program, 'cargo');
      assert.equal(existsSync(f.layout.claim), true);
      assert.equal(options.env.WARDIAN_RUST_CACHE_TARGET, f.layout.target);
      assert.ok(args.includes(f.layout.target));
      return { status: 0 };
    },
  }), 0);
  assert.equal(cargoCalls, 1);
  assert.equal(existsSync(f.layout.claim), false);
  assert.deepEqual(readFileSync(markerPath), markerBytes);
  assert.deepEqual(readFileSync(sentinel), sentinelBytes);

  const nextLease = claimTarget(f.layout, f.env);
  const nextOwner = readFileSync(path.join(f.layout.claim, 'owner.json'));
  const repeated = await runNode([launcher, ...recoveryArgs(ended).slice(0)], { cwd: f.workspace, env });
  assert.equal(repeated.code, 0, repeated.stderr);
  const repeatedOutcome = JSON.parse(repeated.stdout);
  assert.equal(repeatedOutcome.status, 'already_recovered');
  assert.equal(repeatedOutcome.closure_basis, 'owned-process-handles-joined');
  assert.deepEqual(readFileSync(path.join(f.layout.claim, 'owner.json')), nextOwner);
  nextLease.release();
  unrelatedLease.release();
});

test('public recovery reports and verifies owned-job-zero in successful and repeated outcomes', async (t) => {
  const f = fixture(t);
  claimTarget(f.layout, f.env);
  const owner = { token: 'synthetic-job-zero-owner', pid: 987654321, started: '2026-01-01T00:00:00.000Z' };
  const ownerBytes = Buffer.from(JSON.stringify(owner));
  writeFileSync(path.join(f.layout.claim, 'owner.json'), ownerBytes);
  const markerBytes = readFileSync(path.join(f.layout.target, '.wardian-rust-target.json'));
  const receipt = physicalClosureReceipt(f.layout, ownerBytes, markerBytes, owner, owner.pid);
  receipt.evidence_producer = 'deterministic-job-zero-fixture';
  receipt.closure.basis = 'owned-job-zero';
  receipt.closure.root.identity = { basis: 'os-birth', birth_id: 'synthetic-fixture-birth-id' };
  receipt.closure.job = { observed: true, active_process_count: 0 };
  const receiptBytes = Buffer.from(JSON.stringify(receipt));
  const receiptPath = path.join(f.root, 'closure-receipt.json');
  writeFileSync(receiptPath, receiptBytes);
  const ended = { ownerBytes, markerBytes, receiptPath };
  const launcher = fileURLToPath(new URL('./rust-build-cache.mjs', import.meta.url));
  const env = fixtureProcessEnv(f);

  // This synthetic receipt tests public basis serialization, not physical Job evidence.
  const result = await runNode([launcher, ...recoveryArgs(ended)], { cwd: f.workspace, env });
  assert.equal(result.code, 0, result.stderr);
  const outcome = JSON.parse(result.stdout);
  assert.equal(outcome.status, 'recovered');
  assert.equal(outcome.closure_basis, 'owned-job-zero');

  const repeated = await runNode([launcher, ...recoveryArgs(ended)], { cwd: f.workspace, env });
  assert.equal(repeated.code, 0, repeated.stderr);
  const repeatedOutcome = JSON.parse(repeated.stdout);
  assert.equal(repeatedOutcome.status, 'already_recovered');
  assert.equal(repeatedOutcome.closure_basis, 'owned-job-zero');

  const outcomePath = path.join(outcome.archive, 'outcome.json');
  writeFileSync(outcomePath, JSON.stringify({ ...outcome, closure_basis: 'owned-process-handles-joined' }));
  const repeatedWithWrongBasis = await runNode([launcher, ...recoveryArgs(ended)], { cwd: f.workspace, env });
  assert.notEqual(repeatedWithWrongBasis.code, 0);
  assert.match(repeatedWithWrongBasis.stderr, /archived closure basis/);
});

test('recovery refuses live, PID-only and incomplete closure evidence without changing the claim', (t) => {
  const f = fixture(t);
  const lease = claimTarget(f.layout, f.env);
  const ownerBytes = readFileSync(path.join(f.layout.claim, 'owner.json'));
  const markerBytes = readFileSync(path.join(f.layout.target, '.wardian-rust-target.json'));
  const owner = JSON.parse(ownerBytes);
  const receiptPath = path.join(f.root, 'closure-receipt.json');
  const validShape = physicalClosureReceipt(f.layout, ownerBytes, markerBytes, owner, process.pid);
  const originalClaim = readFileSync(path.join(f.layout.claim, 'owner.json'));
  const originalMarker = readFileSync(path.join(f.layout.target, '.wardian-rust-target.json'));

  for (const invalid of [
    { ...validShape, disposition: 'live' },
    { ...validShape, closure: { ...validShape.closure, basis: 'pid-only' } },
    { ...validShape, closure: { ...validShape.closure, complete: false } },
    { ...validShape, closure: { ...validShape.closure, root: { ...validShape.closure.root, joined: false } } },
  ]) {
    writeFileSync(receiptPath, JSON.stringify(invalid));
    assert.throws(() => recoverThroughDispatcher(f, { ownerBytes, markerBytes, receiptPath }), /Closure receipt/);
    assert.deepEqual(readFileSync(path.join(f.layout.claim, 'owner.json')), originalClaim);
    assert.deepEqual(readFileSync(path.join(f.layout.target, '.wardian-rust-target.json')), originalMarker);
    assert.equal(existsSync(f.layout.recoveryBase), false);
  }
  lease.release();
});

test('recovery rejects changed generations, protected destinations and linked claim metadata', async (t) => {
  const f = fixture(t);
  const ended = await createEndedClaim(f);
  const ownerPath = path.join(f.layout.claim, 'owner.json');
  const markerPath = path.join(f.layout.target, '.wardian-rust-target.json');
  const originalOwner = readFileSync(ownerPath);
  const originalMarker = readFileSync(markerPath);

  writeFileSync(ownerPath, JSON.stringify({ ...JSON.parse(originalOwner), token: 'changed-generation' }));
  assert.throws(() => recoverThroughDispatcher(f, ended), /owner bytes changed/);
  assert.equal(existsSync(path.join(f.layout.recoveryBase, hash(originalOwner))), false);
  writeFileSync(ownerPath, originalOwner);

  writeFileSync(markerPath, JSON.stringify({ ...JSON.parse(originalMarker), kind: 'qualified' }));
  assert.throws(() => recoverThroughDispatcher(f, ended), /marker is unknown or changed/);
  writeFileSync(markerPath, originalMarker);
  const markerLink = path.join(f.root, 'linked-marker.json');
  linkSync(markerPath, markerLink);
  assert.throws(() => recoverThroughDispatcher(f, ended), /multiply-linked/);
  unlinkSync(markerLink);

  const manifest = path.join(f.root, 'protected-recovery.json');
  writeFileSync(manifest, JSON.stringify({ files: [{ path: f.layout.recoveryBase }] }));
  const protectedEnv = { ...f.env, WARDIAN_PROTECTED_INPUT_MANIFESTS: JSON.stringify([manifest]) };
  assert.throws(() => main(recoveryArgs(ended), { cwd: f.workspace, env: protectedEnv, sourceRoot: f.source }), /overlaps a protected input/);
  assert.equal(existsSync(f.layout.recoveryBase), false);
  assert.deepEqual(readFileSync(ownerPath), originalOwner);
  assert.deepEqual(readFileSync(markerPath), originalMarker);

  const link = path.join(f.root, 'linked-owner.json');
  linkSync(ownerPath, link);
  assert.throws(() => recoverThroughDispatcher(f, ended), /multiply-linked/);
  assert.deepEqual(readFileSync(ownerPath), originalOwner);
  assert.equal(existsSync(f.layout.recoveryBase), false);
});

test('recovery refuses extra claim files, linked owner files and linked quarantine ancestors', async (t) => {
  const extra = fixture(t);
  const extraEnded = await createEndedClaim(extra);
  writeFileSync(path.join(extra.layout.claim, 'unexpected.json'), '{}');
  assert.throws(() => recoverThroughDispatcher(extra, extraEnded), /unexpected metadata/);
  assert.equal(existsSync(path.join(extra.layout.recoveryBase, hash(extraEnded.ownerBytes))), false);

  const linked = fixture(t);
  const linkedEnded = await createEndedClaim(linked);
  const linkedOwner = path.join(linked.layout.claim, 'owner.json');
  const outsideOwner = path.join(linked.root, 'owner-copy.json');
  writeFileSync(outsideOwner, linkedEnded.ownerBytes);
  unlinkSync(linkedOwner);
  symlinkSync(outsideOwner, linkedOwner, 'file');
  assert.throws(() => recoverThroughDispatcher(linked, linkedEnded), /linked, multiply-linked or non-file/);
  assert.equal(existsSync(path.join(linked.layout.recoveryBase, hash(linkedEnded.ownerBytes))), false);

  const linkedAncestor = fixture(t);
  const ancestorEnded = await createEndedClaim(linkedAncestor);
  mkdirSync(path.dirname(linkedAncestor.layout.recoveryBase), { recursive: true });
  symlinkSync(linkedAncestor.workspace, linkedAncestor.layout.recoveryBase, process.platform === 'win32' ? 'junction' : 'dir');
  assert.throws(() => recoverThroughDispatcher(linkedAncestor, ancestorEnded), /linked/);
  assert.deepEqual(readFileSync(path.join(linkedAncestor.layout.claim, 'owner.json')), ancestorEnded.ownerBytes);
});

test('one reservation serializes recovery with launch and prune; failed finalization remains fail-closed', async (t) => {
  const f = fixture(t);
  const ended = await createEndedClaim(f);
  const markerPath = path.join(f.layout.target, '.wardian-rust-target.json');
  const sentinel = path.join(f.layout.target, 'preserve-me.bin');
  writeFileSync(sentinel, 'unchanged output');
  const markerBytes = readFileSync(markerPath);
  const sentinelBytes = readFileSync(sentinel);
  const archive = path.join(f.layout.recoveryBase, hash(ended.ownerBytes));

  assert.throws(() => recoverThroughDispatcher(f, ended, {
    afterMove() {
      assert.throws(() => claimTarget(f.layout, f.env), /recovery is reserved/);
      assert.throws(() => pruneTargets(f.layout, { keep: 0, maxBytes: 0, env: f.env }), /recovery is reserved/);
      assert.throws(() => recoverThroughDispatcher(f, ended), /prior recovery attempt is incomplete/);
    },
    writeOutcome() { throw new Error('injected receipt finalization failure'); },
  }), /partially completed.*injected receipt finalization failure/);

  assert.equal(existsSync(f.layout.claim), false);
  assert.equal(existsSync(f.layout.recoveryReservation), true);
  assert.deepEqual(readFileSync(path.join(archive, 'claim', 'owner.json')), ended.ownerBytes);
  assert.deepEqual(readFileSync(path.join(archive, 'receipt.json')), ended.receiptBytes);
  assert.equal(existsSync(path.join(archive, 'outcome.json')), false);
  assert.deepEqual(readFileSync(markerPath), markerBytes);
  assert.deepEqual(readFileSync(sentinel), sentinelBytes);
  assert.throws(() => recoverThroughDispatcher(f, ended), /prior recovery attempt is incomplete/);
  assert.equal(existsSync(f.layout.recoveryReservation), true);
  assert.deepEqual(readFileSync(markerPath), markerBytes);
  assert.deepEqual(readFileSync(sentinel), sentinelBytes);
});

test('failed rename preserves the selected claim and repeated recovery refuses the partial archive', async (t) => {
  const f = fixture(t);
  const ended = await createEndedClaim(f);
  const ownerPath = path.join(f.layout.claim, 'owner.json');
  const markerPath = path.join(f.layout.target, '.wardian-rust-target.json');
  const originalMarker = readFileSync(markerPath);
  const archive = path.join(f.layout.recoveryBase, hash(ended.ownerBytes));
  assert.throws(() => recoverThroughDispatcher(f, ended, {
    renameClaim() { throw new Error('injected atomic rename failure'); },
  }), /injected atomic rename failure/);
  assert.deepEqual(readFileSync(ownerPath), ended.ownerBytes);
  assert.deepEqual(readFileSync(markerPath), originalMarker);
  assert.equal(existsSync(path.join(archive, 'claim')), false);
  assert.equal(existsSync(f.layout.recoveryReservation), false);
  assert.throws(() => recoverThroughDispatcher(f, ended), /prior recovery attempt is incomplete/);
  assert.deepEqual(readFileSync(ownerPath), ended.ownerBytes);
  assert.deepEqual(readFileSync(markerPath), originalMarker);
});

test('a pre-existing recovery reservation blocks recovery, launch and prune without cleaning it', async (t) => {
  const f = fixture(t);
  const ended = await createEndedClaim(f);
  mkdirSync(f.layout.recoveryReservation, { recursive: true });
  assert.throws(() => recoverThroughDispatcher(f, ended), /already reserved/);
  assert.throws(() => claimTarget(f.layout, f.env), /recovery is reserved/);
  assert.throws(() => pruneTargets(f.layout, { keep: 0, maxBytes: 0, env: f.env }), /recovery is reserved/);
  assert.deepEqual(readFileSync(path.join(f.layout.claim, 'owner.json')), ended.ownerBytes);
  assert.deepEqual(readFileSync(path.join(f.layout.target, '.wardian-rust-target.json')), ended.markerBytes);
  assert.equal(existsSync(f.layout.recoveryReservation), true);
});

test('existing unowned outputs are never adopted', (t) => {
  const f = fixture(t);
  mkdirSync(f.layout.target, { recursive: true });
  writeFileSync(path.join(f.layout.target, 'Wardian.exe'), 'preserve');
  assert.throws(() => claimTarget(f.layout, f.env), /unowned/);
  assert.equal(readFileSync(path.join(f.layout.target, 'Wardian.exe'), 'utf8'), 'preserve');
  assert.equal(existsSync(f.layout.claim), false);
});

test('protected target ancestor and child both deny claim and prune', (t) => {
  const f = fixture(t);
  const lease = claimTarget(f.layout, f.env);
  lease.release();
  for (const protectedPath of [f.layout.root, path.join(f.layout.target, 'qualified.exe')]) {
    const manifest = path.join(f.root, 'protected.json');
    writeFileSync(manifest, JSON.stringify({ artifacts: [{ path: protectedPath }] }));
    const env = { ...f.env, WARDIAN_PROTECTED_INPUT_MANIFESTS: JSON.stringify([manifest]) };
    assert.throws(() => claimTarget(f.layout, env));
    assert.equal(pruneTargets(f.layout, { keep: 0, maxBytes: 0, env }).removed.length, 0);
    assert.equal(existsSync(f.layout.target), true);
  }
});

for (const boundary of ['claim', 'ancestor', 'descendant']) {
  test(`protected claim ${boundary} denies ownership before any write`, (t) => {
    const f = fixture(t);
    const protectedPath = boundary === 'claim' ? f.layout.claim : boundary === 'ancestor'
      ? path.dirname(f.layout.claim) : path.join(f.layout.claim, 'owner.json');
    const manifest = path.join(f.root, 'protected-claim.json');
    writeFileSync(manifest, JSON.stringify({ files: [{ path: protectedPath }] }));
    const env = { ...f.env, WARDIAN_PROTECTED_INPUT_MANIFESTS: JSON.stringify([manifest]) };
    assert.throws(() => claimTarget(f.layout, env), /Rust cache claim overlaps a protected input/);
    assert.equal(existsSync(f.layout.target), false, 'The target was not created');
    assert.equal(existsSync(path.dirname(f.layout.claim)), false, 'The claim parent was not created');
  });
}

test('owned release preserves a claim that becomes protected', (t) => {
  const f = fixture(t);
  const env = { ...f.env };
  const lease = claimTarget(f.layout, env);
  const ownerPath = path.join(f.layout.claim, 'owner.json');
  const originalOwner = readFileSync(ownerPath);
  const manifest = path.join(f.root, 'protected-owner.json');
  writeFileSync(manifest, JSON.stringify({ files: [{ path: ownerPath }] }));
  env.WARDIAN_PROTECTED_INPUT_MANIFESTS = JSON.stringify([manifest]);
  try {
    assert.throws(() => lease.release(), /Rust cache claim overlaps a protected input/);
    assert.deepEqual(readFileSync(ownerPath), originalOwner);
  } finally {
    delete env.WARDIAN_PROTECTED_INPUT_MANIFESTS;
    if (existsSync(f.layout.claim)) lease.release();
  }
});

test('prune skips claimed outputs and removes only positively marked inactive generation', (t) => {
  const f = fixture(t);
  const lease = claimTarget(f.layout, f.env);
  assert.equal(pruneTargets(f.layout, { keep: 0, maxBytes: 0, env: f.env }).removed.length, 0);
  lease.release();
  writeFileSync(path.join(f.layout.target, 'test-output'), 'compiled');
  const result = pruneTargets(f.layout, { keep: 0, maxBytes: 0, env: f.env });
  assert.deepEqual(result.removed, [f.layout.worktreeKey]);
  assert.equal(existsSync(f.layout.target), false);
  assert.equal(existsSync(f.workspace), true);
});

test('retention byte cap can evict even the most recent inactive target', (t) => {
  const f = fixture(t);
  claimTarget(f.layout, f.env).release();
  assert.equal(pruneTargets(f.layout, { keep: 2, maxBytes: 0, env: f.env }).removed.length, 1);
});

test('prune skips a different worktree while its recovery reservation is active', (t) => {
  const f = fixture(t);
  claimTarget(f.layout, f.env).release();
  const other = path.join(f.root, 'other-worktree');
  mkdirSync(other);
  const otherLayout = cacheLayout(other, f.env, f.source);
  claimTarget(otherLayout, f.env).release();
  mkdirSync(otherLayout.recoveryReservation, { recursive: true });

  const result = pruneTargets(f.layout, { keep: 0, maxBytes: 0, env: f.env });
  assert.deepEqual(result.removed, [f.layout.worktreeKey]);
  assert.equal(existsSync(otherLayout.target), true);
  assert.ok(result.skipped.some((entry) => entry.key === otherLayout.worktreeKey && entry.recovery_reserved));
});

test('qualified marker is refused and never pruned', (t) => {
  const f = fixture(t);
  claimTarget(f.layout, f.env).release();
  const file = path.join(f.layout.target, '.wardian-rust-target.json');
  const record = JSON.parse(readFileSync(file));
  record.kind = 'qualified';
  writeFileSync(file, JSON.stringify(record));
  assert.equal(inspectTargets(f.layout, f.env)[0].refused, 'unowned marker');
  assert.equal(pruneTargets(f.layout, { keep: 0, env: f.env }).removed.length, 0);
});

test('links cannot redirect claim or pruning to unrelated files', (t) => {
  const f = fixture(t);
  mkdirSync(path.dirname(f.layout.target), { recursive: true });
  symlinkSync(f.workspace, f.layout.target, process.platform === 'win32' ? 'junction' : 'dir');
  assert.throws(() => claimTarget(f.layout, f.env), /linked/);
  assert.equal(pruneTargets(f.layout, { keep: 0, env: f.env }).removed.length, 0);
});

test('scope restores environment and releases claims on callback or configuration failure', (t) => {
  const f = fixture(t);
  const old = process.env.WARDIAN_RUST_CACHE_TARGET;
  assert.throws(() => withRustCache(() => { throw new Error('callback failed'); }, { cwd: f.workspace, env: f.env, sourceRoot: f.source, lookup: () => null }), /callback failed/);
  assert.equal(process.env.WARDIAN_RUST_CACHE_TARGET, old);
  assert.equal(existsSync(f.layout.claim), false);
  mkdirSync(f.layout.root, { recursive: true });
  writeFileSync(path.join(f.layout.root, 'sccache-identity.json'), '{}');
  assert.throws(() => withRustCache(() => {}, { cwd: f.workspace, env: f.env, sourceRoot: f.source }), /identity/);
  assert.equal(existsSync(f.layout.claim), false);
});

test('lane strips unique Cargo routing environment; arguments retain exact target and test separator', (t) => {
  const f = fixture(t);
  const env = { ...f.env, CARGO_TARGET_DIR: f.layout.target, CARGO_BUILD_BUILD_DIR: f.layout.target };
  withRustCache(() => {
    assert.equal(process.env.CARGO_TARGET_DIR, undefined);
    const invocation = cargoInvocation(['+1.99.0', 'test', '--', '--test-threads=1'], { cwd: f.workspace });
    assert.equal(invocation.args[0], '+1.99.0');
    assert.ok(invocation.args.indexOf('--target-dir') < invocation.args.indexOf('--'));
    assert.equal(invocation.args[invocation.args.indexOf('--target-dir') + 1], f.layout.target);
    const nested = withRustCache(() => 'nested');
    assert.equal(nested, 'nested');
  }, { cwd: f.workspace, env, sourceRoot: f.source, lookup: () => null });
});

test('metadata uses global Cargo config instead of unsupported target-dir flag', (t) => {
  const f = fixture(t);
  const invocation = cargoInvocation(['metadata', '--no-deps'], { cwd: f.workspace, env: { ...f.env, WARDIAN_RUST_CACHE_TARGET: f.layout.target } });
  assert.equal(invocation.args.includes('--target-dir'), false);
  assert.ok(invocation.args.some((arg) => arg.startsWith('build.target-dir=')));
});

test('custom output overrides are rejected before creating any central targets', (t) => {
  const f = fixture(t);
  assert.throws(() => withRustCache(() => {}, { cwd: f.workspace, env: { ...f.env, CARGO_TARGET_DIR: f.source }, sourceRoot: f.source }), /custom CARGO_TARGET_DIR/);
  assert.equal(existsSync(f.layout.root), false);
});

test('installer checks official checksum before extraction', () => {
  const bytes = Buffer.from('official-fixture');
  const expected = createHash('sha256').update(bytes).digest('hex');
  assert.equal(verifyDownload(bytes, `${expected}  release.zip`), expected);
  assert.throws(() => verifyDownload(bytes, '0'.repeat(64)), /checksum/);
  assert.throws(() => verifyDownload(bytes, 'garbage'), /checksum/);
});

test('wrong setup interface rejects installation flags before any side effects', () => {
  assert.throws(() => main(['setup', '--install']), /rust:cache:setup/);
});

test('identity record never authorizes shared sccache in protected-input lane', (t) => {
  const f = fixture(t);
  mkdirSync(f.layout.root, { recursive: true });
  const executable = path.join(f.root, 'sccache.exe');
  writeFileSync(executable, 'fixture executable');
  const identityFile = path.join(f.layout.root, 'sccache-identity.json');
  writeFileSync(identityFile, JSON.stringify({ schema: 1, version: '0.18.0', wrapper_path: executable,
    wrapper_sha256: createHash('sha256').update(readFileSync(executable)).digest('hex'), cache_dir: f.layout.store }));
  const selected = cacheEnvironment(f.layout, { ...f.env, WARDIAN_PROTECTED_INPUT_MANIFESTS: '[]', WARDIAN_SCCACHE_IDENTITY_MANIFEST: identityFile }, () => executable);
  assert.equal(selected.enabled, false);
  assert.equal(selected.env.RUSTC_WRAPPER, undefined);
  assert.equal(selected.env.CARGO_INCREMENTAL, undefined);
  assert.match(selected.reason, /protected-input/);
});

test('managed direct target routes to separate launcher target and is never pruned', (t) => {
  const f = fixture(t);
  mkdirSync(f.layout.directTarget, { recursive: true });
  writeFileSync(path.join(f.layout.directTarget, 'live-output'), 'keep direct output');
  withRustCache((layout) => {
    assert.notEqual(layout.target, layout.directTarget);
    assert.equal(process.env.WARDIAN_RUST_CACHE_TARGET, layout.target);
  }, { cwd: f.workspace, env: { ...f.env, CARGO_TARGET_DIR: f.layout.directTarget }, sourceRoot: f.source, lookup: () => null });
  pruneTargets(f.layout, { keep: 0, maxBytes: 0, env: f.env });
  assert.equal(readFileSync(path.join(f.layout.directTarget, 'live-output'), 'utf8'), 'keep direct output');
});

test('explicit target override cannot escape the launcher claim and last config wins', (t) => {
  const f = fixture(t);
  const env = { ...f.env, WARDIAN_RUST_CACHE_TARGET: f.layout.target };
  assert.throws(() => cargoInvocation(['check', '--target-dir', f.layout.directTarget], { cwd: f.workspace, env }), /exclusive claim/);
  const invocation = cargoInvocation(['+1.99.0', 'check', '--config', 'build.build-dir="old-target"'], { cwd: f.workspace, env });
  assert.equal(invocation.args[0], '+1.99.0');
  assert.equal(invocation.args.at(-1), `build.build-dir=${JSON.stringify(f.layout.target)}`);
});

test('public cargo dispatch rejects mutators and unknown globals before claim or fake Cargo spawn', (t) => {
  const f = fixture(t);
  mkdirSync(path.join(f.workspace, 'target'));
  const preserved = path.join(f.workspace, 'target', 'qualified-output');
  writeFileSync(preserved, 'keep');
  let calls = 0;
  const fakeCargo = () => { calls += 1; writeFileSync(preserved, 'wrong dispatch'); return { status: 0 }; };
  for (const args of [
    ['clean'], ['--offline', 'clean'], ['+1.99.0', '--config', 'build.target-dir="target"', 'clean'],
    ['install', 'example'], ['run'], ['custom-subcommand'], ['--unknown', 'check'], ['--color'], ['--config'],
  ]) {
    assert.throws(() => main(['cargo', ...args], { cwd: f.workspace, env: f.env, sourceRoot: f.source, spawn: fakeCargo }), /Unsupported Cargo|Missing Cargo/);
    assert.equal(calls, 0);
    assert.equal(existsSync(f.layout.root), false, 'reject before creating a claim or target');
    assert.equal(readFileSync(preserved, 'utf8'), 'keep');
  }
  assert.throws(() => cargoInvocation(['clean'], { cwd: f.workspace, env: f.env }), /Unsupported Cargo/);
});

test('valid global options route fake Cargo into a live claim and fmt remains supported', (t) => {
  const f = fixture(t);
  const orders = [
    ['--offline', 'check'],
    ['+1.99.0', '-vv', '--locked', '--color', 'never', '--config', 'net.offline=true', 'test', '--', '--test-threads=1'],
    ['--frozen', '--config=net.offline=true', '--color=auto', 'build'],
    ['--quiet', 'metadata', '--no-deps'],
    ['test', '--', '--target-dir=literal-test-argument'],
    ['fmt', '--all', '--check'],
  ];
  for (const args of orders) {
    let calls = 0;
    assert.equal(main(['cargo', ...args], { cwd: f.workspace, env: f.env, sourceRoot: f.source, lookup: () => '/tools/sccache',
      spawn(program, forwarded, options) {
        calls += 1;
        assert.equal(program, 'cargo');
        assert.equal(existsSync(f.layout.claim), true);
        assert.equal(options.env.WARDIAN_RUST_CACHE_TARGET, f.layout.target);
        if (args.some((arg) => arg === '--config' || arg.startsWith('--config='))) assert.equal(options.env.RUSTC_WRAPPER, undefined);
        if (args[0].startsWith('+')) assert.equal(forwarded[0], args[0]);
        if (args.includes('fmt')) assert.deepEqual(forwarded, args);
        else {
          assert.ok(forwarded.includes(`build.build-dir=${JSON.stringify(f.layout.target)}`));
          if (args.includes('metadata')) assert.ok(forwarded.includes(`build.target-dir=${JSON.stringify(f.layout.target)}`));
          else assert.equal(forwarded[forwarded.indexOf('--target-dir') + 1], f.layout.target);
        }
        return { status: 0 };
      } }), 0);
    assert.equal(calls, 1);
    assert.equal(existsSync(f.layout.claim), false);
  }
});

test('public CLI config preserves wrapper precedence by declining injection', (t) => {
  const f = fixture(t);
  for (const config of [
    ['--config', 'build."rustc-wrapper"="custom"'],
    ['--config=build."rustc-workspace-wrapper"="custom"'],
    ['--config', 'external-config.toml'],
  ]) {
    assert.equal(main(['cargo', ...config, 'check'], { cwd: f.workspace, env: f.env, sourceRoot: f.source,
      lookup: () => assert.fail('caller CLI config must not inject a cache wrapper'),
      spawn(program, args, options) {
        assert.equal(program, 'cargo');
        assert.equal(options.env.RUSTC_WRAPPER, undefined);
        assert.ok(args.includes(config.at(-1)));
        assert.equal(args[args.indexOf('--target-dir') + 1], f.layout.target);
        return { status: 0 };
      } }), 0);
  }
});

test('incremental opt-in skips both injected wrapper and cache lookup for either Cargo carrier', (t) => {
  const f = fixture(t);
  for (const key of ['CARGO_INCREMENTAL', 'CARGO_BUILD_INCREMENTAL']) {
    for (const value of ['1', 'true']) {
      const selected = cacheEnvironment(f.layout, { ...f.env, [key]: value }, () => {
        assert.fail('incremental lane must not look up sccache');
      });
      assert.equal(selected.enabled, false);
      assert.equal(selected.env[key], value);
      assert.equal(selected.env.RUSTC_WRAPPER, undefined);
      assert.equal(selected.env.SCCACHE_DIR, undefined);
      if (key === 'CARGO_BUILD_INCREMENTAL') assert.equal(selected.env.CARGO_INCREMENTAL, undefined);
      assert.match(selected.reason, /explicit incremental/);
    }
  }
  const zero = cacheEnvironment(f.layout, { ...f.env, CARGO_INCREMENTAL: '0' }, () => '/tools/sccache');
  assert.equal(zero.enabled, true);
  assert.equal(zero.env.CARGO_INCREMENTAL, '0');
  for (const value of ['0', 'false']) {
    const selected = cacheEnvironment(f.layout, { ...f.env, CARGO_BUILD_INCREMENTAL: value }, () => '/tools/sccache');
    assert.equal(selected.enabled, true);
    assert.equal(selected.env.CARGO_BUILD_INCREMENTAL, value);
    assert.equal(selected.env.CARGO_INCREMENTAL, undefined);
  }
});
