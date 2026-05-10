# Spec 034: Agent Watch and Communication Stabilization

- **Status:** Implemented
- **Date:** 2026-05-07
- **Decider:** Wardian Codex

## Context and Problem Statement

Spec 024 gave agents a CLI surface for introspection, and Spec 033 added live control commands such as `wardian send` and `wardian agent wait`. That first control slice is useful, but it is still too narrow for reliable agent-to-agent work.

The current communication path is mostly a one-way PTY injection. A caller can send text and optionally wait for a target status, but it cannot directly observe what the target saw, whether output changed, whether a response was produced, or why a delivery path was unavailable. This makes failures ambiguous. During provider probes on 2026-05-07:

- Codex and Gemini timed out waiting for `idle` even though the last observed status remained `idle`.
- Claude and OpenCode failed with `no input channel`, without enough detail to distinguish off sessions, restored sessions missing senders, or unsupported runtime states.

This matches a broader limitation: Wardian agents need a way to watch peers, not just send to them. Similar systems such as `hcom` expose messaging plus observation of transcripts, terminal screens, status changes, file edits, and event streams. Wardian should adopt the same principle while preserving its own architecture: the Rust backend remains authoritative for PTY lifecycle, telemetry, and live state, and Markdown / local state stay inspectable on disk.

## Decision

Wardian adds a live observation surface for agents and stabilizes message delivery around that surface.

### Command Surface

Add:

```bash
wardian agent watch <target>
```

`watch` is a live-control command. It requires the running desktop app for the same `WARDIAN_HOME` and returns `app_not_running` when no control endpoint is available.

Supported options:

- `--since <cursor>`: return only events and output after a prior cursor.
- `--until <condition>`: block until a condition is reached or timeout expires.
- `--timeout <duration>`: maximum wait time, using the existing `ms`, `s`, and `m` syntax.
- `--include <fields>`: comma-separated data classes. Initial values: `status`, `output`, `events`, `delivery`, `agent`.
- `--tail <bytes>`: cap returned terminal output. Default should be conservative enough for model context.

`watch` accepts only a single agent name or UUID in the first implementation. Selectors that can resolve to multiple agents, such as `class:<ClassName>` and `all`, return `not_supported` with a hint to choose a single target. This keeps the first response schema unambiguous and avoids hiding partial observation gaps behind a list response.

Initial conditions for `--until`:

- `status:<normalized-status>`, for example `status:idle`.
- `output:<substring>`, for example `output:WARDIAN_PROBE_CODEX_OK`.
- `event:<kind>`, for example `event:turn_completed`.
- `delivery:<state>`, for example `delivery:submitted` or `delivery:failed`.

Keep:

```bash
wardian agent wait <target> --until <status>
wardian agent wait <target> --until <status> --next
wardian send ... --wait-until <status>
```

`wait` remains the terse condition command. Plain `agent wait` keeps its readiness semantics: if the agent is already in the requested status, it returns immediately. A new `--next` flag waits for a newer matching observation after the initial snapshot. `send --wait-until` uses `--next`-equivalent semantics internally because a send is waiting for the turn triggered by that send, not merely checking current readiness.

`watch` does not replace `wait`; it provides the evidence and output surface that `wait` intentionally hides.

### Streaming Follow Deferral

`watch --follow` is intentionally deferred from the first implementation. The current control transport is one request with one newline-terminated JSON response over a named pipe or Unix socket. Long-lived NDJSON streaming needs separate transport rules for cancellation, heartbeat, backpressure, connection limits, and preventing a follow request from monopolizing control endpoint workers.

For compatibility with future scripts, the first slice reserves the flag shape: `wardian agent watch <target> --follow` parses successfully, then returns `not_supported` with a hint that streaming follow is deferred. It must not silently ignore `--follow`, and it must not start a partial streaming implementation.

When `--follow` is implemented, it must satisfy these rules before being enabled:

- Each follow connection is handled in its own task and must not block unrelated control requests.
- The server sends a heartbeat event at a documented interval when no agent events occur.
- The server drops or coalesces events only with an explicit `gap_detected` event.
- The client closes the connection on timeout, Ctrl-C, or process exit; the server treats disconnect as cancellation.
- The endpoint enforces a per-agent and global follow connection limit and returns `too_many_watchers` when exceeded.

### Watch Response Schema

Non-following `watch` returns one JSON envelope:

```json
{
  "schema": 1,
  "agent": {
    "name": "Wardian-Codex",
    "uuid": "57244fa9-2b9c-4b45-ba32-6919d2786c29",
    "provider": "codex",
    "status": "idle",
    "last_status_at": "2026-05-07T10:14:32.120Z"
  },
  "cursor": "0000000000000042",
  "events": [
    {
      "cursor": "0000000000000041",
      "time": "2026-05-07T10:14:31.870Z",
      "kind": "status",
      "status": "processing"
    }
  ],
  "output": {
    "text": "WARDIAN_PROBE_CODEX_OK\r\n",
    "truncated": false
  },
  "delivery": {
    "input_available": true,
    "last_state": "submitted",
    "last_error": null
  }
}
```

Cursors are opaque strings. Callers must compare them only for equality or pass them back through `--since`.

Cursor semantics:

- Cursors are scoped to one Wardian session ID. Passing a cursor from another agent returns `invalid_cursor`.
- Events and output batches share one monotonically increasing cursor sequence per agent, so a cursor represents a total order across status, delivery, provider events, and output tap updates.
- If `--since` is older than the retained event/output ring, the command fails with `cursor_expired` and includes the oldest available cursor in error details. Silent gaps are not allowed.
- If retention is lost during a blocking `--until`, the command fails with `gap_detected` rather than continuing against incomplete evidence.
- `--tail <bytes>` truncates on valid UTF-8 character boundaries after ANSI/control bytes are preserved as text. The response includes `truncated: true`, `omitted_bytes`, and `oldest_available_cursor` when output is trimmed.

### Backend State Model

Extend `ActiveAgent` with live observation state:

- `last_status_at`: timestamp updated whenever the normalized status changes.
- `watch_events`: bounded in-memory event ring for status changes, delivery attempts, and provider parser events.
- `output_tap`: bounded, non-draining terminal output ring independent from the UI's drain-on-read `output_buffer`.
- `delivery_state`: latest structured delivery result per target session.

The terminal reader writes to both:

- `output_buffer`, still drained by `read_agent_pty` for the frontend terminal.
- `output_tap`, retained for CLI observation and not drained by UI reads.

This avoids a race where the frontend consumes the only copy of output before another agent can inspect it.

The control endpoint adds request/response types for:

- `AgentWatch`
- `AgentWatchFollow` if streaming is implemented as a distinct request

The response schema remains in `wardian-core` so CLI and Tauri serialization cannot drift.

The retained rings must be bounded by both event count and byte count. The exact defaults are implementation details, but the implementation must expose enough metadata for callers to detect loss: `oldest_available_cursor`, `latest_cursor`, `truncated`, and `omitted_bytes`.

### Status and Wait Semantics

`last_status_at` becomes load-bearing in live snapshots and `watch` responses. It updates whenever Wardian records a status transition. When a provider emits a distinct turn completion event that returns to the same normalized status, Wardian records a new watch event even if the status string is unchanged. This lets `--next` and `send --wait-until` detect a completed turn without changing plain readiness checks.

`wardian agent wait <target> --until idle` returns immediately when the target is currently idle. `wardian agent wait <target> --until idle --next` waits for a matching status observation after the initial cursor.

`wardian send --wait-until idle` should internally perform:

1. Capture a watch cursor for the target.
2. Submit the message.
3. Watch from that cursor until `status:idle` or a delivery failure is observed.

`send --wait-until` remains valid only for a single name or UUID target. Broadcasts and class selectors may send without waiting, but combining them with `--wait-until` returns `not_supported`.

### Provider-Aware Delivery

`wardian send` should use the same provider-aware submission behavior as the GUI `submit_prompt_to_agent` path. It should not blindly write `message + "\r"` for every provider.

Initial provider rules:

- Codex: send normalized text, delay briefly, then send the Codex submit sequence already used by `submit_prompt_via_sender`.
- Gemini: send normalized text, delay briefly, then carriage return.
- Claude: send normalized text, delay briefly, then carriage return unless evidence shows Claude needs a different submit path.
- OpenCode: preserve the existing headless submit behavior used by the GUI when appropriate, because interactive OpenCode PTY input has separate readiness constraints.
- Mock: retain deterministic PTY input for tests.

Delivery details split runtime capability from delivery outcome. Callers must not have to infer why delivery failed from one overloaded field.

Runtime states:

- `target_off`: target exists but its normalized status is `off` or it has no running PTY/headless runtime.
- `live_pty_available`: target has a live PTY input sender.
- `restored_without_sender`: target is live in the roster but has no registered input sender after restore or runtime reconciliation.
- `headless_available`: target supports a non-PTY submit path, such as OpenCode headless submit.
- `queued_not_ready`: target runtime exists but provider-specific readiness has not completed.

Delivery states:

- `pending`: Wardian selected a runtime path but has not submitted yet.
- `submitted`: Wardian wrote to the PTY sender or accepted the headless submit request.
- `failed`: provider-aware submit failed after a runtime path was selected.

`no_input_channel` remains the concrete error code for a missing sender, but delivery details must also include `runtime_state` (`target_off`, `restored_without_sender`, or another state) and `delivery_state` so callers can tell whether the failure is capability-related or submit-related.

Delivery responses should report per-target details:

```json
{
  "schema": 1,
  "ok": false,
  "target": "class:Coder",
  "delivery": [
    {
      "uuid": "agent-1",
      "name": "CoderOne",
      "provider": "codex",
      "runtime_state": "live_pty_available",
      "delivery_state": "submitted",
      "error": null
    },
    {
      "uuid": "agent-2",
      "name": "CoderTwo",
      "provider": "claude",
      "runtime_state": "restored_without_sender",
      "delivery_state": "failed",
      "error": {
        "code": "no_input_channel",
        "message": "Agent is live in the roster but has no PTY input sender"
      }
    }
  ]
}
```

CLI output follows the existing Spec 024 error contract. If any matched target fails, the command exits nonzero, stdout is empty, and stderr contains the standard error envelope with `details.delivery[]`. If every matched target is submitted successfully, stdout contains the success envelope with `delivery[]` and stderr is empty.

### Error Semantics

Add or standardize these delivery and watch error codes:

- `no_input_channel`: target exists but has no live input sender.
- `target_off`: target exists and is currently off.
- `watch_timeout`: requested watch condition was not observed before timeout.
- `unsupported_watch_condition`: condition string is syntactically valid but not supported.
- `invalid_cursor`: cursor is malformed or belongs to a different agent.
- `cursor_expired`: requested cursor is older than retained observation state.
- `gap_detected`: observation state was lost while a watch command was waiting.
- `too_many_watchers`: reserved for the deferred `--follow` implementation.
- `output_truncated`: warning-style field, not necessarily a command failure.

Existing `not_found`, `not_supported`, and `app_not_running` semantics remain unchanged.

### Persistence Boundary

The first slice is live-only. `watch` does not fall back to SQLite because persisted state cannot prove current PTY output or delivery state. Later work may persist event summaries to `state.db`, but this spec keeps the first implementation bounded and avoids treating stale state as live evidence.

### Testing Plan

Unit tests:

- CLI argument parsing for `agent watch`, `--since`, `--until`, `--include`, and `--tail`.
- Reserved `--follow` behavior: the flag parses and returns `not_supported` until the streaming transport slice is implemented.
- Watch response serialization in `wardian-core`.
- Event ring cursor behavior and truncation behavior.
- `agent wait` immediate readiness behavior when the target already starts in the requested status.
- `agent wait --next` and `send --wait-until` behavior when the target begins in the desired status and later produces a newer completion event.
- Provider-aware send byte sequences for Codex, Gemini, Claude, and mock.
- Structured delivery error mapping for off agents and missing input channels, including separate `runtime_state` and `delivery_state` fields.
- Expired cursors, gap detection, output truncation metadata, and UTF-8 boundary handling.

Native E2E tests:

- Mock provider: `send -> watch --until output:<token>` captures the response without relying on frontend terminal reads.
- Mock provider: `send --wait-until idle` succeeds when the target starts idle and completes a fast turn.
- Off target: `send` reports `target_off` or `no_input_channel` with per-target details.
- Multi-target partial delivery: `class:<ClassName>` returns nonzero with stderr `details.delivery[]` when one target cannot receive input.
- Restored session missing sender: live roster entry without an input sender reports `restored_without_sender` rather than a generic request failure.
- OpenCode headless and interactive readiness paths are exercised separately.
- Windows native run verifies provider submit sequences through ConPTY.
- Browser-visible smoke: UI terminal continues to render while CLI `watch` observes the same output, proving the observation tap is non-draining.
- Deferred `--follow` tests must cover heartbeat, cancellation, connection limits, and backpressure before the flag is enabled.

Manual provider probes:

- Codex, Claude, Gemini, and OpenCode should each receive a probe prompt and either return the requested token or produce a structured delivery/runtime diagnostic.
- Use Playwright or Chrome DevTools to inspect the running app only when the CLI observation evidence is insufficient or a UI/runtime state mismatch is suspected.

## Consequences

- **Positive:** agents can observe peers directly instead of relying on status-only waits or human-visible terminal state.
- **Positive:** `wait` becomes more reliable without becoming verbose; `watch` carries the evidence for debugging and coordination.
- **Positive:** frontend PTY reads no longer race CLI/agent observation.
- **Positive:** provider communication failures become diagnosable through structured delivery details.
- **Positive:** the control plane moves closer to Wardian's transparent Habitat model: status, output, and delivery state are visible to agents through a stable textual surface.
- **Negative:** maintaining an output tap and event ring duplicates some terminal data in memory. The implementation must bound retained bytes and event count.
- **Negative:** `watch --follow` is deferred because long-lived control requests are more complex than the current one-request/one-response pipe model.
- **Negative:** provider-aware send paths increase coupling between CLI control and provider runtime behavior. Keeping the provider rules in the Rust backend mitigates drift with the GUI path.
- **Negative:** live-only watch means agents cannot inspect historical output after the app closes in the first slice. Persisted event history should be considered separately.
