import { describe, expect, it } from 'vitest';
import { assertDependencyParity, assertLintPolicyUnchanged, changedLintPolicyFiles, compareMetrics, packageContract } from '../scripts/verify-budgets.mjs';

describe('base-relative debt budget comparison', () => {
  it('reports only metrics that increased over the base', () => {
    expect(compareMetrics(
      { file_lines: { 'a.rs': 12 }, ignored_rust_tests: 4, eslint_warnings: 1 },
      { file_lines: { 'a.rs': 10 }, ignored_rust_tests: 4, eslint_warnings: 2 },
    )).toEqual({
      over: [['a.rs lines', 12, 10]],
      under: [['eslint_warnings', 1, 2]],
    });
  });

  it('allows equal metrics', () => {
    expect(compareMetrics(
      { file_lines: { 'a.rs': 10 }, ignored_rust_tests: 4 },
      { file_lines: { 'a.rs': 10 }, ignored_rust_tests: 4 },
    )).toEqual({ over: [], under: [] });
  });

  it('rejects a head lint policy that could hide newly added warnings', () => {
    expect(changedLintPolicyFiles(['src/new-warning.ts', 'eslint.config.js'])).toEqual(['eslint.config.js']);
    expect(() => assertLintPolicyUnchanged(['src/new-warning.ts', 'eslint.config.js']))
      .toThrow('changes lint policy or dependencies');
  });

  describe('package.json contract comparison', () => {
    const base = JSON.stringify({
      name: 'wardian',
      scripts: { test: 'vitest run' },
      devDependencies: { eslint: '^9.0.0' },
    });
    const securityManifest = (
      braceExpansion: unknown,
      undici: unknown,
      additionalOverrides: Record<string, unknown> = {},
    ) => JSON.stringify({
      devDependencies: { eslint: '^10.9.1', 'typescript-eslint': '^8.68.0' },
      overrides: {
        dompurify: '3.4.13',
        esbuild: '0.28.1',
        'minimatch@10.2.5': { 'brace-expansion': braceExpansion },
        undici,
        vite: '6.4.3',
        ws: '8.21.0',
        ...additionalOverrides,
      },
    });
    const securityBase = securityManifest('5.0.9', '7.29.0');
    const securityHead = securityManifest('5.0.12', '7.29.1');

    it('allows a manifest change that only adds a script', () => {
      const head = JSON.stringify({
        name: 'wardian',
        scripts: { test: 'vitest run', 'site:media': 'node scripts/capture-site-media.mjs' },
        devDependencies: { eslint: '^9.0.0' },
      });
      expect(changedLintPolicyFiles(['package.json'], { base, head })).toEqual([]);
      expect(() => assertLintPolicyUnchanged(['package.json'], { base, head })).not.toThrow();
    });

    it('still rejects a dependency change', () => {
      const head = JSON.stringify({
        name: 'wardian',
        scripts: { test: 'vitest run' },
        devDependencies: { eslint: '^8.0.0' },
      });
      expect(changedLintPolicyFiles(['package.json'], { base, head })).toEqual(['package.json']);
    });

    it('still rejects an embedded eslint configuration change', () => {
      const head = JSON.stringify({
        name: 'wardian',
        scripts: { test: 'vitest run' },
        devDependencies: { eslint: '^9.0.0' },
        eslintConfig: { rules: { 'no-console': 'off' } },
      });
      expect(changedLintPolicyFiles(['package.json'], { base, head })).toEqual(['package.json']);
    });

    it('rejects when a manifest cannot be parsed, rather than allowing it', () => {
      expect(changedLintPolicyFiles(['package.json'], { base, head: '{ not json' })).toEqual(['package.json']);
    });

    it('rejects when manifest contents were not supplied', () => {
      expect(changedLintPolicyFiles(['package.json'])).toEqual(['package.json']);
    });

    it('allows the exact audited security pins in both package comparisons', () => {
      expect(changedLintPolicyFiles(['package.json'], { base: securityBase, head: securityHead })).toEqual([]);
      expect(() => assertLintPolicyUnchanged(['package.json'], { base: securityBase, head: securityHead })).not.toThrow();
      expect(() => assertDependencyParity(securityBase, securityHead)).not.toThrow();
    });

    const rejectedSecurityTransitions = [
      ['an unapproved patch pin', securityManifest('5.0.11', '7.29.1')],
      ['a new override path', securityManifest('5.0.12', '7.29.1', { 'brace-expansion': '5.0.12' })],
      ['a major-version jump', securityManifest('5.0.12', '8.0.0')],
      ['a malformed pin value', securityManifest({ from: '5.0.9', to: '5.0.12' }, '7.29.1')],
    ] as const;

    for (const [description, head] of rejectedSecurityTransitions) {
      it(`rejects ${description} in both package comparisons`, () => {
        expect(changedLintPolicyFiles(['package.json'], { base: securityBase, head })).toEqual(['package.json']);
        expect(() => assertDependencyParity(securityBase, head))
          .toThrow('Debt budget gate cannot resolve base dependencies');
      });
    }

    it('rejects an unexpected starting pin and malformed manifest in both comparisons', () => {
      const unexpectedBase = securityManifest('5.0.8', '7.29.0');
      expect(changedLintPolicyFiles(['package.json'], { base: unexpectedBase, head: securityHead })).toEqual(['package.json']);
      expect(() => assertDependencyParity(unexpectedBase, securityHead))
        .toThrow('Debt budget gate cannot resolve base dependencies');
      expect(changedLintPolicyFiles(['package.json'], { base: securityBase, head: '{ not json' })).toEqual(['package.json']);
      expect(() => assertDependencyParity(securityBase, '{ not json'))
        .toThrow('Debt budget gate cannot resolve base dependencies');
    });

    it('reduces a manifest to its contract fields only', () => {
      expect(packageContract(base)).toBe(
        packageContract(JSON.stringify({ scripts: { anything: 'else' }, devDependencies: { eslint: '^9.0.0' } })),
      );
    });
  });
});
