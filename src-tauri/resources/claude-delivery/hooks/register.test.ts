import { expect, mock, test } from 'claude-code/testing'
import type { Engine } from 'claude-code/testing'
import type { On } from 'claude-code'

// Official engine tests: the runner loads hooks.json/register.ts itself.
// No model, authentication, disk fixtures, or live Claude session is used.
const inbox = '/owned/delivery/7/inbox'
const path = (kind: string, id: string) => `${inbox}/${kind}/${id}.json`
// The engine normalizes fs paths before dispatching hooks on Windows.
const canonical = (file: string) => file.replaceAll('\\', '/').replace(/^[A-Za-z]:/, '')

function fixture($: Engine, on: On) {
  const clock = mock.clock(on)
  const files = new Map<string, string>()
  const submissions: string[] = []
  let submit = async (text: string): Promise<{ text: string } | { drop: string }> => ({ text })
  let beforeWrite = async (_path: string) => {}
  let session = 'session-A'
  let heartbeatAttempts = 0
  on('session.start', async (_$, e) => ({ cwd: e.cwd }))
  on('session.id', async () => ({ value: session }))
  on('fs.read', async (_$, e) => {
    if (canonical(e.path).endsWith('/launch.json')) return { value: JSON.stringify({
      schema: 1, agent_id: 'agent-A', runtime_generation: 7,
      expected_claude_session_id: 'session-A', inbox,
    }) }
    return { value: files.get(canonical(e.path)) ?? '' }
  })
  on('fs.exists', async (_$, e) => ({ value: files.has(canonical(e.path)) }))
  on('fs.list', async (_$, e) => ({ value: [...files.keys()]
    .filter(file => file.startsWith(`${canonical(e.path)}/`) && !file.slice(canonical(e.path).length + 1).includes('/'))
    .map(file => ({
      name: file.slice(canonical(e.path).length + 1), kind: 'file' as const,
      size: files.get(file)!.length, mtimeMs: 0, isLink: false,
    })) }))
  on('fs.write', async (_$, e) => {
    if (canonical(e.path) === `${inbox}/heartbeat.json`) heartbeatAttempts++
    await beforeWrite(canonical(e.path))
    files.set(canonical(e.path), e.text)
    return { value: undefined }
  })
  on('prompt.submit', async (_$, e) => {
    submissions.push(e.text)
    return submit(e.text)
  })
  return {
    files, submissions, clock,
    start: () => $.session.start({ cwd: '/owned/workspace', surface: 'terminal', isInteractive: true }),
    pending(id: string, seq: number, generation = 7) {
      files.set(`${inbox}/pending/${String(seq).padStart(12, '0')}-${id}.json`, JSON.stringify({
        schema: 1, interaction_id: id, runtime_generation: generation, text: `body-${id}`,
      }))
    },
    record(kind: string, id: string) {
      const text = files.get(path(kind, id))
      return text === undefined ? undefined : JSON.parse(text)
    },
    setSubmit(fn: typeof submit) { submit = fn },
    setWrite(fn: typeof beforeWrite) { beforeWrite = fn },
    setSession(id: string) { session = id },
    heartbeatAttempts: () => heartbeatAttempts,
  }
}

test('sequence ordering and busy poll serialize submissions without replay', async ($, on) => {
  const f = fixture($, on)
  const held = Promise.withResolvers<{ text: string }>()
  f.pending('second', 2)
  f.pending('first', 1)
  f.setSubmit(text => text === 'body-first' ? held.promise : Promise.resolve({ text }))
  await f.start()
  await f.clock.advance(1000)
  expect(f.submissions).toEqual(['body-first'])
  await f.clock.advance(1000)
  expect(f.submissions).toEqual(['body-first'])
  held.resolve({ text: 'body-first' })
  await f.clock.settle()
  expect(f.submissions).toEqual(['body-first', 'body-second'])
  expect(f.record('queued', 'first').generation).toBe(7)
  expect(f.record('queued', 'second').interaction_id).toBe('second')
  await f.clock.advance(1000)
  expect(f.submissions).toEqual(['body-first', 'body-second'])
})

test('generation mismatch never claims or submits', async ($, on) => {
  const f = fixture($, on)
  f.pending('foreign', 1, 8)
  await f.start()
  await f.clock.advance(1000)
  expect(f.submissions).toEqual([])
  expect(f.record('claimed', 'foreign')).toBeUndefined()
  expect(f.record('rejected', 'foreign').reason).toBe('generation mismatch')
  expect(f.record('rejected', 'foreign').received_generation).toBe(8)
})

test('claim resolves before submission crosses the provider boundary', async ($, on) => {
  const f = fixture($, on)
  const claim = Promise.withResolvers<void>()
  f.pending('held', 1)
  f.setWrite(file => file === path('claimed', 'held') ? claim.promise : Promise.resolve())
  f.setSubmit(async text => {
    expect(f.record('claimed', 'held').interaction_id).toBe('held')
    return { text }
  })
  await f.start()
  await f.clock.advance(1000)
  expect(f.submissions).toEqual([])
  expect(f.record('claimed', 'held')).toBeUndefined()
  claim.resolve()
  await f.clock.settle()
  expect(f.submissions).toEqual(['body-held'])
  expect(f.record('queued', 'held').generation).toBe(7)
})

test('drop retains exact reason and cannot be queued or replayed', async ($, on) => {
  const f = fixture($, on)
  f.pending('dropped', 1)
  f.setSubmit(async () => ({ drop: 'Stopped by user' }))
  await f.start()
  await f.clock.advance(1000)
  expect(f.record('claimed', 'dropped').interaction_id).toBe('dropped')
  expect(f.record('dropped', 'dropped').reason).toBe('Stopped by user')
  expect(f.record('queued', 'dropped')).toBeUndefined()
  await f.clock.advance(1000)
  expect(f.submissions).toEqual(['body-dropped'])
})

test('rejected submit remains uncertain and a torn claim still prevents replay', async ($, on) => {
  const f = fixture($, on)
  f.pending('uncertain', 1)
  f.setSubmit(async () => { throw new Error('Provider connection lost') })
  await f.start()
  await f.clock.advance(1000)
  expect(f.record('claimed', 'uncertain').interaction_id).toBe('uncertain')
  for (const kind of ['queued', 'dropped', 'rejected']) expect(f.record(kind, 'uncertain')).toBeUndefined()
  f.files.set(path('claimed', 'uncertain'), '{')
  await f.clock.advance(1000)
  expect(f.submissions).toEqual(['body-uncertain'])
})

test('heartbeat survives write failures independently of unresolved submit', async ($, on) => {
  const f = fixture($, on)
  const held = Promise.withResolvers<{ text: string }>()
  f.pending('slow', 1)
  f.setSubmit(() => held.promise)
  f.setWrite(async file => { if (file === `${inbox}/heartbeat.json`) throw new Error('Sharing violation') })
  await f.start()
  await f.clock.advance(2000)
  expect(f.submissions).toEqual(['body-slow'])
  expect(f.record('queued', 'slow')).toBeUndefined()
  expect(f.heartbeatAttempts()).toBe(2)
  f.setWrite(async () => {})
  f.setSession('session-B')
  await f.clock.advance(1000)
  expect(JSON.parse(f.files.get(`${inbox}/heartbeat.json`)!).session_id).toBe('session-B')
  expect(f.record('queued', 'slow')).toBeUndefined()
  held.resolve({ text: 'body-slow' })
  await f.clock.settle()
  expect(f.record('queued', 'slow').session_id).toBe('session-B')
  expect(f.files.has(`${inbox}/sessions/session-A.json`)).toBe(true)
  expect(f.files.has(`${inbox}/sessions/session-B.json`)).toBe(true)
})
