import { beforeEach, describe, expect, it } from "vitest";

import type { FileContentDescriptorV1 } from "../../types";
import {
  filesPresentationBadges,
  filesPresentationIcon,
  filesPresentationTitle,
  useFilesPresentationStore,
} from "./filesPresentationStore";

const descriptor: FileContentDescriptorV1 = {
  schema: 1,
  canonical_path: "/workspace/report.pdf",
  display_name: "Quarterly report.pdf",
  extension: "pdf",
  mime_type: "application/pdf",
  encoding: null,
  renderer_kind: "pdf",
  size_bytes: 100,
  line_count: null,
  content_hash: "hash",
  modified_at_ms: 1,
  capabilities: { preview: true, changes: false, draft: false, stream: true },
  unavailable_reason: null,
};

describe("Files presentation store", () => {
  beforeEach(() => useFilesPresentationStore.getState().reset());

  it("keeps descriptor metadata and status local to a surface presentation", () => {
    useFilesPresentationStore.getState().setPresentation("files-a", {
      descriptor,
      dirty: true,
      attention: true,
    });

    expect(filesPresentationTitle("files-a", "file:/workspace/fallback.txt"))
      .toBe("Quarterly report.pdf");
    expect(filesPresentationIcon("files-a", "file:/workspace/fallback.txt"))
      .toBe("files-pdf");
    expect(filesPresentationBadges("files-a")).toEqual([
      { badge_id: "dirty", label: "Unsaved changes" },
      { badge_id: "attention", label: "Attention requested" },
    ]);
    expect(filesPresentationBadges("files-b")).toEqual([]);
  });

  it("falls back safely from normalized resource identity", () => {
    expect(filesPresentationTitle("missing", "file:C:/workspace/notes/readme.md"))
      .toBe("readme.md");
    expect(filesPresentationIcon("missing", "file:C:/workspace/notes/readme.md"))
      .toBe("files-markdown");
    expect(filesPresentationTitle("missing", "artifact:artifact-123"))
      .toBe("artifact-123");
    expect(filesPresentationIcon("missing", "artifact:artifact-123"))
      .toBe("files-artifact");
    expect(filesPresentationTitle("missing", undefined)).toBe("Files");
    expect(filesPresentationIcon("missing", undefined)).toBe("files");
  });

  it("clears presentation metadata without affecting siblings", () => {
    const store = useFilesPresentationStore.getState();
    store.setPresentation("files-a", { descriptor, dirty: false, attention: false });
    store.setPresentation("files-b", { descriptor, dirty: false, attention: true });
    store.clearPresentation("files-a");

    expect(useFilesPresentationStore.getState().presentations["files-a"]).toBeUndefined();
    expect(useFilesPresentationStore.getState().presentations["files-b"]).toBeDefined();
  });
});
