# Messaging

## Canonical Peer Operations

Run peer commands from an authenticated managed Wardian agent. These use the
same canonical service as the corresponding MCP tools. Never set or replace
sender identity to impersonate another agent. External terminals cannot send
or complete peer tasks.

| CLI | Canonical operation | Meaning |
| --- | --- | --- |
| `message list` | `list_agents` | List recipients visible to this sender. |
| `message send` | `send_message` | Admit and automatically deliver information, waking an idle recipient when its original conversation is available. |
| `message followup` | `followup_task` | Admit a task; the service may wake the recipient. |
| `message reply` | `reply` | Complete a task as its authorized recipient. |
| `message interrupt` | `interrupt_agent` | Explicitly interrupt one current turn when authorized. |

Use one exact name or UUID. Broadcast and class selectors are unsupported.
The CLI uses canonical admission without a legacy queue or automatic replay.
Wardian automatically resumes the requester's original conversation when a
correlated task reply commits and a safe provider route is available.
Agent-facing receive and wait commands are retired; backend mailbox records
remain available for read-only diagnostics.
New informational messages wake an idle recipient through the same durable
delivery service. They create no task, reply obligation, automatic reply, or
task result. Messages stored before wake eligibility was recorded remain
context-only and are never replayed as startup wakeups. Off, pause, session
replacement, and provider changes remain authoritative fences. The runtime uses
existing-session provider integration where supported. A failure or uncertain
receipt does not establish that a provider is unsupported. Provider commands
and approval actions are not peer message modes.

```bash
wardian message list
wardian message send reviewer-a1 "The documentation check passed."
wardian message followup reviewer-a1 --file review-request.md --idempotency-key review-1288
```

For substantial literal text, use a file or stdin:

```bash
cat <<'EOF' | wardian message followup reviewer-a1 --stdin
Review the issue diff. Return blocking findings and concrete validation.
EOF
```

PowerShell:

```powershell
@'
Review the issue diff. Return blocking findings and concrete validation.
'@ | wardian message followup reviewer-a1 --stdin
```

## Automatic Reply Continuation

Save the exact `request_id` from the followup receipt. Admission and provider
acceptance do not establish completion. When a safe provider route is available,
Wardian resumes the original requester conversation with a reply whose
`parent_interaction_id` matches the saved ID. The reply retains its
`reply_status`: `done`, `blocked`, or `failed`. Do not infer completion from
Idle status, terminal text, or an unrelated answer.

Canonical tasks dispatched to an exact native Codex turn automatically return
that turn's final result to the requester unless an explicit reply has already
completed the task. Several tasks steered into one turn share its final result.
The terminal reply continues through the requester's original provider
conversation and creates no new reply obligation.

```bash
wardian message reply '<exact-request-id>' --status done --file findings.md
```

Send and followup accept an optional `--idempotency-key` for stable admission
identity. Preserve that exact key; it is not the task's canonical request ID
and does not authorize autonomous replay. After an admission timeout, the CLI
never resubmits automatically; preserve and report uncertainty. If a receipt
is lost or invalid, do not resubmit, substitute another key or
command, or invent a successful reply. A delayed reply is recovered by Wardian
when the original conversation becomes ready; agents do not poll or replay it.

When separately authorized to interrupt current work:

```bash
wardian message interrupt reviewer-a1
```


## Inspect Conversations

Use `conversation` to retrieve durable message history rather than inferring a
peer's state from terminal scrollback:

```bash
wardian conversation list
wardian conversation list --agent reviewer-a1
wardian conversation list --scope all
wardian conversation show <conversation-id>
```

The default `current` scope is the current agent's conversation set. Use an
explicit agent or `--scope all` only when the coordination task needs a wider
history.

Conversation list/show responses include top-level `status_source` (`live` or
`persisted`). Disk fallback occurs only when the endpoint is unavailable; live
rejections, protocol errors, and timeouts are preserved as errors.

Prefer one agent's index before `show`, which returns the full narrative and
has no CLI pagination or turn selector. For bounded detail, read that known
conversation's `manifest.json` and `turns.jsonl` under
`<wardian-home>/agents/<agent-id>/conversations/<conversation-id>/` before
`conversation.jsonl`. Do not recursively crawl agent directories. Direct
`index.jsonl` readers must keep the latest row per `conversation_id` because
the index is append-only upsert history.
