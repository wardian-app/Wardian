# Release Updates

Wardian uses Tauri's signed updater for in-app desktop updates. The Settings UI checks for stable releases and lets the user explicitly download, install, and restart. Wardian does not silently install updates or restart the app.

## Bridge Release

The first updater-capable release is a bridge release. Users on older builds must install that version manually from GitHub Releases once. After that, Settings can fetch and install newer signed releases.

## Signing Keys

Tauri update signatures are required for release updater artifacts. The public key is committed in `src-tauri/tauri.conf.json`; the private key is release infrastructure and must never be committed.

Generate the key pair outside the repository:

```bash
npm run tauri signer generate -- --ci --write-keys <secure-private-key-path>
```

PowerShell:

```powershell
npm run tauri signer generate -- --ci --write-keys <secure-private-key-path>
```

Configure GitHub Actions secrets:

- `TAURI_SIGNING_PRIVATE_KEY`: the private key content, or use `TAURI_SIGNING_PRIVATE_KEY_PATH` in a controlled runner environment.
- `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`: optional, only when the private key was generated with a password.

For this repository's GitHub Actions workflow, store the private key file content in `TAURI_SIGNING_PRIVATE_KEY`. Do not store the public `.pub` file, the committed `plugins.updater.pubkey`, or a local filesystem path. The generated private key file is base64 text; when decoded it starts with `untrusted comment:` and contains `secret key`.

If GitHub Actions reports `Missing comment in secret key`, replace `TAURI_SIGNING_PRIVATE_KEY` with the contents of the generated private key file and confirm `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` matches the password used when generating it. If the key was generated with a Tauri CLI version between `2.9.3` and `2.10.0` and no password, regenerate it with the current repository CLI before using it for the first updater-capable release.

If the private key or password is lost, existing updater-enabled installs cannot verify future update packages. Recovery requires a manual installer release with a new public key. If the key is compromised, revoke use of the old key, generate a new pair, publish a manual recovery installer, and document the incident in release notes.

## Stable Channel

The first implementation uses only the stable channel:

```text
https://github.com/wardian-app/Wardian/releases/latest/download/latest.json
```

Stable builds must not consume prerelease metadata. Hyphenated version tags such as preview, alpha, beta, rc, or nightly are treated as prereleases and must not publish stable updater metadata. Preview, beta, nightly, staged rollout, or rollback behavior should use a separate channel manifest or a dynamic update endpoint rather than overloading GitHub's stable latest release.

Official stable release builds opt into updater checks with the compile-time marker `WARDIAN_UPDATE_CHANNEL=stable`. Dev builds, local source-built release binaries, dry-run artifacts, and prerelease builds keep the Settings version display but disable update checks and installation controls. In those ineligible builds, Wardian does not register the updater or process restart plugins, so renderer code cannot invoke updater IPC directly. This prevents a locally built app or prerelease from accidentally replacing itself with the public stable installer release.

`WARDIAN_UPDATE_CHANNEL=stable` is a release-build marker, not cryptographic proof that the binary came from the installer. The trust boundary is the protected release workflow, signing key custody, and the published installer/update signatures. Do not distribute locally compiled binaries with this marker.

## Release Workflow

The release workflow builds platform installers, signs update artifacts, uploads `latest.json`, validates updater metadata, and only then publishes the release. Standalone `.sig` assets are not uploaded to GitHub Releases; Tauri embeds the signature content that the app needs inside `latest.json`.

Local `npm run tauri build` creates an installable bundle without updater artifacts, so it does not require `TAURI_SIGNING_PRIVATE_KEY`. Release builds opt into updater artifact generation by passing `--config src-tauri/tauri.updater.conf.json` to Tauri. That overlay sets `bundle.createUpdaterArtifacts` to `true` only inside release infrastructure.

Stable tag-push and stable manual backfill builds set `WARDIAN_UPDATE_CHANNEL=stable` before invoking `tauri-apps/tauri-action`. Prerelease builds intentionally omit the marker and do not upload stable updater metadata. Do not set that marker for ordinary local builds unless you are deliberately producing an official stable release artifact.

After a stable release is published, generate winget, Homebrew, and Linux
direct-install metadata from the published release assets. See
[Package Manager Distribution](./package-manager-distribution.md) for the
post-release package-manager workflow. Package-manager metadata must consume the
published release asset URLs and SHA-256 digests; it must not depend on release
titles.

`latest.json` must contain all stable platform keys:

- `windows-x86_64`
- `linux-x86_64`
- `darwin-aarch64`
- `darwin-x86_64`

Each platform entry must include a URL and inline signature. The metadata version must match the release tag without the leading `v`, and each platform URL must be a canonical GitHub release download URL for that tag whose filename matches an uploaded release asset. This filename-based validation is intentional: GitHub draft releases expose asset URLs under an `untagged-...` placeholder until publish time, while updater metadata must already point at the final tag URL. A missing, incomplete, wrong-version, or wrong-release `latest.json` should leave the release as a draft.

Manual backfill runs must target an explicit `release_tag`. Backfill is draft-only unless the workflow is deliberately changed to support published-release mutation. If the target release is already published, the workflow should fail instead of rewriting updater metadata.

Release dry-run builds still require signing secrets because they use the updater config overlay. Dry runs can prove signed installer generation, but they do not publish `latest.json`; validate release-scoped metadata against a real draft or backfill release before claiming updater release readiness.

## Verification

For update-related changes, run:

```bash
npm run lint
npm run test
npm run build
cd src-tauri && cargo clippy && cargo test && cargo check
```

PowerShell:

```powershell
npm run lint
npm run test
npm run build
Push-Location src-tauri
cargo clippy
cargo test
cargo check
Pop-Location
```

Before opening a PR, run a secrets check:

```bash
git status --short
rg -n "TAURI_SIGNING_PRIVATE_KEY|BEGIN|PRIVATE KEY|\\.env|password" .
```

Only documentation references to secret names should appear. Private key material, generated key files, and `.env` files must not be staged.
