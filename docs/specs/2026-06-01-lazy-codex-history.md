# Lazy Codex History

Codex conversations can become large enough that reading and normalizing the full provider JSONL tail on every chat refresh delays both the desktop main view and the remote view. Wardian should keep recent chat content fast while preserving older provider history automatically.

## Decision

Use bounded transcript reads by default and automatically backfill the larger history after the first render. The initial chat request should load a small recent provider-log tail; a background request should then hydrate the larger retained tail without requiring user action. Terminal scrollback remains separate and continues to use bounded watch-state snapshots.

## Scope

- Add a backend option for chat transcript loading to cap provider-log bytes per request.
- Keep the existing full retained cap as the maximum history that Wardian exposes from provider logs.
- Update the desktop chat view to request a small first slice, then backfill the full retained slice in the background.
- Update remote chat loading to use the same two-phase behavior so mobile/remote views do not block first paint on long Codex logs.
- Preserve existing terminal scrollback behavior in both desktop and remote terminal panes.

## Testing

Backend unit tests should prove the provider log reader respects a caller-supplied tail limit. Frontend tests should prove the desktop chat view and remote store make a small initial transcript request followed by an automatic larger backfill request.
