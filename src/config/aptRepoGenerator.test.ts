import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { chmodSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

const script = "scripts/package-managers/generate-apt-repo.mjs";

function sha256(text: string): string {
  return createHash("sha256").update(text).digest("hex");
}

function fileSha256(path: string): string {
  return createHash("sha256").update(readFileSync(path)).digest("hex");
}

function hasCommand(command: string): boolean {
  try {
    execFileSync("command", ["-v", command], { stdio: "ignore", shell: true });
    return true;
  } catch {
    return false;
  }
}

const canRunAptIntegration =
  process.platform === "linux" &&
  ["dpkg-deb", "apt-ftparchive", "gpg", "gpgv", "gzip"].every((command) => hasCommand(command));

function writeReleaseFixture(dir: string, digest: string): string {
  const fixture = {
    tagName: "v0.3.6",
    assets: [
      {
        name: "Wardian_0.3.6_amd64.deb",
        browser_download_url:
          "https://github.com/wardian-app/Wardian/releases/download/v0.3.6/Wardian_0.3.6_amd64.deb",
        digest: `sha256:${digest}`,
      },
    ],
  };
  const fixturePath = join(dir, "release-assets.json");
  writeFileSync(fixturePath, `${JSON.stringify(fixture, null, 2)}\n`, "utf8");
  return fixturePath;
}

function runScript(args: string[]): string {
  try {
    execFileSync("node", [script, ...args], {
      cwd: process.cwd(),
      encoding: "utf8",
      stdio: ["ignore", "pipe", "pipe"],
    });
    return "";
  } catch (error) {
    const err = error as { stderr?: Buffer | string; message?: string };
    return String(err.stderr ?? err.message ?? "");
  }
}

describe("APT repository generator", () => {
  it("fails before staging when the local Debian package hash does not match the release manifest", () => {
    const dir = mkdtempSync(join(tmpdir(), "wardian-apt-repo-"));
    try {
      const debPath = join(dir, "Wardian_0.3.6_amd64.deb");
      writeFileSync(debPath, "not a real deb\n", "utf8");
      const releasePath = writeReleaseFixture(dir, sha256("different content\n"));

      const output = runScript([
        "--release-assets",
        releasePath,
        "--deb",
        debPath,
        "--out",
        join(dir, "out"),
      ]);

      expect(output).toContain("does not match release asset Wardian_0.3.6_amd64.deb");
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });

  it("fails with a clear message when the Debian package path is missing", () => {
    const dir = mkdtempSync(join(tmpdir(), "wardian-apt-repo-"));
    try {
      const releasePath = writeReleaseFixture(dir, "a".repeat(64));

      const output = runScript(["--release-assets", releasePath, "--out", join(dir, "out")]);

      expect(output).toContain("--deb is required");
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });

  it("refuses to overwrite an existing staged Debian package with different content", () => {
    const dir = mkdtempSync(join(tmpdir(), "wardian-apt-repo-"));
    try {
      const debPath = join(dir, "Wardian_0.3.6_amd64.deb");
      const debContent = "new deb bytes\n";
      writeFileSync(debPath, debContent, "utf8");
      const releasePath = writeReleaseFixture(dir, sha256(debContent));

      const stagedDir = join(dir, "out", "pool", "main", "w", "wardian");
      mkdirSync(stagedDir, { recursive: true });
      writeFileSync(join(stagedDir, "Wardian_0.3.6_amd64.deb"), "old deb bytes\n", "utf8");

      const output = runScript([
        "--release-assets",
        releasePath,
        "--deb",
        debPath,
        "--out",
        join(dir, "out"),
      ]);

      expect(output).toContain("Refusing to overwrite existing Wardian_0.3.6_amd64.deb");
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });

  it.runIf(canRunAptIntegration)(
    "generates and signs a repository from a valid Debian package when Debian tooling is available",
    () => {
      const dir = mkdtempSync(join(tmpdir(), "wardian-apt-repo-"));
      try {
        const packageRoot = join(dir, "package");
        mkdirSync(join(packageRoot, "DEBIAN"), { recursive: true });
        mkdirSync(join(packageRoot, "usr", "share", "wardian"), { recursive: true });
        writeFileSync(
          join(packageRoot, "DEBIAN", "control"),
          [
            "Package: wardian",
            "Version: 0.3.6",
            "Architecture: amd64",
            "Maintainer: Wardian App <support@wardian.org>",
            "Description: Wardian test package",
            "",
          ].join("\n"),
          "utf8",
        );
        writeFileSync(join(packageRoot, "usr", "share", "wardian", "README"), "test package\n", "utf8");

        const debPath = join(dir, "Wardian_0.3.6_amd64.deb");
        execFileSync("dpkg-deb", ["--build", packageRoot, debPath], { stdio: "ignore" });
        const releasePath = writeReleaseFixture(dir, fileSha256(debPath));

        const gnupgHome = join(dir, "gnupg");
        mkdirSync(gnupgHome, { recursive: true });
        chmodSync(gnupgHome, 0o700);
        const env = {
          ...process.env,
          GNUPGHOME: gnupgHome,
          WARDIAN_APT_SIGNING_KEY_PASSPHRASE: "integration-passphrase",
        };
        execFileSync(
          "gpg",
          [
            "--batch",
            "--pinentry-mode",
            "loopback",
            "--passphrase",
            "integration-passphrase",
            "--quick-gen-key",
            "Wardian APT Integration <apt-integration@wardian.local>",
            "ed25519",
            "sign",
            "1d",
          ],
          { env, stdio: "ignore" },
        );

        const outDir = join(dir, "repo");
        execFileSync(
          "node",
          [
            script,
            "--release-assets",
            releasePath,
            "--deb",
            debPath,
            "--out",
            outDir,
            "--signing-key",
            "Wardian APT Integration <apt-integration@wardian.local>",
          ],
          { cwd: process.cwd(), env, stdio: "ignore" },
        );

        const release = readFileSync(join(outDir, "dists", "stable", "Release"), "utf8");
        expect(release).toContain("Suite: stable");
        expect(release).toContain("Codename: stable");
        expect(release).toContain("Components: main");
        expect(readFileSync(join(outDir, "dists", "stable", "main", "binary-amd64", "Packages"), "utf8")).toContain(
          "Package: wardian",
        );
        expect(existsSync(join(outDir, "dists", "stable", "main", "binary-amd64", "Packages.gz"))).toBe(true);
        expect(existsSync(join(outDir, "dists", "stable", "InRelease"))).toBe(true);
        expect(existsSync(join(outDir, "dists", "stable", "Release.gpg"))).toBe(true);
        expect(existsSync(join(outDir, "wardian-archive-keyring.gpg"))).toBe(true);

        execFileSync("gpg", ["--verify", join(outDir, "dists", "stable", "Release.gpg"), join(outDir, "dists", "stable", "Release")], {
          env,
          stdio: "ignore",
        });
        execFileSync("gpg", ["--verify", join(outDir, "dists", "stable", "InRelease")], {
          env,
          stdio: "ignore",
        });
        execFileSync(
          "gpgv",
          [
            "--keyring",
            join(outDir, "wardian-archive-keyring.gpg"),
            join(outDir, "dists", "stable", "Release.gpg"),
            join(outDir, "dists", "stable", "Release"),
          ],
          { stdio: "ignore" },
        );
        execFileSync(
          "gpgv",
          ["--keyring", join(outDir, "wardian-archive-keyring.gpg"), join(outDir, "dists", "stable", "InRelease")],
          { stdio: "ignore" },
        );
      } finally {
        rmSync(dir, { recursive: true, force: true });
      }
    },
  );
});
