import { afterEach, describe, expect, it } from "vitest";
import { resolveCssVar } from "./resolveColor";

afterEach(() => {
  document.documentElement.style.removeProperty("--color-wardian-success");
});

describe("resolveCssVar", () => {
  it("returns concrete colors unchanged", () => {
    expect(resolveCssVar("#10b981")).toBe("#10b981");
    expect(resolveCssVar("rgb(1,2,3)")).toBe("rgb(1,2,3)");
  });

  it("resolves a var() expression off the document root", () => {
    document.documentElement.style.setProperty("--color-wardian-success", "#10b981");
    expect(resolveCssVar("var(--color-wardian-success)")).toBe("#10b981");
  });

  it("falls back when the variable is undefined", () => {
    expect(resolveCssVar("var(--color-wardian-missing)", "#777777")).toBe("#777777");
  });
});
