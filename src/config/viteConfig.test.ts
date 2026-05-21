import { describe, expect, it } from "vitest";
import { viteDevServerHeaders } from "./viteDevServerHeaders";
import { viteWatchIgnored } from "./viteWatchIgnored";

describe("vite dev server config", () => {
  it("disables browser caching for dev modules", async () => {
    expect(viteDevServerHeaders).toMatchObject({
      "Cache-Control": "no-store",
    });
  });

  it("ignores Wardian runtime state that can churn during dev sessions", () => {
    expect(viteWatchIgnored).toEqual(expect.arrayContaining([
      "**/target/**",
      "**/.tmp/**",
      "**/state.db-*",
      "**/agents/*/habitat/**",
      "**/agents/*/worktree/**",
      "**/agents/*/claude/**",
    ]));
    expect(viteWatchIgnored.some((pattern) => (
      pattern instanceof RegExp && pattern.test("target\\debug\\.wardian\\state.db-wal")
    ))).toBe(true);
    expect(viteWatchIgnored.some((pattern) => (
      pattern instanceof RegExp && pattern.test("src\\features\\graph\\GraphCanvas.tsx")
    ))).toBe(false);
    expect(viteWatchIgnored).not.toContain("**/agents/**");
  });
});
