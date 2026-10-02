# Terminal launch parity

Status: accepted
Date: 2026-10-02

## Decision

Interactive provider terminal capabilities belong to Wardian, not its parent
shell. Clear inherited `NO_COLOR`, `NODE_DISABLE_COLORS`, and `FORCE_COLOR` from
the provider command before advertising the existing truecolor/xterm identity.
Preserve other environment values and explicit provider CLI options. The
desktop process itself need not rewrite its global environment.

An interactive Workbench Agent Session can claim an unowned runtime when it is
visible and mounted, using the same activation handshake as Agents. This
applies to restored tabs without requiring DOM focus or a click. Reveal waits
for activation and fitted geometry. Hidden, suspended, and read-only views
remain passive. An existing owner is never automatically displaced.

Both surfaces repeat automatic activation after a deferred registration
recovers. Measure and report the current viewport before activation; recheck
the absence of an owner inside the client's serialized activation queue.

## Evidence and verification

A desktop started from an automation shell retained `NO_COLOR=1`; its live
provider children inherited the same flag despite `TERM=xterm-256color` and
`COLORTERM=truecolor`. The backend regression fails before normalization.

The Agents frontend regression passes for immediate registration and fails
when the first registration returns `SessionNotFound`. Recovery previously
re-registered the view without repeating startup activation. A native fixture
restores a visible tab before resuming its mock runtime, exercising real
registration failure and recovery without provider requests. Live sizing
causality must not be inferred solely from this isolated reproduction.
