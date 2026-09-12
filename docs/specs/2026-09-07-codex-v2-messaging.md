# Codex v2 messaging

Status: implementation in progress. Supersedes the forward contract in the
[initial messaging experiment](./2026-09-07-agent-messaging-tool-experiment.md).
Tracking issue: [#1218](https://github.com/wardian-app/Wardian/issues/1218).

## Decision and ownership

Use Codex v2's separation between information, follow-up work, and interruption.
Wardian send follows `send_message`; ask follows `followup_task`; interruption
is a separate operation. Sending and receiving must share these semantics.
The mailbox implementation may change to satisfy the contract.

The existing interaction store remains authoritative for message bodies,
sender and recipient UUIDs, task identity, replies, and semantic status. MCP
is a model-facing adapter. Provider sessions supply delivery mechanisms and
evidence. Neither provider thread IDs nor MCP connections replace Wardian
identity. No provider handler replacement is selected.

## Operations

| Tool | Behavior |
| --- | --- |
| `send_message(target, message)` | Admit information addressed to one agent. Never start or interrupt a turn. |
| `followup_task(target, message)` | Admit work and return a request ID promptly. Start an eligible idle agent or deliver through a supported active receiver boundary. |
| `receive_messages(...)` | Read a bounded, ordered batch of typed information, tasks, and replies addressed to the calling agent. |
| `reply(request_id, status, message)` | Complete the authorized task and make its correlated result available to the original requester. |
| `interrupt_agent(target)` | Request interruption of the current turn while preserving the session. Report unsupported capabilities before submitting anything. |
| `list_agents()` | Discover Wardian peer identities available to the caller. |

Names resolve once to exact UUIDs; ambiguous names fail. Sender and receiver
identity come from the managed caller context, never tool arguments. Literal
message text is retained independently from any provider-specific envelope.

The additional receive and reply tools make Wardian's peer-to-peer correlation
explicit. Codex's automatic child-to-parent completion is not a substitute for
replying to a particular Wardian request, especially when several peers assign
work to the same agent.

Ordinary chat and native `send` messages retain ordinary input semantics and
complete through assistant output. They must not be labeled as reply-required
peer tasks. Only canonical tasks with an explicit `request_id` use MCP `reply`;
an ordinary delivery's interaction ID does not authorize task completion.

## Receiver and durable mailbox

Represent information as `Message + NotifyOnly`, work as `Task + ReplyRequired`,
and replies as `Reply + NotifyOnly` with the task's ID as
`parent_interaction_id`. Only work can enter runnable dispatch. Information
must never drain into a new prompt merely because a receiver becomes idle.

Index recipient availability with monotonic sequence numbers and references
to canonical interaction records. Do not duplicate bodies into another inbox
store. Admission and availability become durable atomically. Acknowledging
receipt and completing a task are separate operations.

Receive cursors are versioned and bound to the authenticated recipient.
Pagination is bounded; acknowledging a previously issued batch is explicit.
Replaying a cursor returns stable message identities and does not execute work.
Expired or invalid cursors fail explicitly. Waiting times out without deleting
messages, cancelling tasks, or authorizing a resend.

Provider push and tool-based receive must share delivery ownership. They must
not independently submit the same task. Provider visibility requires defined
provider evidence or receiver acknowledgement; writing bytes is insufficient.
No exactly-once model consumption guarantee is made across crashes.

## Tasks, replies, and interruption

Admission returns before task completion. A reply must originate from the
task's recorded recipient. Completing the task, saving its reply, updating the
existing structured-reply projection, and making the reply available to the
requester form one transaction. Identical repeated replies may reconcile to
the same result; conflicting terminal replies fail.

Before background provider submission, a definite startup failure may terminate
the current scheduler claim with a Wardian-attributed `failed` reply. Publication
and claim settlement are atomic and generation fenced. This infrastructure reply
is distinct from a model-authored response. Accepted or uncertain delivery never
permits this conversion or automatic replay.

Accepted background tasks wait for Codex history initialization without a fixed
response deadline. Individual connection attempts and protocol writes remain bounded.
Cancellation of owner preparation, owner disposal, or lease loss cancels initialization and
joins the owned child before releasing ownership. Interactive startup retains its
deadline. Slow history indexing remains a separate performance concern.

An interrupt addresses the observed current turn in the current runtime
generation. It does not remove the agent, clear its history, or imply that
queued work should run next. Requested and confirmed interruption are distinct.
An unsupported live surface must not fall back to arbitrary terminal keys.

Idle, actively working, paused, and deleted identities are distinct. Background
execution follows the agent's existing explicit execution policy. Information
can remain pending for a retained inactive identity; it cannot implicitly
resurrect that identity. Deleted recipients reject new admission.

## Codex integration and compatibility

One broker-owned Codex app-server provides the native session for both
background work and the original interactive TUI. The ordinary TUI discovers
the private local socket under the same managed Codex home. This preserves
Codex's local-workspace behavior; explicit `--remote`, including a Unix socket
address, selects different upstream behavior. Wardian connects through the
configured executable's `app-server proxy --sock` raw WebSocket tunnel.
MCP supplies model-facing operations and explicit receiving; it is not the
transport for unsolicited provider input.

Interactive preparation leaves the new daemon's loaded-thread list empty.
The TUI must be the first loader. Before Wardian subscribes or delivers peer
traffic, it requires exactly one loaded thread matching the selected resume
identity when one exists. A fresh loaded thread receives legitimate developer
identity context to materialize its history before the controller resumes it
to subscribe and validate direct input. This starts no model turn.

Cold background resume supplies configured model and effort through the supported
per-thread request fields; daemon configuration alone does not prevent stock
Codex from clearing an absent persisted effort. The ordinary TUI uses the replayable
`--model` argument to select current configuration on resume, with effort supplied
by its aligned private configuration. It still uses implicit local attachment.
An effort-only preference resolves a launch-only model from unloaded conversation
metadata, effective configuration, then the daemon's catalogue using stock default
selection order. It does not write an explicit model into agent settings. These
reads must leave the daemon's loaded-thread list empty, and the bound response must
match the resolved model and requested effort. Both unspecified preferences retain
stock inheritance behavior. An already-loaded controller resume does not attempt
to reconfigure the TUI's thread.

Missing or ambiguous attachment evidence fails startup. An already-loaded
background owner must exit before this interactive attachment sequence. Loaded
membership establishes a bounded startup observation; it is not a receipt
identifying a TUI subscriber or proof of continuous subscription. The existing
generation-bound owner and terminal exit guards remain necessary.

The local-daemon integration targets stable CLI `0.154.0` or later, with an
explicit test exception for `0.154.0-alpha.6`. Version eligibility never replaces
actual home, protocol, effective launch settings and attachment checks. Windows
transport probes of that exact alpha passed with a local model stub; those probes
do not establish Wardian MCP acceptance or execution on macOS/Linux. The default
socket path must fit the platform's path limit; managed-home layout remains an
implementation constraint until its migration and real harness pass.

### Temporary compact homes

Keep the provider executable unmodified. Until upstream local TUI discovery
supports long canonical homes, move an affected managed home to a private,
durable compact location and retain `habitat/.codex` as an owned directory link.
Short homes remain in place. Path admission counts the canonical socket's
encoded bytes, including the platform-specific limit and terminating NUL;
short aliases to a long physical home do not satisfy this constraint.

Allocation first considers a compact directory within the Wardian home, then
native per-user locations and platform-specific protected durable fallbacks.
Temporary directories must not own persistent provider state. If no candidate
is secure and short enough, startup must report that constraint before any
provider or peer delivery starts. This is not a claim that every filesystem
policy or redirected profile permits a suitable location.

The original agent directory and compact slot carry matching ownership records.
Only that verified mapping authorizes config or MCP writes through a habitat
link. Migration, configuration projection, and index observation share a
preparation fence. Physical migration requires the old provider generation to
have exited; generic refresh may resolve a completed mapping but cannot initiate
migration or recover a live launch overlay. Recover an interrupted launch overlay
before moving its home or reconciling config.

Detached Codex TUI/background handles enter a retained stop record before the
lifecycle operation awaits further work. Only observed child exit clears that
record. Caller cancellation, failed termination and timeout retain both handles
and the startup fence; a new owner checks it before preparing or moving a home.
An explicit lifecycle retry addresses the same retained processes and does not
replay provider messages.

Migration preserves unknown provider files and nested links, including a shared
sessions projection, without traversing them. Prefer a same-volume rename.
Cross-volume transfer must verify the staged copy before switching the habitat
path and retain the original data on failure. Interrupted transitions recover
from their recorded ownership and phase; ambiguity never authorizes replacing
either home. Home relocation does not itself repair pre-existing shared-rollout
writer-lock namespace differences.

[Removal issue #1235](https://github.com/wardian-app/Wardian/issues/1235) tracks
retirement after an upstream release passes ordinary local TUI, long/Unicode
home, resume, restart and messaging acceptance on all supported platforms.

Both local clients must load the selected launch settings. Interactive startup
temporarily overlays generated settings in the private home, then restores
still-owned values after attachment and policy validation, before publishing
the capable binding. A private write-ahead journal records only affected leaves
and fences stale cleanup with a token. Recovery preserves external edits and
precedes generation of arguments derived from configuration, such as additional
writable roots. Background owners retain their CLI overrides and create no file
overlay. Neither path edits the user's global configuration.

Information uses `thread/inject_items` with a named host tool-output item.
Work uses `turn/start` with standalone `toolOutput`. Both carry Wardian
message provenance without inventing a human prompt or an originating model
tool-call ID. Interruption uses the observed thread and turn IDs with
`turn/interrupt`. Native acknowledgement establishes acceptance, not model
consumption or a correlated task reply.

The existing broker registry owns the server process and a continuous event
reader, including turns started from the TUI. Provider input generations and
terminal incarnations are separate. Replacement waits for the old owner to
exit; a late terminal-exit callback cannot stop its replacement. Pause,
restart, clear, failed attachment, and app exit must clean up the matching
owner. A failed shutdown retains ownership protection.

Existing embedded Codex sessions are not attached in place. Explicit restart
is the migration boundary. Background execution must acquire the existing
conversation lease and cannot launch a competing provider against an attached
owner's history. No uncertain submission is replayed after reconnect.

Managed startup registers the MCP server in the agent's private provider home
for both terminal and background launches. It preserves user-owned MCP entries
and approval settings, uses the installed native Wardian CLI, and binds the
correct Wardian home and caller identity. Global configuration is not modified.

Verify installed-version support for native information injection, active-turn
delivery, interruption, and attachment to live Codex sessions. Tool-based
receiving is an explicit model-visible boundary; it must not be described as
unsolicited delivery during sampling without evidence for that provider path.

The unpublished v1 `send_input` tool is replaced. Its experiment report and
frozen evidence remain historical. Existing public CLI callers and historical
interactions retain explicit compatibility treatment; old queued prompts are
not silently reclassified as new informational messages.

## Verification

Prove idle information causes no turn; idle follow-up work starts once;
receivers see literal payloads with correct sender and kind; concurrent requests
receive correctly correlated replies; wait timeout permits a later reply;
interrupt retains session identity; cursor replay and push/receive races do
not schedule duplicate work. Test persistence failure and foreign-caller
rejection at the lowest meaningful layer.

Use the real native harness for sender/receiver acceptance and distinguish
terminal from background evidence. Use the cheapest catalogued model suitable
for correctness checks. The earlier fresh-Astra style experiment need not be
repeated merely to reproduce spelling. Full authoritative checks and an
independent local Wardian-Reviewer verdict remain required for PR delivery.
