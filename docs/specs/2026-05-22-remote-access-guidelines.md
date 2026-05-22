# Remote Access Guidelines

## Status

Accepted.

## Context

Wardian's operating model is local-first. The desktop app, CLI, agent sessions, worktrees, telemetry, and Markdown state are safest when they remain on a trusted host machine. Remote operation is still useful for checking agent progress, restarting a process, or reviewing a local web surface from another device.

The documentation previously had scattered references to shared `WARDIAN_HOME` state and remote testing, but it did not give operators a clear policy for private network access, Tailscale setup, or when public exposure is acceptable.

## Decision

Wardian remote access guidance will prefer a private Tailscale tailnet:

- Tailscale device connectivity and MagicDNS are the default remote access layer.
- Tailscale SSH is recommended for supported Linux and macOS destination hosts when shell access is needed.
- Standard SSH or platform-native remote desktop over Tailscale is recommended for Windows destination hosts.
- Tailscale Serve is the preferred way to expose a localhost web service privately to the tailnet.
- Tailscale Funnel is documented only as a temporary public demo mechanism and must be reset after use.

The guide avoids machine-specific paths and uses placeholders such as `<absolute-wardian-home-path>`, `<wardian-host>`, and `<tailnet-name>` so it remains cross-OS and cross-computer by default.

## Consequences

Operators get a single guide for remote Wardian access without weakening the local-first security posture. The tradeoff is that public sharing requires an explicit extra step and stronger warnings instead of being treated as the normal path.

Future docs that mention remote Wardian use should link to `docs/guide/remote-access-tailscale.md` rather than repeating Tailscale setup details.

## Verification

This is a documentation-only decision. Review should confirm:

- The guide includes POSIX shell examples before PowerShell equivalents where commands differ.
- The guide uses placeholders instead of local machine paths.
- The guide distinguishes Tailscale Serve from Funnel.
- The guide does not recommend public exposure for routine Wardian operation.
- The guide preserves `WARDIAN_HOME` shared-state guidance for app and CLI testing.
