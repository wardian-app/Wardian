import type { Register } from 'claude-code'

// Wardian delivery mod (issue #1725). Wardian publishes one JSON file per
// delivery into this generation's `<inbox>/pending/`. The mod submits each as
// a queued prompt, so a delivery never types into the terminal or touches the
// user's draft. Design: docs/specs/2026-11-12-claude-mod-delivery.md.
//
// The mod's file API has no rename or exclusive create, so every record it
// writes is a write-once file under a name unique to one interaction, never
// rewritten:
//   ready.json           once at start: generation and actual session id
//   claimed/<id>.json    BEFORE submit; its existence alone means "may have crossed"
//   queued/<id>.json     submit resolved { text }: the prompt entered the queue
//   dropped/<id>.json    submit resolved { drop }: proven not entered
//   rejected/<id>.json   malformed or for another generation; never submitted
// heartbeat.json is the only file rewritten. It is diagnostic and readiness
// only, so a failed or torn write is ignored.

type Launch = {
  schema: 1
  agent_id: string
  runtime_generation: number
  expected_claude_session_id: string
  inbox: string
}

type Pending = {
  schema: 1
  interaction_id: string
  runtime_generation: number
  text: string
}

const MOD_VERSION = '0.1.0'
const POLL_MS = 1000
const HEARTBEAT_MS = 1000
const ID = /^[A-Za-z0-9_-]{1,128}$/
const MAX_TEXT = 1_000_000

const parseLaunch = (raw: string): Launch | undefined => {
  try {
    const value = JSON.parse(raw)
    const valid =
      value?.schema === 1 &&
      typeof value.agent_id === 'string' &&
      Number.isSafeInteger(value.runtime_generation) &&
      value.runtime_generation > 0 &&
      typeof value.expected_claude_session_id === 'string' &&
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

// `<seq>-<interaction_id>.json`: the key used for records before the payload
// is known to be valid.
const keyFromName = (name: string) => name.replace(/\.json$/, '').replace(/^[^-]*-/, '')

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
    const generation = launch.runtime_generation
    const currentSession = () => $.session.id().catch(() => null)
    const record = async (path: string, interactionId: string, fields: object) =>
      $.fs.write(
        path,
        JSON.stringify({
          schema: 1,
          generation,
          interaction_id: interactionId,
          session_id: await currentSession(),
          at: Date.now(),
          ...fields,
        }),
      )

    await $.fs
      .write(
        `${inbox}/ready.json`,
        JSON.stringify({
          schema: 1,
          generation,
          session_id: await currentSession(),
          mod_version: MOD_VERSION,
          at: Date.now(),
        }),
      )
      .catch(() => undefined)

    $.clock.every(HEARTBEAT_MS, async () => {
      await $.fs
        .write(
          `${inbox}/heartbeat.json`,
          JSON.stringify({ schema: 1, generation, session_id: await currentSession(), at: Date.now() }),
        )
        .catch(() => undefined)
    })

    const settled = async (key: string) =>
      (await $.fs.exists(`${inbox}/claimed/${key}.json`)) ||
      (await $.fs.exists(`${inbox}/rejected/${key}.json`)) ||
      (await $.fs.exists(`${inbox}/dropped/${key}.json`))

    let busy = false
    const deliver = async () => {
      if (busy) return
      busy = true
      try {
        const entries = await $.fs.list(`${inbox}/pending`).catch(() => [])
        const names = entries
          .filter(entry => entry.kind === 'file' && entry.name.endsWith('.json'))
          .map(entry => entry.name)
          .sort()

        for (const name of names) {
          const raw = await $.fs.read(`${inbox}/pending/${name}`).catch(() => undefined)
          if (raw === undefined) continue
          const parsed = parsePending(String(raw))
          const key = typeof parsed === 'string' ? keyFromName(name) : parsed.interaction_id
          if (!ID.test(key) || (await settled(key))) continue

          if (typeof parsed === 'string') {
            await record(`${inbox}/rejected/${key}.json`, key, { reason: parsed })
            continue
          }
          if (parsed.runtime_generation !== generation) {
            await record(`${inbox}/rejected/${key}.json`, key, {
              reason: 'generation mismatch',
              received_generation: parsed.runtime_generation,
            })
            continue
          }

          // The claim must be on disk before submit; a failed claim write
          // throws out of this loop, so no submit happens without one.
          await record(`${inbox}/claimed/${key}.json`, key, {})
          let outcome: { drop?: string; text?: string } | undefined
          try {
            outcome = await $.prompt.submit({ text: parsed.text })
          } catch {
            // Uncertain: leave the claim alone. Wardian never replays it.
            continue
          }
          if (outcome !== undefined && typeof outcome.drop === 'string') {
            await record(`${inbox}/dropped/${key}.json`, key, { reason: outcome.drop })
          } else if (outcome !== undefined && typeof outcome.text === 'string') {
            await record(`${inbox}/queued/${key}.json`, key, {})
          }
          // Any other shape stays uncertain, like a rejected promise.
        }
      } catch {
        // A failed read, list or record write ends this pass; the next tick
        // starts again from the settled records, so nothing is submitted twice.
      } finally {
        busy = false
      }
    }

    $.clock.every(POLL_MS, () => {
      void deliver()
    })
    return result
  })
}
