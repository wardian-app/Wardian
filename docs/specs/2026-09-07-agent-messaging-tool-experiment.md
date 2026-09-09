# Agent messaging tool experiment

Status: initial adapter implemented and fresh-Astra experiment completed.
The unpublished forward contract is superseded by
[Codex v2 messaging](./2026-09-07-codex-v2-messaging.md); the experiment remains
historical evidence.

## Decision

Provider-native integration must make Wardian agents usable through ordinary
model-facing delegation tools. Structured provider transport alone does not
meet that goal. Start with sending a message, then measure whether a fresh
Astra session naturally uses concise coordination language. Do not replace
provider built-in handlers or add a full subagent lifecycle before this
experiment establishes a need.

## First contract

Expose `send_input` through `wardian mcp serve`, a local standard-input/output
MCP server. Its arguments are `target`, `message`, and optional `interrupt`.
The target is one exact Wardian agent name or UUID. The message is literal
text; the adapter must not rewrite, summarize, or add a compression prompt.
Unknown arguments and broadcast selectors are rejected.

The existing Wardian control endpoint and delivery broker retain authority for
sender identity, target resolution, routing, queuing, and delivery evidence.
The server is an adapter, not another mailbox or provider launcher. Ordinary
messages use queue-until-idle policy. This first contract rejects
`interrupt: true` before submission: generic built-in interruption is not the
same operation as Wardian's explicit invalidate-premise steering.

A successful tool result includes a real Wardian interaction identifier as
`submission_id`. A submission receipt does not claim provider acceptance,
completion, or a reply. Errors must preserve uncertainty and never trigger an
automatic replay. The bridge provides neither arbitrary shell execution nor
agent lifecycle changes. Starting it is explicit; this experiment does not
modify global provider configuration.

## Experiment

Use a new Astra provider thread in an empty, isolated workspace and provider
home, retaining only the authentication needed to access the selected model.
Do not copy this conversation, project instructions, memories, skills, or
past provider sessions. Record the actual model, effort, tool definitions,
configuration, plain-English task, and visible message/tool results. Provider
system instructions remain; claim isolation from this task's context, not an
absence of all model instructions.

Give the agent an ordinary coordination task and the identity of another test
agent. Do not mention token efficiency, compressed language, preferred tool
names, or sample messages in the task. Keep the recipient information and
task content fixed when comparing tool surfaces. First prove literal delivery
through the normal Wardian path independently of model style.

Assess tool discovery, correct target, semantic completeness, factual
submission reporting, message length, and observed coordination style
separately. If token counts are unavailable, label character/word counts as
such; do not call them token measurements. One trial can establish feasibility
or expose a concrete problem, but cannot establish a general behavioral rate
or a training cause. Report ordinary prose as ordinary prose, not as a failed
transport. Preserve unsuccessful trials and avoid coaching retries.

## Implementation sequence

- Implement the narrow MCP adapter and protocol/semantic regression tests.
- Prove its call and literal message receipt against an isolated native
  Wardian runtime, with queue-policy wiring covered at the control boundary.
- Run the fresh Astra task with the same adapter and a real recipient agent.
- Preserve a sanitized report and classify messaging correctness separately
  from language behavior; use that result to decide the next experiment.
- Run affected authoritative checks, obtain an independent local-agent
  review, and publish one issue-linked pull request without merging.

## Open boundaries

Unsolicited completion notifications, waiting, spawning, context inheritance,
rich input items, provider-neutral interruption, and built-in handler
replacement are intentionally outside this first tool. Tool availability in
an interactive CLI and a background session does not, by itself, remove the
recipient's terminal delivery path. The conformance fixes remain separate
work and their existing evidence must not be overwritten by this experiment.

## Initial evidence

The [fresh Astra experiment](../research/astra-messaging-tool-experiment.md)
found that the tool was discovered and used correctly. With explicitly
authorized test-scoped MCP permission, the actual message reached native
completion. Astra used ordinary prose in both observed trials; familiar
message-tool arguments did not by themselves produce compressed wording.
Handler replacement remains an option to evaluate, not an accepted next step.
