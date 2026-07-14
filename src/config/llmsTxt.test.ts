import { execFileSync } from "node:child_process";
import { describe, expect, it } from "vitest";
import llmsTxt from "../../docs/public/llms.txt?raw";

describe("llms.txt docs contract", () => {
  it("keeps the agent-readable docs index valid", () => {
    expect(() => {
      execFileSync(process.execPath, ["scripts/verify-llms-txt.mjs"], {
        cwd: process.cwd(),
        stdio: "pipe",
      });
    }).not.toThrow();
  });

  it("summarizes Wardian and points agents to canonical docs", () => {
    expect(llmsTxt).toContain("local agent work becomes visible, durable, and malleable");
    expect(llmsTxt).toContain("https://docs.wardian.org/guide/cli");
    expect(llmsTxt).toContain("https://docs.wardian.org/developer/provider-runtimes");
  });
});
