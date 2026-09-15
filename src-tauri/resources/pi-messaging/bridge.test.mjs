import test from 'node:test';
import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { createHash } from 'node:crypto';
import { FrameDecoder, encodeFrame, parseConfig, MAX_FRAME_BYTES } from './protocol.mjs';
import { createBridge } from './bridge.mjs';
import wardianPiMessaging from './index.mjs';

const config = { version: 1, host: '127.0.0.1', port: 12345, token: 'a'.repeat(64),
  target_id: 'target', generation: 7, session_id: 'session', session_file: '/owned/session.jsonl' };
function delivery(overrides = {}) {
  const body = 'A peer task, not human input.';
  return { message_id: 'm1', interaction_id: 'i1', sender_id: 'peer', kind: 'task', body,
    body_sha256: createHash('sha256').update(body).digest('hex'), ...overrides };
}
function fixture(t, { idle = true, throwSend = false } = {}) {
  const socket = new EventEmitter();
  const output = [], calls = [];
  socket.destroyed = false;
  socket.destroy = () => { socket.destroyed = true; };
  socket.write = buffer => { new FrameDecoder().feed(buffer, f => output.push(f)); return true; };
  let currentSession = config.session_id;
  const ctx = { mode: 'tui', isIdle: () => idle,
    sessionManager: { getSessionId: () => currentSession, getSessionFile: () => config.session_file } };
  const bridge = createBridge({ config, runtimeNonce: 'runtime', pid: 123, socket, ctx,
    pi: { sendMessage: (...args) => { calls.push(args); if (throwSend) throw new Error('handoff'); } } });
  t.after(bridge.stop);
  let seq = 0;
  const input = (type, payload, overrides = {}) => socket.emit('data', encodeFrame({ version: 1,
    target_id: config.target_id, generation: config.generation, session_id: config.session_id,
    runtime_nonce: 'runtime', seq: ++seq, type, ...payload, ...overrides }));
  socket.emit('connect');
  return { socket, output, calls, bridge, input,
    welcome: overrides => input('welcome', { token: config.token }, overrides),
    deliver: d => input('deliver', { delivery: d ?? delivery() }),
    switchSession: () => { currentSession = 'other'; },
    consume: () => bridge.messageStart({ message: { role: 'custom', ...calls[0][0] } }) };
}

test('session-start rejection diagnostics expose only fixed credential-free codes', () => {
  const originalConfig = process.env.WARDIAN_PI_BRIDGE_CONFIG;
  const originalError = console.error;
  const logs = [];
  const diagnosticConfig = { ...config, session_id: 'pi-session-7', session_file: '/owned/pi-session-7.jsonl' };
  console.error = message => logs.push(message);
  try {
    const cases = [
      ['config_missing', undefined, { mode: 'tui', sessionId: diagnosticConfig.session_id, sessionFile: diagnosticConfig.session_file }],
      ['config_parse_failed', '{bad', { mode: 'tui', sessionId: diagnosticConfig.session_id, sessionFile: diagnosticConfig.session_file }],
      ['mode_not_tui', JSON.stringify(diagnosticConfig), { mode: 'non-tui', sessionId: diagnosticConfig.session_id, sessionFile: diagnosticConfig.session_file }],
      ['session_id_mismatch', JSON.stringify(diagnosticConfig), { mode: 'tui', sessionId: 'other', sessionFile: diagnosticConfig.session_file }],
      ['session_file_mismatch', JSON.stringify(diagnosticConfig), { mode: 'tui', sessionId: diagnosticConfig.session_id, sessionFile: '/other/session.jsonl' }],
    ];
    for (const [code, raw, values] of cases) {
      if (raw === undefined) delete process.env.WARDIAN_PI_BRIDGE_CONFIG;
      else process.env.WARDIAN_PI_BRIDGE_CONFIG = raw;
      const pi = new EventEmitter();
      wardianPiMessaging(pi);
      pi.emit('session_start', {}, {
        mode: values.mode,
        hasUI: true,
        sessionManager: {
          getSessionId: () => values.sessionId,
          getSessionFile: () => values.sessionFile,
        },
      });
      assert.equal(logs.at(-1), `[Wardian] Pi bridge rejection stage=session_start code=${code}`);
      assert.equal(logs.at(-1).includes(config.token), false);
      assert.equal(logs.at(-1).includes(diagnosticConfig.session_id), false);
      assert.equal(logs.at(-1).includes(diagnosticConfig.session_file), false);
    }
  } finally {
    console.error = originalError;
    if (originalConfig === undefined) delete process.env.WARDIAN_PI_BRIDGE_CONFIG;
    else process.env.WARDIAN_PI_BRIDGE_CONFIG = originalConfig;
  }
});

test('successful session-start validation begins the listener handoff with fixed codes', () => {
  const originalConfig = process.env.WARDIAN_PI_BRIDGE_CONFIG;
  const originalError = console.error;
  const logs = [];
  console.error = message => logs.push(message);
  try {
    process.env.WARDIAN_PI_BRIDGE_CONFIG = JSON.stringify(config);
    const pi = new EventEmitter();
    wardianPiMessaging(pi);
    pi.emit('session_start', {}, {
      mode: 'tui',
      sessionManager: {
        getSessionId: () => config.session_id,
        getSessionFile: () => config.session_file,
      },
    });
    assert.deepEqual(logs.slice(0, 2), [
      '[Wardian] Pi bridge stage=session_start code=validated',
      '[Wardian] Pi bridge stage=listener_child_handoff code=socket_connecting',
    ]);
    assert.equal(logs.join('\n').includes(config.token), false);
    pi.emit('session_shutdown');
  } finally {
    console.error = originalError;
    if (originalConfig === undefined) delete process.env.WARDIAN_PI_BRIDGE_CONFIG;
    else process.env.WARDIAN_PI_BRIDGE_CONFIG = originalConfig;
  }
});

test('frames survive every split boundary and coalescing', () => {
  const frame = encodeFrame({ body: 'héllo🙂' });
  for (let cut = 0; cut <= frame.length; cut++) {
    const decoder = new FrameDecoder(), result = [];
    decoder.feed(frame.subarray(0, cut), x => result.push(x));
    decoder.feed(Buffer.concat([frame.subarray(cut), frame]), x => result.push(x));
    assert.deepEqual(result, [{ body: 'héllo🙂' }, { body: 'héllo🙂' }]);
  }
});
test('oversize, zero, invalid UTF-8/JSON fail closed', () => {
  for (const size of [0, MAX_FRAME_BYTES + 1]) {
    const header = Buffer.alloc(4); header.writeUInt32BE(size);
    const decoder = new FrameDecoder();
    assert.throws(() => decoder.feed(header, () => {}));
    assert.throws(() => decoder.feed(encodeFrame({}), () => {}), /decoder_closed/);
  }
  for (const body of [Buffer.from([0xff]), Buffer.from('{bad}')]) {
    const header = Buffer.alloc(4); header.writeUInt32BE(body.length);
    assert.throws(() => new FrameDecoder().feed(Buffer.concat([header, body]), () => {}));
  }
});
test('configuration permits only bounded explicit loopback bindings', () => {
  assert.deepEqual(parseConfig(JSON.stringify(config)), config);
  for (const change of [{ host: 'localhost' }, { host: '0.0.0.0' }, { port: 0 }, { generation: -1 },
    { token: 'short' }, { unexpected: true }, { session_file: 'bad\npath' }]) {
    assert.throws(() => parseConfig(JSON.stringify({ ...config, ...change })));
  }
});
test('authentication, generation, session, runtime and sequence fail closed', t => {
  for (const change of [{ token: 'b'.repeat(64) }, { generation: 8 }, { session_id: 'other' },
    { runtime_nonce: 'old' }, { seq: 0 }, { seq: 1.5 }, { extra: true }]) {
    const f = fixture(t); f.welcome(change); f.deliver();
    assert.equal(f.socket.destroyed, true); assert.equal(f.calls.length, 0);
  }
});
test('delivery before welcome is never submitted', t => {
  const f = fixture(t); f.deliver(); assert.equal(f.socket.destroyed, true); assert.equal(f.calls.length, 0);
});

test('listener handoff reports an early socket error without exposing its payload', t => {
  const originalError = console.error;
  const logs = [];
  console.error = message => logs.push(message);
  try {
    const f = fixture(t);
    f.socket.emit('error', new Error('secret-token-and-session-file'));
    assert.equal(logs.at(-1), '[Wardian] Pi bridge stage=listener_child_handoff code=socket_error_before_ready');
    assert.equal(logs.join('\n').includes('secret-token-and-session-file'), false);
    assert.equal(f.socket.destroyed, true);
  } finally {
    console.error = originalError;
  }
});

test('listener handoff reports a bounded timeout when the child never connects', async t => {
  const originalError = console.error;
  const logs = [];
  console.error = message => logs.push(message);
  const socket = new EventEmitter();
  socket.destroyed = false;
  socket.destroy = () => { socket.destroyed = true; };
  socket.write = () => true;
  const bridge = createBridge({ config, runtimeNonce: 'runtime', pid: 123, socket,
    ctx: { mode: 'tui', isIdle: () => true,
      sessionManager: { getSessionId: () => config.session_id, getSessionFile: () => config.session_file } },
    pi: { sendMessage: () => {} }, handshakeMs: 1 });
  t.after(() => { console.error = originalError; bridge.stop(); });
  await new Promise(resolve => setTimeout(resolve, 10));
  assert.equal(logs.at(-1), '[Wardian] Pi bridge stage=listener_child_handoff code=handshake_timeout');
  assert.equal(socket.destroyed, true);
});
for (const idle of [true, false]) test(`task uses custom attribution and followUp while idle=${idle}`, t => {
  const f = fixture(t, { idle }); f.welcome(); f.deliver();
  assert.equal(f.calls.length, 1);
  assert.deepEqual(f.calls[0][1], { triggerTurn: true, deliverAs: 'followUp' });
  assert.match(f.calls[0][0].content, /sender=peer; message=m1; interaction=i1/);
  assert.equal(f.calls[0][0].details.sender_id, 'peer');
  assert.ok(Object.isFrozen(f.calls[0][0].details));
  assert.equal(JSON.stringify(f.calls).includes(config.token), false);
  assert.deepEqual(f.output.map(x => x.type), ['hello', 'ready', 'received', 'submitted_unconfirmed']);
  f.consume(); f.consume();
  assert.equal(f.output.filter(x => x.type === 'observed_consumption').length, 1);
  f.bridge.settled();
  assert.equal(f.output.some(x => x.type === 'observed_settled'), idle);
  assert.equal(f.output.some(x => x.type === 'completed'), false);
});
test('information is explicitly unsupported and never calls Pi', t => {
  const f = fixture(t, { idle: false }); f.welcome(); f.deliver(delivery({ kind: 'information' }));
  assert.equal(f.calls.length, 0); assert.equal(f.output.at(-1).reason, 'information_unsupported');
  assert.equal(f.output[1].capabilities.information, false);
});
test('duplicates and a second inflight delivery never replay', t => {
  const f = fixture(t); f.welcome(); f.deliver(); f.deliver();
  assert.equal(f.output.at(-1).reason, 'duplicate_no_replay');
  f.deliver(delivery({ message_id: 'm2' })); assert.equal(f.output.at(-1).reason, 'busy');
  f.consume(); f.bridge.settled(); f.deliver();
  assert.equal(f.calls.length, 1);
});
test('wrong body hash or unknown operation fails before handoff', t => {
  for (const d of [delivery({ body: 'tampered' }), delivery({ kind: 'interrupt' })]) {
    const f = fixture(t); f.welcome(); f.deliver(d);
    assert.equal(f.socket.destroyed, true); assert.equal(f.calls.length, 0);
  }
});
test('unrelated custom event cannot establish consumption; settle alone cannot complete', t => {
  const f = fixture(t); f.welcome(); f.deliver();
  const sent = f.calls[0][0];
  f.bridge.messageStart({ message: { role: 'custom', ...sent, details: { ...sent.details, sender_id: 'other' } } });
  f.bridge.settled();
  assert.equal(f.output.at(-1).type, 'submitted_unconfirmed');
});
test('session switch and lifecycle stop prevent stale submission and events', t => {
  const f = fixture(t); f.welcome(); f.switchSession(); f.deliver();
  assert.equal(f.socket.destroyed, true); assert.equal(f.calls.length, 0);
  const g = fixture(t); g.welcome(); g.deliver(); g.bridge.stop(); g.consume(); g.deliver();
  assert.equal(g.calls.length, 1); assert.equal(g.output.at(-1).type, 'submitted_unconfirmed');
});
test('throwing void API and lost connection retain uncertainty, never replay', t => {
  const f = fixture(t, { throwSend: true }); f.welcome(); f.deliver(); f.deliver();
  assert.equal(f.calls.length, 1); assert.equal(f.socket.destroyed, true);
  assert.equal(f.output.at(-1).type, 'submitted_unconfirmed');
  const g = fixture(t); g.welcome(); g.deliver(); g.socket.emit('close'); g.deliver();
  assert.equal(g.calls.length, 1);
});
test('output backpressure closes before Pi handoff without repeating accepted bytes', t => {
  const f = fixture(t); f.welcome();
  f.socket.write = () => false;
  f.deliver(); assert.equal(f.socket.destroyed, true); assert.equal(f.calls.length, 0);
});
