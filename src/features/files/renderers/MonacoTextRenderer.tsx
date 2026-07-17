import { useCallback, useEffect, useRef, useState } from "react";
import type * as Monaco from "monaco-editor";

import type { FileRendererProps } from "../rendererRegistry";

type MonacoApi = typeof Monaco;
type ModelLease = { model: Monaco.editor.ITextModel; references: number };

const models = new Map<string, ModelLease>();
let monacoPromise: Promise<MonacoApi> | null = null;
const MONACO_MAX_SIZE_BYTES = 16 * 1024 * 1024;
const MONACO_MAX_LINE_COUNT = 200_000;

function loadMonaco(): Promise<MonacoApi> {
  if (!monacoPromise) {
    monacoPromise = Promise.all([
      import("monaco-editor"),
      import("monaco-editor/esm/vs/editor/editor.worker.js?worker"),
    ]).then(([monaco, workerModule]) => {
      const WorkerConstructor = workerModule.default;
      globalThis.MonacoEnvironment = {
        ...globalThis.MonacoEnvironment,
        getWorker: () => new WorkerConstructor(),
      };
      return monaco;
    }).catch((error) => {
      monacoPromise = null;
      throw error;
    });
  }
  return monacoPromise;
}

const LANGUAGE_BY_EXTENSION: Readonly<Record<string, string>> = {
  c: "c",
  cc: "cpp",
  cpp: "cpp",
  css: "css",
  go: "go",
  h: "c",
  hpp: "cpp",
  ini: "ini",
  java: "java",
  js: "javascript",
  json: "json",
  jsx: "javascript",
  py: "python",
  rs: "rust",
  sh: "shell",
  sql: "sql",
  toml: "ini",
  ts: "typescript",
  tsx: "typescript",
  xml: "xml",
  yaml: "yaml",
  yml: "yaml",
};

function languageFor({ extension, mime_type }: FileRendererProps["snapshot"]["descriptor"]) {
  if (mime_type === "application/json") return "json";
  if (mime_type === "application/xml") return "xml";
  if (mime_type === "application/javascript") return "javascript";
  return extension ? LANGUAGE_BY_EXTENSION[extension] ?? "plaintext" : "plaintext";
}

function acquireModel(
  monaco: MonacoApi,
  key: string,
  text: string,
  language: string,
): Monaco.editor.ITextModel {
  const existing = models.get(key);
  if (existing) {
    existing.references += 1;
    return existing.model;
  }
  const uri = monaco.Uri.from({ scheme: "wardian-file", path: `/${key}` });
  const model = monaco.editor.getModel(uri) ?? monaco.editor.createModel(text, language, uri);
  models.set(key, { model, references: 1 });
  return model;
}

function releaseModel(key: string, model: Monaco.editor.ITextModel) {
  const lease = models.get(key);
  if (!lease || lease.model !== model) return;
  lease.references -= 1;
  if (lease.references === 0) {
    models.delete(key);
    model.dispose();
  }
}

function errorMessage(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}

function canRenderText(descriptor: FileRendererProps["snapshot"]["descriptor"]) {
  const mime = descriptor.mime_type.trim().toLowerCase();
  const validatedTextMime = mime.startsWith("text/")
    || mime === "application/json"
    || mime === "application/xml"
    || mime === "application/javascript";
  return (descriptor.renderer_kind === "text" || validatedTextMime)
    && mime !== "text/html"
    && mime !== "image/svg+xml"
    && descriptor.encoding === "utf-8"
    && descriptor.capabilities.preview
    && descriptor.unavailable_reason === null
    && descriptor.size_bytes <= MONACO_MAX_SIZE_BYTES
    && descriptor.line_count !== null
    && descriptor.line_count <= MONACO_MAX_LINE_COUNT;
}

function monacoTheme() {
  return document.documentElement.getAttribute("data-theme") === "dark" ? "vs-dark" : "vs";
}

export default function MonacoTextRenderer({ snapshot, client, lifecycle }: FileRendererProps) {
  const hostRef = useRef<HTMLDivElement>(null);
  const [error, setError] = useState<string | null>(null);
  const [retryToken, setRetryToken] = useState(0);
  const retry = useCallback(() => setRetryToken((value) => value + 1), []);

  useEffect(() => {
    if (!lifecycle.visible) return;
    const { descriptor } = snapshot;
    if (!canRenderText(descriptor)) return;
    let cancelled = false;
    let editor: Monaco.editor.IStandaloneCodeEditor | null = null;
    let model: Monaco.editor.ITextModel | null = null;
    let observer: ResizeObserver | null = null;
    let themeObserver: MutationObserver | null = null;
    const modelKey = `${snapshot.resource_id}@${snapshot.revision}`;
    setError(null);

    void Promise.all([
      client.readText(snapshot.resource_id, snapshot.revision),
      loadMonaco(),
    ]).then(([resource, monaco]) => {
      if (cancelled) return;
      const host = hostRef.current;
      if (!host || resource.revision !== snapshot.revision) return;
      model = acquireModel(monaco, modelKey, resource.text, languageFor(descriptor));
      editor = monaco.editor.create(host, {
        automaticLayout: false,
        fontSize: 13,
        minimap: { enabled: false },
        model,
        readOnly: true,
        renderValidationDecorations: "off",
        scrollBeyondLastLine: false,
        theme: monacoTheme(),
        wordWrap: "off",
      });
      themeObserver = new MutationObserver(() => monaco.editor.setTheme(monacoTheme()));
      themeObserver.observe(document.documentElement, {
        attributes: true,
        attributeFilter: ["data-theme"],
      });
      let previousWidth = -1;
      let previousHeight = -1;
      observer = new ResizeObserver((entries) => {
        const entry = entries.find(({ target }) => target === host) ?? entries[0];
        const width = entry?.contentRect.width ?? host.clientWidth;
        const height = entry?.contentRect.height ?? host.clientHeight;
        if (width <= 0 || height <= 0 || (width === previousWidth && height === previousHeight)) {
          return;
        }
        previousWidth = width;
        previousHeight = height;
        editor?.layout({ width, height });
      });
      observer.observe(host);
    }).catch((cause) => {
      if (!cancelled) setError(errorMessage(cause));
    });

    return () => {
      cancelled = true;
      observer?.disconnect();
      themeObserver?.disconnect();
      editor?.dispose();
      if (model) releaseModel(modelKey, model);
    };
  }, [client, lifecycle.visible, retryToken, snapshot]);

  if (!lifecycle.visible) {
    return <div className="files-resource-state" role="status">Text preview suspended.</div>;
  }
  if (!canRenderText(snapshot.descriptor)) {
    return (
      <div className="files-resource-state" role="status">
        {snapshot.descriptor.unavailable_reason ?? "text_preview_unavailable"}
      </div>
    );
  }
  if (error) {
    return (
      <section className="files-resource-state" role="alert">
        <h2>Text preview unavailable</h2>
        <p>{error}</p>
        <button type="button" onClick={retry}>Retry</button>
      </section>
    );
  }
  return <div ref={hostRef} className="files-monaco-renderer" data-testid="monaco-text-renderer" />;
}
