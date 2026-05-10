import { render, screen } from "@testing-library/react";
import { PlaceholderView } from "./PlaceholderView";

describe("PlaceholderView", () => {
  it.each([
    ["workflow-builder", "Workflow Builder", "Advanced workflow-builder features coming in Phase 3.", "Phase 3"],
    ["graph", "Graph", "Advanced graph features coming in Phase 5.", "Phase 5"],
    ["garden", "Garden", "Advanced garden features coming in Phase 5.", "Phase 5"],
  ])("renders the %s placeholder copy", (viewMode, heading, description, phase) => {
    render(<PlaceholderView viewMode={viewMode} />);

    expect(screen.getByRole("heading", { name: heading })).toBeInTheDocument();
    expect(screen.getByText(description)).toBeInTheDocument();
    expect(screen.getByText("Planned")).toBeInTheDocument();
    expect(screen.getByText(phase)).toBeInTheDocument();
  });
});
