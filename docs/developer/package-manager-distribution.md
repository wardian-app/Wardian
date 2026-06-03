# Package Manager Distribution

Wardian's package-manager metadata is generated from stable GitHub Release
assets. GitHub Releases stay canonical; package managers are discovery and
install surfaces that reuse the same desktop installers.

## Scope

Phase 1 covers:

- winget for Windows, using the NSIS `.exe`.
- Homebrew Cask for macOS, using the Apple Silicon and Intel `.dmg` files.
- Linux direct-install docs for `.deb` and AppImage artifacts.

npm bootstrap, standalone CLI-only distribution, signed APT repositories,
Flathub, and Snap are out of scope for Phase 1. Linux package-manager publishing
is tracked in [#324](https://github.com/wardian-app/Wardian/issues/324).
The Phase 1 implementation is tracked in
[#325](https://github.com/wardian-app/Wardian/issues/325).

## Generate Manifests

Run this only after the stable release workflow has published the GitHub
Release and validated updater metadata.

```bash
mkdir -p dist/package-managers/v0.3.6
gh release view v0.3.6 --json tagName,assets > dist/package-managers/v0.3.6/assets.json
npm run release:package-manifests -- --release-assets dist/package-managers/v0.3.6/assets.json --out dist/package-managers/v0.3.6
```

PowerShell:

```powershell
New-Item -ItemType Directory -Force dist/package-managers/v0.3.6 | Out-Null
gh release view v0.3.6 --json tagName,assets > dist/package-managers/v0.3.6/assets.json
npm run release:package-manifests -- --release-assets dist/package-managers/v0.3.6/assets.json --out dist/package-managers/v0.3.6
```

Generated files:

- `dist/package-managers/v0.3.6/winget/WardianApp.Wardian.yaml`
- `dist/package-managers/v0.3.6/winget/WardianApp.Wardian.locale.en-US.yaml`
- `dist/package-managers/v0.3.6/winget/WardianApp.Wardian.installer.yaml`
- `dist/package-managers/v0.3.6/homebrew/wardian.rb`
- `dist/package-managers/v0.3.6/linux/install.md`

The generator fails if a required asset is missing, lacks a SHA-256 digest, or
does not point at the requested release tag.

## winget

Copy the generated winget files into a local clone of
`microsoft/winget-pkgs` under:

```text
manifests/w/WardianApp/Wardian/0.3.6/
```

Validate before submitting:

```bash
winget validate manifests/w/WardianApp/Wardian/0.3.6
```

PowerShell:

```powershell
winget validate manifests\w\WardianApp\Wardian\0.3.6
```

Submit through the normal `microsoft/winget-pkgs` PR flow or with
`wingetcreate submit` after reviewing the generated YAML.

## Homebrew

The `wardian-app/homebrew-tap` repository owns the published Homebrew Cask.
After a stable release, its **Update Wardian Cask** workflow can be run with a
release tag such as `v0.3.6`. The main Wardian release workflow also dispatches
that tap workflow after stable publication when the Wardian repository has a
`HOMEBREW_TAP_DISPATCH_TOKEN` secret with access to create dispatch events in
the tap repository.

The tap workflow reads the published Wardian release assets, rewrites
`Casks/wardian.rb`, runs Homebrew audit, and opens a pull request in the tap.
The generated `dist/package-managers/v0.3.6/homebrew/wardian.rb` file remains a
local fallback and a useful comparison artifact, but maintainers should prefer
the tap workflow for published cask updates.

Validate local cask changes on macOS:

```bash
brew audit --cask wardian
brew install --cask --verbose ./wardian.rb
```

Homebrew metadata declares `auto_updates true` because Wardian has its own
Tauri updater. Package-manager updates may lag behind the in-app updater, but
they must never point at prerelease or draft artifacts.

## Linux Direct Install

Use `dist/package-managers/v0.3.6/linux/install.md` as the hash-verified Linux
install snippet for release notes and documentation. Do not present it as an
APT, Flatpak, Snap, or AppImageUpdate channel. Those choices belong to the
Phase 2 Linux package-manager work.

## Verification

For generator changes, run:

```bash
npm run test -- src/config/packageManagerManifestGenerator.test.ts
npm run lint
```

Before submitting generated package-manager metadata for a release, run the
ecosystem validators for the generated output:

```bash
winget validate <winget-manifest-directory>
brew audit --cask wardian
```
