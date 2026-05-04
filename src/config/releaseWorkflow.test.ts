import { describe, expect, it } from "vitest";
import releasePleaseConfig from "../../release-please-config.json";
import releaseWorkflow from "../../.github/workflows/release.yml?raw";
import releasePleaseWorkflow from "../../.github/workflows/release-please.yml?raw";
import stageCliScript from "../../scripts/stage-cli.mjs?raw";
import tauriConfig from "../../src-tauri/tauri.conf.json?raw";

describe("release workflow contract", () => {
  it("keeps Release Please releases draft until assets are uploaded", () => {
    expect(releasePleaseConfig).toMatchObject({
      draft: true,
      "force-tag-creation": true,
    });
    expect(releasePleaseWorkflow).toContain("actions: write");
    expect(releasePleaseWorkflow).toContain("Dispatch asset build");
    expect(releasePleaseWorkflow).toContain("gh workflow run release.yml");
    expect(releasePleaseWorkflow).toContain("--repo \"$GITHUB_REPOSITORY\"");
    expect(releasePleaseWorkflow).toContain("steps.release.outputs.tag_name");
  });

  it("keeps the artifact release workflow responsible for tag releases", () => {
    expect(releaseWorkflow).toContain("tags:");
    expect(releaseWorkflow).toContain('- "v*"');
    expect(releaseWorkflow).toContain("tauri-apps/tauri-action@v0");
    expect(releaseWorkflow).toContain("releaseId:");
    expect(releaseWorkflow).toContain("Publish release");
  });

  it("can manually backfill assets onto an existing draft release", () => {
    expect(releaseWorkflow).toContain("tag:");
    expect(releaseWorkflow).toContain("release_tag:");
    expect(releaseWorkflow).toContain("Resolve existing release");
    expect(releaseWorkflow).toContain("github.rest.repos.listReleases");
    expect(releaseWorkflow).toContain("candidate.tag_name === releaseTag");
    expect(releaseWorkflow).toContain("needs.resolve-release.outputs.release_id");
    expect(releaseWorkflow).toContain("Publish the release");
    expect(releaseWorkflow).toContain(
      "always() && needs.build.result == 'success' && (github.event_name == 'push'",
    );
    expect(releaseWorkflow).toContain("needs: [create-release, resolve-release, build]");
    expect(releaseWorkflow).not.toContain("needs.build-cli");
  });

  it("publishes unified installers that carry the staged CLI", () => {
    expect(releaseWorkflow).not.toContain("Build CLI");
    expect(releaseWorkflow).not.toContain("cargo build --release -p wardian-cli --target");
    expect(releaseWorkflow).not.toContain("wardian-cli-x86_64-windows.exe");
    expect(releaseWorkflow).not.toContain("wardian-cli-aarch64-macos");
    expect(releaseWorkflow).not.toContain("wardian-cli-x86_64-macos");
    expect(releaseWorkflow).not.toContain("wardian-cli-x86_64-linux");
    expect(releaseWorkflow).not.toContain("gh release upload");
    expect(releaseWorkflow).not.toContain("Upload CLI dry-run artifact");
    expect(releaseWorkflow).toContain("crates/wardian-cli");
    expect(releaseWorkflow).toContain("WARDIAN_CLI_TARGET: ${{ matrix.platform.rust-target || '' }}");
    expect(releaseWorkflow).toContain("target/release/bundle/**/*.exe");
    expect(releaseWorkflow).toContain("target/*/release/bundle/**/*.dmg");
    expect(tauriConfig).toContain('"beforeBuildCommand": "npm run build && npm run stage-cli"');
    expect(tauriConfig).toContain('"resources/bin/*"');
    expect(stageCliScript).toContain("const buildArgs = ['build', '--release', '-p', 'wardian-cli'];");
    expect(stageCliScript).toContain("const target = process.env.WARDIAN_CLI_TARGET?.trim();");
  });
});
