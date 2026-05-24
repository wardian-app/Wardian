import { describe, expect, it } from "vitest";
import { parseQueueActionChoices } from "./actionChoices";

describe("parseQueueActionChoices", () => {
  it("parses explicit numbered provider choices", () => {
    expect(parseQueueActionChoices("Approve the patch?\n1. Yes\n2. No")).toEqual([
      { value: "1", label: "Yes" },
      { value: "2", label: "No" },
    ]);
  });

  it("does not invent yes/no choices for generic approval text", () => {
    expect(parseQueueActionChoices("Approve file write?")).toEqual([]);
  });
});
