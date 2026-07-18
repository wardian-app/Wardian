import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";
import { invoke } from "@tauri-apps/api/core";

import type { CloseDecision, FilesSurfaceState, FilesSurfaceStateV2 } from "../../types";
import { useSettingsStore } from "../../store/useSettingsStore";
import { FilePreview } from "./FilePreview";
import { type FileResourceClient, fileResourceClient } from "./fileResourceClient";
import {
  type FileEditorSnapshot,
  FileEditorControllerRegistry,
} from "./fileEditorController";
import { decodeFileResourceKey } from "./fileResourceKey";
import { FilesModeBar } from "./FilesModeBar";
import {
  type FilesLegacyPresentationIntent,
  normalizeFilesSurfaceState,
} from "./filesSurfaceState";
import { useFilesPresentationStore } from "./filesPresentationStore";
import {
  defaultRendererRegistry,
  type FilePreviewPresentation,
  type RendererRegistry,
} from "./rendererRegistry";
import { useFileResource, type UseFileResourceResult } from "./useFileResource";
import "./FilesSurface.css";

export type FilesSurfaceProps = {
  surface_id: string;
  resource_key: string;
  state: FilesSurfaceState;
  lifecycle: { visible: boolean };
  editor_registry: FileEditorControllerRegistry;
  client?: FileResourceClient;
  registry?: RendererRegistry;
  on_canonical_resource?: (
    resource_key: string,
  ) => CloseDecision | void | Promise<CloseDecision | void>;
  on_open_file?: (path: string) => Promise<void> | void;
  on_open_with?: (path: string) => Promise<void> | void;
  on_reveal?: (path: string) => Promise<void> | void;
  on_state_change?: (state: FilesSurfaceStateV2) => Promise<void> | void;
  legacy_presentation_intent?: FilesLegacyPresentationIntent;
};

function pathFromResourceKey(resourceKey: string) {
  const decoded = decodeFileResourceKey(resourceKey);
  return decoded.resource_kind === "file" ? decoded.path : decoded.artifact_id;
}

async function openWithConfiguredEditor(path: string) {
  const settings = useSettingsStore.getState();
  await invoke("open_in_external_editor", {
    path,
    editor: {
      external_editor: settings.externalEditor,
      external_editor_custom_executable:
        settings.externalEditorCustomExecutable.trim() || null,
    },
  });
}

async function revealPath(path: string) {
  await invoke("reveal_in_explorer", { path });
}

type ActiveFilesSurfaceProps = Required<Pick<
  FilesSurfaceProps,
  "surface_id" | "resource_key" | "state" | "lifecycle" | "client" | "registry"
>> & {
  resource: UseFileResourceResult;
  on_open_file: (path: string) => Promise<void> | void;
  on_open_with: (path: string) => Promise<void> | void;
  on_reveal: (path: string) => Promise<void> | void;
  on_state_change: (state: FilesSurfaceStateV2) => Promise<void> | void;
  legacy_presentation_intent?: FilesLegacyPresentationIntent;
  action_error: string | null;
  editor_error: string | null;
  on_retry_editor: () => void;
  preview_presentation: FilePreviewPresentation;
  on_preview_presentation_change: (presentation: FilePreviewPresentation) => void;
};

type PreviewPresentationState = {
  resource_key: string;
  presentation: FilePreviewPresentation;
};

function ActiveFilesSurface(props: ActiveFilesSurfaceProps) {
  const requestedNormalization = useRef<string | null>(null);
  const resource = props.resource;
  const sourceAvailable = resource.status === "ready" && resource.snapshot
    ? props.registry.resolve(resource.snapshot.descriptor).source !== undefined
    : false;

  useEffect(() => {
    if (!("presentation" in props.state) || !resource.snapshot) return;
    const renderer = props.registry.resolve(resource.snapshot.descriptor);
    const textEditor = renderer.renderer_id === "text";
    const baselineAvailability = props.state.comparison_baseline === null
      ? "unavailable"
      : props.state.comparison_baseline.kind === "saved_file"
        ? renderer.capabilities.changes === "line" ? "available" : "unavailable"
        : "unknown";
    const normalized = normalizeFilesSurfaceState(props.state, {
      default_presentation: textEditor ? "editor" : "rendered",
      rendered: !textEditor || renderer.source !== undefined,
      editor: textEditor || renderer.source !== undefined,
      baseline_availability: baselineAvailability,
    }, {
      presentation_intent: props.legacy_presentation_intent,
    });
    const stateSnapshot = JSON.stringify(props.state);
    const normalizedSnapshot = JSON.stringify(normalized);
    if (props.legacy_presentation_intent === undefined && normalizedSnapshot === stateSnapshot) {
      requestedNormalization.current = null;
      return;
    }
    const requestSnapshot = JSON.stringify({
      surface_id: props.surface_id,
      resource_key: props.resource_key,
      state: stateSnapshot,
      normalized: normalizedSnapshot,
      legacy_presentation_intent: props.legacy_presentation_intent ?? null,
    });
    if (requestedNormalization.current === requestSnapshot) return;
    requestedNormalization.current = requestSnapshot;
    void props.on_state_change(normalized);
  }, [
    props.legacy_presentation_intent,
    props.on_state_change,
    props.registry,
    props.resource_key,
    props.state,
    props.surface_id,
    resource.snapshot,
  ]);

  return (
    <section className="files-surface" data-testid="files-surface">
      <FilesModeBar
        resource_key={props.resource_key}
        state={props.state}
        descriptor={resource.snapshot?.descriptor ?? null}
        preview_presentation={props.preview_presentation}
        source_available={sourceAvailable}
        on_preview_presentation_change={props.on_preview_presentation_change}
        on_open_with={props.on_open_with}
        on_reveal={props.on_reveal}
      />
      <main className="files-content-region" data-testid="files-content-region">
        {props.action_error ? (
          <div className="files-action-error" role="alert">{props.action_error}</div>
        ) : null}
        {props.editor_error ? (
          <section className="files-resource-state" role="alert">
            <p>{props.editor_error}</p>
            <button type="button" onClick={props.on_retry_editor}>Retry Editor</button>
          </section>
        ) : null}
        {resource.status === "loading" ? (
          <div className="files-resource-state" role="status">Loading preview…</div>
        ) : null}
        {resource.status === "error" ? (
          <section className="files-resource-state" role="alert">
            <h2>File unavailable</h2>
            <p>{resource.error?.message ?? "The file resource could not be loaded."}</p>
            <div className="files-resource-actions">
              <button type="button" onClick={() => void resource.retry()}>Retry</button>
              <button type="button" onClick={() => void props.on_open_with(pathFromResourceKey(props.resource_key))}>
                Open With
              </button>
              <button type="button" onClick={() => void props.on_reveal(pathFromResourceKey(props.resource_key))}>
                Reveal
              </button>
            </div>
          </section>
        ) : null}
        {resource.status === "ready" && resource.snapshot ? (
          <FilePreview
            snapshot={resource.snapshot}
            client={props.client}
            lifecycle={props.lifecycle}
            registry={props.registry}
            presentation={props.preview_presentation}
            on_open_file={props.on_open_file}
            on_open_with={props.on_open_with}
            on_reveal={props.on_reveal}
          />
        ) : null}
      </main>
    </section>
  );
}

export function FilesSurface({
  client = fileResourceClient,
  registry = defaultRendererRegistry,
  on_canonical_resource = () => "allow",
  on_open_file = () => undefined,
  on_open_with = openWithConfiguredEditor,
  on_reveal = revealPath,
  on_state_change = () => undefined,
  ...props
}: FilesSurfaceProps) {
  const [actionError, setActionError] = useState<string | null>(null);
  const [editorError, setEditorError] = useState<string | null>(null);
  const [editorRetryToken, setEditorRetryToken] = useState(0);
  const retryEditor = useCallback(() => {
    setEditorRetryToken((value) => value + 1);
  }, []);
  const guardedCanonicalResource = useCallback(async (resourceKey: string) => {
    try {
      const decision = await on_canonical_resource(resourceKey);
      setActionError(decision === "cancel"
        ? "File identity update was interrupted and will be retried."
        : null);
      return decision;
    } catch (error) {
      setActionError(`File identity update failed: ${error instanceof Error ? error.message : String(error)}`);
      return "cancel" as const;
    }
  }, [on_canonical_resource]);
  const notifiedCanonicalSnapshot = useRef<string | null>(null);
  const canonicalCallback = useRef(guardedCanonicalResource);
  const canonicalRetryCount = useRef(0);
  const [canonicalRetryToken, setCanonicalRetryToken] = useState(0);
  const stateCallback = useRef({ state: props.state, on_state_change });
  stateCallback.current = { state: props.state, on_state_change };
  const resource = useFileResource({
    path: pathFromResourceKey(props.resource_key),
    agent_id: null,
    user_file_capability_id: null,
  }, client);
  const editorController = useMemo(() => {
    const snapshot = resource.snapshot;
    if (!snapshot) return null;
    const definition = registry.resolve(snapshot.descriptor);
    const editable = snapshot.descriptor.encoding !== null
      && (definition.renderer_id === "text" || definition.source !== undefined);
    return editable ? props.editor_registry.forResource(snapshot.resource_id) : null;
  }, [props.editor_registry, registry, resource.snapshot]);
  const subscribeEditor = useCallback((listener: () => void) => (
    editorController?.subscribe(listener) ?? (() => undefined)
  ), [editorController]);
  const readEditorSnapshot = useCallback((): FileEditorSnapshot | null => (
    editorController?.getSnapshot() ?? null
  ), [editorController]);
  const editorSnapshot = useSyncExternalStore(
    subscribeEditor,
    readEditorSnapshot,
    readEditorSnapshot,
  );
  const [previewState, setPreviewState] = useState<PreviewPresentationState>({
    resource_key: props.resource_key,
    presentation: "presentation" in props.state && props.state.presentation === "editor"
      ? "source"
      : "rendered",
  });
  const previewPresentation = previewState.resource_key === props.resource_key
    ? previewState.presentation
    : "rendered";

  const updateControllerPresentation = useCallback((patch: Partial<FilesSurfaceStateV2>) => {
    const current = stateCallback.current;
    if (!("presentation" in current.state)) return;
    const next = { ...current.state, ...patch };
    if (JSON.stringify(next) === JSON.stringify(current.state)) return;
    stateCallback.current = { ...current, state: next };
    void current.on_state_change(next);
  }, []);

  useEffect(() => {
    if (!editorController) return;
    const resourceKey = editorController.getSnapshot().resource_key;
    const membership = editorController.attachPresentation(props.surface_id, {
      on_pin: () => updateControllerPresentation({ transient_preview: false }),
      on_open_comparison: () => updateControllerPresentation({ comparison_open: true }),
    });
    return () => {
      membership.detach();
      props.editor_registry.releaseAfterPostcommit(
        resourceKey,
        editorController.getSnapshot().presentation_generation,
      );
    };
  }, [editorController, props.editor_registry, props.surface_id, updateControllerPresentation]);

  useEffect(() => {
    const snapshot = resource.snapshot;
    if (!snapshot || !editorController) return;
    let active = true;
    void props.editor_registry.synchronizeAuthoritative(
      snapshot,
      () => client.readText(snapshot),
    ).then(() => {
      if (active) setEditorError(null);
    }).catch((error) => {
      if (active) {
        setEditorError(`File editor initialization failed: ${
          error instanceof Error ? error.message : String(error)
        }`);
      }
    });
    return () => { active = false; };
  }, [client, editorController, editorRetryToken, props.editor_registry, resource.snapshot]);

  useEffect(() => {
    useFilesPresentationStore.getState().setPresentation(props.surface_id, {
      resource_key: props.resource_key,
      descriptor: resource.snapshot?.descriptor ?? null,
      dirty: editorSnapshot?.dirty ?? false,
      attention: Boolean(
        editorSnapshot?.stale
        || editorSnapshot?.last_error
        || editorSnapshot?.recovery.status === "error"
        || editorError
      ),
    });
  }, [editorError, editorSnapshot, props.resource_key, props.surface_id, resource.snapshot]);

  useEffect(() => {
    canonicalCallback.current = guardedCanonicalResource;
  }, [guardedCanonicalResource]);

  useEffect(() => {
    const canonicalResourceKey = resource.snapshot?.resource_id;
    const subscriptionId = resource.snapshot?.subscription_id;
    if (!canonicalResourceKey || !subscriptionId) return;
    const notificationKey = `${subscriptionId}\0${canonicalResourceKey}`;
    if (notifiedCanonicalSnapshot.current === notificationKey) return;
    notifiedCanonicalSnapshot.current = notificationKey;
    let active = true;
    void Promise.resolve(canonicalCallback.current(canonicalResourceKey)).then((decision) => {
      if (!active) return;
      if (decision === "cancel") {
        if (notifiedCanonicalSnapshot.current === notificationKey) {
          notifiedCanonicalSnapshot.current = null;
        }
        if (canonicalRetryCount.current < 3) {
          canonicalRetryCount.current += 1;
          setCanonicalRetryToken((value) => value + 1);
        }
      } else {
        canonicalRetryCount.current = 0;
      }
    });
    return () => { active = false; };
  }, [canonicalRetryToken, resource.snapshot?.resource_id, resource.snapshot?.subscription_id]);

  useEffect(() => {
    setPreviewState({
      resource_key: props.resource_key,
      presentation: "presentation" in props.state && props.state.presentation === "editor"
        ? "source"
        : "rendered",
    });
  }, [props.resource_key, props.state]);

  const setPreviewPresentation = useCallback((presentation: FilePreviewPresentation) => {
    setPreviewState({ resource_key: props.resource_key, presentation });
    if ("presentation" in props.state) {
      void on_state_change({
        ...props.state,
        presentation: presentation === "source" ? "editor" : "rendered",
        transient_preview: false,
      });
    }
  }, [on_state_change, props.resource_key, props.state]);
  const guardedOpenWith = useCallback(async (path: string) => {
    try {
      await on_open_with(path);
      setActionError(null);
    } catch (error) {
      setActionError(`Open With failed: ${error instanceof Error ? error.message : String(error)}`);
    }
  }, [on_open_with]);
  const guardedReveal = useCallback(async (path: string) => {
    try {
      await on_reveal(path);
      setActionError(null);
    } catch (error) {
      setActionError(`Reveal failed: ${error instanceof Error ? error.message : String(error)}`);
    }
  }, [on_reveal]);

  if (!props.lifecycle.visible) {
    return (
      <section className="files-surface" data-testid="files-surface" data-suspended="true">
        <FilesModeBar
          resource_key={props.resource_key}
          state={props.state}
          descriptor={null}
          preview_presentation={previewPresentation}
          source_available={false}
          on_preview_presentation_change={setPreviewPresentation}
          on_open_with={guardedOpenWith}
          on_reveal={guardedReveal}
        />
        <main className="files-content-region" data-testid="files-content-region">
          {actionError ? (
            <div className="files-action-error" role="alert">{actionError}</div>
          ) : null}
          {editorError ? (
            <section className="files-resource-state" role="alert">
              <p>{editorError}</p>
              <button type="button" onClick={retryEditor}>Retry Editor</button>
            </section>
          ) : null}
          <div className="files-resource-state" role="status">Preview suspended while hidden.</div>
        </main>
      </section>
    );
  }
  return (
    <ActiveFilesSurface
      {...props}
      client={client}
      registry={registry}
      resource={resource}
      on_open_file={on_open_file}
      on_open_with={guardedOpenWith}
      on_reveal={guardedReveal}
      on_state_change={on_state_change}
      action_error={actionError}
      editor_error={editorError}
      on_retry_editor={retryEditor}
      preview_presentation={previewPresentation}
      on_preview_presentation_change={setPreviewPresentation}
    />
  );
}
