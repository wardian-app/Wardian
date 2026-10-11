// @tier ci - CLI/provider worktree handover with durable and visible conversation retention.
import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import { createNativeHarness, ensureNativeAppBuilt, freezeBuiltCliForRun, prebuiltCliForRun, prepareIsolatedHome, startNativeSession, waitForAppShell } from '../lib/harness.mjs';

const skipNativeBuild = process.env.WARDIAN_NATIVE_SKIP_BUILD === '1';
const runId = `${process.pid}-${Date.now()}`;
function git(cwd, args) {
  const result = spawnSync('git', args, { cwd, encoding: 'utf8', windowsHide: true });
  assert.equal(result.status, 0, result.stderr);
  return result.stdout.trim();
}
const normalized = (folder) => {
  const canonical = fs.realpathSync.native(folder).replace(/^\\\\\?\\/, '').replaceAll('\\', '/');
  return process.platform === 'win32' ? canonical.toLowerCase() : canonical;
};
async function invoke(driver, command, args = {}) {
  const result = await driver.executeAsyncScript((command, args, done) => {
    window.__TAURI_INTERNALS__.invoke(command, args).then(value => done({ ok: true, value }), error => done({ ok: false, error: String(error) }));
  }, command, args);
  assert.equal(result.ok, true, result.error);
  return result.value;
}
function cli(binary, harness, args) {
  const env = { ...process.env, WARDIAN_HOME: harness.isolatedHome };
  delete env.WARDIAN_SESSION_ID;
  const result = spawnSync(binary, args, { cwd: harness.repoRoot, env, encoding: 'utf8', windowsHide: true });
  assert.equal(result.status, 0, result.stderr);
  return JSON.parse(result.stdout);
}
async function waitLaunch(recordsPath, owner, count) {
  const deadline = Date.now() + 20000;
  while (Date.now() < deadline) {
    const records = fs.existsSync(recordsPath) ? fs.readFileSync(recordsPath, 'utf8').trim().split('\n').filter(Boolean).map(line => JSON.parse(line)).filter(record => record.owner === owner) : [];
    if (records.length >= count) return records;
    await new Promise(resolve => setTimeout(resolve, 50));
  }
  assert.fail(`Provider did not record ${count} launches`);
}

// Startup publication and lease release settle after the first provider events.
// Observe the real fixture-home fence; never clear it or retry a rejected mutation.
async function waitPublishedProvider(harness, owner) {
  const leasePath = path.join(harness.isolatedHome, 'runtime', 'conversation-leases.json');
  const deadline = Date.now() + 20000;
  while (Date.now() < deadline) {
    if (fs.existsSync(leasePath)) {
      const state = JSON.parse(fs.readFileSync(leasePath, 'utf8'));
      assert.equal(state.schema, 1);
      assert.ok(Array.isArray(state.leases));
      if (!state.leases.some(lease => lease.agent_id === owner)) return;
    }
    await new Promise(resolve => setTimeout(resolve, 50));
  }
  assert.fail('Provider startup did not release its published conversation fence');
}

test('worktree assignment preserves the same conversation through Off, enable, join and disable', { timeout: 600000 }, async t => {
  const harness = await createNativeHarness();
  if (!skipNativeBuild) ensureNativeAppBuilt(harness);
  prepareIsolatedHome(harness);
  const recordsPath = path.join(harness.isolatedHome, 'provider-launches.jsonl');
  const historyTrigger = path.join(harness.isolatedHome, 'history-trigger');
  const closingTrigger = path.join(harness.isolatedHome, 'closing-trigger');
  const mockScript = path.join(harness.isolatedHome, 'workspace-provider.cjs');
  fs.writeFileSync(mockScript, `
const fs = require('node:fs');
const argv = process.argv.slice(2);
const index = argv.indexOf('--resume');
const id = index >= 0 ? argv[index + 1] : 'unexpected-fresh-thread';
const emit = event => { const line = JSON.stringify(event) + '\\n'; process.stdout.write(line); fs.appendFileSync(process.env.WARDIAN_MOCK_LOG, line); };
fs.appendFileSync(${JSON.stringify(recordsPath)}, JSON.stringify({ owner: process.env.WARDIAN_SESSION_ID, cwd: process.cwd(), id, pid: process.pid, argv }) + '\\n');
const firstLaunch = fs.readFileSync(${JSON.stringify(recordsPath)}, 'utf8').trim().split('\\n').filter(Boolean).map(line => JSON.parse(line)).filter(record => record.owner === process.env.WARDIAN_SESSION_ID).length === 1;
emit({ type:'init', session_id:id, timestamp:new Date().toISOString() });
emit({ type:'model', content:'same-thread-workspace:' + process.cwd() });
emit({ type:'result', status:'success' });
process.stdin.resume();
let historyEmitted = false;
let closingEmitted = false;
setInterval(() => {
  if (firstLaunch && !historyEmitted && fs.existsSync(${JSON.stringify(historyTrigger)})) {
    historyEmitted = true; emit({type:'user', content:'retained-history-marker'});
  }
  if (firstLaunch && !closingEmitted && fs.existsSync(${JSON.stringify(closingTrigger)})) {
    closingEmitted = true; emit({type:'user', content:'closing-history-marker'});
  }
}, 25);
`);
  process.env.WARDIAN_MOCK_SCRIPT = mockScript;
  const source = path.join(harness.isolatedHome, 'workspace-repo');
  fs.mkdirSync(source);
  git(source, ['init']);
  git(source, ['config', 'user.email', 'fixture@example.invalid']);
  git(source, ['config', 'user.name', 'Wardian Test']);
  git(source, ['config', 'commit.gpgsign', 'false']);
  fs.writeFileSync(path.join(source, 'README.md'), 'fixture\n');
  git(source, ['add', 'README.md']);
  git(source, ['commit', '-m', 'fixture']);
  let binary = prebuiltCliForRun(harness, skipNativeBuild);
  if (!binary) {
    const built = spawnSync('cargo', ['build', '-p', 'wardian-cli', '--bin', 'wardian-cli'], { cwd: harness.repoRoot, env: process.env, encoding: 'utf8', windowsHide: true });
    assert.equal(built.status, 0, built.stderr);
    binary = freezeBuiltCliForRun(harness);
  }
  const session = await startNativeSession(harness);
  t.after(async () => { await session.close(); });
  await waitForAppShell(session.driver, 20000);
  let name = `Off-Handover-${runId}`;
  let thread = `off-thread-${runId}`;
  let owner = await invoke(session.driver, 'spawn_agent', { req: { sessionName: name, agentClass:'Coder', folder:source, resumeSession:thread, isOff:true, configOverride:{ provider:'mock', session_persistence:'resume', conversation_logging:'enabled' } } });
  const unrelated = await invoke(session.driver, 'spawn_agent', { req: { sessionName:`Unrelated-${runId}`, agentClass:'Coder', folder:source, resumeSession:`unrelated-thread-${runId}`, isOff:true, configOverride:{ provider:'mock' } } });
  const snapshot = async () => (await invoke(session.driver, 'list_agents')).find(agent => agent.session_id === owner.session_id);
  const check = async folder => {
    const config = await snapshot();
    assert.equal(config.resume_session, thread);
    assert.equal(config.session_id, owner.session_id);
    assert.equal(normalized(config.folder), normalized(folder));
    const other = (await invoke(session.driver, 'list_agents')).find(agent => agent.session_id === unrelated.session_id);
    assert.deepEqual(other, unrelated);
    return config;
  };
  const offEnabled = cli(binary, harness, ['agent','worktree','enable', name, '--name', `off-${runId}`]);
  assert.equal(offEnabled.cleared_session, false);
  assert.equal((await check(offEnabled.worktree.worktree_folder)).is_off, true);
  assert.equal(fs.existsSync(recordsPath), false, 'Off assignment must not start a provider');
  cli(binary, harness, ['agent','worktree','disable', name]);
  const offSnapshot = await check(source);
  const offOwner = owner.session_id;
  name = `Handover-${runId}`;
  thread = `fixture-thread-${runId}`;
  owner = await invoke(session.driver, 'spawn_agent', { req: { sessionName:name, agentClass:'Coder', folder:source, resumeSession:thread, isOff:false, configOverride:{ provider:'mock', session_persistence:'resume', conversation_logging:'enabled' } } });
  const initial = await waitLaunch(recordsPath, owner.session_id, 1);
  assert.equal(initial[0].id, thread);
  assert.equal(normalized(initial[0].cwd), normalized(source));
  await waitPublishedProvider(harness, owner.session_id);
  // Establish the resumed source baseline, then emit a real fixture turn.
  await invoke(session.driver, 'load_agent_chat_transcript', { sessionId: owner.session_id });
  fs.writeFileSync(historyTrigger, 'emit');
  const initialTranscript = await session.driver.wait(async () => {
    const events = await invoke(session.driver, 'load_agent_chat_transcript', { sessionId: owner.session_id });
    return events.some(event => event.kind === 'message' && event.text?.includes('retained-history-marker')) ? events : false;
  }, 20000, 'Initial provider history did not reach the app transcript');
  assert.ok(initialTranscript.some(event => event.kind === 'message' && event.text?.includes('retained-history-marker')), 'Initial conversation must reach the durable app transcript');
  await waitPublishedProvider(harness, owner.session_id);
  const freshConfig = { ...(await snapshot()), session_persistence:'fresh' };
  await invoke(session.driver, 'update_agent_config', { newConfig:freshConfig });
  fs.writeFileSync(closingTrigger, 'emit');
  await session.driver.wait(() => fs.readFileSync(path.join(harness.isolatedHome, 'agents', owner.session_id, 'mock-transcript.jsonl'), 'utf8').includes('closing-history-marker'), 20000, 'Closing fixture turn did not reach its provider log');
  const enabled = cli(binary, harness, ['agent','worktree','enable', name, '--name', `active-${runId}`]);
  assert.equal(enabled.cleared_session, false);
  const config = await check(enabled.worktree.worktree_folder);
  assert.equal(config.session_persistence, 'fresh', 'Explicit handover must preserve the configured ordinary resume policy');
  const launches = await waitLaunch(recordsPath, owner.session_id, 2);
  assert.equal(launches[1].id, thread);
  assert.equal(normalized(launches[1].cwd), normalized(enabled.worktree.worktree_folder));
  await waitPublishedProvider(harness, owner.session_id);
  const second = path.join(`${source}.wt`, `second-${runId}`);
  git(source, ['worktree','add','-b', `wardian/second-${runId}`, second, 'HEAD']);
  const joined = cli(binary, harness, ['agent','worktree','join', name, '--worktree', second]);
  assert.equal(joined.cleared_session, false);
  await check(second);
  const joinedLaunches = await waitLaunch(recordsPath, owner.session_id, 3);
  assert.equal(joinedLaunches[2].id, thread);
  assert.equal(normalized(joinedLaunches[2].cwd), normalized(second));
  await waitPublishedProvider(harness, owner.session_id);
  const disabled = cli(binary, harness, ['agent','worktree','disable', name]);
  assert.equal(disabled.cleared_session, false);
  await check(source);
  const finalLaunches = await waitLaunch(recordsPath, owner.session_id, 4);
  assert.equal(finalLaunches[3].id, thread);
  assert.equal(normalized(finalLaunches[3].cwd), normalized(source));
  await waitPublishedProvider(harness, owner.session_id);
  const finalTranscript = await invoke(session.driver, 'load_agent_chat_transcript', { sessionId: owner.session_id });
  assert.ok(finalTranscript.some(event => event.kind === 'message' && event.text?.includes('retained-history-marker')), 'The initial conversation must remain visible after handover');
  assert.ok(finalTranscript.some(event => event.kind === 'message' && event.text?.includes('closing-history-marker')), 'The final provider turn must be retained before log rotation');
  const archiveRoot = path.join(harness.isolatedHome, 'agents', owner.session_id, 'conversations');
  const retained = fs.readdirSync(archiveRoot, { withFileTypes:true })
    .filter(entry => entry.isDirectory())
    .flatMap(entry => {
      const records = path.join(archiveRoot, entry.name, 'conversation.jsonl');
      return fs.existsSync(records) ? fs.readFileSync(records, 'utf8').trim().split('\n').filter(Boolean).map(line => JSON.parse(line)) : [];
    });
  for (const marker of ['retained-history-marker', 'closing-history-marker']) {
    assert.ok(retained.some(record => record.kind === 'message' && record.text?.includes(marker)), `Durable archive lost ${marker}`);
  }
  const { openWorkbenchSurface } = await import('../lib/workbench.mjs');
  await openWorkbenchSurface(session.driver, 'agents-overview', { timeoutMs:30000 });
  await session.driver.wait(async () => session.driver.executeScript((id) => {
    const card = document.getElementById(`agent-card-${id}`);
    if (!card) return false;
    const toggle = [...card.querySelectorAll('button')].find(button => (button.title || '').startsWith('Switch to Chat'));
    toggle?.click();
    const transcript = card.querySelector('[data-testid="agent-chat-transcript"]');
    return !!transcript?.getClientRects().length && ['retained-history-marker', 'closing-history-marker'].every(marker => transcript.innerText.includes(marker));
  }, owner.session_id), 30000, 'Retained conversation did not visibly render after handover');

  assert.deepEqual((await invoke(session.driver, 'list_agents')).find(agent => agent.session_id === offOwner), offSnapshot);
  assert.ok(fs.existsSync(second));
  assert.ok(fs.existsSync(enabled.worktree.worktree_folder));
});
