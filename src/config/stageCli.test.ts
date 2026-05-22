import { describe, expect, it } from "vitest";
import stageCliScript from "../../scripts/stage-cli.mjs?raw";

describe("stage CLI script", () => {
  it("supports debug CLI staging for Tauri dev resources", () => {
    expect(stageCliScript).toContain("export function resolveDevResourcePath");
    expect(stageCliScript).toContain("'debug', 'resources', 'bin'");
    expect(stageCliScript).toContain("if (profile === 'dev')");
    expect(stageCliScript).toContain("copyFileSync(source, devResource)");
  });
});
