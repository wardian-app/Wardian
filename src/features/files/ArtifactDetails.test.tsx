import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type { ArtifactResourceV1 } from "../../types";
import { ArtifactDetails } from "./ArtifactDetails";

const resource: ArtifactResourceV1 = {
  schema: 1,
  manifest: {
    schema: 1,
    artifact_id: "artifact-1",
    canonical_path: "/work/report.md",
    title: "Report",
    description: null,
    origin: { session_id: "agent-1", agent_id: "agent-1", agent_name: "Writer", provider: "codex" },
    status: "updated",
    active: true,
    created_at_ms: 1,
    updated_at_ms: 3,
    versions: [
      { version_id: "v1", sequence: 1, content_hash: "sha256:one", size_bytes: 3, presented_at_ms: 1, addressed_comment_ids: [] },
      { version_id: "v2", sequence: 2, content_hash: "sha256:two", size_bytes: 3, presented_at_ms: 2, addressed_comment_ids: [] },
    ],
    latest_review_id: null,
  },
  selected_version: { version_id: "v2", sequence: 2, content_hash: "sha256:two", size_bytes: 3, presented_at_ms: 2, addressed_comment_ids: [] },
  selected_text: "two",
  working: { canonical_path: "/work/report.md", agent_id: "agent-1", content_hash: "sha256:three", unavailable_reason: null },
  attention: true,
};

describe("ArtifactDetails", () => {
  it("shows provenance and drift and selects immutable versions", () => {
    const onSelect = vi.fn();
    render(
      <ArtifactDetails
        resource={resource}
        current_content_hash="sha256:three"
        on_select_version={onSelect}
      />,
    );

    expect(screen.getByLabelText("Artifact details")).toHaveTextContent("Report");
    expect(screen.getByLabelText("Artifact details")).toHaveTextContent("from Writer");
    expect(screen.getByText("Changed since presented")).toBeInTheDocument();
    fireEvent.change(screen.getByRole("combobox", { name: "Version" }), {
      target: { value: "v1" },
    });
    expect(onSelect).toHaveBeenCalledWith("v1");
  });
});
