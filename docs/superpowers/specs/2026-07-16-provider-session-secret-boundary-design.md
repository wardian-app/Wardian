# Provider Session Secret Boundary Design

## Problem

Wardian has two distinct identities for a running agent:

- `session_id` is Wardian's stable agent UUID.
- `resume_session` is the provider-owned conversation identifier used to resume
  the provider CLI.

Provider output parsers expose structured initialization events containing a
`session_id`. The PTY reader currently treats every non-empty value in that
field as authoritative and copies it into `resume_session`. The same update is
duplicated across the line-oriented Claude path, the general line path, and the
streaming JSON path.

That trust boundary is unsafe. A provider version, wrapper, or test harness can
emit any string in the initialization field. Wardian's native mock harness
reproduces the problem: the caller's UUID remains the stable Wardian identity,
but an independently configured `WARDIAN_MOCK_SESSION_ID` becomes the persisted
resume identifier. If the emitted value is an API key, Wardian logs the key,
writes it to `settings/state.json`, and later passes it back to the provider as
a resume command-line argument. This both exposes the credential to the agent
process and makes provider resume fail.

Current Claude, Gemini, and Codex CLI versions keep authentication credentials
separate from session identity. The security boundary must nevertheless be
enforced by Wardian because provider versions and user-supplied wrappers are
outside Wardian's control.

## Security Invariants

1. Structured provider output is telemetry, not authority to replace a
   persisted resume identifier.
2. A value equal to a known credential environment value must never be used as
   a Wardian or provider session identifier, persisted, or placed in provider
   arguments.
3. Provider arguments and provider-supplied session identifiers must not be
   written verbatim to Wardian debug logs.
4. Wardian's stable `session_id` must never change in response to provider
   output.
5. Previously poisoned state must fail closed before launching a provider and
   recover through the provider's trusted discovery path where one exists.

## Trusted Provider Session Sources

Initialization events will no longer mutate `resume_session`. Each maintained
provider already has a more reliable identity source:

| Provider | Trusted resume identity source |
| --- | --- |
| Claude | The UUID Wardian passes with `--session-id`, promoted through the existing manual-session lifecycle. |
| Gemini | The UUID Wardian passes with `--session-id`, promoted through the existing manual-session lifecycle. |
| Codex | The projected Codex session index, history, or rollout file whose session metadata and local file both exist. |
| OpenCode | The scoped `ses_...` identifier discovered from OpenCode runtime logs/database state. |
| Antigravity | The conversation identifier discovered from Antigravity database state. |
| Mock | Wardian's stable UUID; mock initialization identifiers are diagnostic only. |

The PTY reader will still process initialization timestamps and status events,
but it will not persist the event's `session_id`. Codex may discover its thread
slightly after `thread.started`; its existing watcher and resume preparation
already recover the identifier from projected local evidence. This trades an
untrusted eager update for a trusted, eventually consistent update.

## Credential Guard

A small session-identity boundary will recognize credential-bearing
environment variables by name, including API keys, authentication/access
tokens, secrets, and passwords. It will compare a non-empty candidate against
those values without retaining or logging the credential set.

The guard will be applied at two layers:

- Resume preparation clears a persisted `resume_session` that equals a known
  credential before provider-specific recovery runs. Codex can then recover
  from projected rollout evidence; manual-session providers fall back to their
  stable Wardian-assigned UUID where appropriate.
- Interactive and headless launch boundaries reject any remaining credential
  match before constructing or spawning the provider command. This is the
  final fail-closed protection for direct spawn/resume requests and stale state
  paths that bypass normal preparation.

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

## Recovery

When resume preparation finds a credential match, it clears the poisoned value
in memory before applying the existing provider-specific rules. The corrected
configuration is persisted by the normal lifecycle save following restoration
or resume. If a launch path receives a credential match without an opportunity
to persist recovery, it returns a generic error that identifies the unsafe
field but never includes the value.

The design cannot identify a UUID-shaped credential after that credential has
been removed from the environment. Preventing new untrusted init writes and
validating provider-specific local evidence limits that residual case without
maintaining a credential store inside Wardian.

## Verification

- Unit tests prove a UUID-shaped value equal to a synthetic API-key environment
  value is rejected without exposing the value in errors or formatted launch
  diagnostics.
- Spawn tests prove Claude, Gemini, mock, and Codex initialization events cannot
  replace an existing resume identifier or create one from untrusted PTY data.
- Resume-preparation tests prove poisoned state is cleared and provider-specific
  fallback/recovery still works.
- A native mock regression launches with a caller-owned Wardian UUID and a
  different synthetic `WARDIAN_MOCK_SESSION_ID`, waits for structured output,
  and proves the emitted value is absent from persisted state and debug logs.
- Existing provider parser tests remain unchanged because parsing an init event
  is still valid; only its authority to mutate persisted identity changes.
- Full backend, frontend, native, secret, and git-scope checks follow
  `AGENTS.md`.

## Scope

This fix does not add API-key storage, change provider authentication, alter
Wardian's stable UUIDs, redesign session persistence settings, or remove
structured provider event parsing. It hardens the boundary between provider
telemetry and Wardian-owned persisted state.
