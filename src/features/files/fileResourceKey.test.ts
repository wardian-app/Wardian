import { describe, expect, it } from "vitest";
import { artifactResourceKey, fileResourceKey } from "./fileResourceKey";

describe("file resource identity", () => {
  it("normalizes path separators without folding arbitrary path casing", () => {
    expect(fileResourceKey("C:\\work\\Notes.md")).toBe("file:C:/work/Notes.md");
    expect(fileResourceKey("C:\\work\\Notes.md")).not.toBe(
      fileResourceKey("C:\\WORK\\notes.md"),
    );
  });

  it("keeps file and artifact identities distinct for the same normalized path", () => {
    expect(fileResourceKey("/work/report.md")).toBe("file:/work/report.md");
    expect(artifactResourceKey("\\work\\report.md")).toBe("artifact:/work/report.md");
    expect(fileResourceKey("/work/report.md")).not.toBe(
      artifactResourceKey("/work/report.md"),
    );
  });
});
