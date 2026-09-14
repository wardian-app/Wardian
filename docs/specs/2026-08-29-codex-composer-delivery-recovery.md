# Codex composer delivery recovery

## Decision

Automated delivery to an interactive Codex terminal must not send Return until
Codex has visibly applied the bracketed-paste payload to its composer. A native
PTY write receipt proves only that ConPTY accepted the bytes. It does not prove
that Codex's event loop committed the paste before the submit key arrived.

Wardian accepts either of these provider-owned observations after the delivery
cursor:

- the complete normalized literal payload in the active composer; or
- Codex's collapsed `[Pasted Content N chars]` marker after the current
  transaction's pre-payload cursor. Codex may render a literal leading segment
  and collapse only the remainder, so `N` is not necessarily the total prompt
  length and is not used as a payload checksum. The delivery lock and cursor
  scope the marker to the current transaction. Marker recognition tolerates
  terminal row wrapping and cell-only repaints that do not repeat the `›`
  prompt glyph. Only the retained-output fallback, used after cursor expiry,
  requires the marker to be in the active prompt.

If neither observation arrives within 15 seconds, Wardian withholds Return,
records `payload_apply_unconfirmed`, and does not retry automatically. The
existing 750 ms Codex profile delay remains a minimum settle interval, not the
delivery proof.

On refusal, `observed_state` reports bounded, content-free diagnostics: matched
normalized literal bytes, normalized payload bytes, recognized or unknown
marker format, marker character count when recognized, the running or installed
Codex version, and whether evidence came from the transaction delta or active
prompt fallback. An unknown marker remains unconfirmed and Return remains
withheld.

## Canonical composer proof after a resumed-session repaint

Issue #1192 exposed the next gap. Both observations above are read from the
transaction delta, and the canonical screen was consulted only *after* the delta
had already confirmed the payload. A partial diff repaint following a
pause/resume can redraw the composer without emitting the whole payload
contiguously into that delta, so the delta stalls on an incomplete prefix while
the canonical active composer already holds the exact payload. The recorded
refusal shows `literal_match_bytes=26` against `normalized_payload_bytes=91`
with no marker: proof existed, and the gate could not reach it.

Wardian therefore accepts a third observation, taken from the broker's canonical
screen rather than the delta, admissible only inside all three of these fences:

- the exact, complete payload in the current active composer. This is read only
  after the last prompt caret, so scrollback history and an already-submitted
  turn cannot qualify, and a startup screen or model menu yields nothing;
- the snapshot's runtime generation equal to a baseline captured **before** the
  payload write, so a replaced or foreign runtime cannot qualify;
- a baseline in which the payload was **not** already applied, so a stale draft
  left by an earlier attempt is not mistaken for new evidence.

If no baseline is available there is nothing to fence against, so canonical-only
evidence is refused and the gate falls back to transaction-delta proof alone.
The canonical screen is polled on its own slower cadence than the delta because
it clones the screen and formats scrollback.

This widens the evidence Wardian will accept; it does not weaken what counts as
proof. Return still requires the complete payload in the current composer on the
current runtime, the 15-second refusal and `payload_apply_unconfirmed`
diagnostics are unchanged, and there is still no automatic retry.

## Production repaint regression

Issue #1068 exposed a fixture gap after the original fix: Codex 0.151.0 can
repaint only the changed composer cells after a large paste. The transaction
delta then contains `[Pasted Content N chars]` but no new `›`. Requiring the
prompt glyph inside that already cursor-scoped delta rejected valid evidence.
Fresh-home acceptance happened to repaint the complete prompt row and therefore
did not reproduce the populated-home behavior.

The transaction cursor, not a repeated glyph, establishes causality for the
normal path. Historical-scrollback protection remains on the cursor-expiry
fallback, where Wardian still parses only the active prompt.

Queued sends also preserve their exact delivery result while a caller waits for
a later status or output condition. If delivery fails before submission, the
CLI returns that terminal delivery error (for example
`payload_apply_unconfirmed`) rather than a bare `watch_timeout`. If submission
is confirmed but the requested condition expires, the response contains both
the known delivery state and a separate `watch_error`. If no submit-start
evidence appears, the error is `delivery_submission_timeout`; it does not claim
that the provider accepted the turn.

## Diagnosed failure

Issue #1058 preserved two live Codex sessions. Wardian-Researcher's terminal
ended at `[Pasted Content 6479 chars]`, while its Codex history contained no
matching user turn. Wardian had recorded successful payload and submit-key PTY
writes followed by `provider_turn_start_timeout`. The provider stayed idle.

The prior readiness recognizer accepted any trailing Codex `›` prompt. It
therefore treated a non-empty composer as ready and allowed every later request
to target the same stale draft. Restart appeared ineffective because the same
large request recreated the paste/Return race after each restart. Healthy
controls showed that a generic turn-start timeout can also be transient, so
timeout alone cannot diagnose the persistent state.

Wardian now rejects a current collapsed-paste marker as compose readiness. If a
turn-start timeout leaves that marker active, delivery reports:

- error and delivery phase: `provider_composer_stalled`;
- observed state: `payload_pending_in_composer`; and
- an identity-preserving recovery command.

A timeout without an active pending-paste marker remains
`provider_turn_start_timeout`. Its guidance is reconciliation, not automatic
retry, because a late provider start is still possible.

## Recovery

After installing a build containing this change, clear an already-stalled Codex
composer with:

```bash
wardian agent restart <agent-name-or-uuid>
```

Restart preserves the Wardian UUID, habitat, and provider session history. It
does not replay the failed request. Before issuing a replacement, inspect the
provider history or delivery record and decide explicitly that the original
turn never started. Reuse a caller idempotency key when the calling workflow
supports one.

`wardian agent doctor <agent-name-or-uuid>` reports
`provider_input_state: stalled_composer`, the recovery command, and
`provider_composer_stalled` in `reasons` when the marker is observable. Doctor
uses a 15-second diagnostic control budget so it does not fail at the 500 ms
fast-read threshold.

## Invariants

- Wardian never sends Return based only on a PTY payload-write receipt for an
  accountable Codex delivery.
- Wardian never automatically replays a payload after its bytes may have
  reached the composer.
- A marker in historical scrollback does not block a later empty composer; only
  the active prompt repaint is considered.
- Recovery never deletes or replaces the Wardian agent identity, habitat, or
  session history.
- Human terminal input remains direct and is not delayed by this automated
  delivery gate.

## Verification

Unit and integration coverage must prove:

- a complete literal payload and a transaction-scoped collapsed-paste
  observation release Return only when the terminal broker's canonical active
  screen confirms the same current-composer evidence;
- a transaction-scoped cell-only marker repaint releases Return without a
  repeated prompt glyph only when that marker is present in the active composer;
- a stale redraw, scrollback replay, or unrelated marker-like transaction
  output remains fail-closed when it is absent from the active composer;
- a painted literal prefix alone does not release Return;
- unknown marker-like output reports provider compatibility diagnostics and
  remains fail-closed;
- absent payload-application evidence fails without writing Return;
- an active collapsed paste is not classified ready;
- a historical marker followed by a fresh prompt is ignored;
- confirmed provider turn start still produces `provider_accepted`; and
- queued terminal delivery failure remains distinguishable from a post-submit
  watch timeout in one CLI response; and
- agent doctor uses the diagnostic timeout and reports the stalled state.

Native real-provider acceptance on Windows must deliver a request larger than
the structured-ask inline threshold to a disposable Codex agent, observe
provider turn start, and verify that no payload remains in the composer. The
preserved production specimens are evidence only and must not be mutated by the
test.
