import { lazy } from "react";
import { describe, expect, it } from "vitest";

import type { FileContentDescriptorV1 } from "../../types";
import {
  RendererRegistry,
  type FileRendererDefinition,
} from "./rendererRegistry";

const EmptyRenderer = lazy(async () => ({ default: () => null }));

function renderer(
  renderer_id: string,
  matches: FileRendererDefinition["matches"] = () => false,
): FileRendererDefinition {
  return {
    renderer_id,
    matches,
    capabilities: {
      preview: true,
      changes: "none",
      draft: false,
      annotations: "general",
    },
    render: EmptyRenderer,
  };
}

function descriptor(
  overrides: Partial<FileContentDescriptorV1> = {},
): FileContentDescriptorV1 {
  return {
    schema: 1,
    canonical_path: "C:/work/report.txt",
    display_name: "report.txt",
    extension: "txt",
    mime_type: "application/pdf",
    encoding: null,
    renderer_kind: "pdf",
    size_bytes: 42,
    line_count: null,
    content_hash: "sha256:report",
    modified_at_ms: 1,
    capabilities: { preview: true, changes: false, draft: false, stream: true },
    unavailable_reason: null,
    ...overrides,
  };
}

describe("RendererRegistry", () => {
  it("uses the backend renderer kind before a misleading extension", () => {
    const registry = new RendererRegistry([
      renderer("text", ({ extension }) => extension === "txt"),
      renderer("pdf", ({ mime_type }) => mime_type === "application/pdf"),
      renderer("unsupported", () => true),
    ]);

    expect(registry.resolve(descriptor()).renderer_id).toBe("pdf");
  });

  it("uses only a validated MIME family as fallback and unsupported last", () => {
    const registry = new RendererRegistry([
      renderer("text", ({ mime_type }) => mime_type.startsWith("text/")),
      renderer("pdf", ({ mime_type }) => mime_type === "application/pdf"),
      renderer("unsupported", () => true),
    ]);

    expect(registry.resolve(descriptor({
      renderer_kind: "unsupported",
      mime_type: "text/plain",
      encoding: "utf-8",
    })).renderer_id).toBe("text");
    expect(registry.resolve(descriptor({
      renderer_kind: "unsupported",
      mime_type: "application/octet-stream",
    })).renderer_id).toBe("unsupported");
  });

  it("rejects duplicate renderer identifiers", () => {
    expect(() => new RendererRegistry([
      renderer("pdf"),
      renderer("pdf"),
      renderer("unsupported"),
    ])).toThrow(/duplicate renderer_id.*pdf/i);
  });
});
