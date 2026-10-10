# Claude Inbox Turn Completions

## Decision

Wardian creates a Claude Inbox completion only from a provider-owned verdict,
captured after Claude's final continuation decision and correlated to the
exact session, prompt, and final response. A card records a completed
provider turn. It does not assert that the user's larger task is complete.

Claude Code's interactive surface does not supply that verdict
([capability record](../research/claude-interactive-turn-verdict.md)):

- The `Stop` hook runs before the continuation decision, and other Stop hooks
  may block the answer.
- The OpenTelemetry event `claude_code.hook_execution_complete` reports only
  that a round's Stop hooks settled. Claude's Stop handler can still continue
  the turn with no hook blocking (brief-mode enforcement, goal check-ins,
  structured-output enforcement), or end it through `continue: false`.
- No event follows that decision.

Interactive Claude turns therefore create **no** Inbox completion. They are
treated as unknown. Wardian does not infer success from settled hooks, a turn
duration, a missing `StopFailure`, a hook summary, elapsed time, or the start
of a later prompt. The verdict exists only in the SDK `result` message
(`-p` and Agent SDK sessions) and inside the Remote Control bridge, which
offers no third-party access. The smallest supported options are a
post-decision interactive signal from Claude Code, or an SDK-engine Claude
integration. Either one is a separate decision.

## Settlement evidence

The settlement pipeline is retained as the necessary condition that any
future verdict must also satisfy: the final Stop round settled without a hook
continuation. It is disabled for interactive launches. `launch.json` records
`verification: "unavailable"` with the reason, Wardian injects no Stop round
hook and no telemetry settings, and no receiver runs.

When capture is enabled, each launch owns
`<wardian-home>/agents/<agent-id>/claude/turn-verdicts/<launch>/`:

- `stop-rounds.jsonl`: Wardian's Stop hook appends each invocation's payload
  as one line. The target is passed as an argument, so a lingering earlier
  process cannot write into a newer launch.
- `telemetry.jsonl`: a loopback OTLP/HTTP JSON receiver, bound to 127.0.0.1
  behind a per-launch token, keeps only event name, sequence, prompt ID,
  session ID, timestamp, and hook counts.
  - Each append starts at the last committed newline. A torn tail is
    discarded, and a failed write rolls back to that boundary before the
    receiver answers 503 so the exporter retries.
  - Each batch is synced before acknowledgement.
  - Requests over 4 MiB and ledgers over 64 MiB are refused.
- `committed.jsonl`: prompt IDs whose card is durably in the queue.

A prompt settles only when all of these hold:

1. The ledger holds every sequence number from the prompt's `user_prompt`
   event through the final round's completion.
2. Each Stop round in that window is a `hook_execution_start` followed by its
   `hook_execution_complete`, with every hook settled and none cancelled.
3. Exactly the last round is non-continuing, and no Stop event for the prompt
   follows it.
4. The round log holds exactly one line per round for the prompt, in round
   order, from the same Claude session as the telemetry, and that session
   belongs to the agent.

Losing a prior round's completion together with the final round's Stop record
cannot pair the earlier text with the final round: the lost completion is
either a sequence gap or an unpaired round start.

The tracker consumes complete ledger lines only. A missing prompt start or a
gap leaves the prompt pending. A prompt that cannot settle is logged once per
reason and evaluated again when new evidence arrives, so a delayed batch can
still complete it.

## Telemetry ownership

When capture is enabled, Wardian sets the launch's telemetry through its
`--settings` `env` block only if no user, repository, or organization
telemetry configuration applies. It checks:

- Wardian's own environment, which Claude inherits, for any `OTEL_*`,
  `CLAUDE_CODE_ENABLE_TELEMETRY`, or related variable;
- the managed, user, project, and local settings files for those variables or
  `otelHeadersHelper`;
- any installed managed policy.

An unreadable settings file counts as configured. Existing configuration is
left untouched, and its collectors, headers, and certificates are never
redirected or forwarded. Wardian's own configuration exports logs only, with
metrics, traces, and prompt, response, and tool content off.

## Delivery and identity

- The stable item ID is `agent-completed:<wardian-session>:<prompt-id>`. The
  item stores the exact final response, a bounded summary, and the timestamp
  of the settled-Stop event (`timestamp_source:
  "claude_stop_hooks_settled_event"`).
- Queue persistence is an idempotent upsert that retries while the runtime is
  current. Its commit is then recorded in `committed.jsonl`. Startup replays
  settled, uncommitted prompts from every retained launch before providers
  launch, including saved-Off agents. A dismissal tombstone still consumes a
  replay.
- Wardian keeps the newest eight launch directories per agent. A directory
  that still holds an uncommitted completion is kept beyond that count.
  Deletion is confined to direct children of the agent's `turn-verdicts`
  directory.
- Headless Claude runs install no Stop round hook.

## Compatibility

Legacy completion items keep their IDs, including `hook_outbox_mtime`
timestamps, and remain readable and dismissible. Earlier unverified Stop
outbox records in `claude/turn-completions/` are no longer converted into
cards. Claude Code versions before 2.1.280 do not report
`additional_context_chars`, so their Stop rounds cannot settle.

## Verification boundary

Deterministic backend tests cover:

- the correlator's settlement, continuation, gap, delayed-start, retry,
  duplicate, conflict, unsettled-round, session, and malformed-log cases;
- OTLP parsing, and the receiver's write, refusal, rollback, and torn-tail
  paths;
- the tracker's out-of-order delivery and commit-aware retention;
- the telemetry-ownership checks;
- the disabled interactive launch;
- the generated hook script;
- startup replay.

The opt-in real-provider test
`e2e-native/tests/claude-inbox-stop-real-native.test.mjs` checks the
installed Claude Code. Its workspace Stop hook blocks the first answer once,
and it requires that the finished turn create no completion and that the
launch record verification as unavailable.
