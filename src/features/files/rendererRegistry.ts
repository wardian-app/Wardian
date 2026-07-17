import { lazy, type ComponentType, type LazyExoticComponent } from "react";

import type { FileContentDescriptorV1, FileResourceSnapshotV1 } from "../../types";
import type { FileResourceClient } from "./fileResourceClient";

export type FileRendererProps = {
  snapshot: FileResourceSnapshotV1;
  client: FileResourceClient;
  lifecycle: { visible: boolean };
  on_open_with: (path: string) => Promise<void> | void;
  on_reveal: (path: string) => Promise<void> | void;
};

export type FileRendererDefinition = {
  renderer_id: string;
  matches: (descriptor: FileContentDescriptorV1) => boolean;
  capabilities: {
    preview: boolean;
    changes: "line" | "version" | "none";
    draft: boolean;
    annotations: "line_range" | "spatial" | "general";
  };
  render: LazyExoticComponent<ComponentType<FileRendererProps>>;
};

const MIME_RENDERER_IDS = Object.freeze({
  markdown: "markdown",
  pdf: "pdf",
  image: "image",
  text: "text",
} as const);

function rendererIdForValidatedMime(
  descriptor: FileContentDescriptorV1,
): string | null {
  const mime = descriptor.mime_type.trim().toLowerCase();
  if (mime === "text/markdown") return MIME_RENDERER_IDS.markdown;
  if (mime === "application/pdf") return MIME_RENDERER_IDS.pdf;
  if (mime.startsWith("image/")) return MIME_RENDERER_IDS.image;
  if (
    descriptor.encoding !== null
    && (
      mime.startsWith("text/")
      || mime === "application/json"
      || mime === "application/xml"
      || mime === "application/javascript"
    )
  ) return MIME_RENDERER_IDS.text;
  return null;
}

/** Selects a renderer from backend-vetted content metadata, never an extension alone. */
export class RendererRegistry {
  readonly #definitionsById = new Map<string, FileRendererDefinition>();

  constructor(definitions: readonly FileRendererDefinition[]) {
    for (const definition of definitions) {
      if (!definition.renderer_id.trim()) {
        throw new Error("renderer_id must not be empty");
      }
      if (this.#definitionsById.has(definition.renderer_id)) {
        throw new Error(`duplicate renderer_id: ${definition.renderer_id}`);
      }
      this.#definitionsById.set(definition.renderer_id, Object.freeze({
        ...definition,
        capabilities: Object.freeze({ ...definition.capabilities }),
      }));
    }
    if (!this.#definitionsById.has("unsupported")) {
      throw new Error("renderer registry requires an unsupported renderer");
    }
  }

  resolve(descriptor: FileContentDescriptorV1): FileRendererDefinition {
    const unsupported = this.#definitionsById.get("unsupported");
    if (!unsupported) throw new Error("renderer registry is missing unsupported");
    if (descriptor.unavailable_reason !== null || !descriptor.capabilities.preview) {
      return unsupported;
    }

    if (descriptor.renderer_kind !== "unsupported") {
      const explicit = this.#definitionsById.get(descriptor.renderer_kind);
      if (explicit) return explicit;
    }

    const mimeRendererId = rendererIdForValidatedMime(descriptor);
    const mimeRenderer = mimeRendererId
      ? this.#definitionsById.get(mimeRendererId)
      : undefined;
    try {
      if (mimeRenderer?.matches(descriptor)) return mimeRenderer;
    } catch {
      // A renderer contribution cannot turn fallback selection into an App failure.
    }
    return unsupported;
  }
}

const pendingRenderer = lazy(() => import("./UnsupportedRenderer"));

function pendingDefinition(
  renderer_id: string,
  capabilities: FileRendererDefinition["capabilities"],
): FileRendererDefinition {
  return {
    renderer_id,
    matches: (descriptor) => descriptor.renderer_kind === renderer_id,
    capabilities,
    render: pendingRenderer,
  };
}

export const defaultRendererRegistry = new RendererRegistry([
  pendingDefinition("text", {
    preview: true,
    changes: "line",
    draft: true,
    annotations: "line_range",
  }),
  pendingDefinition("markdown", {
    preview: true,
    changes: "line",
    draft: true,
    annotations: "line_range",
  }),
  pendingDefinition("image", {
    preview: true,
    changes: "version",
    draft: false,
    annotations: "spatial",
  }),
  pendingDefinition("pdf", {
    preview: true,
    changes: "version",
    draft: false,
    annotations: "spatial",
  }),
  pendingDefinition("unsupported", {
    preview: false,
    changes: "none",
    draft: false,
    annotations: "general",
  }),
]);
