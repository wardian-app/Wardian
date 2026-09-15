# Native orchestration delivery

Wardian peer messaging uses the canonical information, task, receive and reply
operations described in [agent messaging tools](./agent-messaging-tools.md).
Native provider connections are delivery mechanisms behind that service.

An attached Codex session uses its generation-bound app-server connection.
Information enters through `thread/inject_items`; tasks enter through
`turn/start` with structured host context. Neither operation uses the terminal
composer. Candidate adapter capabilities alone do not establish that a given
session has a negotiated native delivery connection.

Start an accountable request with a caller-owned idempotency key:

```sh
wardian message followup <agent-name-or-uuid> "Review the change" \
  --idempotency-key <stable-request-key>
```

PowerShell:

```powershell
wardian message followup <agent-name-or-uuid> "Review the change" `
  --idempotency-key <stable-request-key>
```

Use `wardian message send` for information without waking or interrupting the
recipient. Use `wardian message receive` for a bounded inbox read and
`wardian message reply` to complete a canonical task. Interruption is explicit
through `wardian message interrupt`; a correction message does not imply it.

Inspect session capabilities and retained native-delivery evidence:

```sh
wardian delivery show <interaction-id>
wardian delivery capabilities <agent-name-or-uuid>
```

Historical native-delivery evidence is distinct from a canonical task receipt.
Never automatically retry uncertain submission. A task is complete only when
its authorized recipient records a correlated reply; native acceptance and
terminal status do not establish completion.

Provider and provider-session identifiers in capability or evidence output are
diagnostics. Address all operations with Wardian agent UUIDs or names and
Wardian interaction IDs.

Manual receive and automatic dispatch share durable claim ownership. If a native
connection is unavailable, that condition cannot activate the retired Codex
composer fallback or replay the legacy mailbox. Background execution uses the
canonical task's recorded ownership and provider lifecycle.

Validation uses an isolated Wardian home, browser profile, and matching packaged
runtime. Record the actual provider/session binding, task claim, native
acceptance and correlated reply. For Codex, require successful information/task
delivery with terminal writes disabled in the test. Preserve failed or uncertain
attempts; do not replace their evidence with a later run's result.
