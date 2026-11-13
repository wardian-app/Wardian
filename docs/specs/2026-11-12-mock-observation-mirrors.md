# Mock observation mirror identity

Mock writes the same observation to stdout and its owned transcript log. An
incremental capture batch can be empty after earlier batches committed that
transcript. Treating this empty batch as an absent transcript previously
admitted the entire Watch snapshot as additional durable messages. Equal text
and a shared turn cannot establish which observations are mirrors.

## Producer and host binding

Each message, model, or info observation carries one `wardian_mirror` receipt.
The receipt includes a process incarnation UUID, a unique observation sequence,
the provider session, and whether the mirror was written. A configured log is
appended before the corresponding stdout record is emitted. Write failure
preserves stdout with `logged: false`. Restarting the same session creates a
different incarnation. Turn IDs and message content remain unchanged.

The Watch extractor and native normalizer preserve this receipt. Both compute
the digest of the parsed raw object locally; a supplied digest is never proof.
The digest can reject conflicting claims, but never supplies their identity.
The host binds a logged Watch claim to its actual runtime's provider session and
owned source path. A path supplied in streamed JSON creates no authority.
Stateful acquisition separately retains the actual Mock init header's session.

## Archive correspondence

The serialized archive writer strips caller-supplied mirror authority. Only
the actual Watch pass or a matching typed native acquisition proof can attach
its private persisted binding. Native proof includes the admitted physical
coordinate, source identity, policy, provider session, and receipt fields.

A match requires the same agent, source, provider session, incarnation,
observation, record kind, role, and semantic turn, with a consistent payload
digest. There must be one native coordinate and one Watch observation. Duplicate
receipt IDs at distinct native coordinates stop capture without publishing a
new Head. Missing, malformed, conflicting, legacy, and unlogged observations
remain independent. Existing generic source-ownership and alias rules stay
strict.

The native row owns its canonical event ID, source coordinate, and body. The
archive retains the Watch observation as audit evidence under the same
narrative. Receipted observations have distinct audit-source IDs and stable
cursors, so joining a narrative does not collide with another source record.
Already separate narratives reconcile only through the proved pair; retired
sequence markers must be backed by that persisted correspondence.
If events were replaced before the narrative replacement failed, the next
serialized append completes only that validated retirement intent before
checking sequence continuity. A fresh archive owner uses the same recovery.
Invalid or ambiguous markers still fail closed; finishing the old narrative
does not grant current source or privacy admission.

Canonical projection hides the stdout mirror only after the source cursor and
policy commit. A separate namespaced lookup binds recent Watch coverage to its
actual native owner. This lookup supplies no framed source-renewal authority,
fabricated source ID, body freshness claim, or text-derived alias. The existing
bounded recent-page, Older, detail, and client renewal checks remain in force.

Readable canonical Mock headers retain indexing progress while their admitted
source index cannot yet acknowledge renewal. Unchanged replies retain that
qualification too. This preserves early readability without advertising ready
history before the existing strict renewal path can accept an Older window.

Canonical Mock Heads carry an optional writer-produced coverage summary after
the native cursor and policy commit. It binds the source key, native epoch,
privacy admission, captured provider-log extent, and settled normalization.
Archive output size and opaque capture fingerprints do not supply this proof.
Readiness requires a ready canonical Head and complete current source coverage
of that same interval. An old ready Head cannot borrow a later source index's
coverage after an append. Missing legacy proof supplies no readiness authority;
saved conversations without a native source retain their existing path.

The first qualified response under a changed revision returns the complete
bounded canonical recent window and proven Older frontier. Readiness alone does
not reset an already loaded Older window. Stable polls acknowledge the same
qualified progress without repeatedly sending snapshots. Final source, privacy,
logical, body, and mixed-frontier validators remain in force. The coverage
summary grants no new row-renewal or body authority. Desktop and remote merge
unchanged progress and read counters while retaining loaded headers, details,
and the Older cursor.

Transcript completion polling coalesces boundary notifications before archive
capture. One worker owns a runtime status cell through the complete bounded
read, persistence, and emission round. Concurrent boundaries mark one pending
latest signal. Final release and notification admission share one lock, so a
later boundary either keeps that worker or claims a new one. Replacement and
cancellation cannot release another worker's token. Runtime identity is checked
after an awaited read and under lifecycle serialization before persistence.
This retains the existing latest-answer semantics and card IDs; it does not
guarantee delivery of every intermediate completed turn in a burst. SourceIndex
advancement remains with the ordinary coalesced archive capture owner.

## Required evidence

The production fixture must retain all 96 history turns, one lazy long body,
and exactly two equal-text completed turns with distinct identities. Coverage
must include empty and split capture batches, both publication orders, retry
after failed cursor commit, restart, ambiguous receipts, policy boundaries,
and the actual UI pagination and detail path. Producer and unit contracts are
separate evidence from successful native acceptance and hosted CI.
