#!/usr/bin/env node

import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { join } from "node:path";

const packageIdentifier = "Tangemicioglu.Wardian";
const repositoryUrl = "https://github.com/tangemicioglu/Wardian";
const publisher = "Tangemicioglu";
const description = "Local command center for multi-agent CLI workflows.";

function parseArgs(argv) {
  const args = new Map();
  for (let index = 0; index < argv.length; index += 2) {
    const key = argv[index];
    const value = argv[index + 1];
    if (!key?.startsWith("--") || !value || value.startsWith("--")) {
      throw new Error(`Expected --key value argument pair near ${key ?? "<end>"}`);
    }
    args.set(key, value);
  }

  const releaseAssets = args.get("--release-assets");
  const out = args.get("--out") ?? "dist/package-managers";
  if (!releaseAssets) {
    throw new Error("--release-assets is required");
  }

  return { releaseAssets, out };
}

function normalizeSha256(asset) {
  const raw = asset.digest ?? asset.sha256 ?? asset.InstallerSha256;
  if (typeof raw !== "string") {
    throw new Error(`Release asset ${asset.name ?? "<unknown>"} is missing a sha256 digest`);
  }

  const digest = raw.replace(/^sha256:/i, "").trim();
  if (!/^[a-f0-9]{64}$/i.test(digest)) {
    throw new Error(`Release asset ${asset.name ?? "<unknown>"} has invalid sha256 digest ${raw}`);
  }

  return digest.toLowerCase();
}

function normalizeUrl(asset) {
  const url = asset.browser_download_url ?? asset.browserDownloadUrl ?? asset.url;
  if (typeof url !== "string" || !url.startsWith("https://")) {
    throw new Error(`Release asset ${asset.name ?? "<unknown>"} is missing an HTTPS download URL`);
  }

  return url;
}

function loadReleaseManifest(path) {
  const raw = JSON.parse(readFileSync(path, "utf8"));
  const tag = raw.tagName ?? raw.tag_name ?? raw.tag;
  if (typeof tag !== "string" || !/^v\d+\.\d+\.\d+/.test(tag)) {
    throw new Error("Release manifest must include tagName, tag_name, or tag like v0.3.6");
  }

  const assets = Array.isArray(raw.assets) ? raw.assets : [];
  const version = tag.replace(/^v/, "");
  return {
    tag,
    version,
    assets: assets.map((asset) => ({
      name: asset.name,
      url: normalizeUrl(asset),
      sha256: normalizeSha256(asset),
    })),
  };
}

function findAsset(release, kind, expectedName) {
  const asset = release.assets.find((candidate) => candidate.name === expectedName);
  if (!asset) {
    throw new Error(`Missing required release asset for ${kind}: ${expectedName}`);
  }

  if (!asset.url.includes(`/releases/download/${release.tag}/`)) {
    throw new Error(`${expectedName} URL must target ${release.tag}: ${asset.url}`);
  }

  return asset;
}

function resolvePhaseOneAssets(release) {
  const version = release.version;
  return {
    windowsX64: findAsset(release, "winget x64 installer", `Wardian_${version}_x64-setup.exe`),
    macArm64: findAsset(release, "Homebrew Apple Silicon DMG", `Wardian_${version}_aarch64.dmg`),
    macX64: findAsset(release, "Homebrew Intel DMG", `Wardian_${version}_x64.dmg`),
    linuxDeb: findAsset(release, "Linux Debian package", `Wardian_${version}_amd64.deb`),
    linuxAppImage: findAsset(release, "Linux AppImage", `Wardian_${version}_amd64.AppImage`),
  };
}

function writeWingetManifests(outDir, release, assets) {
  const dir = join(outDir, "winget");
  mkdirSync(dir, { recursive: true });

  writeFileSync(
    join(dir, `${packageIdentifier}.yaml`),
    [
      "# yaml-language-server: $schema=https://aka.ms/winget-manifest.version.1.10.0.schema.json",
      `PackageIdentifier: ${packageIdentifier}`,
      `PackageVersion: ${release.version}`,
      "DefaultLocale: en-US",
      "ManifestType: version",
      "ManifestVersion: 1.10.0",
      "",
    ].join("\n"),
    "utf8",
  );

  writeFileSync(
    join(dir, `${packageIdentifier}.locale.en-US.yaml`),
    [
      "# yaml-language-server: $schema=https://aka.ms/winget-manifest.defaultLocale.1.10.0.schema.json",
      `PackageIdentifier: ${packageIdentifier}`,
      `PackageVersion: ${release.version}`,
      "PackageLocale: en-US",
      `Publisher: ${publisher}`,
      "PublisherUrl: https://github.com/tangemicioglu",
      "PublisherSupportUrl: https://github.com/tangemicioglu/Wardian/issues",
      "Author: Tangemicioglu",
      "PackageName: Wardian",
      `PackageUrl: ${repositoryUrl}`,
      "License: MIT",
      `Description: ${description}`,
      "ShortDescription: Local command center for multi-agent CLI workflows.",
      "Tags:",
      "- ai",
      "- agents",
      "- cli",
      "- tauri",
      "ManifestType: defaultLocale",
      "ManifestVersion: 1.10.0",
      "",
    ].join("\n"),
    "utf8",
  );

  writeFileSync(
    join(dir, `${packageIdentifier}.installer.yaml`),
    [
      "# yaml-language-server: $schema=https://aka.ms/winget-manifest.installer.1.10.0.schema.json",
      `PackageIdentifier: ${packageIdentifier}`,
      `PackageVersion: ${release.version}`,
      "InstallerLocale: en-US",
      "InstallerType: nullsoft",
      "InstallModes:",
      "- interactive",
      "- silent",
      "InstallerSwitches:",
      "  Silent: /S",
      "  SilentWithProgress: /S",
      "UpgradeBehavior: install",
      "Installers:",
      "- Architecture: x64",
      `  InstallerUrl: ${assets.windowsX64.url}`,
      `  InstallerSha256: ${assets.windowsX64.sha256}`,
      "ManifestType: installer",
      "ManifestVersion: 1.10.0",
      "",
    ].join("\n"),
    "utf8",
  );
}

function writeHomebrewCask(outDir, release, assets) {
  const dir = join(outDir, "homebrew");
  mkdirSync(dir, { recursive: true });

  writeFileSync(
    join(dir, "wardian.rb"),
    [
      'cask "wardian" do',
      `  version "${release.version}"`,
      "",
      "  on_arm do",
      `    url "${assets.macArm64.url}"`,
      `    sha256 "${assets.macArm64.sha256}"`,
      "  end",
      "",
      "  on_intel do",
      `    url "${assets.macX64.url}"`,
      `    sha256 "${assets.macX64.sha256}"`,
      "  end",
      "",
      '  name "Wardian"',
      `  desc "${description}"`,
      `  homepage "${repositoryUrl}"`,
      "",
      "  livecheck do",
      "    url :url",
      "    strategy :github_latest",
      "  end",
      "",
      "  auto_updates true",
      "",
      '  app "Wardian.app"',
      "",
      "  zap trash: [",
      '    "~/Library/Application Support/org.wardian.desktop",',
      '    "~/Library/Logs/org.wardian.desktop",',
      '    "~/Library/Preferences/org.wardian.desktop.plist",',
      "  ]",
      "end",
      "",
    ].join("\n"),
    "utf8",
  );
}

function writeLinuxInstallDoc(outDir, release, assets) {
  const dir = join(outDir, "linux");
  mkdirSync(dir, { recursive: true });

  writeFileSync(
    join(dir, "install.md"),
    [
      `# Wardian ${release.version} Linux Install`,
      "",
      "GitHub Releases remain the canonical source for Linux Phase 1. Verify the",
      "downloaded artifact hash before installing.",
      "",
      "## Debian/Ubuntu",
      "",
      "```bash",
      `curl -L -o ${assets.linuxDeb.name} ${assets.linuxDeb.url}`,
      `echo "${assets.linuxDeb.sha256}  ${assets.linuxDeb.name}" | sha256sum -c -`,
      `sudo apt install ./${assets.linuxDeb.name}`,
      "```",
      "",
      `sha256sum: ${assets.linuxDeb.sha256}`,
      "",
      "## AppImage",
      "",
      "```bash",
      `curl -L -o ${assets.linuxAppImage.name} ${assets.linuxAppImage.url}`,
      `echo "${assets.linuxAppImage.sha256}  ${assets.linuxAppImage.name}" | sha256sum -c -`,
      `chmod +x ${assets.linuxAppImage.name}`,
      `./${assets.linuxAppImage.name}`,
      "```",
      "",
      `sha256sum: ${assets.linuxAppImage.sha256}`,
      "",
    ].join("\n"),
    "utf8",
  );
}

export function generatePackageManagerManifests(options) {
  const release = loadReleaseManifest(options.releaseAssets);
  const assets = resolvePhaseOneAssets(release);
  const outDir = options.out;
  writeWingetManifests(outDir, release, assets);
  writeHomebrewCask(outDir, release, assets);
  writeLinuxInstallDoc(outDir, release, assets);
  return outDir;
}

if (fileURLToPath(import.meta.url) === process.argv[1]) {
  try {
    const outDir = generatePackageManagerManifests(parseArgs(process.argv.slice(2)));
    console.log(`Generated package-manager files in ${outDir}`);
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error));
    process.exitCode = 1;
  }
}
