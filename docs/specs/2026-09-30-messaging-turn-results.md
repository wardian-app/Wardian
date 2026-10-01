# Requester-directed native task results

## Decision

Canonical tasks dispatched to a native Codex turn return its final result to
the task requester. Wardian retains peer-directed ownership rather than imposing
a spawn-parent tree. Information and completion delivery never start an idle
agent. Mailbox activity wakes an existing bounded `receive_messages` call.

Manual receiver claims, ordinary human input, composer delivery, and providers
without exact native completion support retain their existing explicit-reply
contract. No task association is inferred from terminal text, status, transcripts,
or the most recently observed provider turn.

## Correlation and publication

An exclusive scheduler claim precedes provider submission. Acknowledgement
persists the request ID, claim token, recipient, generation, provider thread,
exact turn ID, and start/steer operation. Several tasks may join one turn; each
gets that shared final result through its own original requester relationship.
This does not establish separate answers for individual steered tasks.

The provider connection retains exact-turn terminal observations, including
completion before acknowledgement. Before recording an outcome, the host
validates runtime generation and deletion under the interaction mutation gate.
The final outcome is committed to a bounded durable outbox before the canonical
reply transaction. That transaction publishes only if no terminal reply already
exists. An earlier explicit `done`, `blocked`, or `failed` reply suppresses the
fallback. A later conflicting explicit reply retains the existing rejection.

Startup recovers captured outcomes without submitting provider work. Bindings
whose live observation continuity was lost become uncertain. The migration
creates no historical bindings. Deleted state cannot be reconstructed by a late
receipt or recovery attempt.

## Final result

Completed assistant items are authoritative by thread, turn, and item ID.
The last item with phase `final_answer` supplies the result; commentary and tool
output do not. Legacy models without phases use the last completed assistant
item of unknown phase. Streaming deltas never extend completed-item text.

| Exact terminal evidence | Reply |
| --- | --- |
| Completed with usable final text within 64 KiB | `done`, text preserved verbatim |
| Completed with missing or oversized final text | Attributed Wardian `blocked` notice |
| Interrupted | Attributed Wardian `blocked` notice |
| Failed | Attributed Wardian `failed` notice |
| Unknown, disconnected, timed out, or stale generation | Uncertain; no invented result or replay |

## Active delivery and waiting

An observed idle Codex owner receives the existing structured `turn/start`
task. An active owner requires an exact turn ID and stable Codex 0.159.2 or newer.
It receives `turn/steer` with `expectedTurnId`, literal task text in `input`,
and body-free host routing metadata in application `additionalContext`.
The returned `turnId` must match. This version floor reflects the inspected
schema and local acceptance evidence; it does not identify the release that
introduced steering. Older supported versions retain idle and information paths.

Information and replies continue through `thread/inject_items` without starting
or steering a turn. Acknowledged history append alone does not prove immediate
model incorporation. Uncertain task submissions cannot fall back to another
transport. Prewrite deferrals retain the exclusive-claim release rules.
A recognized stale-turn rejection or proven writer-fence activity deferral
releases its unadmitted claim before reading current activity. A fresh usable
observation then schedules another dispatch opportunity, including an idle
transition whose ordinary callback ran while the claim was held.

Receive subscribes to a recipient watch before its durable read. Each loop marks
the signal seen before reading; a commit during the read therefore remains
observable. Committed admission, reply, provider-context settlement, and deletion
signal independent waiters. Notifications carry no bodies and acknowledge no
cursors. The original deadline, page bounds, and provider-context wake reason
remain unchanged. All canonical writes pass through the host service.

## Evidence

Storage tests cover exact binding, reply precedence, shared-turn fan-out,
deletion, and captured-outcome recovery without provider execution. Runtime
tests cover final-result publication, mailbox wakes, stale generations, and
completion before acknowledgement. Transport tests cover supported steer shape,
turn fencing, final-item selection, and uncertainty without replay.

A local Responses stub probe of stock Codex 0.159.2 demonstrated active task
incorporation in the same turn and zero turn starts for idle information
injection. It establishes protocol incorporation on the tested installation;
it does not establish real-model behavior or cross-platform provider acceptance.
