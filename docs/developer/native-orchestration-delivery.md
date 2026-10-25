# Native orchestration delivery

Wardian peer messaging uses the canonical information, task and reply
operations described in [agent messaging tools](./agent-messaging-tools.md).
Native provider connections are delivery mechanisms behind that service.

An attached Codex session uses its generation-bound app-server connection.
Previously stored informational messages enter through `thread/inject_items`
as context only. A newly admitted informational message starts an idle turn
with a `wardian_inbox_delivery` item; it creates no task or reply obligation.
Tasks use `turn/start` with structured host context. None of these operations
uses the terminal composer. Candidate adapter capabilities alone do not
establish that a given session has a negotiated native delivery connection.

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

Use `wardian message send` to admit an informational message. New messages
automatically wake an idle recipient when its original conversation and a
provider receipt route are available. They create no task or reply obligation.
Messages stored before wake eligibility was recorded remain context-only and
are never replayed as startup wakes. The agent-facing receive and wait commands are retired; actionable
replies resume the requester's original provider conversation automatically. Use
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

Automatic dispatch uses durable claim ownership. If a native connection is
unavailable, an existing idle surface route is used only with a provider turn
receipt. An uncertain submission is never replayed through another route.
Background execution uses the canonical task's recorded ownership and provider
lifecycle. Older non-Codex information is carried only into a later
Wardian-managed prompt; this does not wrap direct TUI user-started input.

Validation uses an isolated Wardian home, browser profile, and matching packaged
runtime. Record the actual provider/session binding, task claim, native
acceptance and correlated reply. For Codex, require successful information/task
delivery with terminal writes disabled in the test. Preserve failed or uncertain
attempts; do not replace their evidence with a later run's result.
