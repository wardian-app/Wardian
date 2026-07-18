import { lazy, type ComponentType, type LazyExoticComponent } from "react";

import type {
  FileContentDescriptorV1,
  FileResourceSnapshotV1,
  FilesComparisonBaseline,
} from "../../types";
import type { FileEditorController } from "./fileEditorController";
import type { FileResourceClient } from "./fileResourceClient";

export type FileEditorBufferSnapshot = Readonly<{
  resource_id: string;
  revision: number;
  buffer_generation: number;
  text: string;
  dirty: boolean;
}>;

export type FileRendererProps = {
  snapshot: FileResourceSnapshotV1;
  client: FileResourceClient;
  lifecycle: { visible: boolean };
  surface_id?: string;
  editor_controller?: FileEditorController | null;
  buffer_snapshot?: FileEditorBufferSnapshot | null;
  editor_language?: string | null;
  /** Presentation-local comparison context; never inferred by a shared model. */
  comparison_baseline?: FilesComparisonBaseline | null;
  on_open_file: (path: string) => Promise<void> | void;
  on_open_with: (path: string) => Promise<void> | void;
  on_reveal: (path: string) => Promise<void> | void;
};

export type FileContentPresentation = "rendered" | "editor";

export type FileRendererPresentationDefinition = {
  render: LazyExoticComponent<ComponentType<FileRendererProps>>;
  /** Creates a new lazy wrapper so a rejected module load can be retried. */
  create_renderer: () => LazyExoticComponent<ComponentType<FileRendererProps>>;
};

export type FileRendererDefinition = FileRendererPresentationDefinition & {
  renderer_id: string;
  matches: (descriptor: FileContentDescriptorV1) => boolean;
  capabilities: {
    preview: boolean;
    changes: "line" | "version" | "none";
    draft: boolean;
    annotations: "line_range" | "spatial" | "general";
  };
  source?: FileRendererPresentationDefinition;
  /** Conventional presentation contract used by the Files content host. */
  default_presentation?: FileContentPresentation;
  rendered?: FileRendererPresentationDefinition;
  editor?: FileRendererPresentationDefinition;
  editor_language?: (descriptor: FileContentDescriptorV1) => string;
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

const LANGUAGE_BY_EXTENSION: Readonly<Record<string, string>> = {
  c: "c",
  cc: "cpp",
  cpp: "cpp",
  css: "css",
  go: "go",
  h: "c",
  hpp: "cpp",
  html: "html",
  ini: "ini",
  java: "java",
  js: "javascript",
  json: "json",
  jsx: "javascript",
  md: "markdown",
  mdx: "markdown",
  py: "python",
  rs: "rust",
  sh: "shell",
  sql: "sql",
  svg: "xml",
  toml: "ini",
  ts: "typescript",
  tsx: "typescript",
  xml: "xml",
  yaml: "yaml",
  yml: "yaml",
};

export function editorLanguageForDescriptor(descriptor: FileContentDescriptorV1): string {
  const mime = descriptor.mime_type.trim().toLowerCase();
  if (mime === "text/html") return "html";
  if (mime === "text/markdown") return "markdown";
  if (mime === "application/json") return "json";
  if (mime === "application/xml" || mime === "image/svg+xml") return "xml";
  if (mime === "application/javascript") return "javascript";
  return descriptor.extension
    ? LANGUAGE_BY_EXTENSION[descriptor.extension.toLowerCase()] ?? "plaintext"
    : "plaintext";
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
      if (typeof definition.create_renderer !== "function") {
        throw new Error(`renderer ${definition.renderer_id} requires a create_renderer factory`);
      }
      if (definition.source && typeof definition.source.create_renderer !== "function") {
        throw new Error(`renderer ${definition.renderer_id} source requires a create_renderer factory`);
      }
      const defaultPresentation = definition.default_presentation
        ?? (definition.source || definition.renderer_id !== "text" ? "rendered" : "editor");
      const legacyPresentation = Object.freeze({
        render: definition.render,
        create_renderer: definition.create_renderer,
      });
      const rendered = definition.rendered
        ?? (defaultPresentation === "rendered" ? legacyPresentation : undefined);
      const editor = definition.editor
        ?? definition.source
        ?? (defaultPresentation === "editor" ? legacyPresentation : undefined);
      this.#definitionsById.set(definition.renderer_id, Object.freeze({
        ...definition,
        capabilities: Object.freeze({ ...definition.capabilities }),
        source: definition.source ? Object.freeze({ ...definition.source }) : undefined,
        default_presentation: defaultPresentation,
        rendered: rendered ? Object.freeze({ ...rendered }) : undefined,
        editor: editor ? Object.freeze({ ...editor }) : undefined,
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
    const mime = descriptor.mime_type.trim().toLowerCase();
    if (
      (mime === "text/html" || mime === "image/svg+xml")
      && descriptor.encoding !== null
    ) return this.#definitionsById.get("text") ?? unsupported;

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

type RendererLoader = () => Promise<{ default: ComponentType<FileRendererProps> }>;

function rendererPresentation(load: RendererLoader): FileRendererPresentationDefinition {
  const createRenderer = () => lazy(load);
  return { render: createRenderer(), create_renderer: createRenderer };
}

function rendererDefinition(
  renderer_id: string,
  capabilities: FileRendererDefinition["capabilities"],
  load: RendererLoader,
  loadSource?: RendererLoader,
  defaultPresentation?: FileContentPresentation,
): FileRendererDefinition {
  return {
    renderer_id,
    matches: (descriptor) => (
      descriptor.renderer_kind === renderer_id
      || rendererIdForValidatedMime(descriptor) === renderer_id
    ),
    capabilities,
    ...rendererPresentation(load),
    source: loadSource ? rendererPresentation(loadSource) : undefined,
    default_presentation: defaultPresentation
      ?? (loadSource || renderer_id !== "text" ? "rendered" : "editor"),
    editor_language: renderer_id === "text" || renderer_id === "markdown"
      ? editorLanguageForDescriptor
      : undefined,
  };
}

export const defaultRendererRegistry = new RendererRegistry([
  rendererDefinition("text", {
    preview: true,
    changes: "line",
    draft: true,
    annotations: "line_range",
  }, () => import("./renderers/MonacoTextRenderer")),
  rendererDefinition("markdown", {
    preview: true,
    changes: "line",
    draft: true,
    annotations: "line_range",
  }, () => import("./renderers/MarkdownRenderer"), () => import("./renderers/MonacoTextRenderer")),
  rendererDefinition("image", {
    preview: true,
    changes: "version",
    draft: false,
    annotations: "spatial",
  }, () => import("./renderers/ImageRenderer")),
  rendererDefinition("pdf", {
    preview: true,
    changes: "version",
    draft: false,
    annotations: "spatial",
  }, () => import("./renderers/PdfRenderer")),
  rendererDefinition("unsupported", {
    preview: false,
    changes: "none",
    draft: false,
    annotations: "general",
  }, () => import("./UnsupportedRenderer")),
]);
