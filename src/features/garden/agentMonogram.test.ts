import { describe, expect, it, vi } from "vitest";

vi.mock("react-konva", () => { throw new Error("The DOM identity helper must not load the canvas renderer"); });

describe("shared agent identity", () => {
  it("is safe for the DOM interior to import without loading Konva", async () => {
    const { agentMonogram } = await import("./agentMonogram");
    expect(agentMonogram("Garden Builder")).toBe("GB");
    expect(agentMonogram("Alpha")).toBe("A");
    expect(agentMonogram("  ")).toBe("?");
  });
});
