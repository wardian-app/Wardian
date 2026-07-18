import { useCallback, useEffect, useRef, useState } from "react";
import type * as Monaco from "monaco-editor";

import type { FileEditorController } from "../fileEditorController";
import type { FileRendererProps } from "../rendererRegistry";

type MonacoApi = typeof Monaco;
type ModelLease = {
  model: Monaco.editor.ITextModel;
  references: number;
  controller: FileEditorController;
  applying_controller_text: boolean;
  dispose_content_listener: () => void;
  dispose_controller_listener: () => void;
  dispose_controller_lifetime: () => void;
};

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

function disposeLease(key: string, lease: ModelLease): void {
  if (models.get(key) !== lease) return;
  models.delete(key);
  lease.dispose_content_listener();
  lease.dispose_controller_listener();
  lease.dispose_controller_lifetime();
  lease.model.dispose();
}

function acquireModel(
  monaco: MonacoApi,
  key: string,
  controller: FileEditorController,
  language: string,
): Monaco.editor.ITextModel {
  const existing = models.get(key);
  if (existing) {
    if (existing.controller !== controller) {
      throw new Error("The canonical Monaco model belongs to another editor session.");
    }
    existing.references += 1;
    monaco.editor.setModelLanguage(existing.model, language);
    return existing.model;
  }
  const uri = monaco.Uri.from({
    scheme: "wardian-file",
    path: `/${encodeURIComponent(key)}`,
  });
  const current = controller.getSnapshot();
  const model = monaco.editor.getModel(uri)
    ?? monaco.editor.createModel(current.working_text, language, uri);
  const lease: ModelLease = {
    model,
    references: 1,
    controller,
    applying_controller_text: false,
    dispose_content_listener: () => undefined,
    dispose_controller_listener: () => undefined,
    dispose_controller_lifetime: () => undefined,
  };
  const contentListener = model.onDidChangeContent(() => {
    if (lease.applying_controller_text) return;
    controller.mutate(model.getValue());
  });
  lease.dispose_content_listener = () => contentListener.dispose();
  lease.dispose_controller_listener = controller.subscribe(() => {
    const text = controller.getSnapshot().working_text;
    if (model.getValue() === text) return;
    lease.applying_controller_text = true;
    try {
      model.setValue(text);
    } finally {
      lease.applying_controller_text = false;
    }
  });
  lease.dispose_controller_lifetime = controller.onDispose(() => disposeLease(key, lease));
  models.set(key, lease);
  if (model.getValue() !== current.working_text) {
    lease.applying_controller_text = true;
    try {
      model.setValue(current.working_text);
    } finally {
      lease.applying_controller_text = false;
    }
  }
  return model;
}

function releaseModel(key: string, model: Monaco.editor.ITextModel): void {
  const lease = models.get(key);
  if (!lease || lease.model !== model) return;
  lease.references = Math.max(0, lease.references - 1);
  // The controller registry owns model lifetime so hidden/re-rendered panes
  // keep undo history. The controller's exact release disposes the lease.
}

function errorMessage(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}

function canEditText(descriptor: FileRendererProps["snapshot"]["descriptor"]) {
  const mime = descriptor.mime_type.trim().toLowerCase();
  const validatedTextMime = mime.startsWith("text/")
    || mime === "application/json"
    || mime === "application/xml"
    || mime === "application/javascript"
    || mime === "image/svg+xml";
  return (descriptor.renderer_kind === "text" || descriptor.renderer_kind === "markdown" || validatedTextMime)
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

export default function MonacoTextRenderer({
  snapshot,
  lifecycle,
  surface_id,
  editor_controller,
  editor_language,
}: FileRendererProps) {
  const hostRef = useRef<HTMLDivElement>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [retryToken, setRetryToken] = useState(0);
  const retry = useCallback(() => {
    setLoadError(null);
    setRetryToken((value) => value + 1);
  }, []);
  const controller = editor_controller ?? null;
  const modelKey = snapshot.resource_id;
  const language = editor_language ?? "plaintext";
  const editorEligible = canEditText(snapshot.descriptor);

  useEffect(() => {
    if (!lifecycle.visible || !controller || !surface_id) return;
    if (!editorEligible) return;
    let cancelled = false;
    let editor: Monaco.editor.IStandaloneCodeEditor | null = null;
    let model: Monaco.editor.ITextModel | null = null;
    let observer: ResizeObserver | null = null;
    let themeObserver: MutationObserver | null = null;
    setLoadError(null);

    void loadMonaco().then((monaco) => {
      if (cancelled) return;
      const host = hostRef.current;
      if (!host) return;
      model = acquireModel(monaco, modelKey, controller, language);
      editor = monaco.editor.create(host, {
        automaticLayout: false,
        fontSize: 13,
        minimap: { enabled: false },
        model,
        readOnly: false,
        renderValidationDecorations: "off",
        scrollBeyondLastLine: false,
        theme: monacoTheme(),
        wordWrap: "off",
      });
      editor.addCommand(monaco.KeyMod.CtrlCmd | monaco.KeyCode.KeyS, async () => {
        setSaveError(null);
        try {
          await controller.save(surface_id);
          if (!cancelled) setSaveError(null);
        } catch (cause) {
          if (!cancelled) setSaveError(`Save failed: ${errorMessage(cause)}`);
        }
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
      if (!cancelled) setLoadError(errorMessage(cause));
    });

    return () => {
      cancelled = true;
      observer?.disconnect();
      themeObserver?.disconnect();
      editor?.dispose();
      if (model) releaseModel(modelKey, model);
    };
  }, [controller, editorEligible, language, lifecycle.visible, modelKey, retryToken, surface_id]);

  if (!lifecycle.visible) {
    return <div className="files-resource-state" role="status">Text editor suspended.</div>;
  }
  if (!controller || !surface_id || !editorEligible) {
    return (
      <div className="files-resource-state" role="status">
        {snapshot.descriptor.unavailable_reason ?? "text_editor_unavailable"}
      </div>
    );
  }
  if (loadError) {
    return (
      <section className="files-resource-state" role="alert">
        <h2>Text editor unavailable</h2>
        <p>{loadError}</p>
        <button type="button" onClick={retry}>Retry</button>
      </section>
    );
  }
  return (
    <div className="files-monaco-shell">
      <div ref={hostRef} className="files-monaco-renderer" data-testid="monaco-text-renderer" />
      {saveError ? (
        <div className="files-monaco-save-error" role="alert">{saveError}</div>
      ) : null}
    </div>
  );
}
