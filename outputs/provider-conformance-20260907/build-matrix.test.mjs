import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import path from 'node:path';
import { createHash } from 'node:crypto';
import { selectFunctionEvidence } from './matrix-evidence.mjs';
const builder = await fs.readFile(new URL('./build-matrix.mjs', import.meta.url), 'utf8');
const section = (start, end) => {
  const from = builder.indexOf(start), to = builder.indexOf(end, from);
  assert.ok(from >= 0 && to > from, 'Builder data boundaries changed');return builder.slice(from, to);
};
// Exercise actual builder data logic, never Workbook/render/export or real files.
const AsyncFunction = Object.getPrototypeOf(async function () {}).constructor;
const execute = new AsyncFunction('fs', 'path', 'createHash', 'selectFunctionEvidence', 'root', 'out', 'process',
  section('const providers = ', 'const matrixHeaders = ') + section('let selections=[];', 'tableSheet(modelSheet,')
  + 'return { evidence, harnessEvidence, details, modelRows };');
function fixture() {
  const root = path.resolve('inert-matrix-fixture');const out = path.join(root, 'outputs/result');const files = new Map();
  const key = file => path.resolve(root, file);
  const put = (file, data) => files.set(key(file), typeof data === 'string' ? data : JSON.stringify(data));
  const read = file => JSON.parse(files.get(key(file)));
  const virtualFs = {
    async readFile(file, encoding) {
      assert.ok(path.relative(root, file) && !path.relative(root, file).startsWith('..'));
      if (!files.has(file)) throw Object.assign(new Error('Missing fixture input'), { code: 'ENOENT' });
      return encoding ? files.get(file) : Buffer.from(files.get(file));
    },
    async writeFile(file, data) {
      assert.ok(path.relative(root, file) && !path.relative(root, file).startsWith('..'));
      files.set(file, String(data));
    },
  };
  for (const file of ['e2e-native/lib/provider-headless-evidence.mjs', ...['provider-chat-conformance-real-native.test.mjs', 'provider-context-permissions-real-native.test.mjs', 'provider-native-broker-real-native.test.mjs'].map(name => `e2e-native/tests/${name}`)]) put(file, 'fixture-source');
  const models = { collected_at: '2026-09-09T00:00:00Z', selections: ['claude','codex','opencode','antigravity','pi','gemini'].map(provider => ({
    provider, version: 'retained-version', catalog_source: 'retained', model: `retained-${provider}`, basis: 'No price claim.', source_url: 'https://example.invalid/source',
  })) };
  put('outputs/result/model-source.json', models);
  const harness = name => ({ provider: 'harness', function: 'setup_observation', case: name, status: 'blocked', date: '2026-09-07' });
  const row = { provider: 'pi', function: 'native_delivery', case: 'retained-native', status: 'fail', date: '2026-09-07', evidence: 'Retained failure.' };
  put('docs/research/provider-conformance-observations.jsonl', JSON.stringify(row));
  return { files, put, read, row, harness, models,
    run: (env = {}) => execute(virtualFs, path, createHash, selectFunctionEvidence, root, out, { env }),
    snapshotPath: path.join(root, 'snapshot.json') };
}

test('function observations merge, dedupe property order, and survive partial-input reruns', async () => {
  const f = fixture();const other = { ...f.row, provider: 'claude', case: 'another-case' };
  f.put('.task/resume-matrix-results.jsonl', [f.row, Object.fromEntries(Object.entries(f.row).reverse()), other].map(JSON.stringify).join('\n'));
  assert.equal((await f.run()).evidence.length, 2);
  const before = [...f.files.entries()];await f.run();assert.deepEqual([...f.files.entries()], before);
  f.put('.task/resume-matrix-results.jsonl', JSON.stringify(other));
  assert.equal((await f.run()).evidence.length, 2);
});

test('a no-assertion observation preserves the documented Antigravity usage exclusion', async () => {
  const f = fixture();
  const observation = { provider: 'antigravity', function: 'usage_logging', case: 'chat/token-usage', status: 'untested', date: '2026-09-09', evidence: 'No qualifying assertion observed.' };
  f.put('.task/resume-matrix-results.jsonl', JSON.stringify(observation));
  const result = await f.run();
  const usage = result.details.find(row => row.provider === 'antigravity' && row.function === 'usage_logging');
  assert.equal(usage.status, 'Design skip');
  assert.match(usage.evidence, /intentionally unsupported/);
  assert.ok(result.evidence.some(row => row.provider === observation.provider && row.case === observation.case && row.status === 'untested'));
});

test('partial harness inputs merge retained records and deterministically dedupe', async () => {
  const f = fixture();const a = f.harness('a'), b = f.harness('b'), c = f.harness('c');
  f.put('outputs/result/harness-data.json', [a,b]);
  f.put('.task/coordinator-results.jsonl', [c,a,Object.fromEntries(Object.entries(c).reverse())].map(JSON.stringify).join('\n'));
  const first = await f.run();assert.deepEqual(first.harnessEvidence.map(row => row.case), ['a','b','c']);
  const before = f.read('outputs/result/harness-data.json');await f.run();assert.deepEqual(f.read('outputs/result/harness-data.json'), before);
  f.put('.task/coordinator-results.jsonl', JSON.stringify(c));assert.equal((await f.run()).harnessEvidence.length, 3);
});

test('absent private model and harness inputs retain saved public data', async () => {
  const f = fixture();f.put('outputs/result/harness-data.json', [f.harness('old')]);
  const result = await f.run();assert.equal(result.harnessEvidence.length, 1);
  assert.equal(result.modelRows.find(row => row[0] === 'pi')[3], 'retained-pi');
  const before = [...f.files.entries()];await f.run();assert.deepEqual([...f.files.entries()], before);
});

test('partial fresh private selections replace only their provider and persist the union', async () => {
  const f = fixture();f.put('.task/model-selections.json', { generated_on: '2026-09-10', selections: [{ provider: 'claude', selected_model: 'fresh-claude', provider_version: 'fresh-version', cheapest_basis: 'No price claim.', uncertainty: '', launch_limitation: '' }] });
  const result = await f.run();assert.equal(result.modelRows.find(row => row[0] === 'claude')[3], 'fresh-claude');
  assert.equal(result.modelRows.find(row => row[0] === 'pi')[3], 'retained-pi');
  assert.equal(f.read('outputs/result/model-source.json').selections.length, 6);
  const before = f.read('outputs/result/model-source.json');await f.run();assert.deepEqual(f.read('outputs/result/model-source.json'), before);
});

test('older private selections do not replace newer retained models', async () => {
  const f = fixture();f.put('.task/model-selections.json', { generated_on: '2026-09-07', selections: [{ provider: 'codex', selected_model: 'obsolete' }] });
  assert.equal((await f.run()).modelRows.find(row => row[0] === 'codex')[3], 'retained-codex');
});

test('partial refreshed snapshot retains unrelated provider metadata across reruns', async () => {
  const f = fixture();f.put('snapshot.json', { collected_at: '2026-09-10T00:00:00Z', providers: { claude: {
    version: 'new-version', selection: { model: 'fresh-claude', status: 'catalog-only', basis: 'No access claim.' },
  } } });
  const env = { WARDIAN_MATRIX_MODEL_SNAPSHOT: f.snapshotPath };
  const result = await f.run(env);assert.equal(result.modelRows.find(row => row[0] === 'claude')[3], 'fresh-claude');
  assert.equal(result.modelRows.find(row => row[0] === 'pi')[3], 'retained-pi');
  const before = f.read('outputs/result/model-source.json');await f.run(env);assert.deepEqual(f.read('outputs/result/model-source.json'), before);
});

test('private paths are rejected before writing merged harness or model records', async () => {
  const f = fixture();f.put('outputs/result/harness-data.json', [f.harness('safe')]);
  f.put('.task/coordinator-results.jsonl', JSON.stringify({ ...f.harness('bad'), evidence: '/home/private-fixture/raw' }));
  await assert.rejects(() => f.run(), /Private path/);assert.equal(f.read('outputs/result/harness-data.json').length, 1);
  const g = fixture();g.put('snapshot.json', { providers: { claude: { selection: { model: 'x', basis: '/home/private-fixture/raw' } } } });
  const before = g.read('outputs/result/model-source.json');
  await assert.rejects(() => g.run({ WARDIAN_MATRIX_MODEL_SNAPSHOT: g.snapshotPath }), /Private path/);
  assert.deepEqual(g.read('outputs/result/model-source.json'), before);
});


test('Pi initial readiness diagnosis shares core with a later actual readiness assertion', async () => {
  const f = fixture();const harness_sha256 = createHash('sha256').update('fixture-source').digest('hex');
  const diagnosed = { provider: 'pi', function: 'launch_readiness', case: 'chat/initial-idle-readiness', coverage_group: 'core', status: 'fail', date: '2026-09-09', harness_sha256 };
  f.put('.task/resume-matrix-results.jsonl', JSON.stringify(diagnosed));
  assert.equal((await f.run()).details.find(row => row.provider === 'pi' && row.function === 'launch_readiness').status, 'Fail');
  f.put('.task/resume-matrix-results.jsonl', JSON.stringify({ ...diagnosed, case: 'chat/assistant-authorship', status: 'pass', date: '2026-09-10' }));
  assert.equal((await f.run()).details.find(row => row.provider === 'pi' && row.function === 'launch_readiness').status, 'Pass');
});

for (const [caseName, functionId] of [['chat/assistant-authorship', 'short_input'], ['context/managed_instructions', 'instructions'], ['native/direct', 'native_delivery']]) {
  test(`historical ${caseName} pass cannot qualify changed or unbound submitted source`, async () => {
    for (const harness_sha256 of ['a'.repeat(64), undefined]) {
      const f = fixture();
      const row = { provider: 'claude', function: functionId, case: caseName, status: 'pass', date: '2026-09-09', harness_sha256 };
      f.put('.task/resume-matrix-results.jsonl', JSON.stringify(row));
      const result = await f.run();
      assert.equal(result.details.find(value => value.provider === 'claude' && value.function === functionId).status, 'Untested');
      assert.equal(result.evidence.find(value => value.case === caseName).status, 'pass');
    }
    const f = fixture();
    f.put('.task/resume-matrix-results.jsonl', JSON.stringify({ provider: 'claude', function: functionId, case: caseName, status: 'pass', date: '2026-09-09', harness_sha256: createHash('sha256').update('fixture-source').digest('hex') }));
    assert.equal((await f.run()).details.find(value => value.provider === 'claude' && value.function === functionId).status, 'Pass');
  });
}

test('headless context pass also requires the exact evidence reader', async () => {
  const f = fixture();
  const row = { provider: 'claude', function: 'headless_resume', case: 'context/headless_inherited_resume', status: 'pass', date: '2026-09-09', harness_sha256: createHash('sha256').update('fixture-source').digest('hex'), evidence_reader_sha256: 'a'.repeat(64) };
  f.put('.task/resume-matrix-results.jsonl', JSON.stringify(row));
  const result = await f.run();
  assert.equal(result.details.find(value => value.provider === 'claude' && value.function === row.function).status, 'Untested');
  assert.equal(result.evidence.find(value => value.case === row.case).status, 'pass');
  f.put('.task/resume-matrix-results.jsonl', JSON.stringify({ ...row, date: '2026-09-10', evidence_reader_sha256: row.harness_sha256 }));
  assert.equal((await f.run()).details.find(value => value.provider === 'claude' && value.function === row.function).status, 'Pass');
});
