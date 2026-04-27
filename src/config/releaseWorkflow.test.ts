import { describe, expect, it } from "vitest";
import releasePleaseConfig from "../../release-please-config.json";
import releaseWorkflow from "../../.github/workflows/release.yml?raw";
import releasePleaseWorkflow from "../../.github/workflows/release-please.yml?raw";

describe("release workflow contract", () => {
  it("keeps Release Please releases draft until assets are uploaded", () => {
    expect(releasePleaseConfig).toMatchObject({
      draft: true,
      "force-tag-creation": true,
    });
    expect(releasePleaseWorkflow).toContain("actions: write");
    expect(releasePleaseWorkflow).toContain("Dispatch asset build");
    expect(releasePleaseWorkflow).toContain("gh workflow run release.yml");
    expect(releasePleaseWorkflow).toContain("steps.release.outputs.tag_name");
  });

  it("keeps the artifact release workflow responsible for tag releases", () => {
    expect(releaseWorkflow).toContain("tags:");
    expect(releaseWorkflow).toContain('- "v*"');
    expect(releaseWorkflow).toContain("tauri-apps/tauri-action@v0");
    expect(releaseWorkflow).toContain("releaseId:");
    expect(releaseWorkflow).toContain("Publish release");
  });

  it("can manually backfill assets onto an existing release", () => {
    expect(releaseWorkflow).toContain("tag:");
    expect(releaseWorkflow).toContain("release_tag:");
    expect(releaseWorkflow).toContain("Resolve existing release");
    expect(releaseWorkflow).toContain("/releases/tags/");
    expect(releaseWorkflow).toContain("needs.resolve-release.outputs.release_id");
    expect(releaseWorkflow).toContain("Publish the release");
  });
});
