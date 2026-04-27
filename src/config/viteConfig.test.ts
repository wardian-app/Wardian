import { describe, expect, it } from "vitest";
import { viteDevServerHeaders } from "./viteDevServerHeaders";

describe("vite dev server config", () => {
  it("disables browser caching for dev modules", async () => {
    expect(viteDevServerHeaders).toMatchObject({
      "Cache-Control": "no-store",
    });
  });
});
