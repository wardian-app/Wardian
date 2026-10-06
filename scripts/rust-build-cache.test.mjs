import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { homedir } from 'node:os';
import path from 'node:path';
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
