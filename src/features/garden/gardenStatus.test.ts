import { describe, expect, it } from "vitest";
import { isActiveAgentStatus } from "./gardenStatus";

describe("isActiveAgentStatus", () => {
  it("is true only for processing/headless work", () => {
    expect(isActiveAgentStatus("Processing")).toBe(true);
    expect(isActiveAgentStatus("headless")).toBe(true);
    expect(isActiveAgentStatus("Idle")).toBe(false);
    expect(isActiveAgentStatus("Off")).toBe(false);
    expect(isActiveAgentStatus("Action Needed")).toBe(false);
  });
});
