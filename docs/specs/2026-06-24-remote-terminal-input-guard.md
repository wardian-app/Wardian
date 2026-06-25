# Remote Terminal Input Guard

## Problem

The remote PWA terminal uses xterm.js inside a fixed mobile detail view. A previous fix hid xterm's `.composition-view` so provider-echoed composer text would not be visually duplicated, but xterm still owns a hidden helper textarea that it moves and resizes during composition and clipboard handling.

On narrow remote PWA screens, that helper textarea can exceed the terminal host width while the user types. Mobile composition input can also produce cumulative text frames such as `h`, then `he`, then `hel`. If Wardian forwards each frame as raw PTY input, the provider receives real duplicate prompt text rather than a visual-only echo.

## Design

The remote terminal host now has an explicit input guard contract:

- The host remains the only xterm mount point and keeps the existing composition-preview suppression.
- The host clips overflow so xterm helper DOM cannot expand the mobile layout beyond the visible terminal pane.
- xterm helper containers are constrained to the host width.
- During active or just-ended composition, cumulative plain-text input frames are normalized to the new suffix before being sent over the terminal attach websocket.

Terminal attach ownership, websocket transport, terminal resize messages, binary input, control-key input, and provider output normalization stay unchanged.

## Verification

Regression coverage lives in `src/features/remote/RemoteMobileApp.test.tsx`. The selected-agent detail test asserts that the remote terminal attach host carries the input guard and overflow clipping classes, preserving the mobile sizing contract alongside the existing composition suppression. A second regression test simulates cumulative mobile composition frames and verifies that Wardian forwards only `h`, `e`, `l` instead of `h`, `he`, `hel`.
