import { describe, expect, it } from "vitest";
import { artifactResourceKey, fileResourceKey } from "./fileResourceKey";

describe("file resource identity", () => {
  it("normalizes path separators without folding arbitrary path casing", () => {
    expect(fileResourceKey("C:\\work\\Notes.md")).toBe("file:C:/work/Notes.md");
    expect(fileResourceKey("C:\\work\\Notes.md")).not.toBe(
      fileResourceKey("C:\\WORK\\notes.md"),
    );
  });

  it("keeps stable artifact IDs verbatim and distinct from normalized file paths", () => {
    expect(fileResourceKey("/work/report.md")).toBe("file:/work/report.md");
    expect(artifactResourceKey("Artifact\\ID/MixedCase"))
      .toBe("artifact:Artifact\\ID/MixedCase");
    expect(fileResourceKey("/work/report.md")).not.toBe(
      artifactResourceKey("artifact-123"),
    );
    expect(() => artifactResourceKey("   ")).toThrow(/non-empty/i);
  });
});
