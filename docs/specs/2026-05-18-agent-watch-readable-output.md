# Agent Watch Readable Output

- **Status:** Implemented
- **Date:** 2026-05-16
- **Issue:** [#273](https://github.com/wardian-app/Wardian/issues/273)
- **Decider:** Wardian Codex

## Decision

`wardian agent watch` returns agent-readable output by default. The default watch include set is `status,transcript,output,delivery`.

- `transcript` is provider-adapted, provider-neutral answer text.
- `output` is a sanitized terminal fallback with common ANSI, OSC, cursor, and clear-line controls removed.
- `raw_output` is opt-in through `--include raw_output` or `--raw`.

Wardian keeps the internal non-draining raw PTY tap. Raw terminal text remains required for PTY debugging, but it is no longer the default surface returned to agents.

## Schema

Default readable response:

```json
{
  "schema": 1,
  "agent": {
    "uuid": "agent-1",
    "name": "CoderOne",
    "provider": "codex",
    "status": "idle",
    "last_status_at": "2026-05-16T12:00:00.000Z"
  },
  "cursor": "agent-1:0000000000000002",
  "events": [],
  "output": {
    "cursor": "agent-1:0000000000000002",
    "text": "Final answer\n",
    "truncated": false,
    "omitted_bytes": 0
  },
  "transcript": {
    "cursor": "agent-1:0000000000000002",
    "messages": [
      {
        "role": "assistant",
        "text": "Final answer",
        "provider": "codex",
        "turn_id": "turn-1",
        "source": "response_item"
      }
    ],
    "latest_text": "Final answer",
    "truncated": false,
    "omitted_bytes": 0
  },
  "delivery": {
    "delivery": []
  }
}
```

Raw opt-in response:

```bash
wardian agent watch CoderOne --include raw_output --raw
```

```json
{
  "schema": 1,
  "agent": {
    "uuid": "agent-1",
    "name": "CoderOne",
    "provider": "codex",
    "status": "idle",
    "last_status_at": "2026-05-16T12:00:00.000Z"
  },
  "cursor": "agent-1:0000000000000002",
  "events": [],
  "output": {
    "cursor": "agent-1:0000000000000002",
    "text": "",
    "truncated": false,
    "omitted_bytes": 0
  },
  "raw_output": {
    "cursor": "agent-1:0000000000000002",
    "text": "\u001b[31mFinal answer\u001b[0m\r\n",
    "truncated": false,
    "omitted_bytes": 0
  },
  "delivery": {
    "delivery": []
  }
}
```

## Provider Coverage

This slice extracts transcript messages for Codex, Claude, Gemini, Antigravity, OpenCode, and the mock provider. Gemini backfills completed assistant text from Gemini chat logs; Antigravity backfills completed assistant text from its conversation transcript; OpenCode also backfills assistant text from the provider session database when the live TUI/log stream does not provide a clean structured transcript line. Ambiguous structured lines fall back to sanitized terminal `output`. `--until output:<token>` checks transcript text, sanitized output, and the internal raw PTY fallback so marker-based automation continues to work without exposing raw PTY text by default.

## Compatibility

The top-level `output` field remains present for existing consumers, but it now contains sanitized terminal text. Consumers that depended on ANSI/control sequences must opt in to `raw_output`. Existing cursor, `--since`, `--tail`, delivery, status, event, and output wait behavior are preserved.
