# Unified Release Installers

## Status

Accepted for v0.3.4 release packaging.

## Context

Wardian publishes desktop installers through the Tauri release workflow. The CLI is already built during the Tauri `beforeBuildCommand` through `npm run stage-cli`, copied into `src-tauri/resources/bin`, and bundled as an application resource. At runtime, the app installs that bundled CLI into the user's Wardian home `bin` directory.

The release workflow also built and uploaded standalone `wardian-cli-*` assets. That made the GitHub release look like the main app and CLI were separate installation choices, even though the intended user experience is one Wardian install that includes both.

## Decision

The release workflow publishes only the Tauri installer artifacts. It no longer builds or uploads standalone CLI release assets.

The Tauri build continues to pass `WARDIAN_CLI_TARGET` into `stage-cli`, so each installer includes the CLI binary for the platform being packaged.

## Consequences

- Users install Wardian once per platform and receive both the desktop app and CLI.
- Release pages no longer contain separate CLI binaries.
- CI spends less time on duplicate CLI-only release builds.
- CLI packaging remains coupled to the Tauri bundle resource path, so changes to `stage-cli` or `src-tauri/tauri.conf.json` must preserve `resources/bin/*`.
