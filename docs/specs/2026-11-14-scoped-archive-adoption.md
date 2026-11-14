# Scoped archive adoption after source replacement

An explicit operator grant may import a fixed interval of a replaced Codex log
into one existing, enabled, open conversation. Native capture remains blocked
by its original replacement reason. The grant does not translate an old file's
privacy offsets onto the replacement or authorize source renewal.

The operator supplies an authorization UUID, recorded user approval reference
and wording, exact agent/conversation/provider session, canonical source,
replacement native identity, and numeric start/end. The start must equal the
old committed cursor and match its continuity anchor. Preparation pins complete
JSONL records within the supplied end and stores content witnesses; subsequent
growth never expands the grant. Source identity, owned session, framing and
content must remain verifiable.

The canonical authorization is the synced, append-only per-agent
`archive-adoption-history.jsonl` record. It records actual OS issuer, managed
relay, user approval evidence, exact selection, and current restriction/config
fingerprint. Issuer identity alone does not establish consent. Audit records
describe authorization only. The independently atomically persisted
`archive-adoptions/<authorization-id>.json` job records bounded progress and a
separate completion or retirement result. A torn audit fails closed.

Each import pass holds the existing capture policy gate and per-agent archive
gate, in that order. Adoption is serviced before native acquisition's
replacement stop through the existing coalesced background owner. Each pass
uses the existing source and normalization budgets and the ordinary archive
artifact/publication path. The old capture receipt, source policy, excluded
prefix, disabled spans and native normalizer are not modified by adoption.

The job starts an independent normalizer at the granted boundary. Physical
record coordinates, digests and output ordinals determine observation identity;
authorization ID is provenance. Replaying a checkpoint or a duplicate scoped
grant cannot duplicate the same observation. Equal text at distinct coordinates
remains distinct. Full bodies belong to archive artifacts, so paging and lazy
detail never reopen unapproved native bytes. Archive display correspondence
requires the existing complete physical recount. Adoption supplies neither a
native source proof nor verified native aliases.

Archive records, bodies and provenance become durable before the adoption
checkpoint advances. A synced authorization without a job resumes job creation.
A crash after artifact/event publication replays deterministic identities and
uses ordinary narrative recovery before advancing. Derived projection may still
be pending after import completion; it finishes through the gated archive-only
handoff. Quiet passes do not reopen the import.

A complete, scope-bound cold archive cohort with no native epoch may page its
owned rows and bodies while native capture reports a recognized replacement,
truncation or continuity stop. Its read ignores the stopped native index's
pointer, preserves the capture reason, and supplies no native epoch or renewal
acknowledgement. Warm candidates, partial recounts and privacy transition fences
do not qualify for this archive-only read contract.

Qualified responses carry `archive_read_ready: true` on initial, Older, owned
detail and unchanged reads. The optional field defaults to false and is omitted
when false. It certifies a complete owned archive snapshot with matching agent,
conversation and generation, no native epoch/admission/renewal acknowledgement,
and a recognized capture stop. Post-read retirement clears the owned response
and marker. Both qualification checks revalidate the original index, manifest,
narrative and event file stamps, so a close or append cannot retain the marker
while the previous Head awaits retirement. Native cursors, preview rows and
revalidation responses cannot acquire this marker.

Desktop and remote clients share this classification before considering native
producer evidence. A first qualified archive page can bootstrap an empty chat
while preserving the capture diagnostic. Archive display aliases remain derived
presentation relations, including historical source-shaped member IDs; they do
not request native renewal. Same-generation refreshes retain loaded Older rows.
Loss of qualification, a generation/conversation change, or a transition to a
native or preview cohort retires the previous frontier. Metadata-only merges
copy qualification from the new response and cannot inherit a stale true value.
The native revalidation helper rejects marked targets and replies explicitly.

A relevant logging change, clear, source replacement/rewrite/truncation,
configuration retarget, policy change, or destination closure retires pending
work before another archive mutation. Re-enabling logging does not revive a
retired job. A time or opaque identity restriction whose position relative to
the start cannot be proved causes refusal. Existing imported data obeys normal
archive clear/privacy behavior. Other sources retain their original admission
rules.

This adds version-one audit/job documents without changing the native capture
format. Older applications do not execute the new jobs. Reverting the code does
not retract already adopted archive data; normal archive clear remains its
owner. A future schema must be explicitly recognized before execution.
