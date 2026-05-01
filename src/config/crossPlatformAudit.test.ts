import { describe, expect, it } from "vitest";
import headlessManager from "../../src-tauri/src/manager/headless.rs?raw";
import geminiPatchScript from "../../scripts/gemini-patch-skills.cjs?raw";

describe("cross-platform runtime contracts", () => {
  it("applies the macOS GUI PATH repair to every headless provider launch path", () => {
    const pathRepairs = headlessManager.match(/cmd\.env\("PATH", macos_extended_path\(\)\)/g) ?? [];

    expect(pathRepairs.length).toBeGreaterThanOrEqual(2);
  });

  it("discovers Gemini CLI installs instead of assuming a Windows-only npm path", () => {
    expect(geminiPatchScript).toContain("process.env.GEMINI_CLI_DIR");
    expect(geminiPatchScript).toContain("npm");
    expect(geminiPatchScript).toContain("pnpm");
    expect(geminiPatchScript).toContain("resolveCommand('gemini')");
    expect(geminiPatchScript).toContain("require.main === module");
  });
});
