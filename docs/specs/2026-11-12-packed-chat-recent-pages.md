# Packed canonical recent pages

Canonical Chat reads remain bounded, but opening one immutable AVL node and one
Row object per header makes filesystem latency significant. Native measurements
of a saved conversation attributed most canonical-read time to opening files.
Hashing and parsing were small portions of that read. The configured Terminal or
Chat presentation remains a user preference.

## Persisted representation

Each published Head may reference one separately authenticated packed recent
page. Existing Heads without this optional field continue using the tree. The
pack holds at most 80 complete bounded Row headers, their canonical row references,
and the lookahead result used for Older. Its serialized size is at most 256 KiB.
It binds the final published state, including generation and canonical root,
without containing the final Head reference and creating a hash cycle.

Normal capture, generated input, and cold bootstrap prepare the pack after their
body and logical updates. They replace or omit the previous reference at every
publication. The existing typed dependency graph includes the pack and its
canonical dependencies. Dependencies finish before the Head pointer changes.
Publication failure retains the previous usable Head and retryable work.

Source-index replay uses the same bounded checkpoint writer to omit transient
AVL versions. Before publishing its Head, it finishes the visible source root,
all five logical-index roots, and the pending replay queue root. The queue is an
explicit dependency because its reference is encoded inside durable replay
chunks. Direct payloads and chunks remain durable writes. A failed flush leaves
the previous source pointer usable and a fresh pass resumes from that pointer.
Intermediate construction attempts still spend the original allowance.

The optional optimization does not enlarge generic object limits, aggregate
read or write budgets, or pending-memory limits. Admission reserves enough
checkpoint allowance before packing. An oversized page or insufficient remaining
allowance omits the optional representation. Actual attempts, bytes, embedded
header decoding, and failed work remain charged; fallback does not reset counters.

## Read contract

Only the no-before canonical recent interval and its equivalent reset use the
pack. The reader freshly authenticates the Head, canonical root, and packed
bytes. A referenced pack that is absent, oversized, malformed, corrupt, or bound
to different state fails closed. An absent optional field uses the legacy tree.
The complete physical window passes through the existing selection algorithm,
including linked frames and certified members. Detail references receive the
current Head prefix at read time.

Privacy, scope, source admission, logical-watermark checks, and final IPC limits
remain in force. Older, deltas, revalidation, detail lookup, and mixed-source
frontier proofs retain their existing paths.

An initial empty, unavailable window can admit a reset canonical snapshot while
the older saved history is still indexing. That bootstrap requires complete
canonical identity, no producer evidence or source epoch, no prior committed
rows or Older frontier, and compatible positive scope hints. Source-qualified
partial pages retain their stricter admission rules. Initial emptiness must not
strand subsequently published owned headers until the whole archive is indexed.
Replacing a preview with a canonical snapshot ends the preview burst and retains
ordinary polling, so an indexing checkpoint can reach ready without reopening Chat.

A settled canonical capture without source-qualification work keeps its current
Head when scope, context, signatures, capture stamp, archive extent, and generated
input bindings are unchanged. No queued header, body, or logical work may be
discarded. This prevents an empty capture pass from invalidating a concurrent
renewal revision. Actual state changes retain normal publication and strict
revision validation.
The optional canonical source-coverage summary also participates in this
equality and in the packed page's authenticated Head-state binding.

The pack is a persisted representation of authenticated headers. A valid pack
does not require reading each original Row or interior AVL file again. Independent
loss or modification of one of those original files therefore need not invalidate
the packed copy. Reads that consume the original objects, including Older and
detail, continue validating them. This is an explicit representation boundary;
there is no cross-request cache of unchecked file contents.

## Verification

Packed and legacy windows must match in headers, order, aliases, retractions,
body status, and exact Older frontiers for limits 1, 8, 24, and 80. Coverage
includes linked frames, restart, append, older backfill, body completion,
certificate changes, scope and policy boundaries, stale cursors, detail bindings,
legacy Heads, oversize omission, and unchanged aggregate bounds. Corrupt or missing
Head, root, and packed objects must be detected on a later request. Faults at
publication stages preserve the old usable Head. Valid packed reads avoid
per-header files; explicit Older and detail still use their original objects.

The maintained native case retains all six timing phases and every history and
detail assertion. File-open counts and backend service timings explain a
mechanism; only that native case qualifies end-to-end behavior.
