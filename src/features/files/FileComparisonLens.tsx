import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";
import { X } from "lucide-react";
import type * as Monaco from "monaco-editor";

import type {
  FileRecoveryMergeResultV1,
  FilesComparisonBaseline,
  FilesSurfaceStateV2,
} from "../../types";
import type { FileEditorController } from "./fileEditorController";
import { buildFileDiffModel, fileDiffForController } from "./fileDiffModel";
import {
  acquireCanonicalFileModel,
  acquireFileBaselineModel,
  loadFileMonaco,
  releaseCanonicalFileModel,
  releaseFileBaselineModel,
} from "./fileMonacoModels";
import { resolveFilesComparisonLayout } from "./filesSurfaceState";

type ComparisonLayoutPreference = FilesSurfaceStateV2["comparison_layout_preference"];

export type FileComparisonLensProps = {
  controller: FileEditorController;
  surface_id: string;
  baseline: FilesComparisonBaseline;
  layout_preference: ComparisonLayoutPreference;
  language: string;
  lifecycle: { visible: boolean };
  on_close: () => void;
  on_layout_preference_change: (preference: ComparisonLayoutPreference) => void;
  on_reload_from_disk: () => Promise<void>;
  on_keep_working_buffer: () => void;
  on_merge: () => Promise<FileRecoveryMergeResultV1 | void>;
};

function theme(): string {
  return document.documentElement.getAttribute("data-theme") === "dark" ? "vs-dark" : "vs";
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function summaryLabel(summary: ReturnType<typeof buildFileDiffModel>["summary"]): string {
  const parts = [
    `${summary.regions} change ${summary.regions === 1 ? "region" : "regions"}`,
    `${summary.added_lines} added`,
    `${summary.modified_lines} modified`,
    `${summary.deleted_lines} deleted`,
  ];
  return parts.join(", ");
}

function baselineLabel(baseline: FilesComparisonBaseline): string {
  switch (baseline.kind) {
    case "saved_file": return "Saved file";
    case "prompt_checkpoint": return "Since last prompt";
    case "presented_version": return "Presented version";
    case "previous_presented_version": return "Previous presented version";
  }
}

/** Full source comparison that preserves the underlying file presentation. */
export function FileComparisonLens({
  controller,
  surface_id,
  baseline,
  layout_preference,
  language,
  lifecycle,
  on_close,
  on_layout_preference_change,
  on_reload_from_disk,
  on_keep_working_buffer,
  on_merge,
}: FileComparisonLensProps) {
  const rootRef = useRef<HTMLDivElement>(null);
  const hostRef = useRef<HTMLDivElement>(null);
  const diffEditorRef = useRef<Monaco.editor.IStandaloneDiffEditor | null>(null);
  const [contentWidth, setContentWidth] = useState<number | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [action, setAction] = useState<"merge" | "reload" | null>(null);
  const subscribe = useCallback((listener: () => void) => controller.subscribe(listener), [controller]);
  const getSnapshot = useCallback(() => controller.getSnapshot(), [controller]);
  const snapshot = useSyncExternalStore(subscribe, getSnapshot, getSnapshot);
  const authorizationUnavailable = snapshot.authorization.status === "unavailable";
  const authorizationUnavailableRef = useRef(authorizationUnavailable);
  authorizationUnavailableRef.current = authorizationUnavailable;
  const available = baseline.kind === "saved_file"
    && snapshot.status === "ready"
    && snapshot.resource_id !== null
    && snapshot.buffer_base_hash !== null;
  const diff = useMemo(
    () => available ? fileDiffForController(controller) : null,
    [available, controller, snapshot.base_revision, snapshot.buffer_base_hash, snapshot.buffer_generation],
  );
  const effectiveLayout = contentWidth === null
    ? null
    : resolveFilesComparisonLayout(layout_preference, contentWidth, "text");
  const measured = contentWidth !== null;

  useEffect(() => {
    const root = rootRef.current;
    if (!root || !lifecycle.visible) return;
    const observer = new ResizeObserver((entries) => {
      const entry = entries.find(({ target }) => target === root) ?? entries[0];
      const width = entry?.contentRect.width ?? root.clientWidth;
      if (width > 0) setContentWidth((current) => current === width ? current : width);
    });
    observer.observe(root);
    return () => observer.disconnect();
  }, [lifecycle.visible]);

  useEffect(() => {
    if (!available || !lifecycle.visible || !measured) return;
    const host = hostRef.current;
    if (!host) return;
    let cancelled = false;
    let diffEditor: Monaco.editor.IStandaloneDiffEditor | null = null;
    let original: Monaco.editor.ITextModel | null = null;
    let modified: Monaco.editor.ITextModel | null = null;
    let observer: ResizeObserver | null = null;
    let themeObserver: MutationObserver | null = null;
    const resourceId = snapshot.resource_id!;
    const baselineKey = `${resourceId}\0saved_file\0${snapshot.buffer_base_hash}`;
    setLoadError(null);

    void loadFileMonaco().then((monaco) => {
      if (cancelled) return;
      original = acquireFileBaselineModel(
        monaco,
        baselineKey,
        snapshot.saved_text,
        language,
      );
      modified = acquireCanonicalFileModel(monaco, resourceId, controller, language);
      diffEditor = monaco.editor.createDiffEditor(host, {
        automaticLayout: false,
        fontSize: 13,
        minimap: { enabled: false },
        originalEditable: false,
        readOnly: authorizationUnavailableRef.current,
        renderSideBySide: resolveFilesComparisonLayout(
          layout_preference,
          contentWidth!,
          "text",
        ) === "side_by_side",
        useInlineViewWhenSpaceIsLimited: false,
        renderValidationDecorations: "off",
        scrollBeyondLastLine: false,
        theme: theme(),
        wordWrap: "off",
      });
      diffEditorRef.current = diffEditor;
      diffEditor.setModel({ original, modified });
      diffEditor.getModifiedEditor().addCommand(
        monaco.KeyMod.CtrlCmd | monaco.KeyCode.KeyS,
        async () => {
          setActionError(null);
          try {
            await controller.save(surface_id);
            if (!cancelled) setActionError(null);
          } catch (error) {
            if (!cancelled) setActionError(`Save failed: ${errorMessage(error)}`);
          }
        },
      );
      themeObserver = new MutationObserver(() => monaco.editor.setTheme(theme()));
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
        diffEditor?.layout({ width, height });
      });
      observer.observe(host);
    }).catch((error) => {
      if (!cancelled) setLoadError(errorMessage(error));
    });

    return () => {
      cancelled = true;
      observer?.disconnect();
      themeObserver?.disconnect();
      diffEditor?.dispose();
      if (diffEditorRef.current === diffEditor) diffEditorRef.current = null;
      if (modified) releaseCanonicalFileModel(resourceId, modified);
      if (original) releaseFileBaselineModel(baselineKey, original);
    };
  }, [
    available,
    controller,
    language,
    lifecycle.visible,
    measured,
    snapshot.buffer_base_hash,
    snapshot.resource_id,
    snapshot.saved_text,
    surface_id,
  ]);

  useEffect(() => {
    if (effectiveLayout === null) return;
    diffEditorRef.current?.updateOptions({
      renderSideBySide: effectiveLayout === "side_by_side",
    });
  }, [effectiveLayout]);

  useEffect(() => {
    diffEditorRef.current?.updateOptions({ readOnly: authorizationUnavailable });
  }, [authorizationUnavailable]);

  const runReload = useCallback(async () => {
    setAction("reload");
    setActionError(null);
    try {
      await on_reload_from_disk();
      on_close();
    } catch (error) {
      setActionError(`Reload from disk failed: ${errorMessage(error)}`);
    } finally {
      setAction(null);
    }
  }, [on_close, on_reload_from_disk]);
  const runMerge = useCallback(async () => {
    setAction("merge");
    setActionError(null);
    try {
      const result = await on_merge();
      if (result?.status === "conflicted") {
        setActionError("Merge has overlapping changes. Your working buffer was preserved.");
      }
    } catch (error) {
      setActionError(`Merge failed: ${errorMessage(error)}`);
    } finally {
      setAction(null);
    }
  }, [on_merge]);
  const keepWorking = useCallback(() => {
    on_keep_working_buffer();
    on_close();
  }, [on_close, on_keep_working_buffer]);

  return (
    <section className="files-comparison-lens" aria-label="File comparison">
      <header className="files-comparison-toolbar">
        <span className="files-comparison-baseline">{baselineLabel(baseline)}</span>
        {diff ? (
          <span className="files-comparison-summary" aria-label="Changes summary">
            {summaryLabel(diff.summary)}
          </span>
        ) : null}
        {available ? <label className="files-comparison-layout">
          <span className="files-visually-hidden">Comparison layout</span>
          <select
            aria-label="Comparison layout"
            value={layout_preference}
            title={layout_preference === "side_by_side" && effectiveLayout === "unified"
              ? "Side by side needs at least 560 px; unified is active for this pane."
              : "Comparison layout"}
            onChange={(event) => on_layout_preference_change(
              event.currentTarget.value as ComparisonLayoutPreference,
            )}
          >
            <option value="auto">Auto</option>
            <option value="side_by_side">Side by side</option>
            <option value="unified">Unified</option>
          </select>
        </label> : null}
        <button type="button" className="files-comparison-close" aria-label="Close comparison" onClick={on_close}>
          <X size={16} strokeWidth={1.75} aria-hidden="true" />
        </button>
      </header>
      {available && snapshot.stale ? (
        <div className="files-stale-actions" role="group" aria-label="Resolve external changes">
          <span>The saved file changed on disk.</span>
          <button type="button" disabled={action !== null || authorizationUnavailable} onClick={() => void runMerge()}>Merge</button>
          <button type="button" disabled={action !== null || authorizationUnavailable} onClick={() => void runReload()}>Reload from disk</button>
          <button type="button" disabled={action !== null} onClick={keepWorking}>Cancel</button>
        </div>
      ) : null}
      {actionError ? <div className="files-comparison-error" role="alert">{actionError}</div> : null}
      {baseline.kind !== "saved_file" ? (
        <div className="files-resource-state" role="status">
          Comparison baseline unavailable. This baseline provider is not registered yet.
        </div>
      ) : !available ? (
        <div className="files-resource-state" role="status">Comparison baseline unavailable.</div>
      ) : loadError ? (
        <div className="files-resource-state" role="alert">Comparison unavailable: {loadError}</div>
      ) : (
        <div ref={rootRef} className="files-comparison-body" data-layout={effectiveLayout ?? "measuring"}>
          <div ref={hostRef} className="files-comparison-editor" data-testid="file-comparison-editor" />
        </div>
      )}
    </section>
  );
}
