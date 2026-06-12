# Package Manager Distribution

Wardian's package-manager metadata is generated from stable GitHub Release
assets. GitHub Releases stay canonical; package managers are discovery and
install surfaces that reuse the same desktop installers.

## Scope

Phase 1 covers:

- winget for Windows, using the NSIS `.exe`.
- Homebrew Cask for macOS, using the Apple Silicon and Intel `.dmg` files.
- A signed APT repository for Debian/Ubuntu x64, using the stable `.deb`.
- Linux direct-install fallback docs for `.deb` and AppImage artifacts.

npm bootstrap and standalone CLI-only distribution are out of scope for Phase 1.
Linux package-manager publishing is tracked in
[#324](https://github.com/wardian-app/Wardian/issues/324). The Phase 2 decision
is documented in
[Linux Package Manager Distribution](https://github.com/wardian-app/Wardian/blob/main/docs/specs/2026-06-11-linux-package-manager-distribution.md):
use a signed APT repository as the first Linux package-manager channel, defer
Flathub and Snap until Wardian has a deliberate sandbox/permission design, and
keep AppImage as a direct-download artifact.
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
that tap workflow after stable publication when the Wardian repository has the
Wardian release dispatch GitHub App configured. Set
`WARDIAN_RELEASE_DISPATCH_APP_ID` as a repository variable and
`WARDIAN_RELEASE_DISPATCH_PRIVATE_KEY` as a repository secret. The app must be
installed on `wardian-app/homebrew-tap` with Actions write permission.

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
install fallback for release notes and documentation. Debian/Ubuntu users should
prefer the signed APT repository. Do not present direct `.deb` downloads as
Flatpak, Snap, or AppImageUpdate.

## Linux APT Repository

The selected Linux package-manager channel is a signed APT repository that
mirrors stable `.deb` assets from GitHub Releases. GitHub Releases remain the
canonical artifact source; APT metadata is the Linux package-manager integrity
surface and must be generated only after the stable release assets and updater
metadata have been published and validated.

The public repository is `https://packages.wardian.org/apt`. The package host
can later add sibling package-manager paths such as `/rpm` without changing the
APT URL. The first backend is GitHub Pages behind `packages.wardian.org`.

Public Debian/Ubuntu install instructions:

```bash
curl -fsSL https://packages.wardian.org/apt/wardian-archive-keyring.gpg \
  | sudo install -D -m 0644 /dev/stdin /etc/apt/keyrings/wardian.gpg
echo "deb [arch=amd64 signed-by=/etc/apt/keyrings/wardian.gpg] https://packages.wardian.org/apt stable main" \
  | sudo tee /etc/apt/sources.list.d/wardian.list >/dev/null
sudo apt update
sudo apt install wardian
```

Archive key fingerprint:

```text
C956 3C05 D88D B483 748A 5F8B 66E5 FF51 0BCE 9193
```

Local dry runs from Windows should use WSL Ubuntu or another Linux environment
for Debian tooling. Published automation runs in the separate
`wardian-app/packages` repository after stable release publication and:

1. read `gh release view vX.Y.Z --json tagName,assets`;
2. verify the `Wardian_X.Y.Z_amd64.deb` asset name, tag URL, and SHA-256 digest;
3. verify package identity with `dpkg-deb -f`;
4. stage the `.deb` under `pool/main/w/wardian/`;
5. generate `Packages`, `Packages.gz`, and `Release`;
6. sign `Release` as both `InRelease` and `Release.gpg`;
7. validate the repository before upload.

This repository includes an **APT Repository** GitHub Actions workflow for
manual validation. It generates a signed dry-run repository with a temporary
key, validates it with local `file://` APT sources, and uploads the repository
tree as a workflow artifact. It does not publish to the public package host.

Publishing is owned by `wardian-app/packages`. That repository's
`publish-apt.yml` workflow consumes the published Wardian release assets, signs
the repository with the real archive signing key, writes `CNAME`, commits the
`apt/` tree, and publishes `https://packages.wardian.org/apt` through GitHub
Pages. Configure these in `wardian-app/packages`:

- `WARDIAN_APT_SIGNING_PRIVATE_KEY`: armored private archive signing key.
- `WARDIAN_APT_SIGNING_KEY_PASSPHRASE`: optional passphrase for the archive
  signing key.

Configure these in `wardian-app/Wardian` so stable releases can dispatch the
package repository workflow:

- `WARDIAN_RELEASE_DISPATCH_APP_ID`: release dispatch GitHub App ID.
- `WARDIAN_RELEASE_DISPATCH_PRIVATE_KEY`: release dispatch GitHub App private
  key. The app must be installed on `wardian-app/packages` with Actions write
  permission.

Do not use this repository's GitHub Pages deployment for the package repository;
the Wardian docs site already owns that deployment. Use a separate static host
or a separate repository with Pages enabled behind `https://packages.wardian.org`.

Expected static host shape:

```text
CNAME
apt/
  pool/main/w/wardian/Wardian_X.Y.Z_amd64.deb
  dists/stable/main/binary-amd64/Packages
  dists/stable/main/binary-amd64/Packages.gz
  dists/stable/Release
  dists/stable/InRelease
  dists/stable/Release.gpg
  wardian-archive-keyring.gpg
```

Package identity validation:

```bash
dpkg-deb -f Wardian_X.Y.Z_amd64.deb Package Version Architecture
```

Expected values:

- `Package`: `wardian`
- `Version`: `X.Y.Z`
- `Architecture`: `amd64`

Keep previously published `.deb` files in `pool/`; do not overwrite an already
published package version with different contents. If a package must be
republished for packaging-only reasons, publish a new Debian package version
instead of reusing the same version string.

Dry-run repository generation:

```bash
npm run release:apt-repo -- \
  --release-assets dist/package-managers/vX.Y.Z/assets.json \
  --deb Wardian_X.Y.Z_amd64.deb \
  --out dist/apt \
  --signing-key "$WARDIAN_APT_SIGNING_KEY"
```

Manual Wardian repository validation:

```bash
gh workflow run apt-repository.yml \
  -f release_tag=vX.Y.Z
```

Manual package repository publish run, after the package workflow and signing
secrets are configured:

```bash
gh workflow run publish-apt.yml \
  --repo wardian-app/packages \
  -f release_tag=vX.Y.Z
```

The underlying metadata generation commands are:

```bash
apt-ftparchive packages pool/main > dists/stable/main/binary-amd64/Packages
gzip -k -f dists/stable/main/binary-amd64/Packages
apt-ftparchive release dists/stable > dists/stable/Release
gpg --batch --yes --local-user "$WARDIAN_APT_SIGNING_KEY" --clearsign \
  -o dists/stable/InRelease dists/stable/Release
gpg --batch --yes --local-user "$WARDIAN_APT_SIGNING_KEY" -abs \
  -o dists/stable/Release.gpg dists/stable/Release
```

PowerShell should not be the primary form for repository generation because APT
metadata generation and validation depend on Debian/Ubuntu tooling.

Validation before upload:

```bash
test -s dists/stable/main/binary-amd64/Packages
test -s dists/stable/main/binary-amd64/Packages.gz
gpg --verify dists/stable/Release.gpg dists/stable/Release
gpg --verify dists/stable/InRelease
```

End-to-end install validation in a disposable Debian or Ubuntu container or VM:

```bash
sudo install -d -m 0755 /etc/apt/keyrings
curl -fsSL https://packages.wardian.org/apt/wardian-archive-keyring.gpg \
  | sudo tee /etc/apt/keyrings/wardian.gpg >/dev/null
sudo tee /etc/apt/sources.list.d/wardian.sources >/dev/null <<'EOF'
Types: deb
URIs: https://packages.wardian.org/apt
Suites: stable
Components: main
Architectures: amd64
Signed-By: /etc/apt/keyrings/wardian.gpg
EOF
sudo apt update
apt-cache policy wardian
sudo apt install wardian
```

When Wardian is installed through APT, APT owns desktop app updates. Any future
Linux in-app installer behavior must detect package-manager installs before
replacing files outside the package database.

## Deferred Linux Channels

Flathub is deferred until Wardian has a small, intentional Flatpak permission
surface for PTYs, provider CLIs, workspace files, WebKit, and external editors.
Snap is deferred until Wardian can prove strict confinement is practical or
maintainers decide that classic confinement review is worth the support burden.
AppImage remains a direct-download artifact unless Wardian later adopts a real
AppImage update mechanism.

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
