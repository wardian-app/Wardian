import { describe, expect, it } from "vitest";
import releasePleaseConfig from "../../release-please-config.json";
import releaseWorkflow from "../../.github/workflows/release.yml?raw";
import releasePleaseWorkflow from "../../.github/workflows/release-please.yml?raw";
import stageCliScript from "../../scripts/stage-cli.mjs?raw";
import tauriConfig from "../../src-tauri/tauri.conf.json?raw";
import tauriUpdaterConfig from "../../src-tauri/tauri.updater.conf.json?raw";

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

  it("syncs Cargo.lock after Release Please bumps Rust workspace versions", () => {
    expect(releasePleaseConfig.packages["."]["extra-files"]).toContainEqual({
      type: "toml",
      path: "Cargo.toml",
      jsonpath: "$.workspace.package.version",
    });
    expect(releasePleaseWorkflow).toContain("Sync release PR Cargo lockfile");
    expect(releasePleaseWorkflow).toContain("steps.release.outputs.prs_created == 'true'");
    expect(releasePleaseWorkflow).toContain("actions/checkout@v4");
    expect(releasePleaseWorkflow).toContain("fetch-depth: 0");
    expect(releasePleaseWorkflow).toContain("RELEASE_PLEASE_PRS: ${{ steps.release.outputs.prs }}");
    expect(releasePleaseWorkflow).toContain("headBranchName");
    expect(releasePleaseWorkflow).not.toContain("gh pr list");
    expect(releasePleaseWorkflow).not.toContain("npm install --package-lock-only");
    expect(releasePleaseWorkflow).not.toContain("cargo metadata --format-version 1 --no-deps");
    expect(releasePleaseWorkflow).toContain("cargo update --workspace");
    expect(releasePleaseWorkflow).toContain("git diff --quiet -- Cargo.lock");
    expect(releasePleaseWorkflow).toContain("git add Cargo.lock");
    expect(releasePleaseWorkflow).toContain("chore: sync release Cargo.lock");
  });

  it("keeps the artifact release workflow responsible for tag releases", () => {
    expect(releaseWorkflow).toContain("tags:");
    expect(releaseWorkflow).toContain('- "v*"');
    expect(releaseWorkflow).toContain('if [[ "$TAG" == *-* ]]; then');
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
    expect(releaseWorkflow).toContain("core.setOutput('prerelease', String(release.prerelease || releaseTag.includes('-')))");
    expect(releaseWorkflow).toContain("needs.resolve-release.outputs.release_id");
    expect(releaseWorkflow).toContain("Publish the release");
    expect(releaseWorkflow).toContain("always() && needs.build.result == 'success'");
    expect(releaseWorkflow).toContain("needs.create-release.outputs.is_prerelease == 'true'");
    expect(releaseWorkflow).toContain("needs.resolve-release.outputs.is_prerelease == 'true'");
    expect(releaseWorkflow).toContain("needs: [create-release, resolve-release, build]");
    expect(releaseWorkflow).not.toContain("needs.build-cli");
  });

  it("signs updater artifacts with tag-aware release metadata", () => {
    expect(releaseWorkflow).toContain("TAURI_SIGNING_PRIVATE_KEY: ${{ secrets.TAURI_SIGNING_PRIVATE_KEY }}");
    expect(releaseWorkflow).toContain(
      "TAURI_SIGNING_PRIVATE_KEY_PASSWORD: ${{ secrets.TAURI_SIGNING_PRIVATE_KEY_PASSWORD }}",
    );
    expect(releaseWorkflow).toContain("tagName: ${{ github.ref_name }}");
    expect(releaseWorkflow).toContain("tagName: ${{ inputs.release_tag }}");
    expect(releaseWorkflow).toContain("WARDIAN_UPDATE_CHANNEL: ${{ needs.create-release.outputs.is_prerelease == 'false' && 'stable' || '' }}");
    expect(releaseWorkflow).toContain("WARDIAN_UPDATE_CHANNEL: ${{ needs.resolve-release.outputs.is_prerelease == 'false' && 'stable' || '' }}");
    expect(releaseWorkflow).toContain("includeUpdaterJson: ${{ needs.create-release.outputs.is_prerelease == 'false' }}");
    expect(releaseWorkflow).toContain("includeUpdaterJson: ${{ needs.resolve-release.outputs.is_prerelease == 'false' }}");
    expect(releaseWorkflow).not.toContain("uploadUpdaterJson:");
    expect(releaseWorkflow).not.toContain("uploadUpdaterSignatures:");
    expect(releaseWorkflow).toContain("updaterJsonPreferNsis: true");
    expect(releaseWorkflow).toContain("Build (dry run)");
    expect(releaseWorkflow).toContain("ref: ${{ github.event_name == 'workflow_dispatch' && inputs.dry_run != 'true' && inputs.release_tag || github.event_name == 'workflow_dispatch' && inputs.tag != '' && inputs.tag || github.ref }}");
  });

  it("keeps local Tauri bundles free of updater signing while release builds opt in", () => {
    expect(JSON.parse(tauriConfig).bundle.createUpdaterArtifacts).toBe(false);
    expect(JSON.parse(tauriUpdaterConfig).bundle.createUpdaterArtifacts).toBe(true);
    expect(releaseWorkflow.match(/--config src-tauri\/tauri\.updater\.conf\.json/g)).toHaveLength(3);
  });

  it("keeps native window minimums aligned with the frontend resize guard", () => {
    const windowConfig = JSON.parse(tauriConfig).app.windows[0];

    expect(windowConfig.minWidth).toBe(320);
    expect(windowConfig.minHeight).toBe(240);
  });

  it("validates updater metadata before publishing releases", () => {
    expect(releaseWorkflow).toContain("Validate updater metadata");
    expect(releaseWorkflow).toContain("latest.json");
    expect(releaseWorkflow).toContain("windows-x86_64");
    expect(releaseWorkflow).toContain("linux-x86_64");
    expect(releaseWorkflow).toContain("darwin-aarch64");
    expect(releaseWorkflow).toContain("darwin-x86_64");
    expect(releaseWorkflow).toContain("const expectedTag =");
    expect(releaseWorkflow).toContain("const expectedVersion = expectedTag.replace(/^v/, '')");
    expect(releaseWorkflow).toContain("metadata.version !== expectedVersion");
    expect(releaseWorkflow).toContain("const releaseAssetNames = new Set(assets.map((asset) => asset.name))");
    expect(releaseWorkflow).toContain("const releaseAssetNameFromUrl = (candidateUrl) =>");
    expect(releaseWorkflow).toContain("parsed.hostname !== 'github.com'");
    expect(releaseWorkflow).toContain("tag !== expectedTag");
    expect(releaseWorkflow).toContain("releaseAssetNames.has(assetName)");
    expect(releaseWorkflow).toContain("needs.validate-updater-metadata.result == 'success'");
  });

  it("removes loose updater signature assets before release validation and publication", () => {
    expect(releaseWorkflow).toContain("cleanup-updater-signatures:");
    expect(releaseWorkflow).toContain("Remove loose updater signature assets");
    expect(releaseWorkflow).toContain("asset.name.endsWith('.sig')");
    expect(releaseWorkflow).toContain("deleteReleaseAsset");
    expect(releaseWorkflow).toContain("needs: [create-release, resolve-release, build, cleanup-updater-signatures]");
    expect(releaseWorkflow).toContain("needs.cleanup-updater-signatures.result == 'success'");
  });

  it("guards manual updater backfill before publishing", () => {
    expect(releaseWorkflow).toContain("Ensure manual release is draft");
    expect(releaseWorkflow).toContain("manual backfill requires a draft release");
    expect(releaseWorkflow).toContain("release.draft");
    expect(releaseWorkflow).toContain("needs: [create-release, resolve-release, build, cleanup-updater-signatures, validate-updater-metadata]");
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
    expect(stageCliScript).toContain("const buildArgs = ['build', '-p', 'wardian-cli'];");
    expect(stageCliScript).toContain("buildArgs.push('--release');");
    expect(stageCliScript).toContain("const target = process.env.WARDIAN_CLI_TARGET?.trim();");
  });

  it("stages a current worktree CLI before dev startup", () => {
    expect(tauriConfig).toContain('"beforeDevCommand": "npm run stage-cli:dev && npm run vite"');
    expect(stageCliScript).toContain("profile !== 'dev'");
    expect(stageCliScript).toContain("profile === 'release'");
  });
});
