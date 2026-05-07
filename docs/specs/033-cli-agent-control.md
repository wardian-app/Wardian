# Spec 033: CLI Agent Control and Communication

- **Status:** Implemented
- **Date:** 2026-05-06
- **Decider:** Wardian Codex

## Context

Spec 024 introduced the `wardian` CLI as a read-oriented agent introspection surface and explicitly deferred mutating commands. The next slice adds the control-plane commands agents need to act on Wardian state: lifecycle commands, workflow operations, and PTY message delivery.

## Decision

Wardian extends the CLI with:

- `wardian agent kill|pause|resume|spawn|clone`
- `wardian agent wait`
- `wardian workflow list|show|run|stop`
- `wardian send`

All mutable operations go through the local named-pipe or Unix-socket control endpoint keyed by `WARDIAN_HOME`. These commands return `app_not_running` with exit code 6 when the desktop app is unavailable.

Read-only workflow commands try the live control endpoint first and fall back to workflow JSON files on disk when the app is unavailable. `workflow show` returns the full workflow definition, including nodes and role mappings, not just a summary.

`wardian agent spawn` requires explicit `--provider` and `--class` values. This keeps agent identity honest when automation creates specialized roles such as reviewers.

`wardian agent wait <target> --until <status>` blocks in the CLI process until a single live agent reaches the requested normalized status or the timeout expires. This is intentionally status-based for the first slice; it avoids model-visible polling loops without introducing a general event stream yet.

`wardian send` injects newline-terminated text into the same PTY input channel used by the GUI. Targets may be a name, UUID, `class:<ClassName>`, or `all`. `--wait-until <status>` combines delivery with a status wait for single-agent targets. When the target already has the desired status, the CLI waits for it to leave that status and return, preventing immediate false success for already-idle agents. Threaded sends are reserved but not implemented; requests with `--thread` are rejected with `not_supported` rather than silently ignored.

## Error Semantics

The control endpoint preserves stable error codes:

- `bad_request` for malformed control JSON.
- `not_found` for unknown live agent or workflow targets.
- `not_supported` for recognized but unsupported request features.
- `request_failed` for operation failures after a request has parsed.

The CLI preserves `not_found` and `not_supported` for app-reported control errors and maps endpoint unavailability to `app_not_running`.

## State Access

Spec 024 described read-only SQLite fallback for the original introspection commands. The current implementation opens the database normally and runs migrations before reading so older Wardian homes remain queryable from the CLI. This is a deliberate compatibility tradeoff: the CLI may upgrade schema metadata during reads, but it does not mutate agent lifecycle state through SQLite.

## Follow-Ups

- Run native E2E watch mode during manual validation for live app spawn/kill/send behavior.
- Add structured per-target delivery details for `send`; the current implementation fails the request when any matched target cannot receive input, but still returns only a success/error envelope.
- Harden provider status transitions and delivery acknowledgement beyond status-based waiting.
- Implement real threaded send semantics or remove the CLI flag.
