import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

import type { CloseDecision, FilesSurfaceState, FilesSurfaceStateV2 } from "../../types";
import { useSettingsStore } from "../../store/useSettingsStore";
import { FilePreview } from "./FilePreview";
import { type FileResourceClient, fileResourceClient } from "./fileResourceClient";
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
import { useFileResource } from "./useFileResource";
import "./FilesSurface.css";

export type FilesSurfaceProps = {
  surface_id: string;
  resource_key: string;
  state: FilesSurfaceState;
  lifecycle: { visible: boolean };
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
  on_canonical_resource: (
    resource_key: string,
  ) => CloseDecision | void | Promise<CloseDecision | void>;
  on_open_file: (path: string) => Promise<void> | void;
  on_open_with: (path: string) => Promise<void> | void;
  on_reveal: (path: string) => Promise<void> | void;
  on_state_change: (state: FilesSurfaceStateV2) => Promise<void> | void;
  legacy_presentation_intent?: FilesLegacyPresentationIntent;
  action_error: string | null;
  preview_presentation: FilePreviewPresentation;
  on_preview_presentation_change: (presentation: FilePreviewPresentation) => void;
};

type PreviewPresentationState = {
  resource_key: string;
  presentation: FilePreviewPresentation;
};

function ActiveFilesSurface(props: ActiveFilesSurfaceProps) {
  const notifiedCanonicalSnapshot = useRef<string | null>(null);
  const requestedNormalization = useRef<string | null>(null);
  const canonicalCallback = useRef(props.on_canonical_resource);
  const canonicalRetryCount = useRef(0);
  const [canonicalRetryToken, setCanonicalRetryToken] = useState(0);
  const resource = useFileResource({
    path: pathFromResourceKey(props.resource_key),
    agent_id: null,
    user_file_capability_id: null,
  }, props.client);
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

  useEffect(() => {
    canonicalCallback.current = props.on_canonical_resource;
  }, [props.on_canonical_resource]);

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
    useFilesPresentationStore.getState().setPresentation(props.surface_id, {
      resource_key: props.resource_key,
      descriptor: resource.snapshot?.descriptor ?? null,
      dirty: false,
      attention: false,
    });
  }, [props.resource_key, props.surface_id, resource.snapshot]);

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
  const [previewState, setPreviewState] = useState<PreviewPresentationState>({
    resource_key: props.resource_key,
    presentation: "presentation" in props.state && props.state.presentation === "editor"
      ? "source"
      : "rendered",
  });
  const previewPresentation = previewState.resource_key === props.resource_key
    ? previewState.presentation
    : "rendered";

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
      on_canonical_resource={guardedCanonicalResource}
      on_open_file={on_open_file}
      on_open_with={guardedOpenWith}
      on_reveal={guardedReveal}
      on_state_change={on_state_change}
      action_error={actionError}
      preview_presentation={previewPresentation}
      on_preview_presentation_change={setPreviewPresentation}
    />
  );
}
