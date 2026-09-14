# Delivery Receipts and Safe Mailbox Recovery

- **Status:** Implemented
- **Date:** 2026-08-01

## Problem

The mailbox already drains one queued message when a target returns to an idle
boundary. A per-agent polling retry worker does not improve that normal busy
case. It can instead re-send a payload after an ambiguous terminal write,
which risks leaving duplicate text in a provider composer.

Closing Wardian is different: pending mailbox records must survive restart.
The most dangerous case is a shutdown after a payload has crossed the PTY
boundary but before Wardian can prove that the provider accepted it.

## Decision

Use durable state transitions and receipt boundaries instead of a persistent
retry loop.

| Boundary | Required evidence | Result when unavailable |
| --- | --- | --- |
| Target is busy | Existing idle/status observation | Keep the mailbox record pending. |
| App restarts with pending work | Durable pending mailbox record | Restore it and give it one status-gated drain attempt. |
| Prompt payload write | Native PTY writer completes `write_all` and `flush` | Do not press the submit key. Mark the state unknown/failed. |
| Codex submit | Native payload-write receipt plus post-cursor complete literal payload or `[Pasted Content N chars]` composer evidence, or the exact payload in the canonical active composer on the pre-write runtime generation when it was not already applied there | Do not send Enter; record `payload_apply_unconfirmed` and require identity-preserving recovery if the payload remains in the composer. |
| Provider acceptance | A provider-originated `turn_started` event after the submit cursor | Persist `provider_accepted` and mark the interaction delivered. |

Codex receives a fixed 750 ms provider-profile minimum settle window after the
write receipt. That delay is not proof that its TUI applied bracketed paste.
Wardian now waits up to 15 seconds for provider-owned composer evidence before
Enter, recognizing either the complete literal payload or Codex's collapsed
long-paste marker. A literal prefix is insufficient. If that proof is absent,
delivery fails without Enter or automatic replay. See
[Codex composer delivery recovery](2026-08-29-codex-composer-delivery-recovery.md).

The app no longer runs a two-second, per-agent mailbox polling worker and no
longer expires otherwise valid mailbox entries based on age. Idle and
ready-status observations remain the normal delivery trigger.

## Durable State Model

Mailbox records are durable while `pending` or `in_flight`.

- `pending` has not crossed a terminal-input boundary. It is safe to restore
  and attempt after a restart.
- `in_flight` is never automatically replayed after restart. It is completed
  only when the latest durable attempt says `provider_accepted`; otherwise it
  becomes a failed interaction with `delivery_interrupted` evidence.
- `submit_started` means the native writer acknowledged the payload and its
  durable receipt was recorded before the submit key.
- `submit_sent_unconfirmed` remains an observable intermediate state, not a
  delivered state.
- `provider_accepted` means the provider emitted a new turn-start event after
  the native submit. It is the normal terminal state for native message and
  command delivery.

Errors before any terminal input boundary, such as a missing runtime or an
input-readiness failure, remain pending for the next genuine lifecycle or
idle observation; this does not schedule a retry. Once an input request has
been accepted by the broker, a timeout or writer error is treated as ambiguous
and is not automatically retried.

## Limits

This gives an at-most-once recovery policy after terminal injection. It cannot
provide distributed exactly-once delivery because provider CLIs do not expose a
shared idempotency key or an acknowledgement correlated to Wardian's message
id. An interrupted delivery may therefore be marked failed even if the
provider eventually processes it. The UI and CLI must surface that result so a
person can inspect the provider session before choosing to send again.

## Verification

The implementation has focused coverage for:

- restoring pending mailbox work and failing ambiguous in-flight work;
- native writer acknowledgements;
- refusing to press submit if the payload receipt cannot be persisted;
- withholding Codex submit until literal or collapsed-paste application is observed;
- requiring a provider turn-start event after the captured submit cursor; and
- native live delivery reaching `provider_accepted` only after that receipt.
