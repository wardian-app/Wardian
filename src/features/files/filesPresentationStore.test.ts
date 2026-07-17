import { beforeEach, describe, expect, it } from "vitest";

import type { FileContentDescriptorV1, WorkbenchSurfaceV1 } from "../../types";
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

function filesSurface(surfaceId: string, resourceKey: string): WorkbenchSurfaceV1 {
  return {
    surface_id: surfaceId,
    surface_type: "files",
    resource_key: resourceKey,
    state_schema_version: 1,
    state: {},
  };
}

function descriptorAt(canonicalPath: string, displayName = "report.md") {
  return {
    ...descriptor,
    canonical_path: canonicalPath,
    display_name: displayName,
  };
}

describe("Files presentation store", () => {
  beforeEach(() => useFilesPresentationStore.getState().reset());

  it("keeps descriptor metadata and status local to a surface presentation", () => {
    useFilesPresentationStore.getState().setPresentation("files-a", {
      resource_key: "file:/workspace/fallback.txt",
      descriptor,
      dirty: true,
      attention: true,
    });

    expect(filesPresentationTitle("files-a", "file:/workspace/fallback.txt"))
      .toBe("Quarterly report.pdf");
    expect(filesPresentationIcon("files-a", "file:/workspace/fallback.txt"))
      .toBe("files-pdf");
    expect(filesPresentationBadges("files-a", "file:/workspace/fallback.txt")).toEqual([
      { badge_id: "dirty", label: "Unsaved changes" },
      { badge_id: "attention", label: "Attention requested" },
    ]);
    expect(filesPresentationBadges("files-b", "file:/workspace/fallback.txt")).toEqual([]);
  });

  it("ignores metadata and badges retained for a replaced resource identity", () => {
    useFilesPresentationStore.getState().setPresentation("files-a", {
      resource_key: "file:/workspace/old.pdf",
      descriptor,
      dirty: true,
      attention: true,
    });

    expect(filesPresentationTitle("files-a", "file:/workspace/new.md")).toBe("new.md");
    expect(filesPresentationIcon("files-a", "file:/workspace/new.md"))
      .toBe("files-markdown");
    expect(filesPresentationBadges("files-a", "file:/workspace/new.md")).toEqual([]);
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

  it("preserves POSIX backslash basenames and opaque artifact fallback identities", () => {
    expect(filesPresentationTitle("missing", "file:/tmp/a\\b.md")).toBe("a\\b.md");
    expect(filesPresentationTitle("missing", "artifact:opaque\\artifact-id"))
      .toBe("opaque\\artifact-id");

    const store = useFilesPresentationStore.getState();
    const first = filesSurface("artifact-a", "artifact:opaque\\artifact-id");
    const second = filesSurface("artifact-b", "artifact:opaque\\artifact-id");
    store.syncPresentations([second, first]);

    expect(filesPresentationTitle("artifact-a", first.resource_key))
      .toBe("opaque\\artifact-id (1)");
    expect(filesPresentationTitle("artifact-b", second.resource_key))
      .toBe("opaque\\artifact-id (2)");
  });

  it("retains lightweight metadata for hidden open surfaces and prunes closed surfaces", () => {
    const store = useFilesPresentationStore.getState();
    const filesA = filesSurface("files-a", "file:/workspace/a.pdf");
    const filesB = filesSurface("files-b", "file:/workspace/b.pdf");
    store.syncPresentations([filesA, filesB]);
    store.setPresentation("files-a", {
      resource_key: "file:/workspace/a.pdf",
      descriptor,
      dirty: false,
      attention: false,
    });
    store.setPresentation("files-b", {
      resource_key: "file:/workspace/b.pdf",
      descriptor,
      dirty: false,
      attention: true,
    });
    store.syncPresentations([filesA, filesB]);

    expect(useFilesPresentationStore.getState().presentations["files-a"]?.descriptor)
      .toEqual(descriptor);
    store.syncPresentations([filesB]);

    expect(useFilesPresentationStore.getState().presentations["files-a"]).toBeUndefined();
    expect(useFilesPresentationStore.getState().presentations["files-b"]).toBeDefined();
  });

  it("uses the shortest distinguishing parent suffix for POSIX and Windows paths", () => {
    const store = useFilesPresentationStore.getState();
    const posixA = filesSurface("posix-a", "file:/workspace/a/src/index.ts");
    const posixB = filesSurface("posix-b", "file:/workspace/b/src/index.ts");
    const windowsA = filesSurface("windows-a", "file:C:\\workspace\\red\\notes\\readme.md");
    const windowsB = filesSurface("windows-b", "file:C:/workspace/blue/notes/readme.md");
    store.syncPresentations([posixA, posixB, windowsA, windowsB]);

    expect(filesPresentationTitle("posix-a", posixA.resource_key)).toBe("a/src/index.ts");
    expect(filesPresentationTitle("posix-b", posixB.resource_key)).toBe("b/src/index.ts");
    expect(filesPresentationTitle("windows-a", windowsA.resource_key))
      .toBe("red/notes/readme.md");
    expect(filesPresentationTitle("windows-b", windowsB.resource_key))
      .toBe("blue/notes/readme.md");
  });

  it("prefers canonical descriptor paths and updates every colliding title", () => {
    const store = useFilesPresentationStore.getState();
    const artifactA = filesSurface("artifact-a", "artifact:artifact-a");
    const artifactB = filesSurface("artifact-b", "artifact:artifact-b");
    store.syncPresentations([artifactA, artifactB]);
    store.setPresentation("artifact-a", {
      resource_key: artifactA.resource_key!,
      descriptor: descriptorAt("/workspace/client/report.md"),
      dirty: false,
      attention: false,
    });
    store.setPresentation("artifact-b", {
      resource_key: artifactB.resource_key!,
      descriptor: descriptorAt("/workspace/server/report.md"),
      dirty: false,
      attention: false,
    });

    expect(filesPresentationTitle("artifact-a", artifactA.resource_key))
      .toBe("client/report.md");
    expect(filesPresentationTitle("artifact-b", artifactB.resource_key))
      .toBe("server/report.md");

    store.syncPresentations([artifactA]);
    expect(filesPresentationTitle("artifact-a", artifactA.resource_key)).toBe("report.md");
  });

  it("uses a deterministic surface-id fallback for identical resource presentations", () => {
    const store = useFilesPresentationStore.getState();
    const second = filesSurface("files-b", "file:/workspace/report.md");
    const first = filesSurface("files-a", "file:/workspace/report.md");
    store.syncPresentations([second, first]);

    expect(filesPresentationTitle("files-a", first.resource_key)).toBe("report.md (1)");
    expect(filesPresentationTitle("files-b", second.resource_key)).toBe("report.md (2)");
  });

  it("drops stale descriptor metadata when an open surface changes resource", () => {
    const store = useFilesPresentationStore.getState();
    const oldSurface = filesSurface("files-a", "file:/workspace/old/report.pdf");
    store.syncPresentations([oldSurface]);
    store.setPresentation("files-a", {
      resource_key: oldSurface.resource_key!,
      descriptor,
      dirty: true,
      attention: true,
    });

    const replacement = filesSurface("files-a", "file:/workspace/new/readme.md");
    store.syncPresentations([replacement]);

    expect(useFilesPresentationStore.getState().presentations["files-a"]).toEqual({
      resource_key: replacement.resource_key,
      descriptor: null,
      dirty: false,
      attention: false,
    });
    expect(filesPresentationTitle("files-a", replacement.resource_key)).toBe("readme.md");
  });
});
