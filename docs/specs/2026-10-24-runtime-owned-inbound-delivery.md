# Runtime-owned inbound message delivery

Decision for [issue #1691](https://github.com/wardian-app/Wardian/issues/1691).
Execution evidence belongs to the issue and pull request; this document defines
required behavior rather than claiming provider acceptance.

## Problem and decision

A requester can finish its turn before a delegated task completes. Previously,
a reply could remain in the mailbox until another human prompt caused the
requester to call `receive_messages`. The reply notification path supported
Codex context injection and returned without delivery for other providers.

Wardian owns inbound delivery, provider acknowledgement and recovery. Agents
receive messages automatically. Remove model-facing `receive_messages`, CLI
`message receive`, and instructions that require polling. Keep the canonical
durable interaction store and read-only operator diagnostics.

Under the settled default, Wardian wakes an eligible idle agent for newly
admitted peer information. Explicit task nodes and their task replies remain
actionable work under the existing task lifecycle. Passive host-automation
notifications and context remain non-waking and do not use peer-message wake
eligibility. Peer information creates no assigned work or reply obligation.
Interruption remains explicit. No additional public notification mode or
replacement polling tool is introduced.

## Ownership and delivery contract

- The backend interaction and delivery records are the single durable owner.
  Provider adapters implement transport beneath that ownership. A model-facing
  reader must not compete with automatic dispatch for claims.
- Admission, provider acceptance, provider visibility and completion are
  separate facts. Persist actual transport outcomes; do not infer model
  processing or task completion from an idle status or an acknowledgement.
- An incoming task result keeps its reply kind, sender, status, interaction ID
  and parent request. Its continuation is not a new task. Do not run task-final
  publication for a reply or information turn or create acknowledgement loops.
- Automatically resume a legitimate idle requester in its original provider
  conversation. Active receivers use a supported non-interrupting delivery
  boundary or remain queued until ready. Never overwrite a user draft or
  interpret a native failure as permission to switch to terminal paste.
- Explicit user off or pause, deletion, lifecycle ownership, automation leases
  and provider-input readiness remain authoritative. A pending result must not
  force a new conversation or steal another conversation owner.
- Delivery is bounded and ordered through canonical claims. Concurrent
  callbacks, duplicate admission and explicit-versus-automatic completion
  cannot expose the same work twice.

## Requester identity and recovery

Capture the managed requester's genuine provider conversation identity and
observed runtime generation when admitting a follow-up task. Capture and
admission share the requester's lifecycle fence. Persist that origin with the
canonical task and carry it onto its reply.

A runtime generation is a live fence, not a durable conversation identity.
Normal restore can advance it while resuming the same conversation. Recovery
must distinguish that case from New Session, clear, provider change or deletion.
This binding must work with logging disabled and must not require archive
hydration. Missing or stale legacy origins remain explicitly blocked or held;
do not guess their identity from whichever conversation is currently active.

Readiness, reconnect and startup may retry proven-unsent eligible work. A
confirmed rejection before submission can release a claim. Partial writes,
uncertain submission and lost observations cannot trigger automatic replay or
a transport fallback.

## Migration and tool retirement

Apply wake-on-information only to newly admitted peer messages. Historical
notify-only peer backlog must not become a burst of paid wake-up turns after
upgrade. An internal server-owned eligibility or admission-version marker may
distinguish these records within the existing delivery metadata; it is not a
second queue or a public mode. Passive host-automation notifications/context
remain non-waking. Historical peer information can remain available as bounded
context on a legitimate subsequent turn and through diagnostics.

Never requeue historical receiver-owned, accepted or uncertain records as fresh
work. Preserve their identities and evidence. Older clients' receive wire calls
must return a coded retirement error before claiming messages or advancing an
acknowledgement. Remove receive from CLI parsing, MCP discovery and dispatch,
generated provider instructions, bundled guidance and current examples together.
Historical specs remain historical and must not masquerade as current guidance.

A retained wait tool only suspends an already active caller for activity. It
must not read or claim records, become a replacement delivery path, or require
a subsequent receive call. Automatic continuation must work after that wait
expires and after the requester ends its turn.

## Acceptance boundary

Prove a delayed real-provider reply after an Antigravity requester becomes idle,
using an isolated Codex worker and the original requester conversation. The
requester must continue without receive, another human prompt or manual resume.
Use reply-only evidence for this case so an earlier FYI cannot explain the wake.
Separately prove that newly admitted peer information wakes an idle receiver
without creating a task or reply loop.

Lower-layer coverage must distinguish active and idle routes, same-conversation
restore from a new session, logging-disabled identity, admission races,
duplicate and competing completion, off/pause/deletion/lease fences, proven
prewrite rejection from uncertain submission, historical backlog policy, and
old-client rejection without claim or acknowledgement mutation. Fixture success
alone is not real-provider acceptance. Preserve failed attempts and protect the
running user application throughout validation.
