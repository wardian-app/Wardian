import { useCallback, useEffect, useRef, useState } from "react";
import type * as Monaco from "monaco-editor";

import {
  acquireCanonicalFileModel,
  loadFileMonaco,
  releaseCanonicalFileModel,
} from "../fileMonacoModels";
import { fileDiffForController } from "../fileDiffModel";
import type { FileRendererProps } from "../rendererRegistry";
const MONACO_MAX_SIZE_BYTES = 16 * 1024 * 1024;
const MONACO_MAX_LINE_COUNT = 200_000;

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

function installDeletedLineZones(
  editor: Monaco.editor.IStandaloneCodeEditor,
  controller: NonNullable<FileRendererProps["editor_controller"]>,
): () => void {
  let zoneIds: string[] = [];
  let signature = "";
  let disposed = false;
  const rebuild = () => {
    if (disposed) return;
    const snapshot = controller.getSnapshot();
    const nextSignature = `${snapshot.buffer_base_hash ?? ""}:${
      snapshot.base_revision ?? ""
    }:${snapshot.buffer_generation}`;
    if (nextSignature === signature) return;
    signature = nextSignature;
    const savedLines = snapshot.saved_text.split(/\r\n|\r|\n/);
    const deletions = fileDiffForController(controller)
      .changes.filter((change) => change.kind === "deleted");
    editor.changeViewZones((accessor) => {
      for (const zoneId of zoneIds) accessor.removeZone(zoneId);
      zoneIds = deletions.map((change) => {
        const deletedLines = savedLines.slice(
          (change.original_start_line ?? 1) - 1,
          change.original_end_line ?? change.original_start_line ?? 1,
        );
        const domNode = document.createElement("div");
        domNode.className = "files-diff-deleted-zone";
        const toggle = document.createElement("button");
        toggle.type = "button";
        toggle.className = "files-diff-deleted-toggle";
        toggle.setAttribute("aria-expanded", "true");
        const lineLabel = `${deletedLines.length} deleted ${deletedLines.length === 1 ? "line" : "lines"}`;
        toggle.textContent = lineLabel;
        toggle.setAttribute("aria-label", `Collapse ${lineLabel} since Saved file`);
        const body = document.createElement("pre");
        body.className = "files-diff-deleted-content";
        body.textContent = deletedLines.join("\n");
        domNode.append(toggle, body);
        const expandedHeight = Math.max(2, Math.min(9, deletedLines.length + 1));
        const zone: Monaco.editor.IViewZone = {
          afterLineNumber: Math.max(0, change.modified_start_line - 1),
          heightInLines: expandedHeight,
          domNode,
        };
        const zoneId = accessor.addZone(zone);
        toggle.addEventListener("click", () => {
          const expanded = toggle.getAttribute("aria-expanded") === "true";
          toggle.setAttribute("aria-expanded", expanded ? "false" : "true");
          toggle.setAttribute(
            "aria-label",
            `${expanded ? "Expand" : "Collapse"} ${lineLabel} since Saved file`,
          );
          body.hidden = expanded;
          zone.heightInLines = expanded ? 1 : expandedHeight;
          editor.changeViewZones((layoutAccessor) => layoutAccessor.layoutZone(zoneId));
        });
        return zoneId;
      });
    });
  };
  rebuild();
  const unsubscribe = controller.subscribe(rebuild);
  return () => {
    disposed = true;
    unsubscribe();
    editor.changeViewZones((accessor) => {
      for (const zoneId of zoneIds) accessor.removeZone(zoneId);
      zoneIds = [];
    });
  };
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
    let disposeDeletedLineZones: (() => void) | null = null;
    setLoadError(null);

    void loadFileMonaco().then((monaco) => {
      if (cancelled) return;
      const host = hostRef.current;
      if (!host) return;
      model = acquireCanonicalFileModel(monaco, modelKey, controller, language);
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
      disposeDeletedLineZones = installDeletedLineZones(editor, controller);
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
      disposeDeletedLineZones?.();
      editor?.dispose();
      if (model) releaseCanonicalFileModel(modelKey, model);
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
