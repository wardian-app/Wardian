import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

const script = "scripts/package-managers/generate-manifests.mjs";

function hash(ch: string): string {
  return ch.repeat(64);
}

function writeReleaseFixture(dir: string): string {
  const fixture = {
    tagName: "v0.3.6",
    assets: [
      {
        name: "Wardian_0.3.6_x64-setup.exe",
        browser_download_url:
          "https://github.com/tangemicioglu/Wardian/releases/download/v0.3.6/Wardian_0.3.6_x64-setup.exe",
        digest: `sha256:${hash("a")}`,
      },
      {
        name: "Wardian_0.3.6_aarch64.dmg",
        browser_download_url:
          "https://github.com/tangemicioglu/Wardian/releases/download/v0.3.6/Wardian_0.3.6_aarch64.dmg",
        digest: `sha256:${hash("b")}`,
      },
      {
        name: "Wardian_0.3.6_x64.dmg",
        browser_download_url:
          "https://github.com/tangemicioglu/Wardian/releases/download/v0.3.6/Wardian_0.3.6_x64.dmg",
        digest: `sha256:${hash("c")}`,
      },
      {
        name: "Wardian_0.3.6_amd64.deb",
        browser_download_url:
          "https://github.com/tangemicioglu/Wardian/releases/download/v0.3.6/Wardian_0.3.6_amd64.deb",
        digest: `sha256:${hash("d")}`,
      },
      {
        name: "Wardian_0.3.6_amd64.AppImage",
        browser_download_url:
          "https://github.com/tangemicioglu/Wardian/releases/download/v0.3.6/Wardian_0.3.6_amd64.AppImage",
        digest: `sha256:${hash("e")}`,
      },
    ],
  };
  const fixturePath = join(dir, "release-assets.json");
  writeFileSync(fixturePath, `${JSON.stringify(fixture, null, 2)}\n`, "utf8");
  return fixturePath;
}

describe("package-manager manifest generator", () => {
  it("generates winget, Homebrew, and Linux direct-install release files", () => {
    const dir = mkdtempSync(join(tmpdir(), "wardian-package-manifests-"));
    try {
      const fixturePath = writeReleaseFixture(dir);
      const outDir = join(dir, "out");

      execFileSync("node", [script, "--release-assets", fixturePath, "--out", outDir], {
        cwd: process.cwd(),
        stdio: "pipe",
      });

      const wingetInstaller = readFileSync(
        join(outDir, "winget", "Tangemicioglu.Wardian.installer.yaml"),
        "utf8",
      );
      expect(wingetInstaller).toContain("PackageIdentifier: Tangemicioglu.Wardian");
      expect(wingetInstaller).toContain("PackageVersion: 0.3.6");
      expect(wingetInstaller).toContain("InstallerType: nullsoft");
      expect(wingetInstaller).toContain("Silent: /S");
      expect(wingetInstaller).toContain("Architecture: x64");
      expect(wingetInstaller).toContain(`InstallerSha256: ${hash("a")}`);

      const cask = readFileSync(join(outDir, "homebrew", "wardian.rb"), "utf8");
      expect(cask).toContain('cask "wardian" do');
      expect(cask).toContain('version "0.3.6"');
      expect(cask).toContain("on_arm do");
      expect(cask).toContain(`sha256 "${hash("b")}"`);
      expect(cask).toContain("on_intel do");
      expect(cask).toContain(`sha256 "${hash("c")}"`);
      expect(cask).toContain('app "Wardian.app"');
      expect(cask).toContain("auto_updates true");

      const linuxInstall = readFileSync(join(outDir, "linux", "install.md"), "utf8");
      expect(linuxInstall).toContain("# Wardian 0.3.6 Linux Install");
      expect(linuxInstall).toContain("Wardian_0.3.6_amd64.deb");
      expect(linuxInstall).toContain(`sha256sum: ${hash("d")}`);
      expect(linuxInstall).toContain("Wardian_0.3.6_amd64.AppImage");
      expect(linuxInstall).toContain(`sha256sum: ${hash("e")}`);
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });

  it("fails before writing manifests when a required Phase 1 asset is missing", () => {
    const dir = mkdtempSync(join(tmpdir(), "wardian-package-manifests-"));
    try {
      const fixturePath = join(dir, "release-assets.json");
      writeFileSync(
        fixturePath,
        JSON.stringify({
          tagName: "v0.3.6",
          assets: [
            {
              name: "Wardian_0.3.6_x64-setup.exe",
              browser_download_url:
                "https://github.com/tangemicioglu/Wardian/releases/download/v0.3.6/Wardian_0.3.6_x64-setup.exe",
              digest: `sha256:${hash("a")}`,
            },
          ],
        }),
        "utf8",
      );

      expect(() =>
        execFileSync("node", [script, "--release-assets", fixturePath, "--out", join(dir, "out")], {
          cwd: process.cwd(),
          stdio: "pipe",
        }),
      ).toThrow(/Missing required release asset/);
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });
});
