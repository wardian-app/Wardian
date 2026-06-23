import { describe, expect, it } from "vitest";
import { isActiveAgentStatus, isActiveWorkflowStatus, workflowStatusColor } from "./gardenStatus";

describe("isActiveAgentStatus", () => {
  it("is true only for processing/headless work", () => {
    expect(isActiveAgentStatus("Processing")).toBe(true);
    expect(isActiveAgentStatus("headless")).toBe(true);
    expect(isActiveAgentStatus("Idle")).toBe(false);
    expect(isActiveAgentStatus("Off")).toBe(false);
    expect(isActiveAgentStatus("Action Needed")).toBe(false);
  });
});

describe("isActiveWorkflowStatus", () => {
  it("is true only while a run is live or awaiting approval", () => {
    expect(isActiveWorkflowStatus("running")).toBe(true);
    expect(isActiveWorkflowStatus("awaiting_approval")).toBe(true);
    expect(isActiveWorkflowStatus("completed")).toBe(false);
    expect(isActiveWorkflowStatus("failed")).toBe(false);
    expect(isActiveWorkflowStatus("none")).toBe(false);
  });
});

describe("workflowStatusColor", () => {
  it("maps each run status to a theme variable", () => {
    expect(workflowStatusColor("running")).toBe("var(--color-wardian-processing)");
    expect(workflowStatusColor("awaiting_approval")).toBe("var(--color-wardian-warning)");
    expect(workflowStatusColor("completed")).toBe("var(--color-wardian-success)");
    expect(workflowStatusColor("failed")).toBe("var(--color-wardian-error)");
    expect(workflowStatusColor("none")).toBe("var(--color-wardian-text-muted)");
  });
});
