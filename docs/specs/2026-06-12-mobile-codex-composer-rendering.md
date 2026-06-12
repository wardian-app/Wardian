# Mobile Codex Composer Rendering

## Context

PR 530 fixed Codex composer color remapping in the main app terminal renderer. The remote mobile terminal uses a separate xterm write path, so Codex frames that still contain the dark composer fill were not normalized before being written to the mobile renderer.

## Decision

The remote mobile terminal now builds a terminal capability context from its active CSS theme and passes that context through the shared terminal normalization helpers. Snapshot writes and live terminal update writes both reuse the existing Codex composer background remapper, keeping the mobile behavior aligned with the main app.

## Verification

Regression coverage in `src/features/remote/RemoteMobileApp.test.tsx` sends Codex snapshot and live update frames with the dark composer background and asserts both are written with the light-mode fill.
