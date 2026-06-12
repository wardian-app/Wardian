# Claude Interactive Print-Mode Flags

## Context

Claude Code now documents `--input-format` and `--output-format` as print-mode options. Local Claude Code 2.1.174 reports the same contract in `claude --help`, including `--input-format <format>` as only valid with `--print`.

Wardian's interactive Claude agent launch previously passed:

```bash
claude --verbose --input-format stream-json --output-format stream-json --session-id <uuid>
```

That shape made visible Claude agents fail before the TUI could start with:

```text
Error: --input-format=stream-json requires --print
```

## Decision

Wardian strips Claude print-mode stream flags from interactive PTY launches. Interactive Claude agents keep `--settings`, `--session-id`, `--resume`, `--name`, `--add-dir`, model, permission, and tool configuration arguments.

Headless and bootstrap Claude paths continue to use stream-json only where Wardian also passes `--print`.

## Consequences

Interactive Claude status should rely on the visible PTY stream, permission hook events, and transcript/log watchers rather than Claude's print-mode JSON stream. Outside rendering captures must mirror the same interactive launch shape and must not reintroduce print-mode stream flags.
