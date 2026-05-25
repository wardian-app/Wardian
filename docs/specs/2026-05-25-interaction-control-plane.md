# Interaction Control Plane

## Summary

Issue #331 should be implemented as a native interaction control plane, not as
another timing patch around PTY injection. Wardian needs one backend-owned source
of truth for inter-agent communication, delivery state, replies, and Queue
evidence. Provider terminals remain important runtime surfaces, but PTY text
injection is a transport, not the state model.

This spec targets the 0.3.7 reliability gate. It should make status, Queue, and
structured communication predictable enough that the release can safely depend
on them.

## Goals

- Make `wardian send`, `wardian ask`, and `wardian reply` operate on durable
  backend interactions.
- Remove fixed-delay assumptions from Codex delivery. Delivery must wait for
  readiness evidence, not sleep and hope.
- Preserve provider-specific runtime status as the authority for provider
  internals such as permission prompts.
- Create Queue evidence from canonical live events, not startup hydration or
  replayed terminal logs.
- Make structured asks complete only through explicit replies, never prompt
  echoes, output markers, or terminal repaint text.
- Add regression coverage at the lowest layer that can prove each guarantee.

## Non-Goals

- Full Workflow HITL gate nodes.
- Richer Queue card types for artifact review or workflow decisions.
- Provider-specific subagent implementation inside Codex CLI.
- Replacing provider-specific status managers with a generic interaction state
  machine.

## Core Model

Add a backend interaction registry. The durable record should be close to:

```text
Interaction {
  id: "int_...",
  kind: "message" | "task" | "reply" | "notification",
  sender_session_id?: string,
  target_session_ids: string[],
  status:
    "created" | "queued" | "delivering" | "delivered" |
    "awaiting_reply" | "completed" | "failed" | "expired",
  trigger_policy: "notify_only" | "start_turn" | "reply_required",
  body_ref,
  parent_interaction_id?: string,
  delivery_attempts: DeliveryAttempt[],
  created_at,
  updated_at,
  completed_at?
}
```

`wardian send` creates a `message` interaction. `wardian ask` creates a `task`
interaction with `reply_required`. `wardian reply` creates or attaches a `reply`
interaction that completes the parent task.

Large bodies should continue to live as files under the target agent habitat,
with database references. Mailbox entries should become pending delivery work
for an interaction rather than a separate source of truth.

## Delivery and Readiness

Delivery is event-driven and generation-aware. Each live runtime has an input
generation that increments when the provider process is spawned, resumed,
cleared, or reattached.

```text
ProviderInputState {
  session_id,
  generation,
  state: "unknown" | "booting" | "ready" | "busy" | "action_required" | "unavailable",
  ready_evidence: "provider_event" | "prompt_detected" | "title_detected" | "manual_status",
  observed_at
}
```

The delivery router uses this state:

- If the target generation is ready, create a delivery attempt and submit through
  the provider transport.
- If the target is booting, busy, action-required, off, or missing an input
  sender, keep the interaction queued with a reason.
- When a fresh readiness signal arrives for the same generation, drain queued
  interactions exactly once and in order.
- Ignore readiness/status events from prior generations.
- If payload submission starts but final submit fails, mark the attempt with a
  precise partial/failed phase. Retry only when the failure is known to be safe.

For Codex, prompt detection can be readiness evidence for this release. It must
not be implemented as a fixed delay before injection.

## Status Boundary

Provider runtime status remains authoritative for provider-internal state.
Interaction state tracks Wardian-owned communication lifecycle only.

- Provider adapters own `idle`, `processing`, `action_required`, `off`, `error`,
  and provider-specific permission metadata.
- Interactions own `created`, `queued`, `delivering`, `delivered`,
  `awaiting_reply`, `completed`, `failed`, and `expired`.
- Queue projections can consume both provider runtime events and interaction
  events, but must preserve the source.

Examples:

- A Codex or Claude permission prompt creates provider-sourced Queue evidence.
  It is not a Wardian interaction `action_required` state.
- A queued Wardian ask waiting for provider readiness is an interaction
  `queued`, not provider `action_required`.
- A Wardian ask waiting for a reply is `awaiting_reply`.
- A future workflow HITL gate can create Wardian-owned action-required evidence,
  but that is separate from provider permission prompts.

## Queue and Evidence

Queue should consume canonical event projections rather than infer truth from
terminal replay.

```text
InteractionEvent {
  event_id,
  interaction_id,
  session_id,
  kind:
    "interaction_created" |
    "delivery_queued" |
    "delivery_started" |
    "delivery_confirmed" |
    "reply_received" |
    "interaction_completed" |
    "interaction_failed" |
    "provider_action_required",
  generation,
  source: "live_runtime" | "interaction_store" | "provider_runtime",
  occurred_at
}
```

Projection rules:

- Provider permission prompts create `action_needed` Queue items from
  provider-sourced live events.
- Agent completion cards come from accepted live provider turn completion events
  or completed interactions, keyed by stable event IDs.
- Hydration may restore existing Queue items, interactions, and statuses, but it
  must not create new Queue evidence.
- Deduplication should use stable event identity such as `(interaction_id,
  event_kind)` or provider event IDs, not a frontend time window. Existing
  time-window deduplication may remain as a UI guard only.

## Persistence

Persist these records in SQLite:

- interactions
- delivery attempts
- interaction events
- provider input generation/readiness state
- enough provider event cursor metadata to distinguish live events from replay

Startup hydration must load records as hydration state. It must not emit events
that Queue or watch consumers interpret as new live evidence.

## CLI Behavior

`wardian ask` should wait on the parent task interaction reaching `completed`,
`failed`, `expired`, or timeout. It should return the attached structured reply
when completed.

`wardian reply <request-id>` should resolve the target interaction by ID. Unknown
or expired IDs must fail deterministically. Duplicate replies must be rejected or
handled by an explicit idempotency policy.

Output-marker based waiting can remain for compatibility, but it is not the
structured ask/reply path and must be documented as weaker evidence.

## Test Requirements

Backend and native tests should prove:

- Startup hydration restores provider status but creates no new Queue
  completion/action-needed cards.
- Replayed provider logs do not duplicate Queue evidence.
- Live provider status transitions emit once per generation through the
  canonical path.
- Provider `action_required` creates provider-sourced Queue evidence, not a
  Wardian interaction state.
- `wardian ask` creates a task interaction and waits only for a structured reply
  interaction.
- Echoed request IDs, terminal prompt echoes, and marker text cannot complete an
  ask.
- A reply for an unknown or expired interaction fails deterministically.
- Duplicate replies are rejected or idempotently ignored by policy.
- Delivery before provider readiness queues instead of injecting.
- Queued delivery drains exactly once after a fresh readiness signal for the
  same generation.
- Stale readiness from a previous generation cannot drain queued work.
- The Codex provider spawn/resume path has a native/runtime regression test
  proving no fixed sleep is required for reliable first injection.

## Documentation Workstream

Implementation must update public and developer documentation alongside code:

- `docs/guide/cli.md`: explain structured `ask`/`reply`, durable interactions,
  and the distinction between structured reply waiting and output-marker waiting.
- `docs/guide/queue.md`: explain provider-sourced action-needed cards, completed
  interaction cards, persistence, and restart/replay guarantees.
- `docs/developer/ipc-events.md`: document new interaction and provider runtime
  event contracts.
- `docs/developer/pty-lifecycle.md`: document readiness generations and why PTY
  injection is transport-only.
- `docs/developer/tauri-command-reference.md`: update command contracts for
  `send`, `ask`, `reply`, watch delivery snapshots, and Queue evidence.

Do not update guide pages ahead of implementation in a way that implies the new
behavior already exists.

## Release Criteria

0.3.7 should not ship the interaction redesign until:

- all required backend and native tests pass;
- Queue/status evidence no longer depends on replayed terminal output for
  correctness;
- Codex first-delivery reliability no longer depends on a fixed delay;
- public and developer docs reflect the implemented behavior; and
- the release PR cites the new tests as verification evidence for #331.
