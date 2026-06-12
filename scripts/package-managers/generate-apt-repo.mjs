#!/usr/bin/env node

import {
  chmodSync,
  copyFileSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { basename, join } from "node:path";
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { fileURLToPath } from "node:url";
import { tmpdir } from "node:os";

const expectedPackageName = "wardian";
const expectedArchitecture = "amd64";

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
  const deb = args.get("--deb");
  const out = args.get("--out") ?? "dist/apt";
  const signingKey = args.get("--signing-key");

  if (!releaseAssets) {
    throw new Error("--release-assets is required");
  }
  if (!deb) {
    throw new Error("--deb is required");
  }

  return { releaseAssets, deb, out, signingKey };
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

function findLinuxDebAsset(release) {
  const expectedName = `Wardian_${release.version}_amd64.deb`;
  const asset = release.assets.find((candidate) => candidate.name === expectedName);
  if (!asset) {
    throw new Error(`Missing required Linux Debian package release asset: ${expectedName}`);
  }

  if (!asset.url.includes(`/releases/download/${release.tag}/`)) {
    throw new Error(`${expectedName} URL must target ${release.tag}: ${asset.url}`);
  }

  return asset;
}

function sha256File(path) {
  return createHash("sha256").update(readFileSync(path)).digest("hex");
}

function commandOutput(command, args, options = {}) {
  return execFileSync(command, args, {
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
    ...options,
  });
}

function validateDebianPackage(debPath, release) {
  const packageName = commandOutput("dpkg-deb", ["-f", debPath, "Package"]).trim();
  const version = commandOutput("dpkg-deb", ["-f", debPath, "Version"]).trim();
  const architecture = commandOutput("dpkg-deb", ["-f", debPath, "Architecture"]).trim();

  if (packageName !== expectedPackageName) {
    throw new Error(`Expected Debian package name ${expectedPackageName}, got ${packageName}`);
  }
  if (version !== release.version) {
    throw new Error(`Expected Debian package version ${release.version}, got ${version}`);
  }
  if (architecture !== expectedArchitecture) {
    throw new Error(`Expected Debian package architecture ${expectedArchitecture}, got ${architecture}`);
  }
}

function writeAptMetadata(outDir, signingKey) {
  const packagesPath = join(outDir, "dists", "stable", "main", "binary-amd64", "Packages");
  const releasePath = join(outDir, "dists", "stable", "Release");

  const packages = commandOutput("apt-ftparchive", ["packages", "pool/main"], { cwd: outDir });
  writeFileSync(packagesPath, packages, "utf8");
  execFileSync("gzip", ["-k", "-f", packagesPath], { stdio: "inherit" });

  const release = commandOutput("apt-ftparchive", ["release", "dists/stable"], { cwd: outDir });
  const releaseHeader = [
    "Origin: Wardian",
    "Label: Wardian",
    "Suite: stable",
    "Codename: stable",
    "Architectures: amd64",
    "Components: main",
    "Description: Wardian stable APT repository",
    "",
  ].join("\n");
  writeFileSync(releasePath, `${releaseHeader}${release}`, "utf8");

  if (signingKey) {
    const passphrase = process.env.WARDIAN_APT_SIGNING_KEY_PASSPHRASE;
    const passphraseDir = passphrase ? mkdtempSync(join(tmpdir(), "wardian-apt-gpg-")) : null;
    try {
      let passphraseArgs = [];
      if (passphrase && passphraseDir) {
        const passphraseFile = join(passphraseDir, "passphrase");
        writeFileSync(passphraseFile, passphrase, { encoding: "utf8", mode: 0o600 });
        chmodSync(passphraseFile, 0o600);
        passphraseArgs = ["--pinentry-mode", "loopback", "--passphrase-file", passphraseFile];
      }

      execFileSync(
        "gpg",
        [
          "--batch",
          "--yes",
          ...passphraseArgs,
          "--local-user",
          signingKey,
          "--clearsign",
          "-o",
          join(outDir, "dists", "stable", "InRelease"),
          releasePath,
        ],
        { stdio: "inherit" },
      );
      execFileSync(
        "gpg",
        [
          "--batch",
          "--yes",
          ...passphraseArgs,
          "--local-user",
          signingKey,
          "-abs",
          "-o",
          join(outDir, "dists", "stable", "Release.gpg"),
          releasePath,
        ],
        { stdio: "inherit" },
      );
      execFileSync(
        "gpg",
        ["--batch", "--yes", "--output", join(outDir, "wardian-archive-keyring.gpg"), "--export", signingKey],
        { stdio: "inherit" },
      );
    } finally {
      if (passphraseDir) {
        rmSync(passphraseDir, { recursive: true, force: true });
      }
    }
  }
}

export function generateAptRepository(options) {
  const release = loadReleaseManifest(options.releaseAssets);
  const debAsset = findLinuxDebAsset(release);
  const localHash = sha256File(options.deb);
  if (localHash !== debAsset.sha256) {
    throw new Error(
      `Local .deb sha256 ${localHash} does not match release asset ${debAsset.name} sha256 ${debAsset.sha256}`,
    );
  }

  const outDir = options.out;
  const packageDir = join(outDir, "pool", "main", "w", "wardian");
  const metadataDir = join(outDir, "dists", "stable", "main", "binary-amd64");
  mkdirSync(packageDir, { recursive: true });
  mkdirSync(metadataDir, { recursive: true });

  const stagedDeb = join(packageDir, basename(debAsset.name));
  if (existsSync(stagedDeb)) {
    const stagedHash = sha256File(stagedDeb);
    if (stagedHash !== localHash) {
      throw new Error(
        `Refusing to overwrite existing ${basename(debAsset.name)} with different content: existing sha256 ${stagedHash}, new sha256 ${localHash}`,
      );
    }
  }

  validateDebianPackage(options.deb, release);
  if (!existsSync(stagedDeb)) {
    copyFileSync(options.deb, stagedDeb);
  }
  writeAptMetadata(outDir, options.signingKey);

  return outDir;
}

if (fileURLToPath(import.meta.url) === process.argv[1]) {
  try {
    const outDir = generateAptRepository(parseArgs(process.argv.slice(2)));
    console.log(`Generated APT repository in ${outDir}`);
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error));
    process.exitCode = 1;
  }
}
