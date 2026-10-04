import { describe, expect, it } from 'vitest';
import { spawnSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { parseArgs, readVerificationPlan, selectPlan } from '../scripts/verify-ci.mjs';

const workflow = readFileSync('.github/workflows/ci.yml', 'utf8');

describe('compiler protected-input admission', () => {
  it('proves the normal execute boundary and shared wrapper entry with harmless children', () => {
    const result = spawnSync(process.execPath, ['--test', resolve('scripts/compiler-input-guard.test.mjs')], {
      cwd: process.cwd(), encoding: 'utf8', timeout: 30_000,
    });
    expect(result.error).toBeUndefined();
    expect(result.status, result.stdout + result.stderr).toBe(0);
  }, 35_000);
});

describe('verification arguments', () => {
  it('defaults to the full plan only when no category option is supplied', () => {
    expect(parseArgs([])).toEqual({ list: false, only: null });
    expect(parseArgs(['--list'])).toEqual({ list: true, only: null });
  });

  it.each(['frontend', 'backend', 'docs'])('accepts both option forms for %s', (category) => {
    expect(parseArgs(['--only', category, '--list'])).toEqual({ list: true, only: category });
    expect(parseArgs(['--list', `--only=${category}`])).toEqual({ list: true, only: category });
  });

  it.each([
    { label: 'missing value', args: ['--only'] },
    { label: 'empty separate value', args: ['--only', ''] },
    { label: 'empty attached value', args: ['--only='] },
    { label: 'whitespace value', args: ['--only', ' '] },
    { label: 'option used as value', args: ['--only', '--list'] },
    { label: 'unknown category', args: ['--only=other'] },
    { label: 'repeated separate option', args: ['--only', 'frontend', '--only', 'frontend'] },
    { label: 'repeated attached option', args: ['--only=frontend', '--only=backend'] },
    { label: 'mixed separate and attached options', args: ['--only', 'frontend', '--only=docs'] },
    { label: 'mixed attached and separate options', args: ['--only=frontend', '--only', 'docs'] },
  ])('rejects $label', ({ args }) => {
    expect(() => parseArgs(args)).toThrow(/--only/);
  });

  it('rejects unknown options', () => {
    expect(() => parseArgs(['--unknown'])).toThrow(/unknown argument/);
  });
});

describe('literal workflow parser', () => {
  it('preserves shell literals, ordering, line numbers, and CRLF input', () => {
    const command = 'node -e "console.log(1 > 0)" | another-command';
    expect(readVerificationPlan(`steps:\r\n  # local-verify: frontend\r\n  run: ${command}  \r\n`))
      .toEqual([{ category: 'frontend', command, workflowLine: 3 }]);
  });

  it.each(['|', '>'])('rejects every %s block header, including indentation/chomping indicators', (style) => {
    const headers = ['', '-', '+'];
    for (let indent = 1; indent <= 9; indent += 1) {
      headers.push(`${indent}`, `-${indent}`, `+${indent}`, `${indent}-`, `${indent}+`);
    }
    for (const header of headers) {
      for (const suffix of ['', '  ', ' # multiline shell']) {
        const fixture = `  # local-verify: backend\n  run: ${style}${header}${suffix}\n    cargo check\n`;
        expect(() => readVerificationPlan(fixture), fixture).toThrow(/single-line run step/);
      }
    }
  });

  it.each([
    '  # local-verify: frontend\n    run: npm run test',
    '    # local-verify: frontend\n  run: npm run test',
    '  # local-verify: frontend\n\n  run: npm run test',
    '  # local-verify: frontend\n  # another comment\n  run: npm run test',
    '  # local-verify: frontend\n  run:   ',
    '  # local-verify: frontend',
  ])('rejects a marker without an adjacent run at the same indentation: %s', (fixture) => {
    expect(() => readVerificationPlan(fixture)).toThrow(/single-line run step/);
  });

  it('rejects unknown categories and unmarked workflows', () => {
    expect(() => readVerificationPlan('# local-verify: other\nrun: echo test')).toThrow(/unknown local-verify category/);
    expect(() => readVerificationPlan('run: echo test')).toThrow(/no local-verify steps/);
  });
});

describe('verification CLI listing', () => {
  it.each([
    { label: 'missing value', args: ['--list', '--only'] },
    { label: 'empty separate value', args: ['--list', '--only', ''] },
    { label: 'empty attached value', args: ['--list', '--only='] },
    { label: 'duplicate category', args: ['--list', '--only=frontend', '--only=docs'] },
  ])('fails closed for $label without listing or executing the full plan', ({ args }) => {
    const result = spawnSync(process.execPath, [resolve('scripts/verify-ci.mjs'), ...args], {
      cwd: process.cwd(), encoding: 'utf8', timeout: 10_000,
    });
    expect(result.error).toBeUndefined();
    expect(result.status).toBe(1);
    expect(result.stdout).toBe('');
    expect(result.stderr).toContain('--only');
  });

  it('lists the literal selected plan from the repository root without executing it', () => {
    const result = spawnSync(process.execPath, [resolve('scripts/verify-ci.mjs'), '--list', '--only', 'backend'], {
      cwd: process.cwd(), encoding: 'utf8', timeout: 10_000,
    });
    expect(result.error).toBeUndefined();
    expect(result.status).toBe(0);
    expect(result.stderr).toBe('');
    expect(result.stdout.trim().split(/\r?\n/)).toEqual(
      selectPlan(readVerificationPlan(workflow), 'backend').map(({ category, command }) => `${category}\t${command}`),
    );
  });
});

describe('CI verification contract', () => {
  it('derives the exact core CI commands in workflow order', () => {
    expect(readVerificationPlan(workflow).map(({ command }) => command)).toEqual([
      'npm run typecheck',
      'npm run lint',
      'npm run test',
      'npm run build',
      'npm run check:workbench-cutover',
      'npm run check:test-reachability',
      'npm run check:deadcode',
      'npm run check:deadcode:production',
      'npm run check:budgets',
      'npm run check:page-fixtures',
      'cargo clippy --workspace --all-targets -- -D warnings',
      'cargo fmt --all -- --check',
      'cargo test --workspace --all-targets -- --test-threads=1',
      'cargo test --workspace --doc -- --test-threads=1',
      'cargo check --workspace',
      'npm run check:rust-deadcode',
      'npm run docs:check-llms',
      'npm run docs:build',
    ]);
  });

  it('supports a category subset without changing command literals', () => {
    expect(selectPlan(readVerificationPlan(workflow), 'backend').map(({ command }) => command)).toEqual([
      'cargo clippy --workspace --all-targets -- -D warnings',
      'cargo fmt --all -- --check',
      'cargo test --workspace --all-targets -- --test-threads=1',
      'cargo test --workspace --doc -- --test-threads=1',
      'cargo check --workspace',
      'npm run check:rust-deadcode',
    ]);
  });
});
