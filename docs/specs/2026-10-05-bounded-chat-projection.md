# Bounded normal Chat reads

Issue: #1593. Scope: desktop Chat and Remote Chat, including mobile. Archive
capture and full-history inspection retain their existing ownership.

## Read contract

Normal Chat uses `load_agent_chat_page`. It does not await native capture,
archive repair, archive gates or a complete replay. The authenticated remote
Chat route uses the same reader. A page carries compact headers, an opaque
older cursor, a revision, verified aliases and explicit progress. Bodies are
separate bounded requests. The legacy full-transcript command remains an
explicit inspection operation.

`load_agent_chat_transcript` retains its full `Vec<AgentChatEvent>` IPC response
for on-demand compatibility and native full-history diagnostics, including
fresh-session stale-history assertions. Its declared dead-code baseline records
that public compatibility surface. The production turn-completion service also
uses `load_agent_chat_transcript_for_state` for full transcript evidence. These
existing capture and evidence operations retain their contracts independently
of the bounded normal Chat page and detail reader.

The initial window and each older page contain at most 80 headers. Previews
contain at most 1 KiB of UTF-8 text; each body chunk contains at most 16 KiB.
The immutable object reader allows at most 512 object attempts and 1 MiB of
object bytes. A recent-source observation adds at most two 128 KiB tail reads
and two bounded 4 KiB ownership headers, 4 KiB continuity anchors and 32 KiB
source-policy reads. Each of the canonical and provisional head pointers is at
most 4 KiB. Indexed provisional pages add one such bounded ownership/policy
observation. IPC serialization is limited to 256 KiB. These are
implementation limits requiring workload qualification, not measured latency
or memory results. Counters include conservative charges for metadata and
failure paths.

An unchanged revision returns no narrative headers. A compatible refresh
returns at most 80 changed headers across a bounded chain of immutable heads.
If that chain cannot continue, the reader resets to the recent window. It
never resets to a complete transcript. Older indexing updates already loaded
headers without causing the client to load unseen older history.

## Publication and privacy

The archive owner publishes content-addressed row, body and persistent index
objects before replacing the bounded `chat-read-head.json` pointer. Native
publication follows the archive write and required private cursor/policy
commit. A failed cursor commit preserves the previous display head. Output
newlines, index counts and a manifest are not a universal committed-output
seal. Generated rows require their owned conversation identity and primary
narrative reference.

`chat-source-policy.json` is a separate bounded owner publication. Reads never
deserialize the full capture/normalizer state. Unknown earlier history,
closed disabled spans, an open disabled interval, native replacement and
policy transitions constrain provisional observations and their details.
Opaque detail references are checked against agent, source identity, policy,
generation and indexed row membership. They cannot name arbitrary paths.

Recent legacy observations are provisional. Independent owned framing can
make available prompts and replies visible while capture is behind. An
unindexed giant record does not provide a universal bounded way to find its
message body. That condition remains explicit progress and preserves visible
messages; it does not become an empty-history claim.

The existing background owner publishes `chat-source-head.json` after privacy
policy commit and before canonical acquisition or replay. It indexes recent
source headers first, then seeks backward in bounded checkpoints. The actual
conversation remains nullable until canonical admission. A cold cursor already
at EOF can therefore build older seeks without manufacturing a canonical
candidate. Immutable checkpoints resume after restart. Append intervals finish
before the next interval is admitted, so continuous output does not restart the
older walk. Source identity, privacy admission and same-extent modification
changes expire provisional cursors and details. The native continuity proof is
bounded; qualification must cover rewrite and append behavior separately.

The background writer narrows existing-record membership by sequence while
retaining full equality, including duplicate sequences. Generated input binding
skips records that cannot name a generated message. These filters preserve the
existing ambiguity and ownership rules and are independent of normal reads.

## Identity and submitted prompts

A native observation is identified by agent, native file identity, framed byte
offset, row digest and adapter output ordinal. Equal text, timing and sequence
counts do not link independent inputs. Legacy text-selected generated links
remain archive evidence; display projection removes their unverified linkage
without rewriting that evidence. Generated and native rows remain independent
and unresolved when no transport/capture correspondence proves a binding.

UUID-bearing Claude legacy native rows may retain their exact raw-line canonical
identity when the stored conversation/session/source and prior capture continuity
positively prove ownership. A versioned compatibility root in the private capture
checkpoint reserves a single physical coordinate and adapter ordinal through the
existing immutable object store. Prepared reservations are published with the old
cursor before archive work. The next cursor retains the reservation; policy and
visible heads follow. Pending reservations recover publication after restart.
Competing coordinates, missing prior proof, generated rows and ambiguous legacy
aliases cannot borrow the canonical owner. Original narrative, source and event
JSONL bytes remain unchanged. Policy or source changes retire bridge admission
while retaining immutable claim history. Seek operations retain the store budgets.

Codex user mirrors and assistant stream/final records may share a displayed row
through a separate immutable relation index. Both physical observations remain
recoverable. Qualification requires the same owned native log, admitted epoch,
provider session and turn, complementary record types, and the exact original
text or its complete digest captured before preview truncation. Repeated or
conflicting occurrences remain separate; grouping never transfers ownership,
deletes archive evidence or advances a capture cursor.

The index counts admitted occurrences incrementally. Canonical candidates must
finish counting, and acquisition must reach the exact committed source extent,
before certificates activate. Provisional backfill must finish its continuity-
qualified prefix without skipped or disabled records. A persisted activation
cursor revisits earlier buckets in bounded steps; page requests never scan the
history to establish uniqueness. Append, policy or epoch changes suppress stale
certificates until proof is rebuilt. Pages retain stable display IDs, physical
member aliases and explicit alias retractions across older reads and restart.

Codex/Pi legacy compatibility uses a separate persisted one-to-one claim based
on the original legacy ID algorithm and trusted original source sequence, with
matching owned path, session, role and native coordinate. Historical rows without
that sequence evidence remain separate. The optional sequence-free complete-
prefix fallback is disabled. These relations preserve the existing stronger
Claude UUID ownership predicate and all read, object and durability budgets.

An absent legacy checkpoint can bootstrap; an invalid, missing or corrupt modern
root fails closed. Capture assumes one current-version writer per Wardian home,
serialized by the existing per-agent owner. Mixed writers and in-place downgrade
after a reservation are unsupported: an older writer can discard unknown private
checkpoint fields. Preserve checkpoint and object store together and recover with
the current writer. This compatibility mechanism does not provide cross-process
writer locking or a general rollback system.

Native and remote prompt acknowledgements may include `chat_event_id`,
`chat_agent_id`, `chat_conversation_id` and nullable `chat_source_epoch`.
The bounded receipt writer returns the identity it actually committed; it
does not rediscover a row by scanning. Disabled logging, queued delivery,
uncertainty, changed submission scope, owner contention or archive failure
produce no receipt. Provider acceptance stays successful when this optional
archive work fails, so callers must not replay the prompt for a receipt.
Full archive summaries are maintained by the existing background owner.

Both clients consume a submitted row only when that exact generated identity
and all receipt scope fields match a returned canonical header. An
acknowledgement alone does not prove a native/generated alias. Verified alias
replacement retains the visible row key. Refresh and older/detail responses
from previous agent incarnations or generations are rejected.

## Client bounds and qualification

Loaded header state is limited to 640 rows and 2 MiB of serialized UTF-8 data.
Each opened body retains a 64 KiB UTF-8 window; continued reads replace its
oldest bytes. Polling is completion-based and coalesces overlapping reads.
Older pages load near the top of the transcript or through the older-history
control, preserving the scroll anchor. Normal UI state contains no hidden
full-history array.

When an older page reaches either bound, the client keeps that requested page
and evicts the newer end. A page that cannot fit is rejected without advancing
its cursor. While browsing older history, refreshes update or remove loaded
rows and apply verified aliases; unseen recent rows do not move the window.
**Jump to latest** starts a fresh bounded recent read. Submitting from older
history also returns to the recent window. Scroll restoration uses a visible
row's stable key because prepend and eviction can leave total height unchanged.

Headers retain bounded tool-argument previews, complete bounded file paths and
structured edit/write totals computed from the original input. Clipped patches
are marked partial; full arguments and artifact output remain available through
lazy body chunks. Native provisional details use the same representation and
enforce a 16 KiB UTF-8 chunk bound with scoped continuation references.

A same-conversation reload preserves a pending usable first page and queues one
refresh after it settles. It does not invalidate that request. Remote Chat keeps
its 60-second read deadline active until the response body has finished decoding;
an expired Chat read preserves the loaded rows and allows a local retry.
Submission acknowledgements are fenced to the agent activation and any known
conversation and source identity on both clients.

Desktop Clear retires the current read/detail/acknowledgement activation,
empties cached rows, revisions and cursors, and releases the retired send's
busy state. Its physical pending page read settles before a fresh read starts.
A serial guards send completion so the retired acknowledgement cannot unlock
a newer submission. This lifecycle reset is distinct from ordinary reload.

Acceptance requires focused publication-failure, privacy-span, forged-detail,
giant-record, revision, burst, repeated-input, alias and scope-cancellation
regressions. Retained large native archives must separately qualify recent
visibility, older/detail access and fixed budgets. Browser mocks, source
inspection and hosted CI do not establish real-provider archive behavior.

Cold saved archives are admitted by the existing background owner even when
the vendor file is absent or at EOF. Each pass scans at most 24 index rows,
24 narrative rows and 24 event rows through fixed 256 KiB windows, and copies
at most four 16 KiB body chunks. Derived object reads and attempted writes each
have an 8 MiB / 8192-object ceiling. Individual JSONL envelopes larger than the
window fail explicitly. These limits describe background work, separately from
the normal reader's request budget.

Positive open index and manifest identities select the saved conversation;
workspace paths do not participate. Narrative event references establish owned
display rows. Historical native links remain unresolved until a capture owner
commits verified correspondence. Stored event IDs stay unchanged. Private
ordering keys use event-file byte offsets because old envelopes omit sequence;
a later canonical writer may replace this derived generation. Published roots,
provenance, file identities/extents and cursors resume after restart. Changed
files retire the partial generation. Older cursors in the same generation read
the latest root, so later admission remains reachable from a partial page.

Cold checkpoints persist the complete agent, provider, source-key and provider
session-list selection scope. Missing legacy bindings or any scope mismatch
reset the reader before revision shortcuts, provider reads or detail access.
The background owner retires that checkpoint and selects again under the current
scope. Unchanged polls include one bounded head decode in their read accounting.
