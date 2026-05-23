# Agent Delivery Transport

- **Status:** Proposed
- **Date:** 2026-05-22
- **Decider:** Product/Engineering

## Context and Problem Statement

Wardian currently sends live agent messages by writing prompt text into a
provider terminal, sleeping for a fixed delay, and sending the provider submit
key. This preserves the visible interactive provider session, but it has three
reliability problems:

- Provider TUIs can treat a fast submit key as a literal newline or swallow it
  during paste-burst suppression.
- Wardian currently treats a successful PTY write as delivery even when the
  provider has not visibly accepted or submitted the input.
- Sending into a target that is `processing` or stuck in an unknown
  `action_required` prompt can corrupt the provider composer or answer the
  wrong prompt.

The CLI also has an adjacent reliability problem: `wardian ask` can receive a
structured reply, then fail while collecting watch evidence because the watch
cursor has expired. The structured reply store must be the source of truth for
ask completion; watch output is supporting evidence only.

Wardian needs faster and more reliable agent communication without creating a
separate headless provider turn. The live provider session remains the user
visible session of record.

## Goals

- Preserve Wardian's visible live provider sessions.
- Reduce unnecessary fixed submit delay while keeping provider-specific safety.
- Avoid direct PTY injection when the target is not ready for input.
- Make delivery results explicit enough for CLI automation and UI display.
- Support a Wardian-owned queue for messages that cannot be safely submitted
  immediately. In this implementation slice, the queue is live in-memory state;
  durable persistence is future work.
- Treat approval/action-required prompts as provider-specific structured UI,
  not as generic free text.
- Add deterministic fake-TUI and opt-in real-provider tests for Codex, Claude,
  Gemini, OpenCode, and Antigravity.

## Non-Goals

- Replacing `portable-pty` or ConPTY with tmux as the primary backend.
- Driving a separate headless provider turn for normal inter-agent messages.
- Implementing provider-native protocol adapters in this slice.
- Inferring approval intent from arbitrary message text such as `yes`.
- Guaranteeing real-provider behavior without opt-in authenticated provider
  test runs.

## Decision

Add a provider-aware delivery transport with two lanes:

1. **Live submit lane** for targets that are freshly observed as input-ready.
2. **Mailbox lane** for targets that are busy, unavailable, or unsafe for live
   injection.

The live lane remains based on Wardian's existing PTY/ConPTY input channel, but
it becomes a state machine with provider profiles, per-session locking, and
observable postconditions. The mailbox lane is Wardian-owned live state, not
Beads.

## Delivery Intent

Extend the current `MessageInputMode` concept into an explicit delivery intent:

- `message`: ordinary text meant for the provider composer.
- `command`: provider command text that must remain the first input token, such
  as a slash command.
- `approval_action`: a provider approval/action response selected through a
  structured Wardian UI or CLI flag.

Approval intent must not be inferred from the message body. A plain
`wardian send "yes" --to <agent>` remains a message. It is not an approval
action.

The approval action model should include only semantic actions Wardian can map
through a provider recognizer:

- `accept`
- `reject`
- `select { option }`
- `free_text { text }`

`free_text` is still provider-constrained. It is valid only when the recognizer
confirms the visible action-required prompt accepts free-form text.

## Caller Queue Policy

Every send path should declare or default a queue policy:

- `queue_if_busy`: submit live when safe; otherwise enqueue. This is the
  default for normal messages.
- `live_only`: submit only when live delivery is safe; otherwise return a
  structured failure. This is appropriate for approval actions and selected
  commands.
- `mailbox_only`: always enqueue and return the queued message id.

Existing `wardian send` behavior should remain compatible by defaulting to
`message` plus `queue_if_busy` for normal sends. Existing `--as-command` remains
`command`; command sends to broad targets stay unsupported unless a future
design defines safe broadcast command behavior.

## Provider Delivery Profile

Each provider gets a delivery profile. A profile is not just timing constants;
it must also expose recognizers:

- provider id and display name
- submit key bytes
- minimum delay before submit
- multiline strategy: literal, bracketed paste, or provider-specific fallback
- large-prompt threshold for paste strategy
- input-ready recognizer
- submitted-turn recognizer
- optional payload-visible recognizer
- approval prompt recognizers and supported approval actions
- command-specific restrictions
- resumed-session caveats

The first supported providers are:

- `codex`
- `claude`
- `gemini`
- `opencode`
- `antigravity`

Profiles may start conservatively. If a provider does not have a safe
recognizer for a state or approval shape, Wardian must queue or reject instead
of guessing.

## Live Delivery State Machine

Live delivery should advance through explicit states:

1. `resolved_target`: Wardian resolved the target agent and provider profile.
2. `input_channel_ready`: the target has a live input sender.
3. `input_ready_observed`: fresh provider-specific terminal evidence says the
   composer is ready for this intent.
4. `locked`: Wardian acquired the per-session delivery lock.
5. `payload_written`: Wardian wrote literal bytes or bracketed paste bytes.
6. `payload_visible`: optional but preferred; Wardian observed the payload in
   the terminal composer or provider transcript.
7. `submit_sent`: Wardian sent the provider submit key after the configured
   delay.
8. `submitted_observed`: Wardian observed a new provider turn, composer clear,
   processing status, or provider-specific submit evidence.

The returned delivery state should be the most meaningful terminal state, not
just the last write attempt. For example:

- `submitted_observed`
- `submit_sent_unverified`
- `queued`
- `queued_not_live`
- `unsupported_approval_shape`
- `not_input_ready`
- `no_input_channel`
- `send_failed`

`submitted` may remain as a compatibility alias in older CLI fields, but new
code should prefer the more specific states above.

## Prompt Encoding

Prompt normalization remains conservative:

- Normalize CRLF and CR to LF.
- Do not trim meaningful leading content for `command` or `approval_action`.
- Preserve slash commands exactly for `command`.
- Avoid adding the `From <sender>:` attribution prefix for `command` and
  `approval_action`.

For ordinary messages, Wardian may still add sender attribution when the sender
is known. If the message is queued, the same rendered body must be stored so
the live lane and mailbox lane do not disagree about what the target will see.

For multiline or large prompts, profiles may use bracketed paste. Bracketed
paste support must be proven by deterministic fake-TUI tests and opt-in
real-provider tests before it is enabled for a provider by default.

## Action-Required and Approval Handling

`action_required` is not a blanket permission to inject text. Wardian may use
live delivery for an approval action only when all of the following are true:

- the caller used `approval_action` intent;
- the provider profile has an approval recognizer for the current provider;
- Wardian captured a fresh terminal snapshot;
- the snapshot matches a known prompt fingerprint for that provider and
  version/shape;
- the requested semantic action maps to exactly one input sequence;
- the snapshot has not changed before the input sequence is sent.

Approval actions must not auto-retry after input is sent. A retry can answer a
different prompt if the provider advances state. If the recognizer cannot prove
the prompt shape, Wardian returns `unsupported_approval_shape` for `live_only`
or queues/rejects according to the caller policy.

Normal messages to `action_required` targets should queue by default rather
than attempting to answer the visible prompt.

## Mailbox Lane

The mailbox lane is Wardian-owned live state. It is independent of provider
transport and exists to avoid unsafe mid-turn injection. This slice keeps
queued messages in the running backend process and drains them when the target
returns to an idle input-ready boundary. Durable mailbox persistence remains a
follow-up requirement before queued messages can survive app restart.

Each queued item stores:

- message id
- source identity, when known
- target session id
- delivery intent
- queue policy
- rendered body
- created timestamp
- status: `pending`, `in_flight`, `delivered`, `failed`
- FIFO order by enqueue id

Queue behavior:

- Messages for the same target are consumed in order.
- Queue results are explicit in CLI/API JSON and include the message id.
- Mailbox consumption occurs only at safe boundaries: provider startup or a
  fresh input-ready transition after a turn. The first implementation drains
  one pending message on each idle transition so a target is not flooded while
  it is starting work on the first queued message.
- Retry is allowed only when the terminal transaction failed before prompt
  bytes entered the PTY, or when the target had no live input channel. If prompt
  bytes were sent but the submit key failed, terminal state is partial or
  unknown; the item is marked failed instead of retried to avoid duplicate
  prompt submission.
- Durable persistence should add provider id, large-body file references,
  explicit ordering keys, cancellation, and expiration.

The mailbox must not silently masquerade as live delivery. A caller that needs
immediate action must use `live_only` and handle structured failure.

## Structured Ask Reliability

`wardian ask` should treat the structured reply store as completion source of
truth. The successful reply body must be returned even if later watch evidence
collection fails with `cursor_expired` or `gap_detected`.

The ask response should keep returning watch evidence when available. If watch
collection fails after a reply is recorded, the response should include the
reply and a degraded evidence field instead of failing the whole ask. Delivery
and watch cursor failures remain visible for debugging, but they do not discard
the reply body.

Pending ask requests should also be cleaned up after terminal completion,
timeout, duplicate reply, or delivery failure so stale request ids do not
accumulate.

## API and CLI Compatibility

Existing control schema version stays at `schema: 1` unless the implementation
must make an incompatible response shape. New fields should be additive where
possible.

`DeliveryDetail` should continue to expose:

- `uuid`
- `name`
- `provider`
- `runtime_state`
- `delivery_state`
- `input_mode`
- `error`

Additive fields may include:

- `intent`
- `queue_policy`
- `message_id`
- `delivery_phase`
- `observed_state`
- `reason`
- `profile`

CLI exit behavior:

- Live or queued delivery to at least one target is a successful send unless
  the caller used `live_only`.
- `live_only` returns non-zero when live delivery is unsafe or unsupported.
- Partial failures remain non-zero and include per-target delivery details.
- Unsupported approval shape returns a distinct error code so automation can
  decide whether to queue, ask the user, or abandon the action.

## Testing Strategy

Use the lowest test layer that proves the behavior.

### Unit Tests

- Provider profile lookup and defaults for all five providers.
- Prompt normalization for message, command, and approval intents.
- Literal vs bracketed-paste chunk selection.
- Per-session lock serialization.
- Delivery routing by target status, intent, and queue policy.
- Approval recognizer matching and non-matching snapshots.
- Ask reply-store success when watch evidence fails.

### Deterministic Native Fake-TUI Tests

Add fake provider/TUI fixtures that simulate:

- swallowed submit key;
- submit key becoming newline;
- delayed readiness;
- existing composer text before send;
- multiline paste handling;
- long-prompt paste truncation or confirmation;
- stale approval prompt that changes before submit;
- concurrent sends to the same target.

These tests run through the native Tauri/PTY layer, not browser-only E2E.

### Real-Provider Native Tests

Real-provider tests are opt-in because they depend on local provider
installation, authentication, and account state. Run Windows ConPTY first,
then macOS/Linux PTY.

Provider matrix:

- Codex
- Claude
- Gemini
- OpenCode
- Antigravity

Input matrix:

- short single-line message
- multiline message
- long message
- leading slash command
- trailing newline and no trailing newline
- concurrent sends to the same target

State matrix:

- idle/input-ready
- processing
- action_required
- fresh startup
- resumed session

Success criteria:

- Prompt content appears exactly once.
- Submit starts a new provider turn or returns a structured unverified state.
- Multiline input does not become multiple accidental submissions.
- Concurrent sends do not interleave.
- Non-idle targets queue or fail according to policy.
- Unknown approval shapes never receive guessed input.
- `wardian ask` returns reply body even when watch cursor evidence expires.

## Consequences

Positive:

- Wardian keeps the user-visible live session while reducing unsafe injection.
- Automation gets truthful delivery states instead of a binary submitted/failed
  result.
- Mailbox fallback improves reliability for busy agents without changing the
  primary interaction model for idle agents.
- Approval behavior becomes auditable and provider-specific.

Negative:

- Provider profiles and recognizers add maintenance cost.
- Some sends that previously attempted direct injection will now queue or fail
  until recognizers prove safety.
- Real-provider validation remains slower and environment-dependent.
- Mailbox semantics introduce ordering, expiration, and cancellation behavior
  that must be designed and tested carefully.

## Rollout

1. Add delivery intent, queue policy, result schema, and compatibility tests.
2. Add provider profile skeletons and keep current behavior behind conservative
   profiles.
3. Implement the live delivery state machine with locks and observable states.
4. Add deterministic fake-TUI native tests for failure modes.
5. Add live mailbox state, idle-boundary drain, and explicit queued delivery
   responses.
6. Fix structured ask reply-store behavior independently of watch evidence.
7. Enable bracketed paste and reduced provider delays one provider at a time
   after fake-TUI and opt-in real-provider evidence.
8. Add approval recognizers last, provider by provider. Until then,
   `approval_action` should fail closed with `unsupported_approval_shape`.
9. Add durable mailbox persistence, cancellation, expiration, and dedupe keys.
