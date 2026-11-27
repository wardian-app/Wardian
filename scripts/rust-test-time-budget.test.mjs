import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import { TestTimer, overBudget, requireSerial } from './rust-test-time-budget.mjs';

function clock(times) {
  let index = 0;
  return () => times[index++];
}

test('times each serial test from the previous result line in its binary run', () => {
  const timer = new TestTimer(clock([0, 1500, 1600, 10_000, 10_250]));
  timer.stdoutLine('running 3 tests');
  timer.stdoutLine('\x1b[32mtest slow::case ... ok\x1b[0m');
  timer.stdoutLine('test skipped::case ... ignored');
  timer.stdoutLine('running 1 test');
  timer.stdoutLine('test fast::case ... FAILED');
  assert.deepEqual([...timer.durations], [['slow::case', 1.5], ['fast::case', 0.25]]);
});

test('a harness-free target between suites does not shift timing', () => {
  // Cargo prints a banner (stderr) for a harness=false target, which prints no
  // libtest lines. Only stdout drives timing, so the next suite is unaffected.
  const timer = new TestTimer(clock([0, 1000, 50_000, 62_001]));
  timer.stdoutLine('running 1 test');
  timer.stdoutLine('test first ... ok');
  timer.stdoutLine('holder inactive');
  timer.stdoutLine('running 1 test');
  timer.stdoutLine('test after_holder ... ok');
  assert.deepEqual([...timer.durations], [['first', 1], ['after_holder', 12.001]]);
});

test('the slower sample wins when two binaries share a test path', () => {
  const timer = new TestTimer(clock([0, 5000, 6000, 7000]));
  timer.stdoutLine('running 1 test');
  timer.stdoutLine('test tests::same ... ok');
  timer.stdoutLine('running 1 test');
  timer.stdoutLine('test tests::same ... ok');
  assert.deepEqual([...timer.durations], [['tests::same', 5]]);
});

test('compile time before a binary starts is not charged to its first test', () => {
  const timer = new TestTimer(clock([600_000, 600_100]));
  timer.stdoutLine('running 1 test');
  timer.stdoutLine('test first ... ok');
  assert.equal(timer.durations.get('first'), 0.1);
});

test('allowlisted tests use their own ceiling and others the default', () => {
  const durations = new Map([['a::slow', 40], ['a::listed', 40], ['a::fast', 1]]);
  const over = overBudget(durations, { default_seconds: 10, allow: { 'a::listed': 60 } });
  assert.deepEqual(over, [{ name: 'a::slow', seconds: 40, limit: 10 }]);
});

test('parallel test runs are rejected because they cannot be timed per test', () => {
  assert.throws(() => requireSerial(['test', '--workspace']), /test-threads=1/);
  assert.doesNotThrow(() => requireSerial(['test', '--workspace', '--', '--test-threads=1']));
});

test('the checked-in budget is well formed', () => {
  const budget = JSON.parse(readFileSync('scripts/rust-test-time-budget.json', 'utf8'));
  assert.equal(typeof budget.default_seconds, 'number');
  for (const [name, seconds] of Object.entries(budget.allow)) {
    assert.match(name, /^\w+(::\w+)*$/);
    assert.ok(seconds > budget.default_seconds, `${name} needs no allowlist entry at or below the default`);
  }
});
