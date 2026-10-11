import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { chmodSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, symlinkSync, writeFileSync, existsSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { test } from 'node:test';
import { assertCompilerAdmission, compilerWritableRoots } from './compiler-input-guard.mjs';

const runner = fileURLToPath(new URL('./verify-ci.mjs', import.meta.url));
const cli = fileURLToPath(new URL('./compiler-input-guard.mjs', import.meta.url));
const testBase = process.env.WARDIAN_GUARD_TEST_ROOT ?? tmpdir();
mkdirSync(testBase, { recursive: true });

function fixture(t) {
  const root = mkdtempSync(path.join(testBase, 'compiler-guard-'));
  t.after(() => {
    // Delete only this freshly created fixture, never an inferred home/root.
    assert.equal(path.dirname(realpathSync(root)), realpathSync(testBase));
    assert.ok(path.basename(root).startsWith('compiler-guard-'));
    rmSync(root, { recursive: true, force: true });
  });
  const cwd = path.join(root, 'workspace');
  const target = path.join(cwd, 'target');
  const protectedFile = path.join(target, 'debug', 'protected-input.bin');
  mkdirSync(path.dirname(protectedFile), { recursive: true });
  writeFileSync(protectedFile, 'immutable input');
  const manifest = path.join(root, 'inventory.json');
  writeFileSync(manifest, JSON.stringify([{ path: protectedFile, sha256: 'fixture' }]));
  const env = { ...process.env };
  for (const key of Object.keys(env)) {
    if (/^(?:CARGO_|RUST|SCCACHE_|WARDIAN_(?:PROTECTED_INPUT_MANIFESTS|SCCACHE_IDENTITY|RUST_CACHE_)|NODE_OPTIONS)/i.test(key)) delete env[key];
  }
  env.CARGO_HOME = path.join(root, 'cargo-home');
  env.CARGO_TARGET_DIR = target;
  env.WARDIAN_PROTECTED_INPUT_MANIFESTS = JSON.stringify([manifest]);
  // Model a surrounding cache phase's existing lease, so this suite exercises
  // the real verification dispatch without performing Git discovery or cache
  // setup. Compiler admission remains enabled with the fixture inventory.
  const claim = path.join(root, 'cache-claim');
  mkdirSync(claim);
  writeFileSync(path.join(claim, 'owner.json'), JSON.stringify({ token: 'guard-fixture' }));
  env.WARDIAN_RUST_CACHE_CLAIM = claim;
  env.WARDIAN_RUST_CACHE_TOKEN = 'guard-fixture';
  writeFileSync(path.join(cwd, 'Cargo.toml'), '[workspace]\nmembers = []\n');
  mkdirSync(path.join(cwd, '.github', 'workflows'), { recursive: true });
  writeFileSync(path.join(cwd, '.github', 'workflows', 'ci.yml'), '# local-verify: backend\nrun: cargo check\n');
  const hook = path.join(root, 'spawn-hook.mjs');
  const marker = path.join(root, 'child-marker');
  const trace = path.join(root, 'spawn-trace.json');
  writeFileSync(hook, `import cp from 'node:child_process';
import { writeFileSync } from 'node:fs';
import { syncBuiltinESMExports } from 'node:module';
const original = cp.spawnSync;
cp.spawnSync = (command, options) => {
  if (!/^(cargo |npm run check:rust-deadcode$)/.test(command)
    && command !== 'node --test scripts/native-e2e-windows-supervisor.test.mjs') throw new Error('Unexpected instrumented command');
  writeFileSync(process.env.GUARD_TRACE, JSON.stringify({ command, cwd: options.cwd, shell: options.shell }));
  return original(process.execPath, ['-e', 'require("node:fs").writeFileSync(process.env.GUARD_MARKER, "harmless child launched")'], { env: process.env });
};
syncBuiltinESMExports();
`);
  env.GUARD_TRACE = trace;
  env.GUARD_MARKER = marker;
  const run = (command = 'cargo check') => {
    writeFileSync(path.join(cwd, '.github', 'workflows', 'ci.yml'), `# local-verify: backend\nrun: ${command}\n`);
    return spawnSync(process.execPath, ['--import', pathToFileURL(hook).href, runner, '--only', 'backend'], {
      cwd, env, encoding: 'utf8', timeout: 10_000,
    });
  };
  return { root, cwd, target, protectedFile, manifest, env, marker, trace, run };
}

function denied(f, command) {
  const result = f.run(command);
  assert.equal(result.error, undefined);
  assert.equal(result.status, 1, result.stdout + result.stderr);
  assert.match(result.stderr, /Compiler input guard/);
  assert.equal(existsSync(f.trace), false, 'spawnSync hook was never reached');
  assert.equal(existsSync(f.marker), false, 'child was never launched');
  assert.equal(readFileSync(f.protectedFile, 'utf8'), 'immutable input');
}

const supervisorCommand = 'node --test scripts/native-e2e-windows-supervisor.test.mjs';

function supervisorFixture(t) {
  const f = fixture(t);
  // Keep all modeled temporary roots disjoint from the protected target.
  for (const key of Object.keys(f.env)) {
    if (/^(?:TMPDIR|TMP|TEMP|NODE_OPTIONS|WARDIAN_SUPERVISOR_TEST_ROOT)$/i.test(key)) delete f.env[key];
  }
  const temporary = path.join(f.root, 'compiler-temp');
  const fixtures = path.join(f.root, 'supervisor-fixtures');
  mkdirSync(temporary);
  mkdirSync(fixtures);
  Object.assign(f.env, { TEMP: temporary, TMP: temporary, TMPDIR: temporary, WARDIAN_SUPERVISOR_TEST_ROOT: fixtures });
  return f;
}

test('registered supervisor command reaches maintained verification dispatch with disjoint roots', (t) => {
  const f = supervisorFixture(t);
  const admission = assertCompilerAdmission({ program: 'node', args: ['--test', 'scripts/native-e2e-windows-supervisor.test.mjs'], cwd: f.cwd, env: f.env });
  const canonical = (root) => process.platform === 'win32' ? realpathSync(root).toLowerCase() : realpathSync(root);
  assert.deepEqual(admission.writable_roots, [canonical(f.env.WARDIAN_SUPERVISOR_TEST_ROOT), canonical(f.env.TEMP)]);
  const result = f.run(supervisorCommand);
  assert.equal(result.error, undefined);
  assert.equal(result.status, 0, result.stdout + result.stderr);
  assert.equal(JSON.parse(readFileSync(f.trace)).command, supervisorCommand);
  assert.match(readFileSync(f.marker, 'utf8'), /harmless child launched/);
});

test('supervisor default fixture root uses the child temporary environment', (t) => {
  const f = supervisorFixture(t);
  delete f.env.WARDIAN_SUPERVISOR_TEST_ROOT;
  const result = f.run(supervisorCommand);
  assert.equal(result.status, 0, result.stdout + result.stderr);
  assert.equal(existsSync(f.marker), true);
});

test('supervisor admission preserves safe explicit memory and warning tuning', (t) => {
  const f = supervisorFixture(t);
  const args = ['--test', 'scripts/native-e2e-windows-supervisor.test.mjs'];
  const original = assertCompilerAdmission({ program: 'node', args, cwd: f.cwd, env: f.env });
  f.env.NODE_OPTIONS = '--max-old-space-size=512 --disable-warning=DEP0040';
  assert.deepEqual(assertCompilerAdmission({ program: 'node', args, cwd: f.cwd, env: f.env }), original);
  // The memory option is supported by all maintained Node versions. Test the
  // warning option's root classification directly without requiring newer Node.
  f.env.NODE_OPTIONS = '--max-old-space-size=512';
  const result = f.run(supervisorCommand);
  assert.equal(result.status, 0, result.stdout + result.stderr);
  assert.equal(existsSync(f.marker), true);
  assert.equal(f.env.NODE_OPTIONS, '--max-old-space-size=512');
});

test('supervisor admission refuses preload, debugger and output-redirection options', (t) => {
  for (const options of ['--import=./unknown.mjs', '--require=./unknown.cjs', '--inspect', '--redirect-warnings=./warnings.log', '--max-old-space-size=512 --import=./unknown.mjs']) {
    const f = supervisorFixture(t);
    f.env.NODE_OPTIONS = options;
    assert.throws(() => assertCompilerAdmission({ program: 'node', args: ['--test', 'scripts/native-e2e-windows-supervisor.test.mjs'], cwd: f.cwd, env: f.env }), /unsupported supervisor Node options/);
    assert.equal(existsSync(f.trace), false);
    assert.equal(existsSync(f.marker), false);
  }
});

test('unknown Node scripts and extra supervisor options fail before dispatch', (t) => {
  for (const command of [
    'node --test scripts/unknown.test.mjs',
    `${supervisorCommand} scripts/unknown.test.mjs`,
    'node --inspect --test scripts/native-e2e-windows-supervisor.test.mjs',
  ]) denied(supervisorFixture(t), command);
});

test('missing or relative supervisor fixture and compiler temporary roots fail before dispatch', (t) => {
  for (const key of ['WARDIAN_SUPERVISOR_TEST_ROOT', 'TEMP', 'TMP', 'TMPDIR']) {
    if (process.platform === 'win32' && key === 'TMPDIR') continue;
    if (process.platform !== 'win32' && key !== 'WARDIAN_SUPERVISOR_TEST_ROOT' && key !== 'TMPDIR') continue;
    for (const root of ['missing', 'relative']) {
      const f = supervisorFixture(t);
      f.env[key] = root === 'missing' ? path.join(f.root, 'not-created') : 'relative-temp';
      denied(f, supervisorCommand);
      assert.equal(existsSync(path.join(f.root, 'not-created')), false);
    }
  }
});

test('protected supervisor fixture and compiler temporary overlaps fail before dispatch', (t) => {
  const keys = process.platform === 'win32' ? ['WARDIAN_SUPERVISOR_TEST_ROOT', 'TEMP', 'TMP'] : ['WARDIAN_SUPERVISOR_TEST_ROOT', 'TMPDIR'];
  for (const key of keys) {
    const f = supervisorFixture(t);
    f.env[key] = f.target;
    denied(f, supervisorCommand);
  }
});

test('unqualified supervisor Node options fail before maintained dispatch', (t) => {
  const f = supervisorFixture(t);
  f.env.NODE_OPTIONS = '--no-warnings';
  denied(f, supervisorCommand);
});

test('supervisor root aliases resolve before protected-input containment', (t) => {
  const f = supervisorFixture(t);
  const alias = path.join(f.root, 'fixture-alias');
  symlinkSync(f.target, alias, process.platform === 'win32' ? 'junction' : 'dir');
  f.env.WARDIAN_SUPERVISOR_TEST_ROOT = alias;
  denied(f, supervisorCommand);
});

test('shared admission refuses omitted Windows TEMP before a launcher can restore it', { skip: process.platform !== 'win32' }, (t) => {
  for (const omitTmp of [false, true]) {
    const f = supervisorFixture(t);
    delete f.env.TEMP;
    if (omitTmp) delete f.env.TMP;
    // Windows child bootstrap can restore omitted TEMP from the parent, so
    // test the supplied environment at the shared pre-spawn boundary itself.
    assert.throws(() => assertCompilerAdmission({ program: 'node', args: ['--test', 'scripts/native-e2e-windows-supervisor.test.mjs'], cwd: f.cwd, env: f.env }), /supervisor temporary directory is not configured/);
    assert.equal(existsSync(f.trace), false);
    assert.equal(existsSync(f.marker), false);
  }
});

test('Windows supervisor admission follows case-insensitive child environment names', { skip: process.platform !== 'win32' }, (t) => {
  for (const key of ['TEMP', 'TMP', 'WARDIAN_SUPERVISOR_TEST_ROOT', 'NODE_OPTIONS']) {
    const f = supervisorFixture(t);
    delete f.env[key];
    f.env[key.toLowerCase()] = key === 'NODE_OPTIONS' ? '--no-warnings' : f.target;
    denied(f, supervisorCommand);
  }
});

test('supervisor admission includes inherited Windows environment fields', { skip: process.platform !== 'win32' }, (t) => {
  for (const key of ['TMP', 'NODE_OPTIONS']) {
    const f = supervisorFixture(t);
    delete f.env[key];
    // Node deliberately includes enumerable prototype fields when spawning.
    Object.setPrototypeOf(f.env, { [key]: key === 'TMP' ? f.target : '--import=./unknown.mjs' });
    assert.throws(() => assertCompilerAdmission({ program: 'node', args: ['--test', 'scripts/native-e2e-windows-supervisor.test.mjs'], cwd: f.cwd, env: f.env }), /overlaps writable tree|unsupported supervisor Node options/);
    assert.equal(existsSync(f.marker), false);
    assert.equal(readFileSync(f.protectedFile, 'utf8'), 'immutable input');
  }
});

// Match the setup record; it is deliberately not a production trust mechanism.
// The executable really exists and is harmless; a correct self-hash must still
// fail rather than grant arbitrary binaries compiler-wrapper authority.
function declaredSccache(f) {
  const wrapper = path.join(f.root, process.platform === 'win32' ? 'sccache.cmd' : 'sccache');
  const wrapperMarker = path.join(f.root, 'wrapper-marker');
  writeFileSync(wrapper, process.platform === 'win32'
    ? '@echo off\r\necho harmless wrapper launched>"%GUARD_WRAPPER_MARKER%"\r\n'
    : '#!/bin/sh\nprintf "%s" "harmless wrapper launched" > "$GUARD_WRAPPER_MARKER"\n');
  chmodSync(wrapper, 0o755);
  const cacheDir = path.join(f.root, 'cache');
  mkdirSync(cacheDir);
  const identityManifest = path.join(cacheDir, 'sccache-identity.json');
  const identity = {
    schema: 1,
    version: '0.18.0',
    wrapper_path: wrapper,
    wrapper_sha256: createHash('sha256').update(readFileSync(wrapper)).digest('hex'),
    cache_dir: cacheDir,
    cache_size: '10G',
    server_port: '4227',
  };
  writeFileSync(identityManifest, JSON.stringify(identity));
  Object.assign(f.env, {
    RUSTC_WRAPPER: wrapper,
    SCCACHE_DIR: identity.cache_dir,
    SCCACHE_CACHE_SIZE: identity.cache_size,
    SCCACHE_SERVER_PORT: String(identity.server_port),
    WARDIAN_SCCACHE_IDENTITY_MANIFEST: identityManifest,
    GUARD_WRAPPER_MARKER: wrapperMarker,
  });
  return { wrapper, wrapperMarker, identity, identityManifest };
}

test('a real harmless wrapper and matching self-declared identity cannot reach normal dispatch', (t) => {
  const f = fixture(t);
  f.env.CARGO_TARGET_DIR = path.join(f.root, 'disjoint');
  const { wrapper, wrapperMarker, identity, identityManifest } = declaredSccache(f);
  const proof = spawnSync(wrapper, [], {
    env: f.env, shell: process.platform === 'win32', encoding: 'utf8', timeout: 10_000,
  });
  assert.equal(proof.error, undefined);
  assert.equal(proof.status, 0, proof.stdout + proof.stderr);
  assert.match(readFileSync(wrapperMarker, 'utf8'), /harmless wrapper launched/);
  rmSync(wrapperMarker);
  assert.equal(createHash('sha256').update(readFileSync(wrapper)).digest('hex'), identity.wrapper_sha256);
  assert.deepEqual(JSON.parse(readFileSync(identityManifest)), identity);
  const result = f.run();
  assert.equal(result.status, 1, result.stdout + result.stderr);
  assert.match(result.stderr, /unsupported compiler environment RUSTC_WRAPPER/);
  assert.equal(existsSync(f.trace), false);
  assert.equal(existsSync(f.marker), false);
  assert.equal(existsSync(wrapperMarker), false);
  assert.deepEqual(JSON.parse(readFileSync(identityManifest)), identity);
  assert.equal(existsSync(f.env.CARGO_TARGET_DIR), false);
  assert.equal(readFileSync(f.protectedFile, 'utf8'), 'immutable input');
});

test('changed wrapper bytes deny before dispatch even when the caller refreshes its hash', (t) => {
  for (const refreshHash of [false, true]) {
    const f = fixture(t);
    f.env.CARGO_TARGET_DIR = path.join(f.root, 'disjoint');
    const { wrapper, wrapperMarker, identity, identityManifest } = declaredSccache(f);
    writeFileSync(wrapper, readFileSync(wrapper, 'utf8') + (process.platform === 'win32' ? 'rem changed\r\n' : '# changed\n'));
    const changedHash = createHash('sha256').update(readFileSync(wrapper)).digest('hex');
    assert.notEqual(changedHash, identity.wrapper_sha256);
    if (refreshHash) {
      identity.wrapper_sha256 = changedHash;
      writeFileSync(identityManifest, JSON.stringify(identity));
    }
    denied(f);
    assert.equal(existsSync(wrapperMarker), false);
  }
});

test('self-declared sccache denies direct and aliased cache overlap before any spawn', (t) => {
  for (const alias of [false, true]) {
    const f = fixture(t);
    f.env.CARGO_TARGET_DIR = path.join(f.root, 'disjoint');
    const { wrapperMarker, identity, identityManifest } = declaredSccache(f);
    identity.cache_dir = f.target;
    if (alias) {
      identity.cache_dir = path.join(f.root, 'cache-alias');
      symlinkSync(f.target, identity.cache_dir, process.platform === 'win32' ? 'junction' : 'dir');
    }
    f.env.SCCACHE_DIR = identity.cache_dir;
    writeFileSync(identityManifest, JSON.stringify(identity));
    denied(f);
    assert.equal(existsSync(wrapperMarker), false);
  }
});

test('invalid setup records and custom wrappers never confer admission', (t) => {
  for (const mode of ['missing', 'relative', 'missing-file', 'directory', 'mismatched-hash', 'malformed',
    'wrong-version', 'wrong-schema', 'custom-wrapper', 'cache-mismatch', 'manifest-relative', 'manifest-directory', 'manifest-link']) {
    const f = fixture(t);
    f.env.CARGO_TARGET_DIR = path.join(f.root, 'disjoint');
    const { wrapperMarker, identity, identityManifest } = declaredSccache(f);
    if (mode === 'relative') identity.wrapper_path = path.basename(identity.wrapper_path);
    if (mode === 'missing-file') identity.wrapper_path = path.join(f.root, 'absent-sccache');
    if (mode === 'directory') identity.wrapper_path = f.cwd;
    if (mode === 'mismatched-hash') identity.wrapper_sha256 = '0'.repeat(64);
    if (mode === 'wrong-version') identity.version = '0.17.0';
    if (mode === 'wrong-schema') identity.schema = 2;
    if (mode === 'custom-wrapper') f.env.RUSTC_WRAPPER = process.execPath;
    if (mode === 'cache-mismatch') f.env.SCCACHE_DIR = f.target;
    writeFileSync(identityManifest, mode === 'malformed' ? '{' : JSON.stringify(identity));
    if (mode === 'missing') delete f.env.WARDIAN_SCCACHE_IDENTITY_MANIFEST;
    if (mode === 'manifest-relative') f.env.WARDIAN_SCCACHE_IDENTITY_MANIFEST = 'sccache-identity.json';
    if (mode === 'manifest-directory') f.env.WARDIAN_SCCACHE_IDENTITY_MANIFEST = f.cwd;
    if (mode === 'manifest-link') {
      const alias = path.join(f.root, 'manifest-alias');
      symlinkSync(identity.cache_dir, alias, process.platform === 'win32' ? 'junction' : 'dir');
      f.env.WARDIAN_SCCACHE_IDENTITY_MANIFEST = path.join(alias, 'sccache-identity.json');
    }
    denied(f);
    assert.equal(existsSync(wrapperMarker), false);
  }
});

test('workspace and config wrappers remain rejected independently of a sccache carrier', (t) => {
  for (const mode of ['workspace-env', 'cargo-env', 'config', 'inline-config']) {
    const f = fixture(t);
    f.env.CARGO_TARGET_DIR = path.join(f.root, 'disjoint');
    const { wrapper, wrapperMarker } = declaredSccache(f);
    delete f.env.RUSTC_WRAPPER;
    if (mode === 'workspace-env') f.env.RUSTC_WORKSPACE_WRAPPER = wrapper;
    if (mode === 'cargo-env') f.env.CARGO_BUILD_RUSTC_WRAPPER = wrapper;
    if (mode === 'config') {
      mkdirSync(path.join(f.cwd, '.cargo'));
      writeFileSync(path.join(f.cwd, '.cargo', 'config.toml'), `[build]\nrustc-workspace-wrapper = '${wrapper}'\n`);
    }
    denied(f, mode === 'inline-config' ? 'cargo check --config build.rustc-wrapper="sccache"' : undefined);
    assert.equal(existsSync(wrapperMarker), false);
  }
});

test('foreground mode and an explicit daemon endpoint do not make self-declared sccache safe', (t) => {
  const f = fixture(t);
  f.env.CARGO_TARGET_DIR = path.join(f.root, 'disjoint');
  declaredSccache(f);
  Object.assign(f.env, { SCCACHE_NO_DAEMON: '1', SCCACHE_SERVER_PORT: '4227', SCCACHE_ERROR_LOG: f.protectedFile });
  denied(f);
});

test('disjoint ordinary verification still launches after removing the unsupported wrapper', (t) => {
  const f = fixture(t);
  f.env.CARGO_TARGET_DIR = path.join(f.root, 'disjoint');
  const { wrapperMarker } = declaredSccache(f);
  delete f.env.RUSTC_WRAPPER;
  delete f.env.WARDIAN_SCCACHE_IDENTITY_MANIFEST;
  delete f.env.SCCACHE_DIR;
  const result = f.run();
  assert.equal(result.status, 0, result.stdout + result.stderr);
  assert.ok(existsSync(f.marker));
  assert.ok(existsSync(f.trace));
  assert.equal(existsSync(wrapperMarker), false);
  assert.equal(readFileSync(f.protectedFile, 'utf8'), 'immutable input');
});

function nestedRun(f, mode = 'full', metadataTarget = f.env.CARGO_TARGET_DIR) {
  const appDir = path.join(f.cwd, 'src-tauri');
  const lib = path.join(appDir, 'src', 'lib.rs');
  mkdirSync(path.dirname(lib), { recursive: true });
  writeFileSync(lib, 'pub fn fixture() {}\n');
  writeFileSync(path.join(appDir, 'Cargo.toml'), '[package]\nname = "Wardian"\nversion = "0.1.0"\n');
  const metadataFile = path.join(f.root, 'metadata.json');
  writeFileSync(metadataFile, JSON.stringify({ workspace_root: f.cwd, target_directory: metadataTarget }));
  const entry = path.join(f.root, 'nested-dispatch.mjs');
  const loader = path.join(f.root, 'nested-loader.mjs');
  // Async loader hooks work on CI's Node 20. Export private dispatch functions
  // only in this test loader; the TypeScript scanner is not exercised here.
  writeFileSync(loader, `
const sourceUrl = process.env.GUARD_DEADCODE_URL;
export async function resolve(specifier, context, next) {
  if (specifier === 'typescript' && context.parentURL === sourceUrl) return { url: 'data:text/javascript,export default {ScriptKind:{TS:3,TSX:4,JS:1,JSX:2}};', shortCircuit: true };
  return next(specifier, context);
}
export async function load(url, context, next) {
  const loaded = await next(url, context);
  if (url === sourceUrl) return { ...loaded, source: loaded.source.toString() + '\\nexport { cargoMetadata, runRustcPass };\\n' };
  return loaded;
}
`);
  writeFileSync(entry, `import cp from 'node:child_process';
import { readFileSync, writeFileSync } from 'node:fs';
import { syncBuiltinESMExports } from 'node:module';
import path from 'node:path';
const sourceUrl = process.env.GUARD_DEADCODE_URL;
const originalExec = cp.execFileSync;
const trace = [];
function harmless(phase, args, cwd) {
  trace.push({ phase, args, cwd });
  writeFileSync(process.env.GUARD_TRACE, JSON.stringify(trace));
  originalExec(process.execPath, ['-e', 'require("node:fs").writeFileSync(process.env.GUARD_MARKER + "-" + process.argv[1], "harmless child launched")', phase], { env: process.env });
}
cp.execFileSync = (program, args, options) => {
  if (program === 'git') return 'Cargo.toml\\0src-tauri/Cargo.toml\\0src-tauri/src/lib.rs\\0.cargo/config.toml\\0';
  if (program !== 'cargo' || args[0] !== 'metadata') throw new Error('Unexpected exec');
  harmless('metadata', args, options.cwd);
  return readFileSync(process.env.GUARD_METADATA_FILE, 'utf8');
};
cp.spawnSync = (program, args, options) => {
  if (program !== 'cargo' || args[0] !== 'check') throw new Error('Unexpected spawn');
  harmless('check', args, options.cwd);
  if (process.env.GUARD_OUTPUT_PROBE) {
    const output = args[args.indexOf('--target-dir') + 1];
    originalExec(process.execPath, ['-e',
      'const fs=require("node:fs"),p=require("node:path"); const [out,cwd]=JSON.parse(process.argv[1]); fs.mkdirSync(p.join(out,"debug","deps"),{recursive:true}); fs.writeFileSync(p.join(out,"debug","deps","core.rmeta"),cwd); fs.writeFileSync(p.join(out,"debug","deps","core.d"),cwd);',
      JSON.stringify([output, options.cwd])], { env: process.env });
  }
  return { status: 0, stdout: '', stderr: '' };
};
syncBuiltinESMExports();
const { cargoMetadata, runRustcPass } = await import(sourceUrl);
const metadata = process.env.GUARD_NESTED_MODE === 'copy-only'
  ? JSON.parse(readFileSync(process.env.GUARD_METADATA_FILE)) : cargoMetadata();
if (process.env.GUARD_NESTED_MODE !== 'metadata-only') {
  const dir = path.join(process.cwd(), 'src-tauri');
  runRustcPass(metadata, { packages: [{ name: 'Wardian', dir, lib: { name: 'wardian', src_path: path.join(dir, 'src/lib.rs') }, bins: [] }] }, { verbose: false });
}
`);
  const env = { ...f.env, GUARD_DEADCODE_URL: new URL('./verify-rust-deadcode.mjs', import.meta.url).href,
    GUARD_METADATA_FILE: metadataFile, GUARD_NESTED_MODE: mode };
  return spawnSync(process.execPath, ['--loader', pathToFileURL(loader).href, entry], {
    cwd: f.cwd, env, encoding: 'utf8', timeout: 10_000,
  });
}

test('actual Rust dead-code metadata dispatch rejects overlap with zero child launches', (t) => {
  const f = fixture(t);
  const result = nestedRun(f, 'metadata-only');
  assert.equal(result.status, 1, result.stdout + result.stderr);
  assert.match(result.stderr, /protected input overlaps/);
  assert.equal(existsSync(f.trace), false);
  assert.equal(existsSync(f.marker + '-metadata'), false);
  assert.equal(existsSync(path.join(f.target, 'rust-deadcode')), false);
});

test('actual source-copy admission rejects returned metadata overlap before writes or check', (t) => {
  const f = fixture(t);
  f.env.CARGO_TARGET_DIR = path.join(f.root, 'disjoint');
  const result = nestedRun(f, 'copy-only', f.target);
  assert.equal(result.status, 1, result.stdout + result.stderr);
  assert.match(result.stderr, /protected input overlaps/);
  assert.equal(existsSync(f.trace), false);
  assert.equal(existsSync(f.marker + '-check'), false);
  assert.equal(existsSync(path.join(f.target, 'rust-deadcode')), false);
  assert.equal(readFileSync(f.protectedFile, 'utf8'), 'immutable input');
});

test('actual metadata and check dispatches launch harmless children for a known disjoint target', (t) => {
  const f = fixture(t);
  f.env.CARGO_TARGET_DIR = path.join(f.root, 'disjoint');
  const result = nestedRun(f);
  assert.equal(result.status, 0, result.stdout + result.stderr);
  const trace = JSON.parse(readFileSync(f.trace));
  assert.deepEqual(trace.map(({ phase }) => phase), ['metadata', 'check']);
  assert.ok(existsSync(f.marker + '-metadata'));
  assert.ok(existsSync(f.marker + '-check'));
  assert.equal(trace[1].args[trace[1].args.indexOf('--target-dir') + 1], path.join(f.env.CARGO_TARGET_DIR, 'deadcode'));
  assert.equal(path.dirname(path.dirname(trace[1].cwd)), f.env.CARGO_TARGET_DIR);
  assert.ok(existsSync(path.join(trace[1].cwd, 'src-tauri', 'src', 'lib.rs')));
});

test('visibility-copy dispatch preserves normal artifacts and routes copied metadata separately', (t) => {
  const f = fixture(t);
  const target = path.join(f.root, 'disjoint');
  f.env.CARGO_TARGET_DIR = target;
  f.env.GUARD_OUTPUT_PROBE = '1';
  const normalFiles = [
    ['debug/deps/core.rmeta', 'normal workspace metadata'],
    ['debug/deps/core.d', f.cwd],
    ['debug/.fingerprint/core/state', 'normal fingerprint'],
    ['debug/incremental/core/state', 'normal incremental'],
  ];
  for (const [file, text] of normalFiles) {
    mkdirSync(path.dirname(path.join(target, file)), { recursive: true });
    writeFileSync(path.join(target, file), text);
  }
  const result = nestedRun(f);
  assert.equal(result.status, 0, result.stdout + result.stderr);
  const trace = JSON.parse(readFileSync(f.trace));
  const copied = trace.find(({ phase }) => phase === 'check');
  assert.equal(copied.args[copied.args.indexOf('--target-dir') + 1], path.join(target, 'deadcode'));
  assert.equal(readFileSync(path.join(target, 'deadcode', 'debug', 'deps', 'core.rmeta'), 'utf8'), copied.cwd);
  assert.equal(readFileSync(path.join(target, 'deadcode', 'debug', 'deps', 'core.d'), 'utf8'), copied.cwd);
  for (const [file, text] of normalFiles) assert.equal(readFileSync(path.join(target, file), 'utf8'), text);
  // This exercises actual argument dispatch with a harmless compiler stand-in;
  // the real Cargo normal/copy/normal lineage remains a separate proof.
});

test('actual copied-cwd Cargo check resolves a protected separate build tree before dispatch', (t) => {
  const f = fixture(t);
  const outputParent = path.join(f.root, 'output-parent');
  f.env.CARGO_TARGET_DIR = path.join(outputParent, 'disjoint');
  mkdirSync(path.join(outputParent, '.cargo'), { recursive: true });
  writeFileSync(path.join(outputParent, '.cargo', 'config.toml'), `[build]\nbuild-dir = '${f.target}'`);
  const result = nestedRun(f);
  assert.equal(result.status, 1, result.stdout + result.stderr);
  assert.match(result.stderr, /protected input overlaps/);
  assert.deepEqual(JSON.parse(readFileSync(f.trace)).map(({ phase }) => phase), ['metadata']);
  assert.ok(existsSync(f.marker + '-metadata'));
  assert.equal(existsSync(f.marker + '-check'), false);
  assert.equal(readFileSync(f.protectedFile, 'utf8'), 'immutable input');
});

test('actual copied-cwd unknown Cargo config fails before check dispatch', (t) => {
  const f = fixture(t);
  const outputParent = path.join(f.root, 'output-parent');
  f.env.CARGO_TARGET_DIR = path.join(outputParent, 'disjoint');
  mkdirSync(path.join(outputParent, '.cargo'), { recursive: true });
  writeFileSync(path.join(outputParent, '.cargo', 'config.toml'), 'include = ["unknown.toml"]');
  const result = nestedRun(f);
  assert.equal(result.status, 1, result.stdout + result.stderr);
  assert.match(result.stderr, /cannot derive writable roots/);
  assert.deepEqual(JSON.parse(readFileSync(f.trace)).map(({ phase }) => phase), ['metadata']);
  assert.equal(existsSync(f.marker + '-check'), false);
});

test('normal execute rejects an inventory overlap before its spawn hook', (t) => {
  denied(fixture(t));
});

test('normal execute allows a disjoint existing reusable target', (t) => {
  const f = fixture(t);
  f.env.CARGO_TARGET_DIR = path.join(f.root, 'reusable');
  mkdirSync(f.env.CARGO_TARGET_DIR);
  writeFileSync(path.join(f.env.CARGO_TARGET_DIR, 'prior-output.bin'), 'ordinary output');
  const result = f.run();
  assert.equal(result.status, 0, result.stderr);
  assert.equal(readFileSync(f.marker, 'utf8'), 'harmless child launched');
  assert.equal(JSON.parse(readFileSync(f.trace)).command, 'cargo check');
});

test('normal execute allows the same reusable target without an inventory', (t) => {
  const f = fixture(t);
  delete f.env.WARDIAN_PROTECTED_INPUT_MANIFESTS;
  assert.equal(f.run().status, 0);
  assert.ok(existsSync(f.marker));
  assert.ok(existsSync(f.trace));
});

test('ordinary npm verification inherits the scheduler carrier automatically', (t) => {
  for (const mode of ['overlap', 'disjoint', 'no-inventory']) {
    const f = fixture(t);
    writeFileSync(path.join(f.cwd, 'package.json'), JSON.stringify({ scripts: { 'verify:ci': `node "${runner}"` } }));
    const hook = path.join(f.root, 'spawn-hook.mjs');
    f.env.NODE_OPTIONS = `--import=${pathToFileURL(hook).href}`;
    if (mode === 'disjoint') f.env.CARGO_TARGET_DIR = path.join(f.root, 'disjoint');
    if (mode === 'no-inventory') delete f.env.WARDIAN_PROTECTED_INPUT_MANIFESTS;
    const result = spawnSync('npm run verify:ci -- --only backend', {
      cwd: f.cwd, env: f.env, shell: true, encoding: 'utf8', timeout: 10_000,
    });
    assert.equal(result.error, undefined);
    assert.equal(result.status, mode === 'overlap' ? 1 : 0, result.stdout + result.stderr);
    assert.equal(existsSync(f.marker), mode !== 'overlap');
    assert.equal(existsSync(f.trace), mode !== 'overlap');
    if (mode === 'overlap') assert.match(result.stderr, /protected input overlaps/);
  }
});

test('rust-deadcode guards its metadata target parent and admits a disjoint target', (t) => {
  denied(fixture(t), 'npm run check:rust-deadcode');
  const disjoint = fixture(t);
  disjoint.env.CARGO_TARGET_DIR = path.join(disjoint.root, 'disjoint');
  assert.equal(disjoint.run('npm run check:rust-deadcode').status, 0);
  assert.ok(existsSync(disjoint.marker));
  const ordinary = fixture(t);
  delete ordinary.env.WARDIAN_PROTECTED_INPUT_MANIFESTS;
  assert.equal(ordinary.run('npm run check:rust-deadcode').status, 0);
  assert.ok(existsSync(ordinary.marker));
});

test('rust-deadcode metadata config/default derivation guards overlaps and rejects unknowns', (t) => {
  const config = fixture(t);
  delete config.env.CARGO_TARGET_DIR;
  mkdirSync(path.join(config.cwd, '.cargo'));
  writeFileSync(path.join(config.cwd, '.cargo', 'config.toml'), '[build]\ntarget-dir = "target"');
  denied(config, 'npm run check:rust-deadcode');
  const workspaceDefault = fixture(t);
  delete workspaceDefault.env.CARGO_TARGET_DIR;
  denied(workspaceDefault, 'npm run check:rust-deadcode');
  const unknown = fixture(t);
  delete unknown.env.CARGO_TARGET_DIR;
  writeFileSync(path.join(unknown.cwd, 'Cargo.toml'), '[package]\nname = "fixture"\nversion = "0.1.0"\n');
  denied(unknown, 'npm run check:rust-deadcode');
  const separate = fixture(t);
  separate.env.CARGO_TARGET_DIR = path.join(separate.root, 'disjoint');
  separate.env.CARGO_BUILD_BUILD_DIR = separate.target;
  denied(separate, 'npm run check:rust-deadcode');
});

test('pinned Rust dead-code source distinguishes the copied cwd from the compiler target', () => {
  const text = readFileSync(new URL('./verify-rust-deadcode.mjs', import.meta.url), 'utf8');
  assert.match(text, /const copyRoot = prepareCopyRoot\(metadata\.target_directory, hash\)/);
  assert.match(text, /const analysisTarget = path\.join\(metadata\.target_directory, "deadcode"\)/);
  assert.match(text, /"--target-dir",\s*analysisTarget,/);
  assert.match(text, /const invocation = cargoInvocation\(args, \{ cwd: copyRoot, output: "deadcode" \}\)/);
  assert.match(text, /spawnSync\("cargo", invocation\.args, \{\s*cwd: copyRoot,/);
});

test('normal execute rejects a writable ancestor of a protected file', (t) => {
  const f = fixture(t);
  f.env.CARGO_TARGET_DIR = f.root;
  denied(f);
});

test('normal execute uses Windows case-insensitive containment', { skip: process.platform !== 'win32' }, (t) => {
  const f = fixture(t);
  f.env.CARGO_TARGET_DIR = f.target.toUpperCase();
  denied(f);
});

test('normal execute resolves target junction/symlink aliases', (t) => {
  const f = fixture(t);
  const alias = path.join(f.root, 'alias');
  symlinkSync(f.target, alias, process.platform === 'win32' ? 'junction' : 'dir');
  f.env.CARGO_TARGET_DIR = alias;
  denied(f);
});

test('normal execute resolves protected-input junction/symlink aliases', (t) => {
  const f = fixture(t);
  const alias = path.join(f.root, 'input-alias');
  symlinkSync(f.target, alias, process.platform === 'win32' ? 'junction' : 'dir');
  writeFileSync(f.manifest, JSON.stringify({ files: [{ path: path.join(alias, 'debug', 'protected-input.bin'), length: 15 }] }));
  denied(f);
});

test('normal execute resolves an existing aliased ancestor of a nonexistent target', (t) => {
  const f = fixture(t);
  const alias = path.join(f.root, 'parent-alias');
  symlinkSync(f.target, alias, process.platform === 'win32' ? 'junction' : 'dir');
  f.env.CARGO_TARGET_DIR = path.join(alias, 'not-created', 'tree');
  writeFileSync(f.manifest, JSON.stringify([{ path: path.join(f.target, 'not-created', 'tree', 'protected.bin') }]));
  denied(f);
  assert.equal(existsSync(path.join(f.target, 'not-created')), false);
});

test('containment compares path segments, allowing a sibling with a shared prefix', (t) => {
  const f = fixture(t);
  f.env.CARGO_TARGET_DIR = `${f.target}-other`;
  assert.equal(f.run().status, 0);
  assert.ok(existsSync(f.marker));
});

test('unknown config fails before normal execute spawns', (t) => {
  const f = fixture(t);
  mkdirSync(path.join(f.cwd, '.cargo'));
  writeFileSync(path.join(f.cwd, '.cargo', 'config.toml'), 'include = ["other.toml"]');
  denied(f);
});

test('malformed inventory fails before normal execute spawns', (t) => {
  const f = fixture(t);
  f.env.WARDIAN_PROTECTED_INPUT_MANIFESTS = '';
  denied(f);
});

test('CLI target overrides env; relative targets resolve at the compiler cwd', (t) => {
  const f = fixture(t);
  denied(f, `cargo check --target-dir "${f.target}"`);
  const roots = compilerWritableRoots(['check', '--target-dir=disjoint'], f.cwd, f.env);
  const expected = path.join(f.cwd, 'disjoint');
  assert.deepEqual(roots, [process.platform === 'win32' ? expected.toLowerCase() : expected]);
});

test('config hierarchy, legacy config precedence and config-relative roots', (t) => {
  const f = fixture(t);
  delete f.env.CARGO_TARGET_DIR;
  mkdirSync(f.env.CARGO_HOME);
  writeFileSync(path.join(f.env.CARGO_HOME, 'config.toml'), '[build]\ntarget-dir = "home-target"');
  mkdirSync(path.join(f.root, '.cargo'));
  writeFileSync(path.join(f.root, '.cargo', 'config.toml'), '[build]\ntarget-dir = "parent-target"');
  mkdirSync(path.join(f.cwd, '.cargo'));
  writeFileSync(path.join(f.cwd, '.cargo', 'config.toml'), '[build]\ntarget-dir = "disjoint"');
  writeFileSync(path.join(f.cwd, '.cargo', 'config'), '[build]\ntarget-dir = "target"');
  denied(f);
});

test('inline --config and config-file target overrides are guarded', (t) => {
  const f = fixture(t);
  delete f.env.CARGO_TARGET_DIR;
  assert.throws(() => assertCompilerAdmission({ program: 'cargo', args: ['check', '--config', 'build.target-dir="target"'], cwd: f.cwd, env: f.env }), /overlaps/);
  mkdirSync(path.join(f.cwd, '.config'));
  const extra = path.join(f.cwd, '.config', 'extra.toml');
  writeFileSync(extra, '[build]\ntarget-dir = "target"');
  assert.throws(() => assertCompilerAdmission({ program: 'cargo', args: ['check', '--config', extra], cwd: f.cwd, env: f.env }), /overlaps/);
});

test('conflicting special environment and --config target settings fail closed', (t) => {
  const f = fixture(t);
  f.env.CARGO_TARGET_DIR = path.join(f.root, 'disjoint');
  assert.throws(() => assertCompilerAdmission({ program: 'cargo', args: ['check', '--config', 'build.target-dir="target"'], cwd: f.cwd, env: f.env }), /conflicting/);
  f.env.CARGO_BUILD_TARGET_DIR = f.target;
  denied(f);
});

test('a separate build tree must also be disjoint', (t) => {
  const f = fixture(t);
  f.env.CARGO_TARGET_DIR = path.join(f.root, 'disjoint');
  f.env.CARGO_BUILD_BUILD_DIR = f.target;
  denied(f);
});

test('workspace default target is guarded when no explicit root exists', (t) => {
  const f = fixture(t);
  delete f.env.CARGO_TARGET_DIR;
  denied(f);
});

test('a package default that needs Cargo workspace discovery fails before spawn', (t) => {
  const f = fixture(t);
  delete f.env.CARGO_TARGET_DIR;
  writeFileSync(path.join(f.cwd, 'Cargo.toml'), '[package]\nname = "fixture"\nversion = "0.1.0"\n');
  denied(f);
});

test('parent traversal, dangling links and Windows path ambiguities fail before spawn', (t) => {
  const f = fixture(t);
  f.env.CARGO_TARGET_DIR = path.join(f.root, 'alias') + '/../workspace/target';
  denied(f);
  const dangling = path.join(f.root, 'dangling');
  symlinkSync(path.join(f.root, 'absent'), dangling, process.platform === 'win32' ? 'junction' : 'dir');
  f.env.CARGO_TARGET_DIR = path.join(dangling, 'target');
  assert.throws(() => assertCompilerAdmission({ program: 'cargo', args: ['check'], cwd: f.cwd, env: f.env }));
  if (process.platform === 'win32') {
    for (const ambiguous of ['C:', 'C:target', f.target + '.', f.target + ':stream']) {
      f.env.CARGO_TARGET_DIR = ambiguous;
      denied(f);
    }
  }
});

test('a config-selected separate build directory is guarded', (t) => {
  const f = fixture(t);
  f.env.CARGO_TARGET_DIR = path.join(f.root, 'disjoint');
  mkdirSync(path.join(f.cwd, '.cargo'));
  writeFileSync(path.join(f.cwd, '.cargo', 'config.toml'), '[build]\nbuild-dir = "target"');
  denied(f);
});

test('the generic config target environment resolves against cwd', (t) => {
  const f = fixture(t);
  delete f.env.CARGO_TARGET_DIR;
  f.env.CARGO_BUILD_TARGET_DIR = 'target';
  denied(f);
});

test('CLI target selection permits disjoint reuse even with a protected env target', (t) => {
  const f = fixture(t);
  const result = f.run('cargo check --target-dir disjoint');
  assert.equal(result.status, 0, result.stderr);
  assert.ok(existsSync(f.marker));
});

test('ambiguous placement options, environment and shell commands deny before spawn', (t) => {
  const cases = [
    ['cargo check -Z unstable-options', {}],
    ['cargo check --artifact-dir elsewhere', {}],
    ['cargo check -- --out-dir elsewhere', {}],
    ['cargo clippy -- -C incremental=elsewhere', {}],
    ['cargo check && cargo build', {}],
    ['cargo check', { RUSTFLAGS: '-Cincremental=elsewhere' }],
    ['cargo check', { CARGO_BUILD_BUILD_DIR: '{workspace-root}/elsewhere' }],
  ];
  for (const [command, extra] of cases) {
    const f = fixture(t);
    Object.assign(f.env, extra);
    denied(f, command);
  }
});

test('shared wrapper CLI accepts actual toolchain arguments and rejects overlap', (t) => {
  const f = fixture(t);
  const args = [cli, '--cwd', f.cwd, '--program', 'cargo.exe', '--', '+1.99.0', 'clippy', '--locked', '--offline', '--workspace', '--all-targets', '--', '-D', 'warnings'];
  const rejected = spawnSync(process.execPath, args, { env: f.env, encoding: 'utf8' });
  assert.equal(rejected.status, 1);
  assert.match(rejected.stderr, /overlaps/);
  f.env.CARGO_TARGET_DIR = path.join(f.root, 'disjoint');
  const admitted = spawnSync(process.execPath, args, { env: f.env, encoding: 'utf8' });
  assert.equal(admitted.status, 0, admitted.stderr);
  assert.equal(JSON.parse(admitted.stdout).guarded, true);
  assert.equal(existsSync(f.marker), false, 'guard CLI itself never launches a compiler');
});
