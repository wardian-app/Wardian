import { describe, expect, it } from "vitest";

import { resolveWorkbenchFlags } from "./workbenchFlags";

describe("resolveWorkbenchFlags", () => {
  it("keeps the developer workbench flag off by default", () => {
    expect(resolveWorkbenchFlags({})).toEqual({ workbench_enabled: false });
  });

  it("enables the developer workbench only for the explicit value 1", () => {
    expect(resolveWorkbenchFlags({ VITE_WARDIAN_WORKBENCH: "1" })).toEqual({
      workbench_enabled: true,
    });
  });
});
