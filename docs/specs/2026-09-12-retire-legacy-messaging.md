# Retire legacy peer-message delivery

## Decision

Wardian peer communication uses the canonical agent-messaging service for information, tasks, receipt, and replies. Remove the older send/ask control operations and the live-surface mailbox scheduler. A Codex peer message must never fall back to pasting into its terminal composer.

Human terminal input remains a terminal operation. Provider adapters required by the canonical service remain separately owned; removing the old queue must not remove the canonical scheduler's startup and idle triggers.

Close native delivery gaps into existing provider sessions before retiring their canonical composer adapters. Retain a composer adapter only for a documented provider/session case that cannot support equivalent native delivery. A protocol codec or a separately resumed process does not establish that equivalence. A transient native failure or uncertain submission must never trigger a second submission through the composer.

For each provider, qualify idle delivery, busy-turn queueing or steering, session identity, lifecycle generation, manual-receive exclusion, and correlated completion using the real provider with terminal writes disabled. Native delivery may wait for a provider-defined turn boundary; immediate submission does not imply immediate model incorporation. Preserve the existing interactive experience and one authoritative process owner.

## Current failure

A Codex session can have a negotiated native app-server connection while legacy mailbox records still drain through its PTY. A failed composer-application check can leave the message visible with Return withheld. New native messaging does not retire that independently persisted queue.

## Existing-session integration assessment

The following are investigation leads, not real-provider acceptance results:

| Provider | Candidate input boundary | Evidence still required |
| --- | --- | --- |
| Codex | Existing shared app-server owner | Preserve canonical claims and prove removal of the independent mailbox cannot affect native delivery. |
| OpenCode | HTTP session API on the backend used by the TUI | Bind the launch-owned endpoint, authentication and exact session; preserve model, agent, permissions, fresh/resume and human interaction. |
| Pi | Supported extension messages in the running interactive process | Verify supported versions, session lifecycle rebinding, authenticated delivery acknowledgement and task/information semantics. |
| Claude | MCP channel events into the interactive session | Verify custom-channel availability, authentication and organization policy, and distinguish channel delivery from plain stream-JSON print mode. |
| Antigravity | Not yet established for the interactive process | Print-mode stream JSON alone does not prove attachment to the interactive owner. |
| Gemini | Not yet established | The provider is user-facing but absent from the current five-protocol native broker; assess it explicitly. |

OpenCode documents its [shared TUI/server architecture and session API](https://opencode.ai/docs/server/). Do not use its TUI append/submit endpoints as evidence of composer-free delivery. Claude documents [channel delivery and its availability restrictions](https://code.claude.com/docs/en/channels). Capability discovery and an isolated real-provider run must establish which of these mechanisms the installed version actually supports.

## Canonical task contract

1. Authenticate the managed sender and resolve one exact recipient.
2. Persist the task once, returning its canonical request ID.
3. Acquire a durable delivery claim shared by manual receive and automatic dispatch.
4. For an attached Codex session, validate the generation-bound native owner and submit structured task context through `turn/start` with `wardian_task_delivery`.
5. Record provider acceptance separately from task completion. Completion requires a correlated reply.
6. Preserve uncertain delivery without replay. A missing native binding does not authorize a composer fallback.

Information and replies can be read explicitly or pushed as `wardian_inbox_delivery` through `thread/inject_items`; pushing information does not create a turn. Background execution is an execution mode of this service, not another mailbox.

## Removal and persisted history

- Remove old public control variants, CLI dispatch, automatic drain callbacks, live queue hydration, and obsolete delivery tests.
- Route supported CLI and automation peer operations through canonical admission; preserve the distinction between information and tasks and the managed-origin authentication boundary.
- In-process automation is a trusted host caller, not an impersonated managed agent. Preserve its node/run attribution and wait on the canonical task's correlated reply. Any host-admission entry point must be private to trusted application code; MCP callers cannot supply that identity.
- Remove obsolete queue-policy, broadcast, and legacy reply instructions from active guidance rather than offering a compatibility route.
- Keep historical interaction and mailbox evidence inspectable. Do not reinterpret old messages as new tasks or replay pending, in-flight, failed, or uncertain legacy records.
- Retired records must be unreachable from startup, restore, status observations, and explicit delivery entry points. Preserve their original bodies, identities, and recorded submission evidence.
- Old clients receive an unsupported-operation or argument error; they cannot reactivate the deleted scheduler.

This source change takes effect when the user chooses to run the updated runtime. Development and validation must not close, restart, replace, or mutate the active Wardian instance or its live message queue.

## Acceptance

- An old wire request cannot enqueue work or write provider input.
- Restoring historical legacy rows and observing Idle produces zero PTY writes and zero new canonical tasks.
- Canonical receive and native task delivery retain exclusive claim ownership, correlated replies, and uncertainty handling.
- Native Codex information/task delivery works with the terminal write path disabled in the test.
- CLI, automation templates, bundled skills, and current guides contain no callable legacy messaging route.
- Human terminal typing and supported canonical provider adapters remain reachable.
- Remove a provider/session composer adapter only after native acceptance proves the same running conversation is reached. Record any retained exception and its concrete missing capability; do not retain a blanket fallback.
- Provider transcripts and the chat archive retain peer/task provenance; native delivery must not introduce fictitious human prompts or duplicate tool-output records.

Use isolated fixtures for migration tests and independently owned app/profile directories for native checks. Never submit or clear the active user's composer as part of validation.
