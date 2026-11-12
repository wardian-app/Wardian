# Claude Delivery Through a Wardian Mod

- **Status:** Proposed
- **Date:** 2026-11-12
- **Issue:** #1725
- **Research:** [`docs/research/claude-delivery-transports.md`](../research/claude-delivery-transports.md)

## Problem

Wardian delivers peer messages and task replies to an interactive Claude Code
agent by writing them into the agent's terminal input. A delivery that arrives
while the user is typing interrupts them, and can mix with their draft. The
user's terminal interface must stay, so running Claude without it (the
stream-json SDK route) is not an option.

## Decision

Ship a Wardian mod, a Claude Code plugin with a hooks module, and load it into
every Wardian-launched interactive Claude agent. The mod receives Wardian
deliveries for its agent, and submits each one with the documented
`$.prompt.submit({ text })` call. That call queues the prompt and starts a turn
"once the session is idle", the same queue-until-idle policy Wardian uses
everywhere else. Terminal keystroke delivery stays only as the fallback for a
session where the mod is not running.

The research record compares 13 options. The mod was the only one that kept
the terminal interface, woke an idle session, left the user's draft untouched,
queued until idle, used a documented API, and needed no launch dialog. All of these were confirmed by probe on Claude Code 2.1.322 on Windows.

## Design

### Packaging and loading

- The mod source lives in `src-tauri/resources/claude-delivery/`, next to
  `pi-messaging`. It holds `.claude-plugin/plugin.json` (name `wardian`),
  `hooks/hooks.json` and `hooks/register.ts`.
- Each runtime generation owns its own namespace. Wardian never shares or
  rewrites anything across generations:

  ```text
  <wardian-home>/agents/<agent-id>/claude/delivery/<generation>/
    mod/          immutable copy of the bundled mod, plus launch.json
    inbox/        this generation's deliveries and the mod's records
  ```

  Before launch, Wardian creates `mod/` and writes `launch.json` once. If either
  already exists with different content, the launch refuses this transport, as
  `PiBridgeLaunchPlan::materialize_extension` does. The mod reads only its own
  `launch.json`:

  ```json
  {
    "schema": 1,
    "agent_id": "<wardian agent uuid>",
    "runtime_generation": 7,
    "expected_claude_session_id": "<the --session-id or resume id Wardian passed>",
    "inbox": "<absolute path of this generation's inbox>"
  }
  ```

  An old generation's mod therefore cannot see, reject or claim a newer
  generation's deliveries. The folder never changes after launch, so Claude's
  plugin-folder watcher never reloads the module in the middle of a session.

- Wardian adds `delivery/<generation>/mod` to `CLAUDE_CODE_PLUGIN_DIRS` in the
  launch environment. It appends to any existing value with the platform
  path-list separator, never replacing it. This sits beside the variables
  `claude_terminal_runtime_env` already sets in
  `src-tauri/src/manager/mod.rs`. No install, marketplace or dialog is involved.
- The plugin name `wardian` makes the transcript row read "The wardian plugin
  sent a message:", which the user and Wardian can both recognise.

### Handoff: files, not a socket

A mod cannot listen inbound. It can poll files (`$.fs.list` and `$.fs.read`)
or fetch outbound over HTTP through the host (`$.http.fetch`). The handoff uses
the generation's inbox directory:

- It needs no credential and no open port.
- It is inspectable on disk.
- It is not subject to an organisation's web-fetch policy.
- It costs up to one poll interval of latency (1 second).

The mod's file surface is `read`, `write`, `list`, `exists` and `stat`. It has
no rename, no exclusive create and no durability contract. The protocol is
built so that it never needs them:

- **Wardian's own files** (`pending/`) are published with
  `wardian_core::atomic_file`: staged, synced, then moved into place, using
  `ReplaceFileW` on Windows as in `engine/store/checkpoint.rs`. The mod never
  sees a partial delivery.
- **The mod's records** are write-once files under names unique to one
  interaction, never rewritten and never replaced. A record counts only if it
  parses and its `generation` and `interaction_id` match its path. The one
  exception is a claim, which counts **by existence alone**, whatever its
  content (see below).
- **The heartbeat** is the only file the mod rewrites. It is diagnostic and
  readiness-only, never used to decide a delivery's fate, so a torn or blocked
  write is harmless. Wardian reads it with full sharing and keeps the last
  complete value. The mod ignores a failed write, for example a Windows sharing
  violation while Wardian holds the file open, and tries again next tick.

| Path under `inbox/` | Writer | Meaning |
| --- | --- | --- |
| `pending/<seq>-<interaction_id>.json` | Wardian | `{ schema, interaction_id, runtime_generation, text }` |
| `ready.json` | mod, once | `{ generation, session_id, mod_version }` at `session.start` |
| `heartbeat.json` | mod, every second | `{ generation, session_id, at }`, diagnostic only |
| `claimed/<interaction_id>.json` | mod, once | Written **before** `submit`. Its existence means the payload may have crossed. |
| `queued/<interaction_id>.json` | mod, once | `submit` resolved with `{ text }`: the prompt entered Claude's queue. |
| `dropped/<interaction_id>.json` | mod, once | `submit` resolved with `{ drop }`: proven not entered. Records the reason. |
| `rejected/<interaction_id>.json` | mod, once | Malformed, or for another generation. Never submitted. |

The mod handles pending files serially, in `seq` order. Before claiming one it
checks that no `claimed`, `rejected` or `dropped` record exists for that
interaction, and it calls `submit` only after its claim write has resolved. A
failed claim write therefore means no submit. Because the module never reloads
(see above) and a generation has one Claude process, there is never a second
claimer.

### Submit outcomes

`$.prompt.submit` resolves to `{ text, ... }` when the prompt entered, or to
`{ drop }` when it did not, for example after the user stopped a turn or a
hook refused it. The mod records each case distinctly:

- `{ text }` → `queued/`.
- `{ drop }` → `dropped/` with the reason.
- A rejected promise or an unexpected shape → no outcome record. The claim
  stays alone, which means "uncertain".

The probe saw `submit` resolve within about 30 ms while another turn was
running, that is, on entry to the queue. The API text describes a turn "once
the session is idle". The design depends on neither reading: the delivery loop
simply waits for `submit`, and the heartbeat runs on its own timer, so a slow
`submit` never stalls liveness.

### Delivery states and arbitration

The mod reuses the existing `NativeDeliveryPhase` vocabulary
(`crates/wardian-core/src/native_transport.rs`), recorded with transport kind
`ProviderPlugin`. That kind already exists in `DeliveryTransportKind` and is
otherwise unused for Claude.

| Evidence | Phase |
| --- | --- |
| `pending` published | `SubmittedUnconfirmed`, not `Dispatching`, so the broker never retries it automatically |
| `queued` record | `ProviderAccepted` |
| Transcript user row "The wardian plugin sent a message:" containing the `interaction_id`, in the session named by the claim | `TurnStarted` |
| `dropped` or `rejected` record | `FailedBeforeSubmit`, with the recorded reason |

**Arbitration happens through generation ownership, not timing.** Once a
delivery is published into a generation's inbox, it belongs to that
generation's mod. Wardian never withdraws or reroutes it while that
generation's Claude process may be alive, however stale the heartbeat looks.
When the generation ends, Wardian resolves each unfinished delivery:

1. **Prove exit first.** Wardian proves the generation's process tree has
   exited, through the kill-on-close containment job required by the Claude
   containment spec and used for #1662. The mod is then provably gone, so no
   claimer remains.
2. **No claim record:** the mod never called `submit`, so the delivery is
   `FailedBeforeSubmit` and is redelivered as a new attempt, to the next
   generation or the terminal fallback.
3. **Claim but no outcome:** the delivery stays `SubmittedUnconfirmed`. It is
   reconciled only by the transcript row and never resubmitted. This matches
   the broker rule that an uncertain submission is not replayed.

If exit cannot be proven, the deliveries stay as they are and Wardian reports
them as uncertain. It does not guess.

### Prompt text

The submitted text is the existing delivery prompt
(`information_delivery_prompt`, `reply_continuation_prompt`, and the task
prompt), unchanged. The engine already frames it as a plugin message, so the
model sees who sent it. The `interaction_id` inside the canonical context is
what transcript reconciliation matches.

### Readiness and routing

The routing decision for an interactive Claude target
(`delivery/live_surface.rs`) is made **before** publication, and only then:

1. The current generation's `ready.json` exists and parses, and its
   `session_id` equals `launch.json`'s `expected_claude_session_id`. A mismatch
   means the session is not the one Wardian launched, so the generation is not
   ready and Wardian logs why.
2. Its heartbeat reports the same generation and is fresh, at most 5 seconds
   old.
3. If both hold, Wardian publishes to the inbox. Otherwise it uses today's
   terminal path and records why: `mod_not_loaded`, `session_mismatch`,
   `heartbeat_stale` or `generation_mismatch`.

A wrong readiness answer can only delay a delivery, never duplicate it. A
delivery published into a mod that has silently died waits until its
generation ends, then is redelivered under rule 2 above. While it waits,
Wardian shows it as "waiting on the Claude delivery mod", with its age, so the
stall is visible.

The mod can be missing for several reasons: an older Claude Code without the
mods API, `--safe-mode`, `disableAllHooks`, organisation policy, or a load
failure. Each of these fails readiness, so delivery degrades to current
behaviour rather than silently dropping messages.

### Session changes

`/clear`, or a resume inside the running process, changes Claude's session ID
without changing the generation. Every mod record carries the session ID that
was current when it was written, and transcript reconciliation looks in that
session's transcript. The heartbeat reports the current session ID. After a
change, readiness for **new** deliveries also requires the heartbeat's session
to be one Wardian has recorded for the agent through its existing
session-tracking rules. Until Wardian has recorded it, new deliveries use the
terminal fallback with the reason `session_mismatch`. Deliveries already
published are unaffected, because they are bound to the generation.

## Non-goals

- The user's own prompts from Wardian's Chat composer keep the current path for
  now. Routing them through the mod (`asUser: true`) is a possible follow-up
  once the mod path has proven itself.
- Permission prompts and approvals are unchanged.
- Claude's cross-session inbox and channels are not used. Channels remain the
  documented fallback if the mods API is withdrawn.
- No change to headless or native stream-json delivery for agents without a
  terminal.

## Verification

- **Frontend unit:** none; the routing is backend.
- **Backend unit:**
  - inbox writer atomicity and ordering;
  - per-generation namespaces, including refusal of a pre-existing differing
    `launch.json`;
  - phase projection for every record combination, including a claim with no
    outcome;
  - end-of-generation resolution, with and without proven exit;
  - routing for each fallback reason, including `session_mismatch`;
  - environment composition that appends to an existing
    `CLAUDE_CODE_PLUGIN_DIRS`;
  - stale-claim reconciliation with no replay.
- **Mod:** `claude plugin validate`, plus `claude plugin test` cases covering:
  ordering; generation mismatch; claim-before-submit; a `{ drop }` result; a
  rejected `submit`; and a heartbeat write that throws.
- **Native E2E:** a real interactive Claude launched by Wardian, with a peer
  message delivered while the user has a draft. The draft must survive, the
  reply must arrive, and no terminal keystrokes may be written by the delivery.
  A second case removes the mod and shows the terminal fallback with the
  recorded reason.

## Risks

- **The mods API changes.** Its type declarations are marked "EARLY ACCESS:
  this surface may change between releases without notice". The mod uses
  only six calls (`session.start`, `$.clock.every`, `$.fs.*`, `$.session.id`,
  `$.prompt.submit`). The heartbeat fallback keeps delivery working through a
  breaking release, and native E2E pins the Claude Code version it was
  verified on.
- **Dialogs.** Claude's own onboarding dialogs can appear in a session. The
  probe showed a plugin submission still delivered while one was open, and
  nothing is typed, so a dialog cannot capture a delivery.
- **Polling cost.** A one-second `$.fs.list` per agent is negligible next to
  Claude's own file watching.
