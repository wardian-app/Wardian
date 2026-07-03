# Agent Conversation Logging Override

## Context

Wardian already has a global conversation logging setting for persisted conversation archives. Issue 609 needs the same control at agent scope so operators can disable archive logging for one agent without changing runtime behavior or turning logging off for the whole workspace.

## Design

Each agent config can set `conversation_logging` to `default`, `enabled`, or `disabled`. The default value follows the global conversation logging setting. Enabled and disabled are explicit per-agent overrides for persisted conversation archives.

The backend computes effective logging from the global setting and the agent override. If the effective result is disabled, archive write paths discard the current capture cursor instead of writing conversation records. This prevents later re-enable operations from backfilling events observed while logging was disabled.

Conversation listing remains archive history, matching the global setting semantics. Disabling logging stops new records from being persisted; it does not delete or hide archives created before logging was disabled.

## UI

Agent advanced settings expose a `Conversation Logging` selector with `Use global setting`, `Enabled`, and `Disabled`. Toggling this setting is an archive governance change and does not require a provider restart.

## Non-Goals

- No graph or communication-topology semantics.
- No deletion, shredding, or hiding of previously written archive files.
- No provider-specific logging mode; enforcement is in Wardian's archive layer.
