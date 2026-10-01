# Claude Inbox Turn Completions

## Decision

Wardian creates a Claude Inbox completion when the interactive Claude session
finishes a successful response to a user prompt and provides non-empty final
assistant text. This records a completed provider turn. It does not assert that
the user's larger task is complete.

Use Claude Code's `Stop` hook as the completion boundary. The hook supplies
`prompt_id` and `last_assistant_message`; transcript files may lag this event.
User interruption does not fire `Stop`, and API failures use `StopFailure`, so
neither creates a completion item. A missing prompt ID or empty final response
fails closed.

## Delivery and identity

- The injected per-agent hook settings append a complete hook payload to a
  private outbox through an atomic temporary-file rename. The hook does not
  alter the user's Claude settings or block the provider turn.
- Wardian validates the hook name, configured provider session, prompt UUID,
  and final assistant text before creating an Inbox item.
- The stable item ID is `agent-completed:<wardian-session>:<prompt-id>`.
  The item stores the exact response, a bounded display summary, and the
  outbox file's modification timestamp. Replaying a pending event after an app
  restart therefore preserves the original completion chronology. Processing
  time is used only when filesystem timestamp metadata is unavailable, and the
  item's `timestamp_source` records that fallback.
- Queue persistence is an idempotent upsert. An outbox record remains until
  the item is durably present. Startup replays pending records; an existing
  dismissal tombstone consumes the replay without showing the item again.
- The UI event carries the canonical persisted item. The frontend treats it as
  a projection hint; normal queue hydration recovers a missed event.
- Whole-queue snapshots cannot replace or delete backend-owned completion
  records. Explicit dismissal writes a durable tombstone. Read state remains
  monotonic.

## Compatibility

Legacy completion items retain their existing IDs and remain readable and
dismissible. Wardian does not reinterpret historical transcript records as new
completion events during startup hydration.

## Verification boundary

Deterministic tests cover hook filtering, special-character path quoting,
generated outbox writes, outbox-time replay, canonical payload preservation,
and queue dismissal merging. They do not prove the installed Claude Code
version invokes Wardian's injected interactive `Stop` hook or composes it with
a workspace hook; that requires one isolated real interactive turn.
