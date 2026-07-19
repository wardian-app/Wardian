import { useCallback, useEffect, useRef, useState } from "react";
import { ChevronDown, ChevronUp } from "lucide-react";
import type * as Monaco from "monaco-editor";

import {
  acquireCanonicalFileModel,
  loadFileMonaco,
  releaseCanonicalFileModel,
} from "../fileMonacoModels";
import {
  fileDiffDecorations,
  fileDiffForController,
  type FileLineChange,
} from "../fileDiffModel";
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

type ViewAnnotations = {
  setBaseline: (baseline: FileRendererProps["comparison_baseline"]) => void;
  dispose: () => void;
};

/** Owns annotations for one editor presentation while its writable model stays shared. */
function installViewAnnotations(
  editor: Monaco.editor.IStandaloneCodeEditor,
  controller: NonNullable<FileRendererProps["editor_controller"]>,
  initialBaseline: FileRendererProps["comparison_baseline"],
  onChanges: (changes: readonly FileLineChange[]) => void,
): ViewAnnotations {
  let zoneIds: string[] = [];
  let decorationIds: string[] = [];
  let signature = "";
  let disposed = false;
  let baseline = initialBaseline ?? null;
  const rebuild = () => {
    if (disposed) return;
    const snapshot = controller.getSnapshot();
    const baselineKey = baseline?.kind ?? "none";
    const nextSignature = `${baselineKey}:${snapshot.buffer_base_hash ?? ""}:${
      snapshot.base_revision ?? ""
    }:${snapshot.buffer_generation}`;
    if (nextSignature === signature) return;
    signature = nextSignature;
    const savedLines = snapshot.saved_text.split(/\r\n|\r|\n/);
    const diff = baseline?.kind === "saved_file"
      ? fileDiffForController(controller)
      : null;
    const changes = diff?.changes ?? [];
    decorationIds = editor.deltaDecorations(
      decorationIds,
      diff ? fileDiffDecorations(diff) : [],
    );
    onChanges(changes);
    const deletions = changes.filter((change) => change.kind === "deleted");
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
          afterLineNumber: change.modified_after_line ?? 0,
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
  return {
    setBaseline(nextBaseline) {
      baseline = nextBaseline ?? null;
      signature = "";
      rebuild();
    },
    dispose() {
      disposed = true;
      unsubscribe();
      decorationIds = editor.deltaDecorations(decorationIds, []);
      editor.changeViewZones((accessor) => {
        for (const zoneId of zoneIds) accessor.removeZone(zoneId);
        zoneIds = [];
      });
    },
  };
}

export default function MonacoTextRenderer({
  snapshot,
  lifecycle,
  surface_id,
  editor_controller,
  buffer_snapshot,
  editor_language,
  comparison_baseline,
}: FileRendererProps) {
  const hostRef = useRef<HTMLDivElement>(null);
  const editorRef = useRef<Monaco.editor.IStandaloneCodeEditor | null>(null);
  const annotationsRef = useRef<ViewAnnotations | null>(null);
  const baselineRef = useRef(comparison_baseline);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [changes, setChanges] = useState<readonly FileLineChange[]>([]);
  const [activeChangeIndex, setActiveChangeIndex] = useState(-1);
  const [changeAnnouncement, setChangeAnnouncement] = useState("");
  const [retryToken, setRetryToken] = useState(0);
  const retry = useCallback(() => {
    setLoadError(null);
    setRetryToken((value) => value + 1);
  }, []);
  const controller = editor_controller ?? null;
  const modelKey = snapshot.resource_id;
  const language = editor_language ?? "plaintext";
  const editorEligible = canEditText(snapshot.descriptor);
  const readOnly = buffer_snapshot?.read_only ?? false;
  const readOnlyRef = useRef(readOnly);
  readOnlyRef.current = readOnly;

  baselineRef.current = comparison_baseline;
  useEffect(() => {
    annotationsRef.current?.setBaseline(comparison_baseline);
  }, [comparison_baseline]);

  const updateChanges = useCallback((nextChanges: readonly FileLineChange[]) => {
    setChanges(nextChanges);
    setActiveChangeIndex(-1);
    setChangeAnnouncement("");
  }, []);

  const navigateChange = useCallback((direction: -1 | 1) => {
    if (changes.length === 0) return;
    const nextIndex = activeChangeIndex < 0
      ? direction > 0 ? 0 : changes.length - 1
      : (activeChangeIndex + direction + changes.length) % changes.length;
    const change = changes[nextIndex]!;
    const line = Math.max(1, change.modified_start_line);
    const editor = editorRef.current;
    editor?.revealLineInCenter(line);
    editor?.setPosition({ lineNumber: line, column: 1 });
    editor?.focus();
    setActiveChangeIndex(nextIndex);
    setChangeAnnouncement(
      `${change.kind} change ${nextIndex + 1} of ${changes.length}, line ${line}, against Saved file`,
    );
  }, [activeChangeIndex, changes]);
  const navigateChangeRef = useRef(navigateChange);
  navigateChangeRef.current = navigateChange;

  useEffect(() => {
    if (!lifecycle.visible || !controller || !surface_id) return;
    if (!editorEligible) return;
    let cancelled = false;
    let editor: Monaco.editor.IStandaloneCodeEditor | null = null;
    let model: Monaco.editor.ITextModel | null = null;
    let observer: ResizeObserver | null = null;
    let themeObserver: MutationObserver | null = null;
    let annotations: ViewAnnotations | null = null;
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
        readOnly: readOnlyRef.current,
        renderValidationDecorations: "off",
        scrollBeyondLastLine: false,
        theme: monacoTheme(),
        wordWrap: "off",
      });
      editorRef.current = editor;
      annotations = installViewAnnotations(editor, controller, baselineRef.current, updateChanges);
      annotationsRef.current = annotations;
      editor.addCommand(monaco.KeyCode.F7, () => navigateChangeRef.current(1));
      editor.addCommand(
        monaco.KeyMod.Shift | monaco.KeyCode.F7,
        () => navigateChangeRef.current(-1),
      );
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
      annotations?.dispose();
      if (annotationsRef.current === annotations) annotationsRef.current = null;
      editor?.dispose();
      if (editorRef.current === editor) editorRef.current = null;
      if (model) releaseCanonicalFileModel(modelKey, model);
    };
  }, [controller, editorEligible, language, lifecycle.visible, modelKey, retryToken, surface_id, updateChanges]);

  useEffect(() => {
    editorRef.current?.updateOptions({ readOnly });
  }, [readOnly]);

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
      {changes.length > 0 ? (
        <div className="files-monaco-change-navigation" role="group" aria-label="Saved file changes">
          <button
            type="button"
            aria-label="Previous Saved file change"
            title="Previous Saved file change"
            onClick={() => navigateChange(-1)}
          >
            <ChevronUp size={14} aria-hidden="true" />
          </button>
          <span role="status" aria-label="Current file change" aria-live="polite">
            {changeAnnouncement}
          </span>
          <button
            type="button"
            aria-label="Next Saved file change"
            title="Next Saved file change"
            onClick={() => navigateChange(1)}
          >
            <ChevronDown size={14} aria-hidden="true" />
          </button>
        </div>
      ) : null}
      {saveError ? (
        <div className="files-monaco-save-error" role="alert">{saveError}</div>
      ) : null}
    </div>
  );
}
