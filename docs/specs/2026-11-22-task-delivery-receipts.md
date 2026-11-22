# Exact task delivery receipts and session reset

## Decision

Automatic native terminal publication requires a delivery receipt for the task
bound to the output turn. A scheduler claim and a successful turn-start or
steer acknowledgement establish admission, rather than accepted task input.
Explicit correlated MCP or CLI replies retain their existing terminal contract.

A receipt uses existing native identities. Literal task input carries the
canonical request ID through `clientUserMessageId`. An accepted native
`userMessage` must return that exact ID in `clientId`, a stable nonempty item
ID, and the exact submitted input bytes. Its thread, runtime generation, and
turn must match the admission. Body equality without this identity join is
insufficient, including for identical tasks steered into one turn.

A function-call output can qualify only as the answer to the exact receive or
wait `call_id` issued in that same current generation and thread. A retained,
foreign, unissued, or missing call ID supplies no authority. Retired mailbox
interfaces remain retired; this rule does not reintroduce them.

No wire field or trust token is introduced. The host records the observed
receipt alongside the existing durable task-turn outbox. Migration leaves old
bindings unreceipted. Recovery validates the receipt against the original task
body and binding before terminal publication. Runtime user-message item IDs
are native acceptance identities; they are not claimed to be JSONL row IDs.

## Publication

With a valid receipt, the existing exact-turn final result and explicit-reply
precedence apply. Each task in a shared turn requires its own accepted input.
Without a receipt, the final is an informational canonical message with no
terminal reply status. The request stays open and receives the diagnostic
`delivered_without_receipt`: awaiting explicit reply or redelivery. Output,
diagnostic notice, and outbox settlement commit atomically. The notice and exact
output use the normal informational delivery route, bound to the original
requester's conversation; the notice never carries terminal reply status.
Recovery cannot create
duplicate messages or promote an unreceipted output to a terminal result.

Literal peer text remains outside trusted application context. The host's
body-free routing context retains canonical sender and request provenance.
This change does not implement the separate Chat pending-prompt bridge or
attribute forked-subagent completion under issue #1757.

## Reset boundary

Session clear and worktree changes stop and join the old native owner before
replacement. Old connections and their pending RPCs cannot write afterward.
Stored information and reply continuations belonging to the closing
conversation become stale context, while their canonical records remain
inspectable. Unclaimed tasks and captured outcomes retain their existing
contracts; unresolved observations of old live turns become uncertain.

The existing cleared-thread exclusions must be enforced before attachment
injects startup context and before provider identity is published. An excluded
historical thread cannot be adopted as a fresh replacement. A failed freshness
check blocks delivery rather than injecting into the retired conversation.

## Evidence

Storage regressions cover receipt mismatches, informational publication,
explicit completion after an unreceipted final, atomic recovery, and per-task
shared-turn authority. Owned protocol peers cover exact accepted input,
completion before acknowledgement, same-body ambiguity, and absent or foreign
receipts. Isolated lifecycle regressions exercise the production reset and
attachment paths without operating live agents or archives.
