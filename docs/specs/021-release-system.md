# Spec 021: Release System

- **Status:** Proposed
- **Date:** 2026-04-19
- **Decider:** Wardian Claude

## Context and Problem Statement

Wardian has no formal release pipeline. Installable artifacts are not produced, versions exist only as numbers in `package.json`, `src-tauri/tauri.conf.json`, and `src-tauri/Cargo.toml` that drift independently, and there is no changelog. Users cannot download a build without cloning the repo and running `npm run tauri build` themselves.

This spec defines a Tier B release system: reproducible, multi-OS binaries published to GitHub Releases with a curated changelog and synchronized version numbers, automated via release-please and `tauri-apps/tauri-action`. Code signing, auto-updates, and OS package-manager submissions are deferred to follow-up specs; the pipeline is designed so they can be added without re-architecting.

The system must also be forward-compatible with a planned CLI companion (`wardian-cli`), since agents — Wardian's primary users — will install it via `npm`, `pip`, or `cargo` wrappers that download prebuilt binaries from the same GitHub Releases.

## Proposed Decision

### Scope

**In scope:**

- Synchronized version bumps across `package.json`, `src-tauri/tauri.conf.json`, `src-tauri/Cargo.toml`.
- Automated CHANGELOG.md generation from Conventional Commits, via release-please.
- GitHub Actions workflow that builds unsigned multi-OS bundles on tag push and publishes them to a GitHub Release.
- Stable and Preview release channels, both manually cut.
- Pipeline-level awareness of future CLI binaries (stubbed but disabled).

**Out of scope (deferred to follow-up specs):**

- Windows EV code-signing and macOS Developer ID + notarization.
- Tauri auto-updater (`tauri-plugin-updater`).
- OS package managers: winget, Scoop, AUR, Homebrew Cask, Flathub.
- The `wardian-cli` crate, its first-run installer, and npm/pip/cargo distribution wrappers.

### Component 1 — Version & Changelog Automation (release-please)

**Tooling:** `googleapis/release-please-action@v4` runs on every push to `main`.

**Files introduced at repo root:**

- `release-please-config.json` — configures Wardian as a single-package manifest release, enables CHANGELOG generation with Keep-a-Changelog section taxonomy (Features, Bug Fixes, Performance, Documentation, Miscellaneous), and uses `extra-files` to keep `src-tauri/tauri.conf.json` and `src-tauri/Cargo.toml` in sync with `package.json`.
- `.release-please-manifest.json` — tracks the current version (`0.2.1`).
- `CHANGELOG.md` — Keep-a-Changelog format, seeded with the `[0.2.1]` entry so release-please has a known anchor.

**Flow:**

1. Every push to `main` updates a single long-lived "chore(main): release X.Y.Z" PR. The PR accumulates CHANGELOG entries and bumps version numbers in all three files.
2. Maintainer merges the Release PR when a release is desired. The action creates a git tag `v{version}` and a draft GitHub Release pointing at that tag.
3. Tag push triggers the build workflow (Component 2).

**Semver mapping** (enforced at commit time by convention, not tooling):

- `feat:` → minor.
- `fix:` / `perf:` → patch.
- `feat!:` / `BREAKING CHANGE:` footer → major (suppressed to minor while `<1.0.0` via release-please's `bump-minor-pre-major` option).
- `chore:` / `docs:` / `refactor:` / `test:` / `ci:` → no release, appears under Miscellaneous in CHANGELOG when configured.

**Preview channel:** label the Release PR `autorelease: pre-release` (or manually edit the version to `X.Y.Z-preview.N`). The resulting tag has a prerelease suffix; the build workflow detects the `-preview.` / `-beta.` / `-rc.` substring and flips the GitHub Release to pre-release.

**Manual override:** maintainer can edit the Release PR body, version numbers, or CHANGELOG entries before merging. release-please re-derives from commits but respects manual overrides if committed onto the release branch.

### Component 2 — Build & Publish Workflow

**File:** `.github/workflows/release.yml` (new, independent of `ci.yml`).

**Triggers:**

- `push` of tags matching `v*`.
- `workflow_dispatch` for dry-run validation from a feature branch (produces artifacts, does not create a release).

**Jobs:**

1. `create-release` — runs once on a Linux runner. Creates a draft GitHub Release from the tag. Extracts the CHANGELOG entry for this version and sets it as the release body. Detects prerelease suffix and sets the `prerelease` flag accordingly.
2. `build` — matrix over four runners, all running `tauri-apps/tauri-action@v0` with `releaseId` pointing at the draft from step 1:
   - `windows-latest`: produces NSIS `.exe`.
   - `macos-latest` (arm64): produces `.dmg` for Apple Silicon.
   - `macos-13` (x86_64): produces `.dmg` for Intel Macs.
   - `ubuntu-22.04`: produces `.AppImage` and `.deb`. Pinned to 22.04 (not `ubuntu-latest`) for glibc compatibility on older Linux distros.
3. `publish` — depends on all `build` matrix jobs succeeding. Flips the draft release to published.

**Failure behavior:** if any matrix entry fails, the release stays as a draft. Maintainer fixes the issue and either re-runs the workflow against the same tag or pushes a new patch tag.

**Artifacts produced per release (5):**

- `Wardian_{version}_x64-setup.exe` (Windows NSIS)
- `Wardian_{version}_aarch64.dmg` (macOS Apple Silicon)
- `Wardian_{version}_x64.dmg` (macOS Intel)
- `Wardian_{version}_amd64.AppImage` (Linux portable)
- `wardian_{version}_amd64.deb` (Debian/Ubuntu)

Exact filenames follow Tauri's default bundle naming.

### Component 3 — CLI-Aware Pipeline (stub)

The `release.yml` matrix contains a commented-out job for building `wardian-cli` per-platform binaries and uploading them as release assets with a predictable naming scheme (`wardian-cli-{arch}-{os}[.exe]`). Disabled until the CLI crate lands.

The Tauri bundle configuration (`src-tauri/tauri.conf.json` → `bundle.resources`) is documented in the spec as the intended mechanism for shipping `wardian-cli` inside the desktop app bundle at `resources/bin/wardian-cli[.exe]`. Actual inclusion happens when the CLI crate exists; this spec only records the requirement so the pipeline does not need to be redesigned later.

### File Changes Introduced

**New:**

- `.github/workflows/release.yml`
- `release-please-config.json`
- `.release-please-manifest.json`
- `CHANGELOG.md`

**Modified:**

- `README.md` — add a "Download" section linking to the GitHub Releases page.
- `src-tauri/tauri.conf.json` — verify `bundle.targets` is set to produce NSIS (not MSI) on Windows and both `.deb` and `.appimage` on Linux.

### Testing & Rollout Plan

1. **Dry-run build** via `workflow_dispatch` on `feat/release-system` branch. Download each artifact, install on each host OS, confirm the app launches and basic UI renders.
2. **Dry-run release-please** by merging synthetic `feat:` and `fix:` commits and observing the Release PR output. Verify all three version files bump together and CHANGELOG renders as expected.
3. **First real release**: cut `v0.2.2` from the next trivial patch to validate the tag-triggered build + publish path with low stakes.
4. **First preview release**: cut `v0.3.0-preview.1` shortly after to validate the prerelease marking path.

**Rollout order on the implementation branch:**

1. Add `CHANGELOG.md` stub entry for `0.2.1`.
2. Add release-please config and workflow. Merge → confirm Release PR opens.
3. Add `release.yml`. Validate via `workflow_dispatch`.
4. Update `README.md` with a Download section.
5. Merge the Release PR to cut the first real release.

**Rollback:** all release-please state lives in the manifest file and CHANGELOG. No external service state to clean up. If the pipeline misbehaves, revert the workflow commit and delete any bad tags.

## Consequences

- **Positive:** users can download installable Wardian builds from GitHub Releases on all three major OSes without cloning the repo.
- **Positive:** versions cannot drift across `package.json`, `tauri.conf.json`, and `Cargo.toml` — release-please syncs all three.
- **Positive:** CHANGELOG stays curated with minimal maintainer effort; commit discipline (Conventional Commits) is the only ongoing cost.
- **Positive:** pipeline is sized to accept code signing, auto-updates, OS package managers, and the CLI without restructuring.
- **Negative:** Windows users see a SmartScreen warning on first install; macOS users must right-click → Open or `xattr -cr` the app. Acceptable for a 0.x developer-audience project, unacceptable long-term.
- **Negative:** Conventional Commits become load-bearing. Sloppy commit messages directly degrade the CHANGELOG. Mitigated by the Release PR being editable before merge.
- **Negative:** release cadence is coupled to maintainer attention — if the Release PR is never merged, no releases ship. Acceptable given the project's size and single-maintainer status.
