import { describe, expect, it } from "vitest";

import { buildFileDiffModel } from "./fileDiffModel";

describe("buildFileDiffModel", () => {
  it("returns no annotations for a clean saved-file buffer", () => {
    expect(buildFileDiffModel("alpha\nbeta\n", "alpha\nbeta\n")).toEqual({
      changes: [],
      summary: {
        regions: 0,
        added_lines: 0,
        modified_lines: 0,
        deleted_lines: 0,
      },
    });
  });

  it("maps inserted lines to stable one-based modified ranges", () => {
    expect(buildFileDiffModel("alpha\nbeta", "alpha\ninserted\nbeta").changes).toEqual([{
      kind: "added",
      original_start_line: null,
      original_end_line: null,
      modified_start_line: 2,
      modified_end_line: 2,
    }]);
  });

  it("maps replacements to modified lines instead of unrelated add/delete markers", () => {
    expect(buildFileDiffModel("alpha\nbeta\ngamma", "alpha\nBETA\ngamma").changes).toEqual([{
      kind: "modified",
      original_start_line: 2,
      original_end_line: 2,
      modified_start_line: 2,
      modified_end_line: 2,
    }]);
  });

  it("keeps deleted-line locations anchored between surviving modified lines", () => {
    expect(buildFileDiffModel("alpha\nbeta\ngamma", "alpha\ngamma").changes).toEqual([{
      kind: "deleted",
      original_start_line: 2,
      original_end_line: 2,
      modified_start_line: 2,
      modified_end_line: 2,
    }]);
  });

  it("reports region and line counts without conflating modifications", () => {
    const diff = buildFileDiffModel(
      "one\ntwo\nthree\nfour\nfive",
      "one\nTWO\nthree\ninserted\nfour",
    );
    expect(diff.summary).toEqual({
      regions: 3,
      added_lines: 1,
      modified_lines: 1,
      deleted_lines: 1,
    });
    expect(diff.changes.map((change) => change.kind)).toEqual([
      "modified",
      "added",
      "deleted",
    ]);
  });

  it("keeps large mostly-stable files off the quadratic fallback", () => {
    const original = Array.from({ length: 20_000 }, (_, index) => `line ${index}`);
    const modified = [...original];
    modified[10_000] = "changed center line";
    expect(buildFileDiffModel(original.join("\n"), modified.join("\n")).summary).toEqual({
      regions: 1,
      added_lines: 0,
      modified_lines: 1,
      deleted_lines: 0,
    });
  });
});
