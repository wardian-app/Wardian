import { spawn } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { createInterface } from 'node:readline';
import { pathToFileURL } from 'node:url';
import { assertCompilerAdmission } from './compiler-input-guard.mjs';
import { cargoInvocation } from './rust-build-cache.mjs';

const BUDGET_PATH = resolve('scripts/rust-test-time-budget.json');
const BINARY_START = /^running \d+ tests?$/;
const RESULT = /^test (\S+) \.\.\. (ok|FAILED|ignored)/;
// CI sets CARGO_TERM_COLOR=always.
const ANSI = /\x1b\[[0-9;]*m/g;

/**
 * Time each test from serial `cargo test` stdout. With one test thread, libtest
 * prints a result line as each test finishes, so the gap since the previous
 * line of the same binary run (or its `running N tests` line) is that test's
 * duration. Tests are keyed by their path alone: Cargo's binary banners go to
 * stderr and cannot be ordered against stdout reliably. When two binaries share
 * a test path, the slower sample counts, which can only tighten the budget.
 */
export class TestTimer {
  constructor(now = () => performance.now()) {
    this.now = now;
    this.mark = null;
    this.durations = new Map();
  }

  stdoutLine(raw) {
    const line = raw.replace(ANSI, '');
    if (BINARY_START.test(line.trim())) {
      this.mark = this.now();
      return;
    }
    const result = line.match(RESULT);
    if (!result || this.mark === null) return;
    const at = this.now();
    if (result[2] !== 'ignored') {
      const seconds = (at - this.mark) / 1000;
      this.durations.set(result[1], Math.max(seconds, this.durations.get(result[1]) ?? 0));
    }
    this.mark = at;
  }
}

/** Return tests over budget, slowest first. Allowlisted tests use their own ceiling. */
export function overBudget(durations, budget) {
  const over = [];
  for (const [name, seconds] of durations) {
    const limit = budget.allow?.[name] ?? budget.default_seconds;
    if (seconds > limit) over.push({ name, seconds, limit });
  }
  return over.sort((a, b) => b.seconds - a.seconds);
}

/**
 * Hosted runners stall: one Windows run measured a 0.4 s test at 9 s. A test
 * over budget is re-timed alone, and only a test that is over again (or that
 * the retry did not run) fails.
 */
export function confirmOverBudget(over, retried, budget) {
  return overBudget(new Map(over.map(({ name, seconds }) => [name, retried.get(name) ?? seconds])), budget);
}

/** Re-run only the named tests, exactly, keeping the original cargo selection. */
export function retryArgs(args, names) {
  const separator = args.indexOf('--');
  return [...args.slice(0, separator), '--', '--exact', '--test-threads=1', ...names];
}

export function requireSerial(args) {
  const separator = args.indexOf('--');
  const testArgs = separator < 0 ? [] : args.slice(separator + 1);
  if (!testArgs.includes('--test-threads=1')) {
    throw new Error('rust-test-time-budget needs `-- --test-threads=1`; parallel output cannot be timed per test');
  }
}

function invocation(args) {
  if (process.env.WARDIAN_RUST_CACHE_TARGET) return cargoInvocation(args);
  assertCompilerAdmission({ program: 'cargo', args, cwd: process.cwd(), env: process.env });
  return { program: 'cargo', args, cwd: process.cwd(), env: process.env };
}

function report(durations, over, enforce) {
  const slowest = [...durations].sort((a, b) => b[1] - a[1]).slice(0, 15);
  const total = [...durations.values()].reduce((sum, value) => sum + value, 0);
  console.log(`\nRust test time: ${durations.size} tests, ${total.toFixed(0)}s. Slowest:`);
  for (const [name, seconds] of slowest) console.log(`  ${seconds.toFixed(1).padStart(7)}s  ${name}`);
  if (over.length === 0) return;
  const label = enforce ? 'FAILED' : 'warning';
  console.error(`\nRust test time budget ${label}: ${over.length} test(s) over budget (scripts/rust-test-time-budget.json).`);
  for (const { name, seconds, limit } of over) {
    console.error(`  ${seconds.toFixed(1)}s > ${limit}s  ${name}`);
  }
  console.error('Make the test cheaper (smaller fixture, no real waits) or delete it if another test already pins the behaviour.');
}

export async function main(argv = process.argv.slice(2)) {
  if (argv[0] !== 'cargo' || argv[1] !== 'test') throw new Error('Usage: node scripts/rust-test-time-budget.mjs cargo test ... -- --test-threads=1');
  const args = argv.slice(1);
  requireSerial(args);
  const budget = JSON.parse(readFileSync(BUDGET_PATH, 'utf8'));
  const { status, durations } = await timedRun(args);
  // Timing depends on the machine, so local runs only warn. CI enforces.
  const enforce = process.env.CI === 'true' || process.env.WARDIAN_TEST_TIME_BUDGET === 'enforce';
  let over = overBudget(durations, budget);
  if (status === 0 && enforce && over.length > 0) {
    console.log(`\nRe-timing ${over.length} test(s) over budget alone to rule out a runner stall.`);
    const retry = await timedRun(retryArgs(args, over.map(({ name }) => name)));
    if (retry.status !== 0) return retry.status;
    over = confirmOverBudget(over, retry.durations, budget);
  }
  report(durations, over, enforce);
  if (status !== 0) return status;
  return enforce && over.length > 0 ? 1 : 0;
}

async function timedRun(args) {
  const run = invocation(args);
  const timer = new TestTimer();
  const child = spawn(run.program, run.args, { cwd: run.cwd, env: run.env, stdio: ['inherit', 'pipe', 'pipe'], windowsHide: true });
  createInterface({ input: child.stdout }).on('line', (line) => {
    process.stdout.write(`${line}\n`);
    timer.stdoutLine(line);
  });
  createInterface({ input: child.stderr }).on('line', (line) => process.stderr.write(`${line}\n`));
  const status = await new Promise((done, fail) => {
    child.on('error', fail);
    child.on('close', (code) => done(code ?? 1));
  });
  return { status, durations: timer.durations };
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  main().then((code) => { process.exitCode = code; }, (error) => {
    console.error(error instanceof Error ? error.message : error);
    process.exitCode = 1;
  });
}
