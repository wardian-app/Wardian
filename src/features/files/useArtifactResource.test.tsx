import { act, renderHook, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type { ArtifactResourceV1 } from "../../types";
import { FileResourceClient } from "./fileResourceClient";
import { useArtifactResource } from "./useArtifactResource";

const resource: ArtifactResourceV1 = {
  schema: 1,
  manifest: {
    schema: 1,
    artifact_id: "artifact-1",
    canonical_path: "C:/work/report.md",
    title: "Report",
    description: "Review this",
    origin: {
      session_id: "agent-1",
      agent_id: "agent-1",
      agent_name: "Writer",
      provider: "codex",
    },
    status: "presented",
    active: true,
    created_at_ms: 1,
    updated_at_ms: 2,
    versions: [{
      version_id: "version-1",
      sequence: 1,
      content_hash: "sha256:one",
      size_bytes: 3,
      presented_at_ms: 2,
      addressed_comment_ids: [],
    }],
    latest_review_id: null,
  },
  selected_version: {
    version_id: "version-1",
    sequence: 1,
    content_hash: "sha256:one",
    size_bytes: 3,
    presented_at_ms: 2,
    addressed_comment_ids: [],
  },
  selected_text: "one",
  working: {
    canonical_path: "C:/work/report.md",
    agent_id: "agent-1",
    content_hash: "sha256:two",
    unavailable_reason: null,
  },
  attention: true,
};

describe("useArtifactResource", () => {
  it("loads only the selected version and clears attention explicitly", async () => {
    const client = new FileResourceClient();
    const get = vi.spyOn(client, "getArtifactResource").mockResolvedValue(resource);
    const clear = vi.spyOn(client, "markArtifactAttentionRead").mockResolvedValue(undefined);
    const { result } = renderHook(() => useArtifactResource(
      "artifact-1",
      "version-1",
      client,
    ));

    await waitFor(() => expect(result.current.status).toBe("ready"));
    expect(get).toHaveBeenCalledWith("artifact-1", "version-1");
    expect(result.current.resource?.selected_text).toBe("one");

    await act(() => result.current.clearAttention());
    expect(clear).toHaveBeenCalledWith("artifact-1");
    expect(result.current.resource?.attention).toBe(false);
  });
});
