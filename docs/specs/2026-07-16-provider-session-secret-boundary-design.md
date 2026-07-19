# Provider Session Secret Boundary Design

## Problem

Wardian has two distinct identities for a running agent:

- `session_id` is Wardian's stable agent UUID.
- `resume_session` is the provider-owned conversation identifier used to resume
  the provider CLI.

Provider output parsers expose several fields called `session_id`, `sessionId`,
or `thread_id`. Those fields do not share one contract. Depending on the
provider, such a value can be a caller-selected conversation UUID, a
provider-created durable thread ID, an event-routing key, or request/cache
affinity metadata. Wardian currently flattens these meanings into
`AgentEvent::Init { session_id }` and copies any non-empty value into
`resume_session`. The same mutation is duplicated across the line-oriented
Claude path, the general line path, and the streaming JSON path.

That abstraction is the root cause. It lets a value from the wrong identity
domain become a durable provider resume handle. If the value is an API key,
Wardian logs the key, writes it to `settings/state.json`, and later passes it
back to the provider as a resume command-line argument. The native mock can
demonstrate that unsafe sink, but it does not reproduce or explain the source
of a real provider's value.

An audit of the maintained providers and open-source harnesses found no
legitimate provider implementation that assigns its API key as its durable
conversation identifier. Authentication and resume identity are separate in
their implementations. The exact upstream producer of the reported literal
key therefore remains unproven without the affected provider version or its
raw event stream. Wardian can still remove the class of failure by enforcing
each provider's actual identity contract instead of trusting a generic field.

## Security Invariants

1. Session identity authority is provider-specific. There is no generic
   provider initialization event that may update `resume_session`.
2. A value equal to a known credential environment value must never be used as
   a Wardian or provider session identifier, persisted, or placed in provider
   arguments.
3. Provider arguments and provider-supplied session identifiers must not be
   written verbatim to Wardian debug logs.
4. Wardian's stable `session_id` must never change in response to provider
   output.
5. Missing, malformed, or conflicting authoritative identity must fail closed
   before state mutation or provider resume.
6. Wardian must not use filesystem recency, `latest`/`--continue`, another
   session's metadata, or any other heuristic as a fallback for a missing
   exact identity. A clear failure is preferable to nondeterministic resume.

## Trusted Provider Session Sources

The maintained providers have distinct authoritative sources and validation
rules:

| Provider | Authoritative source | Mutation rule |
| --- | --- | --- |
| Claude | The UUID Wardian supplies with `--session-id` for a fresh conversation, or the exact persisted UUID supplied with `--resume`. | `system/init.session_id` is confirmation only. It must equal the expected UUID and can never replace it. |
| Gemini | The ID Wardian supplies with `--session-id` for a fresh conversation, or the exact persisted ID supplied with `--resume`. | Stream-JSON `init.session_id` is confirmation only. It must equal the expected ID. Footer/stats IDs and filesystem recency are never identity sources. |
| Codex | The validated `thread.started.thread_id` emitted by `codex exec --json`. | A fresh run may set `resume_session` once from this event. A resumed run must emit the already expected ID. Missing or conflicting events fail. |
| OpenCode | The exact `ses_...` ID returned by the session-creation path or an equivalently bound provider API response. | Global events are accepted only when their session ID equals the bound ID. Logs, database recency, and `latest` are not fallbacks. |
| Antigravity | The exact workspace-to-conversation mapping produced for the current launch, with a pre-launch baseline proving the mapping changed for this run. | An unchanged, absent, ambiguous, or concurrently replaced cache mapping fails. |
| Mock | An explicitly configured test contract. | Mock identity may exercise the same provider-policy code, but it is not evidence about a production provider. |

The provider policy consumes typed identity evidence containing the provider,
launch mode, expected identifier when one exists, and the specific upstream
event kind. It returns one of three outcomes: confirm the expected identifier,
set a provider-owned fresh identifier, or reject the evidence. PTY readers may
still process timestamps and status events, but they do not mutate persisted
identity directly.

Provider-owned identifiers are validated by their real grammar before use.
Codex requires a UUID-shaped thread ID and OpenCode requires its `ses_` form.
Claude and Gemini compare against the exact caller-owned value instead of
trying to infer validity from shape alone.

## Upstream Contract Evidence

- Codex defines `thread.started.thread_id` as the identifier used to resume the
  thread, and its JSONL event processor emits that value from the configured
  thread ID:
  [event schema](https://github.com/openai/codex/blob/315195492c80fdade38e917c18f9584efd599304/sdk/typescript/src/events.ts),
  [event processor](https://github.com/openai/codex/blob/315195492c80fdade38e917c18f9584efd599304/codex-rs/exec/src/event_processor_with_jsonl_output.rs).
- Gemini stream-JSON emits `config.getSessionId()`. Gemini CLI 0.36 also had a
  confirmed macOS/API-key-authentication bug where its footer and stats could
  show a stale startup UUID instead of the active resumed UUID. That display
  value is therefore not authoritative:
  [stream output](https://github.com/google-gemini/gemini-cli/blob/3ff5ba20fc1ad7d867218bbdb34756eb54d6eccb/packages/cli/src/nonInteractiveCli.ts),
  [upstream root-cause report](https://github.com/google-gemini/gemini-cli/issues/18369),
  [Darwin API-key report](https://github.com/google-gemini/gemini-cli/issues/24820).
- OpenCode creates provider-owned `ses_...` IDs. The open-source Harness
  integration binds the ID returned by session creation and filters every
  global event against it:
  [OpenCode ID schema](https://github.com/anomalyco/opencode/blob/3a1c6df9e24672f0761a6ced18e1315d89334baf/packages/schema/src/session-id.ts),
  [Harness adapter](https://github.com/rumpl/harness/blob/3f63cb8efc0530ce1593e710c6bcad6b381e13da/opencode/runner.go).
- SDK fields named `sessionId` are not necessarily resume handles. Pi-mono, for
  example, uses the field for affinity headers and prompt-cache keys while
  keeping API-key authentication separate:
  [option contract](https://github.com/badlogic/pi-mono/blob/216e672e7c9fc65682553394b74e483c0c9e47f7/packages/ai/src/types.ts),
  [request mapping](https://github.com/badlogic/pi-mono/blob/216e672e7c9fc65682553394b74e483c0c9e47f7/packages/ai/src/api/openai-responses.ts).

## Credential Guard

A small session-identity boundary will recognize credential-bearing
environment variables by name, including API keys, authentication/access
tokens, secrets, and passwords. It will compare a non-empty candidate against
those values without retaining or logging the credential set.

The guard will be applied at two layers:

- Resume preparation rejects a persisted `resume_session` that equals a known
  credential. It does not clear the value and continue through discovery,
  `latest`, a caller UUID, or any other substitute.
- Interactive and headless launch boundaries reject any credential match
  before constructing or spawning the provider command. This is the final
  fail-closed protection for direct spawn/resume requests and stale state paths
  that bypass normal preparation.

Identifier shape is not a sufficient security boundary. API keys can be
UUID-shaped, and provider identifier formats vary, so format checks remain
provider correctness checks rather than secret detection.

## Logging

Wardian will preserve useful launch diagnostics without recording sensitive
values:

- PTY and headless launch logs record provider, executable, argument count,
  working directory, and whether resume is present, but not raw arguments or
  the resume identifier.
- Initialization handling records that an untrusted provider identifier was
  ignored or that trusted discovery updated state, without recording the
  identifier.
- Legacy headless bootstrap diagnostics record whether an identifier was found,
  not its value or raw provider stderr.

Wardian's stable UUID may continue to appear where it is needed for operational
correlation, but generic argument dumps are removed because custom provider
arguments may also contain credentials.

## Failure Semantics

Identity validation happens before changing the in-memory agent configuration,
writing state, or constructing resume arguments. Rejection leaves the last
known configuration unchanged and returns a generic error naming the provider
and failed identity contract without including the candidate value.

There is deliberately no automatic recovery path. Wardian does not scan for a
newest session, invoke provider `latest`/`--continue` behavior, substitute the
Wardian agent UUID, clear an invalid value and continue, or select another
workspace's metadata. A user can explicitly start a new conversation or repair
the stored provider identity, but Wardian will not guess which conversation to
resume.

The credential comparison cannot identify a UUID-shaped credential after that
credential has been removed from the environment. Provider-specific provenance,
exact equality checks, and identifier grammar prevent arbitrary output from
becoming a new resume identifier even in that residual case.

## Verification

- Unit tests prove a UUID-shaped value equal to a synthetic API-key environment
  value is rejected without exposing the value in errors or formatted launch
  diagnostics.
- Claude and Gemini tests prove matching init events confirm the expected ID,
  while missing or conflicting IDs cannot mutate it.
- Codex tests prove a fresh validated `thread.started.thread_id` is persisted,
  while malformed, missing, or conflicting IDs fail without mutation.
- OpenCode tests prove only an exactly bound `ses_...` ID is accepted and that
  unrelated global events are ignored rather than used for resume.
- Antigravity tests prove only a current-launch workspace mapping is accepted;
  unchanged or ambiguous cache state fails.
- Resume-preparation tests prove credential matches and unavailable exact IDs
  return errors without clearing state or invoking any fallback.
- Provider-faithful parser fixtures replace the synthetic native mock as the
  primary regression evidence. Real-provider native tests remain isolated and
  opt-in where credentials are required.
- Full backend, frontend, native, secret, and git-scope checks follow
  `AGENTS.md`.

## Scope

This fix does not add API-key storage, change provider authentication, alter
Wardian's stable UUIDs, redesign session persistence settings, or remove
structured provider event parsing. It hardens the boundary between provider
telemetry and Wardian-owned persisted state.
