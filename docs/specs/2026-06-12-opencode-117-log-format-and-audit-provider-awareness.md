# OpenCode 1.17 Log Format Support and Provider-Aware Rendering Audit

- **Date**: 2026-06-12
- **Status**: Accepted
- **Context branch**: `fix/terminal-webgl-context-churn`

## Context

Live validation of gemini and opencode rendering (native real-provider audit)
surfaced two distinct problem classes:

1. **OpenCode ≥ 1.17 changed its log layout.** Older opencode wrote one
   timestamped log file per process with `service=session ... created
   id=ses_x` lines. 1.17 writes a single rolling `opencode.log` shared by
   every concurrently running instance, tags each line with a `run=<id>`
   token, and logs session creation as `message=created id=ses_x ...` with no
   `service=` tokens at all. Wardian's session-id capture
   (`opencode_extract_created_session_id`) and status/metrics parsing
   (`opencode_metrics_from_log`) silently stopped matching, so
   `resume_session` was never recorded for opencode agents — breaking session
   continuity (pause/resume, external session reuse).

2. **The rendering-audit lab checks assumed scrollback-stream TUIs.** The
   numbered-response completeness/duplication checks, resize marker checks,
   and pause buffer-equality check were designed against codex/claude, whose
   diff renderers append content to the xterm stream. gemini (bottom-anchored
   repaints, `✦`-prefixed first response line) and opencode (full-screen
   in-place TUI that owns its own scrolling and repaints idle chrome)
   structurally cannot satisfy several of those checks, producing false
   failures unrelated to Wardian rendering defects.

## Decisions

### 1. Parse both opencode log formats, scoped per agent

`opencode_extract_created_session_id` accepts both the pre-1.17
(`service=session` + `created`) and 1.17+ (`message=created`) session-created
line shapes. A new `opencode_extract_created_session_id_for_agent(log_path,
agent_marker)` scopes extraction in the shared rolling log: it collects the
`run=` ids from lines mentioning the agent's Wardian session UUID (the
`OPENCODE_CONFIG` path embeds it and is logged by `message=loading`), then
returns the last session-created entry belonging to those runs, falling back
to unscoped extraction for legacy per-instance logs. The pause-capture path
(`capture_opencode_pause_resume_session`) and the telemetry watcher both use
the scoped variant, preventing cross-agent session-id contamination when
multiple opencode agents run concurrently.

`opencode_metrics_from_log` likewise accepts the 1.17 markers
(`message=loop ... step=N`, `message="exiting loop"`, `level=ERROR`,
`timestamp=<rfc3339>` tokens) alongside the legacy
`service=session.prompt`/positional-timestamp format.

### 2. Provider-aware rendering audit strictness

The real-provider rendering audit (native test + `rendering-audit.mjs`)
classifies providers by TUI architecture:

- **Diff renderers (codex, claude)** — full strictness: complete numbered
  response in scrollback, no duplicated rows (warnings for diff-renderer
  resize repaints), audit marker visible after every resize, paused buffer
  exactly equal to pre-pause buffer.
- **Bottom-anchored repainter (gemini)** — `✦` accepted as a response-line
  prefix; the post-resize marker may live in xterm scrollback instead of the
  visible viewport (gemini repaints only its input chrome after some window
  transitions).
- **In-place TUI (opencode)** — owns scrolling and repaints in place, so
  xterm scrollback holds repaint overflow (expected duplicates) and never the
  complete response. Completeness/duplication/marker history checks are
  skipped; live turn completion is asserted by a contiguous visible numbered
  tail ending at the expected maximum, and pause must not blank the terminal.

Supporting harness hardening, all provider-quirk driven: auto-answer shell
approval dialogs, resubmit the prompt when a model ends its turn with an
incomplete numbered response, tolerate TUI input boxes that wrap typed text
around border glyphs or truncate with ellipses at narrow widths (echo
confirmation is best-effort and recorded as `echo_confirmed`), treat wheel
events forwarded to TUI-owned scrolling as expected behavior, and poll for
the asynchronously recorded opencode session id.

## Consequences

- **Positive**: OpenCode session continuity works again on opencode ≥ 1.17,
  with correct attribution under concurrent multi-agent use.
- **Positive**: The live gemini+opencode rendering audit passes end-to-end
  (14 window/lifecycle states per provider), confirming the PTY-size restore
  fix on this branch with real providers.
- **Negative**: Audit coverage for opencode content integrity is inherently
  weaker (visible-tail contiguity instead of full-history completeness); a
  dropped line outside the visible viewport would not be detected for
  opencode.
- **Risk**: Future opencode log format changes can silently break parsing
  again; the unit tests pin both known formats so a third format surfaces as
  a test gap rather than a silent regression.
