# Release Updates

Wardian uses Tauri's signed updater for in-app desktop updates. The Settings UI checks for stable releases and lets the user explicitly download and install an update. After the user starts installation and the installer completes, Wardian relaunches so the updated app comes back up without a separate manual launch.

On Windows, Wardian uses the Tauri updater for update discovery, download, and signature verification, then hands installation to a detached helper process. The helper is launched outside Wardian's kill-on-close process supervisor job, waits for the running `Wardian.exe` process to exit, and then starts the NSIS installer with the updater flags. This avoids racing the installer against a still-running executable while preserving Tauri's signed update verification. The Tauri Rust updater API returns the verified installer as bytes rather than streaming directly to a file, so Wardian rejects installer payloads larger than 512 MiB before writing or launching them.

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
- `WARDIAN_RELEASE_DISPATCH_APP_ID`: optional GitHub App ID for dispatching package repository workflows after stable release publication.
- `WARDIAN_RELEASE_DISPATCH_PRIVATE_KEY`: optional GitHub App private key for the release dispatch app. The app must be installed on `wardian-app/homebrew-tap` and `wardian-app/packages` with Actions write permission.

For this repository's GitHub Actions workflow, store the private key file content in `TAURI_SIGNING_PRIVATE_KEY`. Do not store the public `.pub` file, the committed `plugins.updater.pubkey`, or a local filesystem path. The generated private key file is base64 text; when decoded it starts with `untrusted comment:` and contains `secret key`.

If GitHub Actions reports `Missing comment in secret key`, replace `TAURI_SIGNING_PRIVATE_KEY` with the contents of the generated private key file and confirm `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` matches the password used when generating it. If the key was generated with a Tauri CLI version between `2.9.3` and `2.10.0` and no password, regenerate it with the current repository CLI before using it for the first updater-capable release.

If the private key or password is lost, existing updater-enabled installs cannot verify future update packages. Recovery requires a manual installer release with a new public key. If the key is compromised, revoke use of the old key, generate a new pair, publish a manual recovery installer, and document the incident in release notes.

## macOS Signing and Notarization

Stable and prerelease macOS release builds use a Developer ID Application certificate, Apple notarization, and stapling. The release workflow imports the certificate into an ephemeral runner keychain, signs the app and nested code, submits the macOS artifacts to Apple, and validates the stapled DMG and updater archive before the draft release can publish.

Configure these GitHub Actions secrets for macOS release jobs:

- `APPLE_CERTIFICATE`: base64-encoded, password-protected Developer ID Application `.p12` export.
- `APPLE_CERTIFICATE_PASSWORD`: password used for that `.p12` export.
- `APPLE_ID`: Apple ID email that belongs to the Developer Program team.
- `APPLE_PASSWORD`: app-specific password for that Apple ID, created only for Wardian CI notarization.
- `APPLE_TEAM_ID`: Developer Program team ID.

The certificate private key, its export password, the app-specific password, and the Tauri updater private key are distinct secrets. Back up each outside the repository. Do not commit them, send them in chat, or substitute the normal Apple ID password for `APPLE_PASSWORD`.

The workflow fails macOS builds before artifact publication when any required secret is missing. It verifies the signed app inside each DMG with `codesign`, validates stapled tickets with `xcrun stapler`, assesses the DMG and app with Gatekeeper, and checks the signed app inside each Tauri updater `.app.tar.gz` archive.

## Stable Channel

The first implementation uses only the stable channel:

```text
https://github.com/wardian-app/Wardian/releases/latest/download/latest.json
```

Stable builds must not consume prerelease metadata. Hyphenated version tags such as preview, alpha, beta, rc, or nightly are treated as prereleases and must not publish stable updater metadata. Preview, beta, nightly, staged rollout, or rollback behavior should use a separate channel manifest or a dynamic update endpoint rather than overloading GitHub's stable latest release.

Official stable release builds opt into updater checks with the compile-time marker `WARDIAN_UPDATE_CHANNEL=stable`. Dev builds, local source-built release binaries, dry-run artifacts, and prerelease builds keep the Settings version display but disable update checks and installation controls. In those ineligible builds, Wardian does not register the updater or process restart plugins, so renderer code cannot invoke updater IPC directly. This prevents a locally built app or prerelease from accidentally replacing itself with the public stable installer release.

On Windows, official stable builds also verify that the running `Wardian.exe` lives in the install directory recorded by the Wardian installer registry keys. If the registered install path is missing or points somewhere else, Settings disables in-app updates and asks the user to run the latest installer manually. This prevents the NSIS updater from silently updating a different Wardian copy while the user's shortcut continues launching an older executable.

`WARDIAN_UPDATE_CHANNEL=stable` is a release-build marker, not cryptographic proof that the binary came from the installer. The trust boundary is the protected release workflow, signing key custody, and the published installer/update signatures. Do not distribute locally compiled binaries with this marker.

## Release Workflow

The release workflow builds platform installers, signs update artifacts, uploads `latest.json`, removes loose `.sig` release assets, validates updater metadata, and only then publishes the release. Tauri embeds the signature content that the app needs inside `latest.json`.

Local `npm run tauri build` creates an installable bundle without updater artifacts, so it does not require `TAURI_SIGNING_PRIVATE_KEY`. Release builds opt into updater artifact generation by passing `--config src-tauri/tauri.updater.conf.json` to Tauri. That overlay sets `bundle.createUpdaterArtifacts` to `true` only inside release infrastructure.

Stable tag-push and stable manual backfill builds set `WARDIAN_UPDATE_CHANNEL=stable` before invoking `tauri-apps/tauri-action`. Prerelease builds intentionally omit the marker and do not upload stable updater metadata. Do not set that marker for ordinary local builds unless you are deliberately producing an official stable release artifact.

After a stable release is published, generate winget and Linux direct-install
metadata from the published release assets. Stable releases also dispatch the
`wardian-app/homebrew-tap` cask update workflow and the `wardian-app/packages`
APT publish workflow when the Wardian release dispatch GitHub App is configured.
See
[Package Manager Distribution](./package-manager-distribution.md) for the
post-release package-manager workflow. Package-manager metadata must consume the
published release asset URLs and SHA-256 digests; it must not depend on release
titles.

The Wardian repository's **APT Repository** workflow is validation-only. It
builds a signed dry-run repository with a temporary key and uploads the generated
repository as an artifact. Published APT repository updates are owned by the
separate `wardian-app/packages` workflow, which stores the real archive signing
key, writes `CNAME`, commits the `apt/` tree, and publishes through GitHub Pages.

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

## Local Update Testing

Do not test updater changes against the public stable latest release unless you intend to exercise the real production channel. Use a local or draft-only updater endpoint that serves a signed `latest.json` and installer artifacts built from a lower test version to a higher test version.

The useful no-public-release test is:

1. Build and install a signed test artifact with an older version and `WARDIAN_UPDATE_CHANNEL=stable`.
2. Serve a signed `latest.json` that points to a newer local or draft artifact.
3. Launch the older installed app and use **Settings > Install update**.
4. Confirm the app exits, the installer runs after the process exits, and the next launch reports the newer version.

Keep test updater artifacts separate from production releases, and use a disposable `WARDIAN_HOME` when testing app state that should not affect your normal Wardian installation.

On Windows, local updater tests should also use a disposable installer identity. Build the test artifacts with a distinct Tauri `productName`, `mainBinaryName`, and bundle publisher, then set matching compile-time registry overrides such as:

```bash
WARDIAN_UPDATE_REGISTRY_PUBLISHER="wardian-test"
WARDIAN_UPDATE_REGISTRY_PRODUCT_NAME="Wardian Updater Test"
```

PowerShell:

```powershell
$env:WARDIAN_UPDATE_REGISTRY_PUBLISHER = "wardian-test"
$env:WARDIAN_UPDATE_REGISTRY_PRODUCT_NAME = "Wardian Updater Test"
```

The test installer identity, registry overrides, and install directory must all be disposable so the local smoke does not read or modify the production Wardian install record.
