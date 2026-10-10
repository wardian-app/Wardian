# Claude Inbox Turn Completions

## Decision

Wardian creates a Claude Inbox completion only for a prompt whose final
answer Claude Code itself has settled: every Stop hook of the last round has
finished, and none of them continued the turn. The card records a completed
provider turn. It does not assert that the user's larger task is complete.

A Stop hook alone cannot provide that verdict. Claude Code runs all matching
Stop hooks in parallel, before it decides whether to continue, and any of them
may block the answer (`decision: "block"`, exit code 2, or
`additionalContext`). The interactive surface exposes the decision only
through the OpenTelemetry log event `claude_code.hook_execution_complete`
with `hook_event = "Stop"`. Claude emits that event after every hook of the
round has settled, with the round's `num_blocking`, `num_cancelled`,
`additional_context_chars`, `prompt.id`, `session.id`, and a per-process
`event.sequence`. The SDK `result` message carries an explicit verdict, but
only in `-p` and Agent SDK sessions, not in the interactive terminal Wardian
runs. The transcript's `stop_hook_summary` and `turn_duration` records are
marked internal by Claude Code and are not used.

## Evidence

Each interactive Claude launch owns
`<wardian-home>/agents/<agent-id>/claude/turn-verdicts/<launch>/`:

- `stop-rounds.jsonl`: Wardian's injected Stop hook appends each invocation's
  payload, including `prompt_id` and `last_assistant_message`, as one line.
  The hook's target is passed as an argument, so a lingering earlier process
  cannot write into a newer launch.
- `telemetry.jsonl`: a loopback OTLP/HTTP JSON receiver, bound to 127.0.0.1
  with a per-launch token in its path, keeps only event name, sequence,
  prompt ID, session ID, timestamp, and hook counts. It syncs each batch to
  disk before acknowledging it, answers 503 on a failed write so Claude's
  exporter retries, and refuses requests over 4 MiB and ledgers over 64 MiB.
- `committed.jsonl`: prompt IDs whose card is durably in the queue.
- `launch.json`: whether verification is available, and why not.

A prompt is verified only when all of these hold:

1. The ledger holds every sequence number from the prompt's `user_prompt`
   event through the final round's completion. Claude numbers events per
   process, so a lost batch is a gap.
2. Each Stop round in that window is a `hook_execution_start` followed by its
   `hook_execution_complete`, with every hook settled and none cancelled.
   Claude awaits every hook before emitting the completion, so no hook of an
   earlier round can append after a later round begins.
3. Exactly the last round is non-continuing: no blocking decision and no
   additional context. No Stop event for the prompt follows it.
4. The round log holds exactly one line per round for the prompt, in round
   order, all from the same Claude session as the telemetry, and that session
   belongs to the agent.

Under these rules, losing a prior round's completion together with the final
round's Stop record cannot pair the earlier text with the final verdict: the
lost completion is either a sequence gap or an unpaired round start.

A gap leaves the prompt pending, because a retried batch may still fill it.
Any other failure makes the prompt unknown: no card is created, and the reason
is logged. Wardian never infers success from a turn duration, a missing
`StopFailure`, a hook summary, or elapsed time.

## Telemetry ownership

Wardian sets the launch's telemetry through its `--settings` `env` block only
when no telemetry configuration of the user's, the repository's, or the
organization's applies. It checks Wardian's own environment, which Claude
inherits, for any `OTEL_*`, `CLAUDE_CODE_ENABLE_TELEMETRY`, or related
variable. It also checks the managed, user, project, and local settings files
for those variables or for `otelHeadersHelper`, and checks for any installed
managed policy (the Windows `Policies\ClaudeCode` registry key or the macOS
managed preferences file). An unreadable settings file also counts as
configured. When configuration exists, Wardian leaves it untouched, installs
no Stop round hook, and records the launch as unavailable. Its collectors,
headers, and certificates are never redirected or forwarded.

Wardian's own configuration exports logs only (`http/json`, one-second
interval). Metrics and traces are off. No prompt, response, or tool content is
enabled. Server-managed settings that Wardian cannot read may still override
these variables. Claude then removes Wardian's endpoint, and every prompt in
that launch stays unknown.

## Delivery and identity

- The stable item ID is `agent-completed:<wardian-session>:<prompt-id>`. The
  item stores the exact final response, a bounded summary, and the timestamp
  of the settled-Stop event (`timestamp_source:
  "claude_stop_hooks_settled_event"`). Processing time is used only when that
  timestamp cannot be parsed, and `timestamp_source` records the fallback.
- Queue persistence is an idempotent upsert that retries while the runtime is
  current. Its commit is then recorded in `committed.jsonl`. Startup replays
  verified, uncommitted prompts from every retained launch, including those of
  saved-Off agents, before providers launch. A dismissal tombstone still
  consumes a replay.
- Wardian keeps the newest eight launch directories per agent. It deletes only
  direct children of the agent's `turn-verdicts` directory.
- Headless Claude runs install no Stop round hook.

## Compatibility

Legacy completion items keep their IDs, including `hook_outbox_mtime`
timestamps, and remain readable and dismissible. Earlier unverified Stop
outbox records in `claude/turn-completions/` are no longer converted into
cards. Claude Code versions before 2.1.280 do not report
`additional_context_chars`, so their Stop rounds stay unknown.

## Verification boundary

Deterministic backend tests cover:

- the correlator's success, continuation, gap, retry, duplicate, conflict,
  missing-start, unsettled-round, session, and malformed-log cases;
- OTLP parsing and the receiver's write and refusal paths;
- the telemetry-ownership checks;
- the generated hook script;
- startup replay.

They cannot prove that the installed Claude Code emits these events in
interactive sessions or composes Wardian's hook with a workspace hook. The
opt-in real-provider test `e2e-native/tests/claude-inbox-stop-real-native.test.mjs`
does that. Its workspace Stop hook blocks the first answer once, and the
test requires the card to hold only the post-continuation answer.
