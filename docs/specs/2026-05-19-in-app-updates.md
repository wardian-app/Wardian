# In-App Updates

- **Status:** Proposed
- **Date:** 2026-05-19
- **Decider:** Wardian Codex and user

## Context and Problem Statement

Wardian publishes installable desktop bundles through GitHub Releases, but users must currently discover and download newer installers manually. Settings also does not expose the currently running Wardian version, so users cannot quickly confirm what build they are on.

Wardian needs an in-app update system that can check GitHub Releases, verify update artifacts, download the correct platform bundle, and install it without asking users to manually fetch the setup installer from GitHub. The first version should be explicit and user-controlled: Wardian may check silently, but it must not silently install, restart, or exit the app before the user starts installation.

This extends the existing release system. The first updater-capable release is a bridge release: users on older builds must install that release manually once, then future releases can be installed from inside Wardian.

## Proposed Decision

Use Tauri v2's official updater plugin and GitHub Releases static updater metadata.

### Product Behavior

Settings shows the running app version at the top, using Tauri's app metadata API. The Updates section checks silently on mount, reports whether a newer version is available, and lets the user explicitly start `Download & Install`. If installation completes and the platform supports relaunch, Wardian relaunches immediately so the updated app comes back up. Wardian never installs or restarts before the user starts installation.

Windows installation uses a backend handoff instead of the frontend calling the updater plugin's install method directly. The backend still uses Tauri updater discovery, download, and signature verification, but after downloading the verified installer it writes the installer to a temporary updater path, starts a detached helper, and exits. The helper waits for the Wardian process ID to terminate, pauses briefly, then launches the NSIS installer with `/P /R /UPDATE /ARGS`. This keeps the installer from trying to replace `Wardian.exe` while the app process is still shutting down. The handoff verifies that the backend-found update version still matches the version shown in Settings before downloading. Because Tauri's verified Rust updater API returns installer bytes, Wardian also enforces a 512 MiB payload ceiling before writing or launching the installer.

Update checks are enabled only for official stable release artifacts. Dev builds, browser/Vite runs, local source-built release binaries, dry-run artifacts, and prerelease builds still show the current version, but they do not call GitHub updater APIs or expose install controls.

The first UI surface stays inside Settings. A global badge or titlebar indicator can be added later without changing the updater core.

### Update Infrastructure

Wardian configures:

- `tauri-plugin-updater` in the Rust app.
- `tauri-plugin-process` in the Rust app for controlled relaunch.
- `@tauri-apps/plugin-updater` in the frontend.
- `@tauri-apps/plugin-process` for relaunch.
- `bundle.createUpdaterArtifacts: true` in the release-only Tauri config overlay (`src-tauri/tauri.updater.conf.json`).
- A stable endpoint at `https://github.com/wardian-app/Wardian/releases/latest/download/latest.json`.
- Tauri updater permissions in `src-tauri/capabilities/default.json`.
- Tauri process restart permission in `src-tauri/capabilities/default.json`.
- Windows updater install mode `passive`.

The release workflow signs update artifacts with `TAURI_SIGNING_PRIVATE_KEY` and optional `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` secrets, embeds updater signatures in `latest.json`, deletes loose `.sig` release assets before publication, and publishes `latest.json` only as part of successful release builds. The updater consumes inline signatures from `latest.json`, not separate signature URLs. The public key is committed in `tauri.conf.json`; the private key never appears in the repository or documentation beyond secret-name references.

The app exposes update eligibility through a backend command rather than letting Settings infer it from frontend runtime details. Official stable tag-push and stable manual backfill release builds set `WARDIAN_UPDATE_CHANNEL=stable` at compile time. Prerelease, debug, and unmarked release builds return disabled eligibility, and the backend skips updater/process plugin registration for those builds. This keeps local builds and prereleases from replacing themselves with public stable installer releases and keeps updater IPC unavailable outside official stable builds.

Windows stable builds also compare the running executable directory against the install location recorded by the Wardian installer registry keys. If those paths diverge, Settings must disable in-app updates and direct the user to the manual installer so the registered install path, shortcuts, and updater target are realigned before future in-app updates run.

The compile-time marker is advisory release metadata, not cryptographic provenance. Wardian relies on protected release workflow permissions, updater signature verification, and signing-key custody for trust. If Wardian later needs to prove that a running binary was installed through an official installer, add an installer-written provenance marker or signed release attestation instead of treating `WARDIAN_UPDATE_CHANNEL` as proof.

Because Wardian creates the GitHub release before invoking `tauri-apps/tauri-action`, the action must receive both `releaseId` and the tag name. Tauri Action documents that using `releaseId` without `tagName` can make `latest.json` point at `releases/latest/download/<bundle>` in ways that fail when the latest release does not contain updater bundles.

The release workflow must verify updater metadata after all matrix builds finish. `latest.json` is not considered valid unless it contains complete entries for every supported stable platform in the release matrix, reports the expected release version, and points platform URLs at canonical GitHub release download URLs for the expected tag with filenames that match uploaded release assets. The validator must account for GitHub draft releases exposing asset URLs under an `untagged-...` placeholder before publish, while updater metadata must already use the final tag URL. Manual backfill runs must pass the same metadata gate before any publish step. If Wardian keeps manual backfill as draft-only, the workflow should fail early for published releases instead of silently mutating or publishing an unintended release. In all cases, manual backfill must build and upload against the explicit `release_tag`; it must not accidentally rewrite updater metadata for an unrelated latest release.

### Channels and Rollback

The initial implementation supports the stable channel only. Stable builds consume GitHub's latest non-prerelease release. Preview, beta, nightly, staged rollout, rollback, and dynamic update-server behavior are deferred.

If Wardian later adds channels, the updater logic should move endpoint/channel decisions behind a small configuration boundary rather than embedding channel assumptions in Settings UI.

Rollback through static GitHub `latest.json` is intentionally limited. Emergency rollback, staged rollout, and channel-specific eligibility should move to a dynamic update endpoint or explicit channel manifest rather than overloading the stable GitHub latest release.

### Security and Future-Proofing

Tauri update signature verification is required and cannot be disabled. Wardian treats the updater private key as release infrastructure, not application state. Losing the key means installed updater-enabled clients cannot verify future updates and need a manual reinstall path. Compromise of the key requires revocation, a new key, and a manual recovery release.

The first implementation should document key ownership and recovery in developer docs, but should not invent custom cryptography, custom download logic, or custom installer behavior.

The frontend updater calls should live in a focused helper or hook, such as `src/features/settings/useAppUpdate.ts`, so Settings renders state while the helper owns update check, download progress, install, relaunch, and error mapping.

Updater integration tests should not require publishing a production release. A local or draft-only signed updater endpoint can serve an older installed test build a newer signed `latest.json` and installer artifact. That test should verify the complete handoff behavior: download progress reaches the UI, Wardian exits, the helper starts the installer only after the original process exits, and the subsequent launch reports the newer version.

### File Changes Expected

- `package.json` / `package-lock.json`: add updater and process plugin JavaScript packages.
- `src-tauri/Cargo.toml` / `src-tauri/Cargo.lock`: add `tauri-plugin-updater` and `tauri-plugin-process`.
- `src-tauri/src/lib.rs`: initialize the updater and process plugins.
- `src-tauri/tauri.conf.json`: configure updater artifacts, public key, endpoint, and Windows install mode.
- `src-tauri/capabilities/default.json`: grant updater permissions and `process:allow-restart` or `process:default`.
- `.github/workflows/release.yml`: pass signing secrets, tag name, and updater JSON settings to `tauri-action`.
- `src/config/releaseWorkflow.test.ts`: extend the release workflow contract tests for updater signing, tag-aware updater JSON generation, manual backfill behavior, and metadata validation.
- `src/features/settings/useAppUpdate.ts`: add focused updater state and actions.
- `src/features/settings/useAppUpdate.test.ts`: test updater state transitions with mocked Tauri plugin APIs.
- `src/features/settings/SettingsPanel.tsx`: render version and update controls at the top of Settings.
- `src/features/settings/SettingsPanel.test.tsx`: test version rendering and Settings integration.
- `docs/developer/release-updates.md` or an existing release developer guide: document signing secret setup, bridge release behavior, and recovery constraints.
- `docs/guide/settings.md`: document the Settings Updates section.

## Consequences

- **Positive**: Users can update Wardian from inside Settings after installing one updater-capable bridge release.
- **Positive**: Update integrity is handled by Tauri's signed updater instead of custom download logic.
- **Positive**: The Settings version display makes support and troubleshooting easier.
- **Positive**: A focused updater helper leaves room for future channels and dynamic update servers.
- **Negative**: Current users still need one manual update before in-app updates are available.
- **Negative**: Release signing key custody becomes operationally important.
- **Negative**: Static GitHub `latest.json` does not support staged rollout, rollback, or separate preview channels without additional release/channel design.
