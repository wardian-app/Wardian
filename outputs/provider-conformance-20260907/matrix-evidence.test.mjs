import test from 'node:test';
import assert from 'node:assert/strict';
import { selectFunctionEvidence, selectMatrixCellEvidence, importCodexBuild13Messaging,
  CODEX_BUILD13, CODEX_MESSAGING_FUNCTIONS, normalizeFunctionEvidence } from './matrix-evidence.mjs';

test('a normal resume pass cannot erase a failed resume after New Session', () => {
  const special = { date: '2026-09-07T17:59:00Z', coverage_group: 'after-new-session', status: 'fail' };
  const normal = { date: '2026-09-07T18:38:00Z', status: 'pass' };
  assert.equal(selectFunctionEvidence([special, normal]), special);
  const fixed = { ...special, date: '2026-09-07T19:00:00Z', status: 'pass' };
  assert.equal(selectFunctionEvidence([special, normal, fixed]), fixed);
});

test('same-run failure wins and every required scenario must use a qualified oracle', () => {
  const rows = [{ date: '2026-09-07', status: 'fail' }, { date: '2026-09-07', status: 'pass' }];
  assert.equal(selectFunctionEvidence(rows).status, 'fail');
  const special = { date: '2026-09-07', coverage_group: 'after-new-session', status: 'pass', historical: true };
  const current = { date: '2026-09-08', status: 'pass' };
  assert.equal(selectFunctionEvidence([special, current], row => row.historical ? { ...row, status: 'untested' } : row).status, 'untested');
  assert.equal(special.status, 'pass', 'qualification must preserve the original ledger');
});

test('five new messaging cells never qualify legacy native delivery, secret recall or recovery', () => {
  const legacy = ['native_delivery', 'native_continuity', 'secret_recall', 'cancellation'].map(id => ({
    provider: 'codex', function: id, status: 'untested', date: '2026-09-07',
  }));
  const messaging = CODEX_MESSAGING_FUNCTIONS.map(row => ({ provider: 'codex', function: row.id,
    status: 'pass', date: '2026-09-08', coverage_group: 'codex-v2-build13' }));
  const all = [...legacy, ...messaging];
  for (const row of legacy) assert.equal(selectMatrixCellEvidence(all, 'codex', row.function), row);
  for (const row of messaging) {
    assert.equal(selectMatrixCellEvidence(all, 'codex', row.function), row);
    assert.equal(selectMatrixCellEvidence(all, 'claude', row.function), undefined);
  }
  assert.equal(selectMatrixCellEvidence(messaging, 'codex', 'native_continuity'), undefined);
});

test('build13 importer rejects unpinned reports and retains the pre-followup revision', () => {
  assert.equal(CODEX_BUILD13.sourceCommit, '06be93d983bbb239b971b3c192158512596557b6');
  assert.equal(CODEX_BUILD13.manifestSha256, '53a33a2e3d0bb0e71ce1cd659b33a0fb59ef9c29ead945ffa131caa323493305');
  assert.throws(() => importCodexBuild13Messaging({ reportBytes: Buffer.from('{"status":"pass"}'), manifestBytes: Buffer.from('{}') }), /Unrecognized build13 report SHA/);
});

const scenarioPairs = {
  chat_log_link: ['chat/current-log-link', 'chat/fresh-session-log-link'],
  user_provenance: ['chat/user-prompt-provenance', 'chat/tool-result-provenance'],
};
const observation = (functionId, scenario, status, date = '2026-09-09T10:00:00Z', extra = {}) => ({
  provider: 'antigravity', function: functionId, case: scenario, status, date, ...extra,
});

test('known grouping is stable, nonmutating and preserves explicit groups', () => {
  for (const [fn, scenarios] of Object.entries(scenarioPairs)) {
    for (const scenario of scenarios) {
      const row = Object.freeze(observation(fn, scenario, 'pass'));
      const normalized = normalizeFunctionEvidence(row);
      assert.equal(normalized.coverage_group, scenario);
      assert.equal(normalizeFunctionEvidence(normalized), normalized);
      assert.equal(row.coverage_group, undefined);
      const explicit = { ...row, coverage_group: 'retained-explicit-group' };
      assert.equal(normalizeFunctionEvidence(explicit), explicit);
    }
  }
  const unrelated = observation('native_delivery', 'chat/current-log-link', 'pass');
  assert.equal(normalizeFunctionEvidence(unrelated), unrelated);
});

test('later current-link or human-prompt pass cannot clear the other scenario failure', () => {
  for (const [fn, [first, second]] of Object.entries(scenarioPairs)) {
    const failed = observation(fn, second, 'fail');
    const partial = observation(fn, first, 'pass', '2026-09-09T11:00:00Z');
    for (const rows of [[failed, partial], [partial, failed]]) {
      const result = selectFunctionEvidence(rows);
      assert.equal(result.status, 'fail');
      assert.equal(result.case, second);
    }
  }
});

test('both exact scenarios must pass; missing scenarios and aggregate passes stay untested', () => {
  for (const [fn, [first, second]] of Object.entries(scenarioPairs)) {
    const pass = observation(fn, first, 'pass');
    const partial = selectFunctionEvidence([pass]);
    assert.equal(partial.status, 'untested');
    assert.ok(partial.evidence.includes(second));
    assert.equal(pass.status, 'pass');
    const aggregate = observation(fn, 'unknown/aggregate', 'pass');
    assert.equal(selectFunctionEvidence([aggregate]).status, 'untested');
    assert.equal(selectFunctionEvidence([aggregate, pass]).status, 'untested');
    assert.equal(selectFunctionEvidence([pass, observation(fn, second, 'pass')]).status, 'pass');
    assert.equal(selectFunctionEvidence([pass, observation(fn, second, 'intentional_skip')]).status, 'untested');
  }
  assert.equal(selectFunctionEvidence([]), undefined);
});

test('same-scenario qualifying retest clears its failure without requiring file order', () => {
  for (const [fn, [first, second]] of Object.entries(scenarioPairs)) {
    const old = observation(fn, second, 'fail');
    const fixed = observation(fn, second, 'pass', '2026-09-09T11:00:00Z');
    const other = observation(fn, first, 'pass');
    assert.equal(selectFunctionEvidence([fixed, other, old]).status, 'pass');
    assert.equal(selectFunctionEvidence([other, old]).status, 'fail');
  }
});

test('explicit groups cannot collapse known scenarios or disappear on normalization', () => {
  const fn = 'chat_log_link';
  const [first, second] = scenarioPairs[fn];
  const a = observation(fn, first, 'pass', '2026-09-09T11:00:00Z', { coverage_group: 'core' });
  const b = observation(fn, second, 'fail', undefined, { coverage_group: 'core' });
  assert.equal(selectFunctionEvidence([b, a]).status, 'fail');
  assert.equal(selectFunctionEvidence([a, { ...b, status: 'pass' }]).status, 'pass');
  const independent = { ...b, coverage_group: 'after-new-session' };
  assert.equal(selectFunctionEvidence([a, independent, { ...b, status: 'pass' }]).status, 'fail');
});

const baselineHumanFailure = {
  provider: 'antigravity', function: 'user_provenance', phase: 'after', case: 'mailbox-short',
  build: '39d996c896de2e5a4883ff325984889c851348e2', date: '2026-09-07', status: 'fail',
  evidence: 'replayed user event lacked metadata.input_origin',
};

test('the inspected baseline alias seeds only human provenance and can be superseded', () => {
  const [human, tool] = scenarioPairs.user_provenance;
  assert.equal(normalizeFunctionEvidence(baselineHumanFailure).coverage_group, human);
  const passes = [human, tool].map(scenario => observation('user_provenance', scenario, 'pass'));
  assert.equal(selectFunctionEvidence([baselineHumanFailure, passes[1]]).status, 'fail');
  assert.equal(selectFunctionEvidence([baselineHumanFailure, passes[0]]).status, 'untested');
  assert.equal(selectFunctionEvidence([...passes, baselineHumanFailure]).status, 'pass');
  const hypotheticalAliasPass = { ...baselineHumanFailure, status: 'pass' };
  assert.equal(selectFunctionEvidence([hypotheticalAliasPass, passes[1]]).status, 'untested');
  const explicit = { ...baselineHumanFailure, coverage_group: 'independent-baseline' };
  assert.equal(selectFunctionEvidence([explicit, ...passes]).status, 'fail');
});

test('baseline supersession matches exact provider, function, phase and source identity', () => {
  for (const change of [{ provider: 'pi' }, { function: 'chat_log_link' }, { case: 'mailbox-long' },
    { phase: 'before' }, { build: 'unknown' }, { wardian_revision: 'different-source' }]) {
    const row = { ...baselineHumanFailure, ...change };
    assert.equal(normalizeFunctionEvidence(row), row);
    assert.equal(row.coverage_group, undefined);
  }
  const normalizedRevision = { ...baselineHumanFailure, build: 'artifact-label', wardian_revision: baselineHumanFailure.build };
  assert.equal(normalizeFunctionEvidence(normalizedRevision).coverage_group, 'chat/user-prompt-provenance');
});

test('unknown aggregate failures remain obligations, even after unrelated newer aggregate passes', () => {
  const fn = 'user_provenance';
  const passes = scenarioPairs[fn].map(scenario => observation(fn, scenario, 'pass'));
  const unknown = { ...baselineHumanFailure, build: 'unknown', case: 'legacy/aggregate' };
  const otherAggregate = observation(fn, 'different/aggregate', 'pass', '2026-09-10');
  assert.equal(selectFunctionEvidence([unknown, ...passes, otherAggregate]).status, 'fail');
  assert.equal(selectFunctionEvidence([...passes, otherAggregate]).status, 'pass');
});

test('unqualified newer passes cannot erase failures or establish scenario completeness', () => {
  const fn = 'chat_log_link';
  const [first, second] = scenarioPairs[fn];
  const failure = observation(fn, second, 'fail');
  const stale = observation(fn, second, 'pass', '2026-09-10', { harness_sha256: 'old' });
  const other = observation(fn, first, 'pass');
  const qualify = row => row.harness_sha256 === 'old' ? { ...row, status: 'untested' } : row;
  for (const rows of [[failure, stale, other], [stale, other, failure]]) {
    assert.equal(selectFunctionEvidence(rows, qualify).status, 'fail');
  }
  assert.equal(selectFunctionEvidence([other, stale], qualify).status, 'untested');
  assert.equal(stale.status, 'pass');
});

test('cell selection isolates providers and functions before applying completeness', () => {
  const fn = 'chat_log_link';
  const [first, second] = scenarioPairs[fn];
  const partial = observation(fn, first, 'pass');
  const otherProvider = observation(fn, second, 'pass', undefined, { provider: 'codex' });
  const otherFunction = observation('user_provenance', second, 'pass');
  assert.equal(selectMatrixCellEvidence([partial, otherProvider, otherFunction], 'antigravity', fn).status, 'untested');
  assert.throws(() => selectFunctionEvidence([partial, otherProvider]), /one provider and function/);
  assert.throws(() => selectFunctionEvidence([partial, otherFunction]), /one provider and function/);
});


test('explicit untested observations cannot clear retained failure or block in any function', () => {
  for (const status of ['fail', 'blocked']) {
    const old = { provider: 'claude', function: 'instructions', status, date: '2026-09-08' };
    const absent = { ...old, status: 'untested', date: '2026-09-09' };
    for (const rows of [[old, absent], [absent, old]]) assert.equal(selectFunctionEvidence(rows).status, status);
    const retest = { ...old, status: 'pass', date: '2026-09-10' };
    assert.equal(selectFunctionEvidence([absent, retest, old]).status, 'pass');
  }
});
