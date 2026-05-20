# OS Package Managers Phase 1

- **Status:** Accepted
- **Date:** 2026-05-20
- **Decider:** Wardian Codex and user
- **Issue:** [#325](https://github.com/tangemicioglu/Wardian/issues/325)
- **Follow-up:** Linux package-manager publishing is tracked in [#324](https://github.com/tangemicioglu/Wardian/issues/324).

## Context and Problem Statement

Wardian now publishes signed updater artifacts and unified desktop installers
through GitHub Releases. The next distribution step should improve install
discovery through conventional desktop package managers without creating a
separate npm or CLI-only channel.

npm is deferred. Wardian is currently desktop-first, and the npm package would
only be a bootstrapper that redirects to GitHub Releases. That can be revisited
when Wardian has a web, headless, or standalone CLI runtime that maps naturally
to `npx wardian`.

Linux package-manager publishing is also deferred. Wardian already emits `.deb`
and AppImage artifacts, but a real Linux package-manager channel requires a
choice between a signed APT repository, Flathub, Snap, or another ecosystem.
That decision has enough policy, hosting, and sandbox implications to deserve
its own follow-up.

## Decision

Phase 1 adds release-support automation and documentation for:

- **Windows:** winget manifests that reuse the release NSIS installer.
- **macOS:** a Homebrew Cask that reuses the release DMG files.
- **Linux:** direct GitHub Release installation instructions for `.deb` and
  AppImage artifacts, including SHA-256 verification.

GitHub Releases remain canonical. Package-manager metadata must be generated
from a published stable release after asset validation succeeds. Package-manager
automation must key on tag names, asset names, URLs, and hashes, never on the
GitHub release title.

## Artifact Contract

For a stable release tag `vX.Y.Z`, Phase 1 expects these release assets:

- `Wardian_X.Y.Z_x64-setup.exe`
- `Wardian_X.Y.Z_aarch64.dmg`
- `Wardian_X.Y.Z_x64.dmg`
- `Wardian_X.Y.Z_amd64.deb`
- `Wardian_X.Y.Z_amd64.AppImage`

The package-manager generator consumes the GitHub release asset JSON, including
GitHub's SHA-256 `digest` field, and fails if any required asset is missing,
missing a hash, or points at a different release tag.

## Workflow

After a stable release is published:

1. Export the GitHub Release asset JSON with `gh release view`.
2. Run `npm run release:package-manifests` with the asset JSON and an ignored
   output directory under `dist/`.
3. Validate the winget directory with `winget validate`.
4. Submit the winget manifests to `microsoft/winget-pkgs`.
5. Copy the generated Homebrew Cask into the Wardian tap or a Homebrew Cask PR
   and run Homebrew audit/install validation.
6. Use the generated Linux install markdown as the source for release notes or
   documentation until the Linux package-manager follow-up lands.

## Consequences

- **Positive:** Windows and macOS package-manager metadata is repeatable and
  derived from release assets instead of hand-copied hashes.
- **Positive:** The workflow preserves GitHub Releases and Tauri updater
  artifacts as the trust anchor.
- **Positive:** Linux users get hash-verified direct-install instructions now,
  while the package-manager choice stays explicit in #324.
- **Negative:** Phase 1 still requires maintainer action after publishing a
  release. Fully automated external repository PRs are deferred.
- **Negative:** Homebrew distribution still needs a tap or upstream Cask
  submission target outside this repository.
