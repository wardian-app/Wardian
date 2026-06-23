import { describe, expect, it } from "vitest";
import { unitKey } from "./garden.types";

describe("unitKey", () => {
  it("namespaces by kind so agent and workflow ids never collide", () => {
    expect(unitKey({ kind: "agent", id: "abc" })).toBe("agent:abc");
    expect(unitKey({ kind: "workflow", id: "abc" })).toBe("workflow:abc");
  });
});
