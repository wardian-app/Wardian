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
queued until idle, used a documented non-preview API, and needed no launch
dialog. All of these were confirmed by probe on Claude Code 2.1.322 on Windows.

## Design

### Packaging and loading

- The mod source lives in `src-tauri/resources/claude-delivery/`, next to
  `pi-messaging`. It holds `.claude-plugin/plugin.json` (name `wardian`),
  `hooks/hooks.json` and `hooks/register.ts`.
- At each interactive Claude launch, Wardian materialises a per-launch plugin
  folder at `<wardian-home>/agents/<agent-id>/claude/delivery-mod/`. It
  contains the bundled files plus a `launch.json` written by Wardian:

  ```json
  {
    "schema": 1,
    "agent_id": "<wardian agent uuid>",
    "runtime_generation": 7,
    "claude_session_id": "<the --session-id or resume id Wardian passed>",
    "inbox": "<absolute inbox directory>"
  }
  ```

- Wardian adds that folder to `CLAUDE_CODE_PLUGIN_DIRS` in the launch
  environment. It appends to any existing value with the platform path-list
  separator, never replacing it. This sits beside the variables
  `claude_terminal_runtime_env` already sets in `src-tauri/src/manager/mod.rs`.
  No install, marketplace or dialog is involved.
- The plugin name `wardian` makes the transcript row read "The wardian plugin
  sent a message:", which the user and Wardian can both recognise.

### Handoff: files, not a socket

A mod cannot listen inbound. It can poll files (`$.fs.list` and `$.fs.read`)
or fetch outbound over HTTP through the host (`$.http.fetch`). The handoff uses
a per-agent inbox directory:

- It needs no credential and no open port. HTTP would need one handed over
  through a file anyway.
- It is inspectable on disk, in line with Wardian's Markdown-as-Truth
  principle.
- It is not subject to an organisation's web-fetch policy, which could refuse
  `$.http.fetch`.
- The cost is up to one poll interval of extra latency (1 second), which is
  irrelevant for agent messaging.

Layout under `<wardian-home>/agents/<agent-id>/claude/inbox/`:

| Path | Writer | Meaning |
| --- | --- | --- |
| `pending/<seq>-<interaction_id>.json` | Wardian | A delivery for one generation. Written to a temporary name and renamed into place, so the mod never reads a partial file. |
| `claimed/<interaction_id>.json` | mod | Written before `submit`. The payload may be about to cross into Claude. |
| `queued/<interaction_id>.json` | mod | Written after `submit` resolves. Claude has queued the prompt. |
| `heartbeat.json` | mod | `{ generation, session_id, at }`, rewritten every poll tick. |

Pending files carry `{ schema, interaction_id, runtime_generation, text }`. The
mod processes them in `seq` order, one per tick. It skips, and moves to
`rejected/` with a reason, any file whose generation differs from its
`launch.json`.

### Delivery states

The mod reuses the existing `NativeDeliveryPhase` vocabulary
(`crates/wardian-core/src/native_transport.rs`), recorded with transport kind
`ProviderPlugin`. That kind already exists in `DeliveryTransportKind` and is
otherwise unused for Claude.

| Evidence | Phase |
| --- | --- |
| Pending file written | `Dispatching` |
| `claimed` present, no `queued` | `SubmittedUnconfirmed`, never retried automatically |
| `queued` present | `ProviderAccepted` |
| Transcript user row "The wardian plugin sent a message:" containing the `interaction_id` | `TurnStarted` |

A stale `claimed` entry, left after a crash between claim and submit, is
reconciled forward only by the transcript row, never by resubmitting. This
matches the broker rule that an uncertain submission is not replayed.

### Prompt text

The submitted text is the existing delivery prompt
(`information_delivery_prompt`, `reply_continuation_prompt`, and the task
prompt), unchanged. The engine already frames it as a plugin message, so the
model sees who sent it. The `interaction_id` inside the canonical context is
what the transcript reconciliation matches.

### Readiness and fallback

The routing decision for an interactive Claude target (`delivery/live_surface.rs`)
becomes:

1. If `heartbeat.json` reports the current generation and is fresh (no older
   than 5 seconds), deliver through the inbox.
2. Otherwise, use today's terminal path. Record in the delivery attempt why:
   `mod_not_loaded`, `heartbeat_stale` or `generation_mismatch`.

The mod can be missing for several reasons: an older Claude Code without the
mods API, `--safe-mode`, `disableAllHooks`, organisation policy, or a load
failure. A missing mod therefore degrades to current behaviour rather than
silently dropping messages. A pending file that has not been claimed within 30
seconds of a stale heartbeat is withdrawn and re-routed. It was never claimed,
so nothing crossed into Claude.

### Session changes

`/clear` or a resume inside the running process changes Claude's session ID.
The mod reports `$.session.id()` in each heartbeat, and Wardian keeps its
existing session-tracking rules. Inbox entries are bound to the Wardian
runtime generation, not to the Claude session ID, so a `/clear` does not strand
pending deliveries.

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
  - generation rejection;
  - phase projection from the `claimed` and `queued` files;
  - fallback routing for each reason;
  - environment composition that appends to an existing
    `CLAUDE_CODE_PLUGIN_DIRS`;
  - stale-claim reconciliation with no replay.
- **Mod:** `claude plugin validate`, plus a `claude plugin test` case covering
  ordering, generation mismatch and claim-before-submit.
- **Native E2E:** a real interactive Claude launched by Wardian, with a peer
  message delivered while the user has a draft. The draft must survive, the
  reply must arrive, and no terminal keystrokes may be written by the delivery.
  A second case removes the mod and shows the terminal fallback with the
  recorded reason.

## Risks

- **The mods API changes.** It is documented without a preview label, but it
  is young. The heartbeat fallback keeps delivery working through a breaking
  release.
- **Dialogs.** Claude's own onboarding dialogs can appear in a session. The
  probe showed a plugin submission still delivered while one was open, and
  nothing is typed, so a dialog cannot capture a delivery.
- **Polling cost.** A one-second `$.fs.list` per agent is negligible next to
  Claude's own file watching.
