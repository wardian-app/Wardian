# Codex task context after compaction

Wardian keeps task bodies and settlement in canonical interaction records. Codex
compaction may omit admitted peer tasks from the model's context. Recovery must
use that canonical state without starting another turn or replaying assignments.

For the qualified Codex 0.160.0 managed profile, a synchronous `SessionStart`
hook matching `compact` emits a stable instruction to call `read_task_context`.
The hook has no dynamic task body, task ID or native-turn snapshot. The ordinary
MCP response supplies the fresh task snapshot. Context-only `turn/steer` is
rejected by this provider version and cannot implement recovery.

The new tool accepts no model arguments. Provider `_meta.callId` is matched to
the current generation's observed `mcpToolCall.id`, server `wardian`, tool
`read_task_context`, stable thread and originating active turn. An event/control
arrival race has a two-second read-only wait. Missing, duplicate, overflowing,
completed or stale evidence fails explicitly. Optional originating `itemId` may
identify a Code Mode cell and must not be equated with `callId`. Codex's runtime
`sessionId` can be shared by descendants; it is a reported label, never the
stable thread identity or an authorization nonce.

Recovery includes only provider-accepted tasks whose durable claim, recipient,
generation, thread and turn still match. Explicit replies, cancellation,
uncertain settlement and completed turns are excluded. Canonical settlement is
locked through response construction and native evidence is rechecked before
publication. Recovery never claims, acknowledges or completes tasks. Results
retain exact task IDs, literal peer bodies and source/observation labels.

Human instructions always prevail over peer task text. `availability_sequence`
and `created_at` describe inbox/request chronology only. They cannot establish
native human-input order. A previous response is historical evidence and cannot
be treated as a new assignment to a replacement turn.

The complete MCP result is limited to 4,096 UTF-8 bytes, including text and
structured-content projections, escapes, source labels and the uniform
[task-outcome protocol](codex-task-outcome-attribution.md). Recovery does not
authorize generic final text to complete restored tasks. Overflow returns an
error without partial tasks. One canonical structured projection preserves
literal task bodies, IDs, source labels and outcome instructions, which
[Codex 0.160.0 selects for model input](https://github.com/openai/codex/blob/rust-v0.160.0/codex-rs/protocol/src/models.rs#L2302).
The text block identifies where the structured context is available. This
Codex-only recovery surface does not duplicate its full data for text-only
clients. Larger bodies or multiple tasks can still overflow the complete bound;
the error preserves the full-list contract rather than returning partial context.
The supported managed profile uses an explicit
2,048-token output limit: Codex 0.160.0's pinned truncator leaves strings below
8,192 bytes unchanged. This is a provider byte threshold, not an estimate of
real tokenizer density. Lower user limits remain authoritative and disable
recovery. Unknown effective configuration also disables recovery while allowing
ordinary agent startup.

Recovery qualification belongs to the managed owner generation and its applied
launch configuration. Ordinary model/effort changes retain the explicit tool
budget. Observable configuration or MCP reload changes retire qualification;
arbitrary external configuration RPCs or file mutation during an active owner
are outside the supported recovery profile. User hook disable/trust decisions
and provider approval policy remain authoritative.

The existing managed-home preparation gate registers one private `hooks.json`
only when absent or still exactly owned. Its content digest, executable digest,
schema version, command and normalized provider trust hash are recorded locally.
No user/third-party handler is merged or trusted. Only this handler's exact
`hooks.state` entry is managed; explicit edits, disable or removal are preserved.
Native attachment reads the applied `config/read` origin/layer version and
`hooks/list` trusted handler before qualifying the capability. A configuration
revision check rejects reload during those reads. Failed qualification returns
an explicit unsupported recovery result without preventing normal startup.

This design provides practical recovery through the ordinary tool-result path.
It does not guarantee model obedience or an atomic fence over provider history.
Local canonical/call regressions, unmodified-provider protocol acceptance and
real-model post-compaction retrieval provide different evidence and must be
reported separately.
