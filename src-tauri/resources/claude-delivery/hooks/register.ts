import type { Register } from 'claude-code'

// Wardian delivery mod (issue #1725). Wardian writes one JSON file per
// delivery into `<inbox>/pending/`. This mod submits each one as a queued
// prompt, so a delivery never types into the terminal or touches the user's
// draft. See docs/specs/2026-11-12-claude-mod-delivery.md.
//
// The mod never deletes or rewrites a file Wardian owns. It writes only:
//   claimed/<id>.json   before submit (the payload may be about to cross)
//   queued/<id>.json    after submit resolves (Claude queued the prompt)
//   failed/<id>.json    submit rejected (nothing was queued)
//   rejected/<id>.json  the delivery was not for this launch, or malformed
//   heartbeat.json      every tick, so Wardian can route around a dead mod
// A file in claimed/ is the idempotency record: once it exists the delivery is
// never submitted again, even across a reload or a crash.

type Launch = {
  schema: 1
  agent_id: string
  runtime_generation: number
  inbox: string
}

type Pending = {
  schema: 1
  interaction_id: string
  runtime_generation: number
  text: string
}

const POLL_MS = 1000
const ID = /^[A-Za-z0-9_-]{1,128}$/
const MAX_TEXT = 1_000_000

const record = (value: unknown) => JSON.stringify({ ...(value as object), at: Date.now() })

const parseLaunch = (raw: string): Launch | undefined => {
  try {
    const value = JSON.parse(raw)
    const valid =
      value?.schema === 1 &&
      typeof value.agent_id === 'string' &&
      Number.isSafeInteger(value.runtime_generation) &&
      value.runtime_generation > 0 &&
      typeof value.inbox === 'string' &&
      value.inbox.length > 0
    return valid ? value : undefined
  } catch {
    return undefined
  }
}

const parsePending = (raw: string): Pending | string => {
  let value: Pending
  try {
    value = JSON.parse(raw)
  } catch {
    return 'not valid JSON'
  }
  if (value?.schema !== 1) return 'unsupported schema'
  if (typeof value.interaction_id !== 'string' || !ID.test(value.interaction_id)) {
    return 'invalid interaction_id'
  }
  if (!Number.isSafeInteger(value.runtime_generation)) return 'invalid runtime_generation'
  if (typeof value.text !== 'string' || value.text.trim() === '') return 'empty text'
  if (value.text.length > MAX_TEXT) return 'text too long'
  return value
}

export const register: Register = on => {
  on('session.start', async ($, e, next) => {
    const result = await next(e)
    if (!e.isInteractive) return result

    const launch = await $.fs
      .read(`${$.plugin.root}/launch.json`)
      .then(raw => parseLaunch(String(raw)))
      .catch(() => undefined)
    if (launch === undefined) return result

    const inbox = launch.inbox
    let busy = false

    const tick = async () => {
      if (busy) return
      busy = true
      try {
        const sessionId = await $.session.id().catch(() => null)
        await $.fs.write(
          `${inbox}/heartbeat.json`,
          record({ generation: launch.runtime_generation, session_id: sessionId }),
        )

        const entries = await $.fs.list(`${inbox}/pending`).catch(() => [])
        const names = entries
          .filter(entry => entry.kind === 'file' && entry.name.endsWith('.json'))
          .map(entry => entry.name)
          .sort()

        for (const name of names) {
          const raw = await $.fs.read(`${inbox}/pending/${name}`).catch(() => undefined)
          if (raw === undefined) continue
          const parsed = parsePending(String(raw))
          const key = typeof parsed === 'string' ? name.replace(/\.json$/, '') : parsed.interaction_id
          if (!ID.test(key)) continue

          const settled =
            (await $.fs.exists(`${inbox}/claimed/${key}.json`)) ||
            (await $.fs.exists(`${inbox}/rejected/${key}.json`))
          if (settled) continue

          if (typeof parsed === 'string') {
            await $.fs.write(`${inbox}/rejected/${key}.json`, record({ reason: parsed }))
            continue
          }
          if (parsed.runtime_generation !== launch.runtime_generation) {
            await $.fs.write(
              `${inbox}/rejected/${key}.json`,
              record({
                reason: 'generation mismatch',
                expected: launch.runtime_generation,
                received: parsed.runtime_generation,
              }),
            )
            continue
          }

          await $.fs.write(`${inbox}/claimed/${key}.json`, record({ session_id: sessionId }))
          try {
            await $.prompt.submit({ text: parsed.text })
            await $.fs.write(`${inbox}/queued/${key}.json`, record({ session_id: sessionId }))
          } catch (error) {
            await $.fs.write(
              `${inbox}/failed/${key}.json`,
              record({ error: error instanceof Error ? error.message : String(error) }),
            )
          }
        }
      } finally {
        busy = false
      }
    }

    $.clock.every(POLL_MS, () => {
      void tick()
    })
    return result
  })
}
