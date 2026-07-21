# Messaging

## Send A Live Message

Use `wardian send` for one-way inter-agent communication:

```bash
wardian send "review this patch" --to reviewer-a1
wardian send --stdin --to reviewer-a1
wardian send --file prompt.md --to reviewer-a1
wardian send "status?" --to class:Coder
wardian send "stand down" --to all
wardian send "review this patch" --to reviewer-a1 --wait-until idle --timeout 10m
wardian send "queue this review" --to reviewer-a1 --queue-policy queue-if-busy
wardian send "only submit live" --to reviewer-a1 --queue-policy live-only
wardian send "deliver on the next safe turn" --to reviewer-a1 --queue-policy mailbox-only
```

Targets can be an agent name, UUID, `class:<ClassName>`, or `all`. By default,
`all` and class targets resolve among neighbors, and a bare name resolves among
neighbors before an exact global fallback. UUIDs and exact names cross that soft
boundary. Add `--scope all` only for global broadcast or class resolution in a
real orchestration task. Use `--wait-until` only for a single-agent target.

For substantial text, prefer stdin or a file to protect quoting:

```bash
cat <<'EOF' | wardian send --stdin --to reviewer-a1 --wait-until idle --timeout 10m
Review the changes since origin/main.
Return findings first, then tests run, then any residual risk.
EOF
```

PowerShell:

```powershell
@"
Review the changes since origin/main.
Return findings first, then tests run, then any residual risk.
"@ | wardian send --stdin --to reviewer-a1 --wait-until idle --timeout 10m
```

Normal sends retain the sender attribution when Wardian knows it. Inspect the
returned `delivery[]`; errors contain per-target delivery details including
runtime state, delivery state, input mode, and channel errors.

`--queue-policy queue-if-busy` is the default and queues delivery when the
provider cannot safely accept it. `live-only` fails instead of queueing, while
`mailbox-only` queues without attempting a live submit. Use the policy that
matches whether the work can wait for a safe provider turn.

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

## Send Approval Actions

Use an approval action only to answer an outstanding provider approval. It is
distinct from a normal agent message and supplies `accept` or `reject` as the
default body:

```bash
wardian send --to reviewer-a1 --approval accept
wardian send --to reviewer-a1 --approval reject
```

Do not combine `--approval` with `--as-command`.

## Send Provider Commands

Use `--as-command` only when a provider slash command must be the first input
token. It sends the exact body without `From <sender>:` attribution while using
the provider-aware submit path:

```bash
wardian send --as-command "/goal test" --to reviewer-a1
printf '%s' '/status' | wardian send --stdin --as-command --to reviewer-a1
```

PowerShell:

```powershell
"/status" | wardian send --stdin --as-command --to reviewer-a1
```

Command mode accepts exactly one explicit agent name or UUID. It rejects
`all`, `class:<ClassName>`, and `--thread` with `not_supported`. Successful
responses report `input_mode: "command"`. Threading itself is reserved until
it works end-to-end; do not assume `--thread` silently succeeds.

## Ask For A Structured Reply

Use `ask` when the task needs a named peer's accountable result, not just a
message. It captures a pre-send cursor, sends a backend-owned request ID, and
waits for that peer's `reply`:

```bash
cat <<'EOF' | wardian ask reviewer-a1 --stdin --timeout 10m
Review this patch.
EOF
wardian ask reviewer-a1 --file review-request.md --timeout 10m
```

The JSON response includes `request_id`, reply status/body, delivery evidence,
watch events, and retained output. A reply status is `done`, `blocked`, or
`failed`; a timeout remains a separate `watch_timeout` error. Use output-marker
matching only when explicit compatibility requires it. `ask` accepts one agent
name or UUID; broadcasts, class selectors, and `--thread` are unsupported.

Complete an ask from a managed agent terminal with the request ID supplied by
the ask response:

```bash
cat <<'EOF' | wardian reply ask_0123456789abcdef --status done --stdin
Reviewed the patch. No blocking findings.
EOF
wardian reply ask_0123456789abcdef --status blocked --file findings.md
```

Wardian validates sender identity when `WARDIAN_SESSION_ID` is available.
Replies from ordinary terminals remain available to let a human unblock work,
but their caller identity is not authenticated.
