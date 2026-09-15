# Messaging

## Canonical Peer Operations

Run peer commands from an authenticated managed Wardian agent. All six use the
same canonical service as the corresponding MCP tools. Never set or replace
sender identity to impersonate another agent. External terminals cannot send
or complete peer tasks.

| CLI | Canonical operation | Meaning |
| --- | --- | --- |
| `message list` | `list_agents` | List recipients visible to this sender. |
| `message send` | `send_message` | Admit information without starting or interrupting a turn. |
| `message followup` | `followup_task` | Admit a task; the service may wake the recipient. |
| `message receive` | `receive_messages` | Read this recipient's page and optionally acknowledge a consumed page. |
| `message reply` | `reply` | Complete a task as its authorized recipient. |
| `message interrupt` | `interrupt_agent` | Explicitly interrupt one current turn when authorized. |

Use one exact name or UUID. Broadcast and class selectors are unsupported.
The CLI uses canonical admission without a legacy queue or automatic replay.
The runtime uses native existing-session integration where supported; composer
delivery is retained only for demonstrably unsupported cases. A native failure
or uncertain receipt does not establish that a provider is unsupported.
Provider commands and approval actions are not peer message modes.

```bash
wardian message list
wardian message send reviewer-a1 "The documentation check passed."
wardian message followup reviewer-a1 --file review-request.md --idempotency-key review-1288
wardian message receive --timeout-ms 60000
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

## Receive The Correlated Result

Save the exact `request_id` from the followup receipt. Admission and provider
acceptance do not establish completion. A received reply belongs to that task
only when its `parent_interaction_id` equals the saved ID. Check `reply_status`:
`done`, `blocked`, or `failed`. Do not infer completion from Idle status,
terminal text, an empty page, or an unrelated answer.

```bash
wardian message receive --cursor '<next-cursor>' --timeout-ms 60000
wardian message receive --cursor '<next-cursor>' --ack-cursor '<consumed-page-ack-cursor>'
wardian message reply '<exact-request-id>' --status done --file findings.md
```

Cursors are opaque and recipient-bound. After a timed-out receive, preserve
its `next_cursor` and continue within the caller's overall deadline.
Acknowledge only a page already consumed. Each call accepts a limit of 1–100
and a timeout of 0–60000 milliseconds; there is no hidden unbounded wait.

Send and followup accept an optional `--idempotency-key` for stable admission
identity. Preserve that exact key; it is not the task's canonical request ID
and does not authorize autonomous replay. If a receipt is lost or invalid,
preserve and report uncertainty. Do not resubmit, substitute another key or
command, or invent a successful reply. A receive timeout never resubmits work.

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
