import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";
import { invoke } from "@tauri-apps/api/core";

import type {
  CloseDecision,
  FileRecoveryV1,
  FilesComparisonBaseline,
  FilesSurfaceState,
  FilesSurfaceStateV2,
} from "../../types";
import { useSettingsStore } from "../../store/useSettingsStore";
import { FileContentHost } from "./FileContentHost";
import { FileComparisonLens } from "./FileComparisonLens";
import { type FileResourceClient, fileResourceClient } from "./fileResourceClient";
import {
  type FileEditorSnapshot,
  FileEditorControllerRegistry,
} from "./fileEditorController";
import { decodeFileResourceKey } from "./fileResourceKey";
import { FilesHeader } from "./FilesHeader";
import { fileDiffForController } from "./fileDiffModel";
import {
  type FilesLegacyPresentationIntent,
  normalizeFilesSurfaceState,
} from "./filesSurfaceState";
import { useFilesPresentationStore } from "./filesPresentationStore";
import {
  defaultRendererRegistry,
  type FileContentPresentation,
  type FileEditorBufferSnapshot,
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
  editor_recovery_conflict: Extract<
    FileEditorSnapshot["recovery"],
    { status: "conflict" }
  > | null;
  resolving_recovery_conflict: boolean;
  restoring_access: boolean;
  on_retry_editor: () => void;
  on_resolve_recovery_conflict: (choice: "keep_current" | "use_recovered") => Promise<void>;
  recovery_only: RecoveryOnlyState;
  on_restore_access: () => void;
  on_discard_editor_recovery: () => Promise<void>;
  on_discard_recovery: () => void;
  on_retry_recovery: () => void;
  editor_controller: ReturnType<FileEditorControllerRegistry["forResource"]> | null;
  editor_snapshot: FileEditorSnapshot | null;
  presentation: FileContentPresentation;
  on_presentation_change: (presentation: FileContentPresentation) => void;
  on_save: () => Promise<void>;
  on_save_as: () => Promise<void>;
};

type PresentationState = {
  resource_key: string;
  presentation: FileContentPresentation;
};

type RecoveryOnlyState =
  | { status: "idle" | "loading" | "none" }
  | {
      status: "ready";
      recovery: FileRecoveryV1;
      operation: "idle" | "restoring" | "discarding";
    }
  | { status: "error"; message: string };

async function loadNewestRecovery(
  client: FileResourceClient,
  resourceKey: string,
): Promise<FileRecoveryV1 | null> {
  const recoveries = await client.listRecoveries({ resource_key: resourceKey });
  const newest = [...recoveries].sort((left, right) => (
    right.updated_at_ms - left.updated_at_ms
    || right.recovery_revision - left.recovery_revision
    || right.recovery_id.localeCompare(left.recovery_id)
  ))[0];
  if (!newest) return null;
  const recovered = await client.getRecovery({
    recovery_id: newest.recovery_id,
    resource_key: resourceKey,
  });
  if (
    recovered.resource_key !== resourceKey
    || recovered.recovery_id !== newest.recovery_id
  ) throw new Error("The recovered buffer did not match this Files surface.");
  return recovered;
}

function ActiveFilesSurface(props: ActiveFilesSurfaceProps) {
  const requestedNormalization = useRef<string | null>(null);
  const resource = props.resource;
  const renderer = resource.status === "ready" && resource.snapshot
    ? props.registry.resolve(resource.snapshot.descriptor)
    : null;
  const activePresentation = renderer
    ? props.presentation === "editor" && renderer.editor
      ? "editor"
      : props.presentation === "rendered" && renderer.rendered
        ? "rendered"
        : renderer.default_presentation ?? (renderer.editor ? "editor" : "rendered")
    : props.presentation;
  const presentationToggleAvailable = Boolean(renderer?.rendered && renderer.editor);
  const editorReady = props.editor_snapshot?.status === "ready";
  const editorAvailable = Boolean(renderer?.editor && props.editor_controller && editorReady);
  const authorizationUnavailable = props.editor_snapshot?.authorization.status === "unavailable";
  const editorInitializing = Boolean(
    activePresentation === "editor"
    &&
    renderer?.editor
    && props.editor_controller
    && !editorReady
    && !props.editor_error,
  );
  const editorBlocked = Boolean(
    editorInitializing
    || props.editor_error
    || props.editor_recovery_conflict,
  );
  const bufferSnapshot = useMemo<FileEditorBufferSnapshot | null>(() => {
    const editor = props.editor_snapshot;
    if (!editor || editor.status !== "ready" || editor.resource_id === null) return null;
    return Object.freeze({
      resource_id: editor.resource_id,
      revision: editor.disk_head_revision ?? resource.snapshot?.revision ?? 0,
      buffer_generation: editor.buffer_generation,
      text: editor.working_text,
      dirty: editor.dirty,
      read_only: editor.authorization.status === "unavailable",
    });
  }, [props.editor_snapshot, resource.snapshot?.revision]);
  const stateV2 = "presentation" in props.state ? props.state : null;
  const inlineComparisonBaseline = useMemo<FilesComparisonBaseline | null>(() => {
    if (!stateV2) return null;
    if (stateV2.comparison_baseline?.kind === "saved_file") {
      return stateV2.comparison_baseline;
    }
    // Historical baselines require their own provider. Never substitute saved bytes.
    if (stateV2.comparison_baseline !== null) return null;
    if (
      stateV2.resource_kind === "file"
      && props.editor_snapshot?.status === "ready"
      && props.editor_snapshot.dirty
    ) return { kind: "saved_file" };
    return null;
  }, [props.editor_snapshot?.dirty, props.editor_snapshot?.status, stateV2]);
  const savedFileDiff = useMemo(() => (
    inlineComparisonBaseline?.kind === "saved_file" && props.editor_snapshot?.status === "ready"
      ? fileDiffForController(props.editor_controller!)
      : null
  ), [
    props.editor_controller,
    props.editor_snapshot?.base_revision,
    props.editor_snapshot?.buffer_base_hash,
    props.editor_snapshot?.buffer_generation,
    props.editor_snapshot?.status,
    inlineComparisonBaseline,
  ]);
  const comparisonOpen = Boolean(stateV2?.comparison_open && stateV2.comparison_baseline);
  const savedComparisonOpen = Boolean(
    stateV2?.comparison_open && stateV2.comparison_baseline?.kind === "saved_file",
  );
  const updateComparisonState = useCallback((patch: Partial<FilesSurfaceStateV2>) => {
    if (!stateV2) return;
    void props.on_state_change({ ...stateV2, ...patch });
  }, [props.on_state_change, stateV2]);
  const toggleSavedComparison = useCallback(() => {
    if (!stateV2) return;
    if (savedComparisonOpen) {
      updateComparisonState({ comparison_open: false });
      return;
    }
    updateComparisonState({
      comparison_open: true,
      comparison_baseline: { kind: "saved_file" },
    });
  }, [savedComparisonOpen, stateV2, updateComparisonState]);

  useEffect(() => {
    if (!("presentation" in props.state) || !resource.snapshot) return;
    const renderer = props.registry.resolve(resource.snapshot.descriptor);
    const baselineAvailability = props.state.comparison_baseline === null
      ? "unavailable"
      : props.state.comparison_baseline.kind === "saved_file"
        ? renderer.capabilities.changes === "line" ? "available" : "unavailable"
        : "unknown";
    const normalized = normalizeFilesSurfaceState(props.state, {
      default_presentation: renderer.default_presentation ?? "rendered",
      rendered: renderer.rendered !== undefined,
      editor: renderer.editor !== undefined,
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
      <FilesHeader
        resource_key={props.resource_key}
        descriptor={resource.snapshot?.descriptor ?? null}
        presentation={activePresentation}
        presentation_toggle_available={presentationToggleAvailable && !editorBlocked}
        dirty={props.editor_snapshot?.dirty ?? false}
        save_available={editorAvailable && !editorBlocked}
        save_disabled={authorizationUnavailable}
        save_as_available={editorAvailable && !editorBlocked}
        saving={props.editor_snapshot?.save_state === "saving"}
        changes={savedFileDiff?.summary ?? null}
        comparison_open={savedComparisonOpen}
        resource_actions_available={props.recovery_only.status !== "ready"}
        on_presentation_change={props.on_presentation_change}
        on_comparison_toggle={toggleSavedComparison}
        on_save={props.on_save}
        on_save_as={props.on_save_as}
        on_open_with={props.on_open_with}
        on_reveal={props.on_reveal}
      />
      <main className="files-content-region" data-testid="files-content-region">
        {props.action_error ? (
          <div className="files-action-error" role="alert">{props.action_error}</div>
        ) : null}
        {authorizationUnavailable ? (
          <section
            className="files-resource-state files-access-unavailable"
            role="alert"
            aria-label="File access unavailable"
          >
            <h2>File access unavailable</h2>
            <p>
              Unsaved content is read-only until Wardian verifies access to this exact file again.
            </p>
            <div className="files-resource-actions">
              <button
                type="button"
                disabled={props.restoring_access}
                onClick={props.on_restore_access}
              >
                Restore access
              </button>
              {props.editor_snapshot?.dirty ? (
                <button
                  type="button"
                  disabled={props.restoring_access}
                  onClick={() => void props.on_discard_editor_recovery()}
                >
                  Discard unsaved changes
                </button>
              ) : null}
            </div>
          </section>
        ) : null}
        {props.editor_error ? (
          <section className="files-resource-state files-content-blocker" role="alert">
            <p>{props.editor_error}</p>
            <button type="button" onClick={props.on_retry_editor}>Retry Editor</button>
          </section>
        ) : null}
        {!props.editor_error && props.editor_recovery_conflict ? (
          <section className="files-resource-state files-content-blocker" role="alert">
            <h2>Recovery conflict</h2>
            <p>{props.editor_recovery_conflict.message}</p>
            <p>
              Current edits are checkpointed separately. Choose which version to continue
              editing; the rejected recovery is discarded after the choice is durable.
            </p>
            <div className="files-resource-actions">
              <button
                type="button"
                disabled={
                  !props.editor_recovery_conflict.current_durable
                  || props.resolving_recovery_conflict
                }
                onClick={() => void props.on_resolve_recovery_conflict("keep_current")}
              >
                Keep current edits
              </button>
              <button
                type="button"
                disabled={
                  !props.editor_recovery_conflict.current_durable
                  || props.resolving_recovery_conflict
                }
                onClick={() => void props.on_resolve_recovery_conflict("use_recovered")}
              >
                Use recovered edits
              </button>
            </div>
          </section>
        ) : null}
        {editorInitializing ? (
          <div className="files-resource-state files-content-blocker" role="status">
            Preparing editor…
          </div>
        ) : null}
        {resource.status === "loading" ? (
          <div className="files-resource-state files-content-blocker" role="status">
            Loading preview…
          </div>
        ) : null}
        {resource.status === "error" && props.recovery_only.status === "ready" ? (
          <section
            className="files-resource-state files-content-blocker"
            aria-label="Read-only recovery"
          >
            <h2>Recovered unsaved changes</h2>
            <p>
              File access is unavailable. The recovered content is read-only until access is
              restored.
            </p>
            <pre aria-label="Recovered buffer">{props.recovery_only.recovery.buffer}</pre>
            <details>
              <summary>Saved base</summary>
              <pre aria-label="Recovered saved base">{props.recovery_only.recovery.base}</pre>
            </details>
            <div className="files-resource-actions">
              <button
                type="button"
                disabled={props.recovery_only.operation !== "idle"}
                onClick={props.on_restore_access}
              >
                Restore access
              </button>
              <button
                type="button"
                disabled={props.recovery_only.operation !== "idle"}
                onClick={props.on_discard_recovery}
              >
                Discard recovery
              </button>
            </div>
          </section>
        ) : null}
        {resource.status === "error" && props.recovery_only.status !== "ready" ? (
          <section className="files-resource-state files-content-blocker" role="alert">
            <h2>File unavailable</h2>
            <p>{resource.error?.message ?? "The file resource could not be loaded."}</p>
            {props.recovery_only.status === "loading" ? (
              <p role="status">Looking for recovered unsaved changes…</p>
            ) : null}
            {props.recovery_only.status === "error" ? (
              <p>
                Recovery could not be checked: {props.recovery_only.message}
              </p>
            ) : null}
            <div className="files-resource-actions">
              <button type="button" onClick={() => void resource.retry()}>Retry</button>
              {props.recovery_only.status === "error" ? (
                <button type="button" onClick={props.on_retry_recovery}>Retry recovery</button>
              ) : null}
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
          <div
            className="files-content-host-shell"
            data-testid="files-content-host-shell"
            data-blocked={editorBlocked ? "true" : "false"}
            data-comparison-open={comparisonOpen ? "true" : "false"}
            aria-hidden={editorBlocked || comparisonOpen ? true : undefined}
            inert={editorBlocked || comparisonOpen ? true : undefined}
          >
            <FileContentHost
              snapshot={resource.snapshot}
              client={props.client}
              lifecycle={props.lifecycle}
              registry={props.registry}
              presentation={activePresentation}
              surface_id={props.surface_id}
              editor_controller={props.editor_controller}
              buffer_snapshot={bufferSnapshot}
              comparison_baseline={inlineComparisonBaseline}
              on_open_file={props.on_open_file}
              on_open_with={props.on_open_with}
              on_reveal={props.on_reveal}
            />
          </div>
        ) : null}
        {comparisonOpen && stateV2?.comparison_baseline ? (
          props.editor_controller && renderer?.capabilities.changes === "line" ? (
            <FileComparisonLens
              controller={props.editor_controller}
              surface_id={props.surface_id}
              baseline={stateV2.comparison_baseline}
              layout_preference={stateV2.comparison_layout_preference}
              language={renderer.editor_language?.(resource.snapshot!.descriptor) ?? "plaintext"}
              lifecycle={props.lifecycle}
              on_close={() => updateComparisonState({ comparison_open: false })}
              on_layout_preference_change={(preference) => updateComparisonState({
                comparison_layout_preference: preference,
              })}
              on_reload_from_disk={() => props.editor_controller!.reloadFromDisk()}
              on_keep_working_buffer={() => props.editor_controller!.keepWorkingBuffer()}
              on_merge={() => props.editor_controller!.mergeStaleBuffer()}
            />
          ) : (
            <section className="files-comparison-lens" aria-label="File comparison">
              <div className="files-resource-state" role="status">
                Comparison baseline unavailable. This renderer does not have a registered
                comparison provider.
              </div>
              <button
                type="button"
                className="files-comparison-unavailable-close"
                onClick={() => updateComparisonState({ comparison_open: false })}
              >
                Close comparison
              </button>
            </section>
          )
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
  const [recoveryOnlyRetryToken, setRecoveryOnlyRetryToken] = useState(0);
  const [recoveryOnlyState, setRecoveryOnlyState] = useState<RecoveryOnlyState>({
    status: "idle",
  });
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
  const recoveryOnlyContext = useRef({
    resource_key: props.resource_key,
    status: resource.status,
    client,
  });
  recoveryOnlyContext.current = {
    resource_key: props.resource_key,
    status: resource.status,
    client,
  };
  const recoveryOnlyOperationGeneration = useRef(0);
  const recoveryOnlyActionPending = useRef(false);
  const [restoringAccess, setRestoringAccess] = useState(false);
  useEffect(() => {
    const generation = ++recoveryOnlyOperationGeneration.current;
    const resourceKey = props.resource_key;
    const operationClient = client;
    const ownsOperation = () => {
      const context = recoveryOnlyContext.current;
      return recoveryOnlyOperationGeneration.current === generation
        && context.resource_key === resourceKey
        && context.status === "error"
        && context.client === operationClient;
    };
    recoveryOnlyActionPending.current = false;
    setRestoringAccess(false);
    if (resource.status !== "error") {
      setRecoveryOnlyState({ status: "idle" });
      return () => {
        recoveryOnlyOperationGeneration.current += 1;
        recoveryOnlyActionPending.current = false;
      };
    }
    setRecoveryOnlyState({ status: "loading" });
    void loadNewestRecovery(client, props.resource_key).then((recovered) => {
      if (!ownsOperation()) return;
      if (!recovered) {
        setRecoveryOnlyState({ status: "none" });
        return;
      }
      setRecoveryOnlyState({ status: "ready", recovery: recovered, operation: "idle" });
    }).catch((error) => {
      if (!ownsOperation()) return;
      setRecoveryOnlyState({
        status: "error",
        message: error instanceof Error ? error.message : String(error),
      });
    });
    return () => {
      recoveryOnlyOperationGeneration.current += 1;
      recoveryOnlyActionPending.current = false;
    };
  }, [client, props.resource_key, recoveryOnlyRetryToken, resource.status]);
  const editorController = useMemo(() => {
    const snapshot = resource.snapshot;
    if (!snapshot) return null;
    const definition = registry.resolve(snapshot.descriptor);
    const editable = snapshot.descriptor.encoding !== null
      && definition.editor !== undefined;
    return editable ? props.editor_registry.forResource(snapshot.resource_id) : null;
  }, [props.editor_registry, registry, resource.snapshot]);
  const editorControllerContext = useRef(editorController);
  editorControllerContext.current = editorController;
  const recoveryConflictOperationGeneration = useRef(0);
  const recoveryConflictOperationPending = useRef(false);
  const [resolvingRecoveryConflict, setResolvingRecoveryConflict] = useState(false);
  useEffect(() => {
    recoveryConflictOperationGeneration.current += 1;
    recoveryConflictOperationPending.current = false;
    setResolvingRecoveryConflict(false);
    return () => {
      recoveryConflictOperationGeneration.current += 1;
      recoveryConflictOperationPending.current = false;
    };
  }, [editorController]);
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
  const [presentationState, setPresentationState] = useState<PresentationState>({
    resource_key: props.resource_key,
    presentation: "presentation" in props.state && props.state.presentation === "editor"
      ? "editor"
      : "rendered",
  });
  const presentation = presentationState.resource_key === props.resource_key
    ? presentationState.presentation
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
      on_open_comparison: () => {
        const current = stateCallback.current.state;
        if (!("presentation" in current)) return;
        updateControllerPresentation({
          comparison_open: true,
          comparison_baseline: { kind: "saved_file" },
        });
      },
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
      props.surface_id,
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

  const restoreAccess = useCallback(() => {
    if (
      recoveryOnlyActionPending.current
      || restoringAccess
      || recoveryOnlyState.status === "ready" && recoveryOnlyState.operation !== "idle"
    ) return;
    recoveryOnlyActionPending.current = true;
    const generation = ++recoveryOnlyOperationGeneration.current;
    const resourceKey = props.resource_key;
    const operationClient = client;
    const recovered = recoveryOnlyState.status === "ready"
      ? recoveryOnlyState.recovery
      : null;
    setRestoringAccess(true);
    if (recovered) {
      setRecoveryOnlyState({ status: "ready", recovery: recovered, operation: "restoring" });
    }
    const finish = () => {
      const context = recoveryOnlyContext.current;
      if (
        recoveryOnlyOperationGeneration.current !== generation
        || context.resource_key !== resourceKey
        || context.status !== "error"
        || context.client !== operationClient
      ) return;
      recoveryOnlyActionPending.current = false;
      setRestoringAccess(false);
      if (recovered) {
        setRecoveryOnlyState((current) => (
          current.status === "ready" && current.recovery.recovery_id === recovered.recovery_id
            ? { status: "ready", recovery: current.recovery, operation: "idle" }
            : current
        ));
      }
    };
    void resource.retry().then(finish, finish);
  }, [client, props.resource_key, recoveryOnlyState, resource, restoringAccess]);
  const resolveRecoveryConflict = useCallback(async (
    choice: "keep_current" | "use_recovered",
  ) => {
    if (!editorController || recoveryConflictOperationPending.current) return;
    const generation = ++recoveryConflictOperationGeneration.current;
    const operationController = editorController;
    const ownsOperation = () => (
      recoveryConflictOperationGeneration.current === generation
      && editorControllerContext.current === operationController
    );
    recoveryConflictOperationPending.current = true;
    setResolvingRecoveryConflict(true);
    try {
      await editorController.resolveRecoveryConflict(choice);
      if (ownsOperation()) {
        setEditorError(null);
      }
    } catch (error) {
      if (ownsOperation()) {
        setEditorError(`Recovery conflict resolution failed: ${
          error instanceof Error ? error.message : String(error)
        }`);
      }
    } finally {
      if (ownsOperation()) {
        recoveryConflictOperationPending.current = false;
        setResolvingRecoveryConflict(false);
      }
    }
  }, [editorController]);
  const discardRecovery = useCallback(() => {
    if (
      recoveryOnlyActionPending.current
      || restoringAccess
      || recoveryOnlyState.status !== "ready"
      || recoveryOnlyState.operation !== "idle"
    ) return;
    recoveryOnlyActionPending.current = true;
    const recovered = recoveryOnlyState.recovery;
    const resourceKey = props.resource_key;
    const operationClient = client;
    const generation = ++recoveryOnlyOperationGeneration.current;
    const ownsOperation = () => {
      const context = recoveryOnlyContext.current;
      return recoveryOnlyOperationGeneration.current === generation
        && context.resource_key === resourceKey
        && context.status === "error"
        && context.client === operationClient;
    };
    setRecoveryOnlyState({ status: "ready", recovery: recovered, operation: "discarding" });
    void client.discardRecovery({
      recovery_id: recovered.recovery_id,
      expected_recovery_revision: recovered.recovery_revision,
      resource_key: resourceKey,
    }).then(async () => {
      const next = await loadNewestRecovery(operationClient, resourceKey);
      if (!ownsOperation()) return;
      recoveryOnlyActionPending.current = false;
      setRecoveryOnlyState(next
        ? { status: "ready", recovery: next, operation: "idle" }
        : { status: "none" });
    }).catch((error) => {
      if (!ownsOperation()) return;
      recoveryOnlyActionPending.current = false;
      setRecoveryOnlyState({
        status: "error",
        message: error instanceof Error ? error.message : String(error),
      });
    });
  }, [client, props.resource_key, recoveryOnlyState, restoringAccess]);
  const discardEditorRecovery = useCallback(async () => {
    const snapshot = editorController?.getSnapshot();
    if (!editorController || !snapshot || snapshot.status !== "ready") return;
    try {
      await editorController.discard(snapshot.buffer_generation);
      setActionError(null);
    } catch (error) {
      const message = `Discard failed: ${error instanceof Error ? error.message : String(error)}`;
      setActionError(message);
      throw error;
    }
  }, [editorController]);

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
        || recoveryOnlyState.status === "ready"
        || recoveryOnlyState.status === "error"
      ),
    });
  }, [
    editorError,
    editorSnapshot,
    props.resource_key,
    props.surface_id,
    recoveryOnlyState.status,
    resource.snapshot,
  ]);

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
    setPresentationState({
      resource_key: props.resource_key,
      presentation: "presentation" in props.state && props.state.presentation === "editor"
        ? "editor"
        : "rendered",
    });
  }, [props.resource_key, props.state]);

  const setFilePresentation = useCallback((nextPresentation: FileContentPresentation) => {
    setPresentationState({ resource_key: props.resource_key, presentation: nextPresentation });
    updateControllerPresentation({
      presentation: nextPresentation,
      transient_preview: false,
    });
  }, [props.resource_key, updateControllerPresentation]);
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
  const saveFile = useCallback(async () => {
    if (!editorController) throw new Error("The file editor is unavailable.");
    try {
      await editorController.save(props.surface_id);
      setActionError(null);
    } catch (error) {
      const message = `Save failed: ${error instanceof Error ? error.message : String(error)}`;
      setActionError(message);
      throw error;
    }
  }, [editorController, props.surface_id]);
  const saveFileAs = useCallback(async () => {
    const snapshot = editorController?.getSnapshot();
    const descriptor = resource.snapshot?.descriptor;
    if (!editorController || !snapshot || snapshot.status !== "ready" || !descriptor) {
      throw new Error("The file editor is unavailable.");
    }
    try {
      const grant = await client.pickSaveTarget({
        title: "Save As",
        default_name: descriptor.display_name,
      });
      if (!grant) return;
      const saved = await client.saveAsText({
        save_target_grant_id: grant.save_target_grant_id,
        text: snapshot.working_text,
      });
      await on_open_file(saved.canonical_path);
      setActionError(null);
    } catch (error) {
      const message = `Save As failed: ${error instanceof Error ? error.message : String(error)}`;
      setActionError(message);
      throw error;
    }
  }, [client, editorController, on_open_file, resource.snapshot?.descriptor]);

  if (!props.lifecycle.visible) {
    return (
      <section className="files-surface" data-testid="files-surface" data-suspended="true">
        <FilesHeader
          resource_key={props.resource_key}
          descriptor={null}
          presentation={presentation}
          presentation_toggle_available={false}
          dirty={editorSnapshot?.dirty ?? false}
          save_available={false}
          save_as_available={false}
          saving={editorSnapshot?.save_state === "saving"}
          on_presentation_change={setFilePresentation}
          on_save={saveFile}
          on_save_as={saveFileAs}
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
      editor_recovery_conflict={editorSnapshot?.recovery.status === "conflict"
        ? editorSnapshot.recovery
        : null}
      resolving_recovery_conflict={resolvingRecoveryConflict}
      restoring_access={restoringAccess}
      on_retry_editor={retryEditor}
      on_resolve_recovery_conflict={resolveRecoveryConflict}
      recovery_only={recoveryOnlyState}
      on_restore_access={restoreAccess}
      on_discard_editor_recovery={discardEditorRecovery}
      on_discard_recovery={discardRecovery}
      on_retry_recovery={() => setRecoveryOnlyRetryToken((value) => value + 1)}
      editor_controller={editorController}
      editor_snapshot={editorSnapshot}
      presentation={presentation}
      on_presentation_change={setFilePresentation}
      on_save={saveFile}
      on_save_as={saveFileAs}
    />
  );
}
