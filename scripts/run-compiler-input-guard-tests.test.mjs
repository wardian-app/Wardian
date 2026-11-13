import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, mkdirSync, readFileSync, realpathSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { runCompilerInputGuardTests } from './run-compiler-input-guard-tests.mjs';

function observe(result, throws = false) {
  const output = [];
  const errors = [];
  const calls = [];
  const exit = runCompilerInputGuardTests({
    spawn(...args) {
      calls.push(args);
      if (throws) throw result;
      return result;
    },
    stdout: { write: (chunk) => output.push(chunk) },
    stderr: { write: (chunk) => errors.push(chunk) },
  });
  const context = JSON.parse(errors.at(-1).slice('COMPILER_GUARD_SUITE_RESULT '.length));
  return { exit, output, errors, calls, context };
}

test('whole-suite execution keeps the existing command and thirty-second boundary', () => {
  const observed = observe({ status: 0, signal: null, pid: 42, stdout: 'all output\n', stderr: 'diagnostic\n' });
  assert.equal(observed.exit, 0);
  assert.deepEqual(observed.calls, [[process.execPath, ['--test', path.resolve('scripts/compiler-input-guard.test.mjs')], {
    cwd: process.cwd(), encoding: 'utf8', timeout: 30_000,
  }]]);
  assert.deepEqual(observed.output, ['all output\n']);
  assert.equal(observed.errors[0], 'diagnostic\n');
  assert.equal(observed.context.pid, 42);
  assert.equal(observed.context.error, null);
  assert.ok(observed.context.elapsed_ms >= 0);
});

test('timeout preserves partial streams and original system fields without replay', () => {
  const error = Object.assign(new Error('controlled timeout'), { code: 'ETIMEDOUT', errno: -4039, syscall: 'spawnSync node' });
  const observed = observe({ error, status: null, signal: 'SIGTERM', pid: 43, stdout: 'partial ✔\n', stderr: 'child detail\n' });
  assert.equal(observed.exit, 1);
  assert.equal(observed.calls.length, 1);
  assert.deepEqual(observed.output, ['partial ✔\n']);
  assert.equal(observed.errors[0], 'child detail\n');
  assert.equal(observed.context.error.code, 'ETIMEDOUT');
  assert.equal(observed.context.error.errno, -4039);
  assert.equal(observed.context.error.message, error.message);
  assert.equal(observed.context.signal, 'SIGTERM');
  assert.equal(observed.context.stdout_bytes, Buffer.byteLength('partial ✔\n'));
});

for (const [status, expected] of [[1, 1], [17, 17], [null, 1], [-1, 1], [256, 1]]) {
  test(`nonzero or missing child status ${status} cannot become success`, () => {
    const observed = observe({ status, signal: null, stdout: 'partial\n', stderr: '' });
    assert.equal(observed.exit, expected);
    assert.equal(observed.calls.length, 1);
    assert.deepEqual(observed.output, ['partial\n']);
  });
}

test('a signal or execution error cannot be hidden by a zero status', () => {
  assert.equal(observe({ status: 0, signal: 'SIGABRT', stdout: '', stderr: '' }).exit, 1);
  assert.equal(observe({ error: new Error('failed spawn'), status: 0, signal: null, stdout: '', stderr: '' }).exit, 1);
});

test('a thrown execution error is recorded once and fails the gate', () => {
  const observed = observe(Object.assign(new Error('controlled access error'), { code: 'EACCES' }), true);
  assert.equal(observed.exit, 1);
  assert.equal(observed.calls.length, 1);
  assert.equal(observed.context.error.code, 'EACCES');
});

test('normal npm test requires the bounded gate before Vitest and retains existing Node contracts', () => {
  const pkg = JSON.parse(readFileSync(new URL('../package.json', import.meta.url), 'utf8'));
  const steps = pkg.scripts.test.split(' && ');
  assert.equal(steps.length, 3);
  assert.ok(steps[0].startsWith('node --test '));
  for (const file of ['scripts/rust-build-cache.test.mjs', 'scripts/claude-turn-ended-mod.test.mjs', 'scripts/run-compiler-input-guard-tests.test.mjs',
    'e2e-native/tests/chat-saved-fixture.test.mjs', 'e2e-native/tests/harness-invoke.test.mjs']) {
    assert.ok(steps[0].split(' ').includes(file));
  }
  assert.equal(steps[1], 'node scripts/run-compiler-input-guard-tests.mjs');
  assert.equal(steps[2], 'vitest run');
});

test('CLI retains a real failing child output and executes that child only once', () => {
  const parent = realpathSync(tmpdir());
  const fixture = mkdtempSync(path.join(parent, 'guard-runner-contract-'));
  try {
    mkdirSync(path.join(fixture, 'scripts'));
    writeFileSync(path.join(fixture, 'scripts/compiler-input-guard.test.mjs'), `
import assert from 'node:assert/strict';
import { appendFileSync } from 'node:fs';
import test from 'node:test';
appendFileSync(new URL('../executions.txt', import.meta.url), 'x');
console.log('retained child stdout');
console.error('retained child stderr');
test('controlled child failure', () => assert.fail('controlled failure'));
`);
    // This controlled child fails immediately; its contract budget leaves the
    // production suite's independent thirty-second bound unchanged.
    // Model an ordinary CLI launch: Node's internal test-worker marker makes a
    // nested --test command silently skip its files as a recursive test run.
    const env = { ...process.env };
    delete env.NODE_TEST_CONTEXT;
    const result = spawnSync(process.execPath, [fileURLToPath(new URL('./run-compiler-input-guard-tests.mjs', import.meta.url))], {
      cwd: fixture, encoding: 'utf8', timeout: 10_000, env,
    });
    assert.equal(result.error, undefined, result.stdout + result.stderr);
    assert.equal(result.status, 1, JSON.stringify({ stdout: result.stdout, stderr: result.stderr, signal: result.signal }));
    assert.match(result.stdout + result.stderr, /retained child stdout/);
    assert.match(result.stdout + result.stderr, /retained child stderr/);
    assert.match(result.stderr, /COMPILER_GUARD_SUITE_RESULT/);
    assert.equal(readFileSync(path.join(fixture, 'executions.txt'), 'utf8'), 'x');
  } finally {
    assert.equal(path.dirname(realpathSync(fixture)), parent);
    assert.ok(path.basename(fixture).startsWith('guard-runner-contract-'));
    rmSync(fixture, { recursive: true, force: true });
  }
});
