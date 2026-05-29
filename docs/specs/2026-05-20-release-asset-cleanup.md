# Release Asset Cleanup

- **Status:** Accepted
- **Date:** 2026-05-20
- **Decider:** Wardian Codex and user

## Context

The v0.3.6 GitHub Release exposed installer assets, updater bundles,
`latest.json`, and standalone `.sig` files in one flat asset list. This made the
human download path confusing, especially for macOS users choosing between
Apple Silicon and Intel builds.

Wardian still needs signed updater metadata for in-app updates, but Tauri's
static updater JSON embeds the signature content directly in `latest.json`.
The app does not need separate `.sig` release assets.

## Decision

The release workflow keeps generating updater signatures and `latest.json`, then
deletes loose `.sig` release assets from the draft release before metadata
validation and publication. GitHub Releases should show user-installable
packages, updater bundles referenced by `latest.json`, and `latest.json` itself,
without loose `.sig` files.

Package-manager metadata continues to use the installer assets and GitHub's
SHA-256 release asset digests. It must not depend on standalone `.sig` files.

## Consequences

- **Positive:** The release asset list is easier for users to scan.
- **Positive:** In-app updates keep signature verification through
  `latest.json`.
- **Positive:** The package-manager manifest workflow remains based on
  canonical installer assets and SHA-256 digests.
- **Negative:** Maintainers inspecting updater signatures must read
  `latest.json` instead of downloading adjacent `.sig` files from the release.
