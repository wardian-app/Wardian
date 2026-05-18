# Real Provider PTY Rendering Lab

## Problem

Wardian's PTY rendering failures are provider-sensitive and do not reproduce reliably with mock providers alone. Issue #110 and related bugs need repeatable evidence from real Codex and Claude PTYs before renderer behavior is changed.

## Decision

`e2e-native/tests/real-provider-rendering-native.test.mjs` is the real-provider PTY lab. It remains opt-in behind `WARDIAN_E2E_REAL_RENDERING=1`, defaults to `codex,claude`, and captures Wardian-side artifacts that can be audited mechanically before outside-terminal screenshots are inspected.

The lab records initial, settled, narrow, wide, card-maximized/restored, window minimized/restored, window maximized/restored, rapid-resize, explicit scrollback, clear, pause, and resume states. Before those captures, it submits the configured provider input by sending the text followed by a PTY submit sequence, defaulting to carriage return (`\r`). When no custom input is configured, the default prompt asks the provider to print 50 numbered `WARDIAN_SCROLL_` lines, and the response marker defaults to `WARDIAN_SCROLL_050`. This is required so resize, scrollback, clear, pause, and resume evidence includes real conversation history and enough terminal history to inspect, rather than only prompt-editing echo. Every state records xterm parser rows, DOM rows, screen geometry, renderer cell metrics, terminal debug geometry, native window geometry, browser viewport/app-shell geometry, screenshot and artifact timestamps, and row-stability timing. Resize and disruptive-action states also record before/after native window geometry, before/after browser viewport geometry, before/after terminal columns and rows, action duration, stable-row duration, and any fit or resize counters exposed by the debug snapshot. The terminal debug surface is gated to development or `VITE_WARDIAN_TERMINAL_DEBUG=1` builds so production native bundles do not expose parser/session history by default.

When scrollback exists, the lab captures top-of-history and mid-history artifacts after disruptive actions. This is required because issue #110 class failures often appear in history rows rather than the visible bottom viewport.

OpenCode runs default to the free remote model `opencode/deepseek-v4-flash-free` through `WARDIAN_E2E_RENDERING_OPENCODE_MODEL`, preventing rendering tests from accidentally using local model backends. The model remains configurable for targeted provider comparisons.

## Audit Rules

`auditRenderingEvidence` now has strict Wardian lab checks for:

- non-empty screenshots and parseable JSON artifacts
- rendered rows stabilizing before the configured settle timeout
- xterm screen rectangles matching renderer cell dimensions
- fixed audit text remaining visible after resize stress states
- submitted audit input remaining present in resumed terminal history
- terminal columns changing when a resize state expects a geometry change
- paused parser rows preserving the latest pre-pause buffer

Outside-terminal parity capture remains separate through `scripts/capture-outside-provider-rendering.ps1`. The Wardian-side lab must be strong enough to diagnose stale geometry, missing rows, row instability, and resize lag without requiring manual screenshot inspection first.

## Runbook

The exact commands and tuning variables are documented in `docs/developer/native-e2e.md` under "Real Provider PTY Rendering Lab".
