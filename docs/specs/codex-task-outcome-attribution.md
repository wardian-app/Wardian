# Codex task-outcome attribution

## Decision

An exact provider turn is a routing boundary, not proof that its final answer
fulfills every admitted task. Compaction can restore earlier work, and several
requesters can steer different tasks into one turn. Generic terminal text must
not automatically mark those requests done.

Canonical completion requires either the existing explicit `reply` or a
versioned per-request disposition in one reserved final appendix. Results return
to the task requester. This preserves Wardian's peer model.

## Protocol

The host supplies the same instructions on native start, active steering and
the bounded `read_task_context` result after compaction. No output schema is
forced, and peer body text remains untrusted. Human instructions retain priority.

The final appendix has column-zero opening and closing lines
`<wardian_task_outcomes>` and `</wardian_task_outcomes>`, separated from ordinary
prose by a blank line. Its JSON is `schema_version: 1` with `outcomes`, each
containing only `request_id`, `status` and `result`. Status is `done`, `failed` or
`blocked`. At most 16 unique request IDs and nonempty UTF-8 results of at most
64 KiB each are accepted; the JSON packet is bounded at 512 KiB. Code fences,
quotations and HTML examples cannot authorize outcomes. Prose is preserved.

The entire packet validates before any positive write. Every entry must match a
durable provider-accepted claim and the actually observed original turn,
recipient, generation and provider session. Start and steer share this rule.
Unknown, foreign, stale, ambiguous, malformed or oversized entries reject the
whole packet. A valid subset can settle only the named tasks. Explicit replies
always win, including between capture and outbox publication.

## Finished turns without attributed outcomes

A known terminal turn with an unresolved task remains awaiting reply. An
additive observation table retains its exact binding, provider terminal status,
full-answer SHA-256 and byte count, bounded UTF-8 preview, diagnostic and
optional requester information ID. Recording an observation and its requester
information availability is atomic. Identical repeated observations produce no
new availability; conflicting evidence is rejected.

This information wakes an existing mailbox wait and follows informational
delivery without starting an idle requester. It is not a structured completion
reply. Interruption, failure, empty answers and oversized prose do not invent
task dispositions. A later genuine explicit reply can still complete the task.

Known finished bindings are excluded from observation jobs and compaction task
restoration. Startup preserves them instead of marking their continuity lost.
Already captured positive outcomes recover through the existing terminal
outbox without provider execution. Lost continuity remains uncertain. Automatic
carryover to another turn would require new exact ownership and context
evidence; this change does not introduce a carryover scheduler.

The old binding-state constraints and already published historical replies are
preserved. Agent deletion removes observations explicitly even when SQLite
foreign-key enforcement is disabled. Observation capture, positive outcome
recording and informational admission commit together; canonical reply and
requester availability retain the existing atomic outbox publication boundary.

## Required evidence

Parser tests establish framing and bounds. Storage regressions establish
whole-packet eligibility, wrong-scope negatives, per-request routing, explicit
precedence, idempotency, restart recovery, migration and deletion. Existing local
protocol loopback tests establish native observer and mailbox behavior.

Those tests do not establish model efficacy. Acceptance also requires the stock
provider's native start and steer paths and automatic compaction on the original
turn, with observed recovery call identity, correct task-specific result and a
wrong-scope negative. Synthetic peer events, manual compaction or a new turn are
not substitutes for that acceptance.
