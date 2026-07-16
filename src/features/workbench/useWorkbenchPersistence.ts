import { useCallback, useEffect, useRef, useState } from "react";

import type { WorkbenchShellV1 } from "../../types";
import type { WorkbenchStore } from "./useWorkbenchStore";
import { createWorkbenchStore } from "./useWorkbenchStore";
import {
  createWorkbenchSaveQueue,
  readLegacyWorkbenchMigration,
  type LegacyWorkbenchViewMode,
  type LegacyWorkbenchViewport,
  type WorkbenchLoadSource,
  type WorkbenchLoadResult,
  type WorkbenchPersistenceAdapter,
  type WorkbenchPendingSaveEnvelope,
  type WorkbenchSaveQueue,
} from "./workbenchPersistence";

export type WorkbenchPersistenceStatus =
  | "disabled"
  | "loading"
  | "ready"
  | "error";

export type WorkbenchLocalExport = {
  filename: string;
  mime_type: "application/json";
  json: string;
};

export type UseWorkbenchPersistenceOptions = {
  enabled: boolean;
  adapter: WorkbenchPersistenceAdapter;
  store?: WorkbenchStore;
  request_id?: () => string;
  legacy_storage?: {
    getItem: (key: string) => string | null;
    removeItem: (key: string) => void;
  };
  viewport?: () => LegacyWorkbenchViewport;
  initial_view_mode?: LegacyWorkbenchViewMode;
  shell_projection?: {
    read: () => WorkbenchShellV1;
    write: (shell: WorkbenchShellV1) => void;
    subscribe: (listener: () => void) => () => void;
  };
};

export type UseWorkbenchPersistenceResult = {
  store: WorkbenchStore;
  status: WorkbenchPersistenceStatus;
  safe_mode: boolean;
  source: WorkbenchLoadSource | null;
  notice: string | null;
  conflict: string | null;
  save_error: string | null;
  is_dirty: boolean;
  save_pending: boolean;
  resolving_conflict: boolean;
  flush: () => Promise<void>;
  reset: (expected_transaction_version?: number) => Promise<boolean>;
  use_disk: () => Promise<boolean>;
  replace_disk: () => Promise<boolean>;
  export_local_json: () => WorkbenchLocalExport;
};

type HookStatus = Pick<
  UseWorkbenchPersistenceResult,
  "status" | "safe_mode" | "source" | "notice" | "conflict" | "save_error"
  | "is_dirty" | "save_pending"
  | "resolving_conflict"
>;

function initialStatus(enabled: boolean): HookStatus {
  return {
    status: enabled ? "loading" : "disabled",
    safe_mode: false,
    source: null,
    notice: null,
    conflict: null,
    save_error: null,
    is_dirty: false,
    save_pending: false,
    resolving_conflict: false,
  };
}

function shellEquals(left: WorkbenchShellV1, right: WorkbenchShellV1): boolean {
  return left.left_sidebar_collapsed === right.left_sidebar_collapsed
    && left.left_sidebar_width === right.left_sidebar_width
    && left.right_sidebar_collapsed === right.right_sidebar_collapsed
    && left.right_sidebar_width === right.right_sidebar_width
    && left.bottom_terminal_open === right.bottom_terminal_open
    && left.bottom_terminal_height === right.bottom_terminal_height;
}

/** Owns the persistence controller lifecycle without owning workbench documents. */
export function useWorkbenchPersistence(
  options: UseWorkbenchPersistenceOptions,
): UseWorkbenchPersistenceResult {
  const storeRef = useRef<WorkbenchStore | null>(null);
  if (storeRef.current === null) {
    storeRef.current = options.store ?? createWorkbenchStore({ loading: options.enabled });
  }
  const store = storeRef.current;
  const queueRef = useRef<WorkbenchSaveQueue | null>(null);
  const effectGenerationRef = useRef(0);
  const mountedRef = useRef(false);
  const resolutionInFlightRef = useRef<number | null>(null);
  const requestIdRef = useRef(options.request_id ?? (() => crypto.randomUUID()));
  requestIdRef.current = options.request_id ?? requestIdRef.current;
  const legacyStorageRef = useRef(options.legacy_storage);
  legacyStorageRef.current = options.legacy_storage;
  const viewportRef = useRef(options.viewport);
  viewportRef.current = options.viewport;
  const initialViewModeRef = useRef(options.initial_view_mode);
  initialViewModeRef.current = options.initial_view_mode;
  const shellProjectionRef = useRef(options.shell_projection);
  shellProjectionRef.current = options.shell_projection;
  const [hookStatus, setHookStatus] = useState<HookStatus>(() => initialStatus(options.enabled));
  const isCurrentGeneration = useCallback((generation: number): boolean => (
    mountedRef.current && effectGenerationRef.current === generation
  ), []);

  useEffect(() => {
    const generation = effectGenerationRef.current + 1;
    effectGenerationRef.current = generation;
    mountedRef.current = true;
    resolutionInFlightRef.current = null;
    const isCurrentEffect = (): boolean => isCurrentGeneration(generation);
    const invalidateEffect = (): void => {
      if (effectGenerationRef.current !== generation) return;
      mountedRef.current = false;
      effectGenerationRef.current = generation + 1;
      if (resolutionInFlightRef.current === generation) {
        resolutionInFlightRef.current = null;
      }
    };
    let unsubscribeStore: (() => void) | null = null;
    let unsubscribeShell: (() => void) | null = null;
    let projectingShell = false;
    if (queueRef.current !== null) void queueRef.current.shutdown();
    queueRef.current = null;

    if (!options.enabled) {
      if (isCurrentEffect()) store.getState().set_loading(false);
      if (isCurrentEffect()) setHookStatus(initialStatus(false));
      return invalidateEffect;
    }

    if (isCurrentEffect()) store.getState().set_loading(true);
    if (isCurrentEffect()) setHookStatus(initialStatus(true));

    void Promise.all([options.adapter.boot(), options.adapter.load()])
      .then(([boot, loaded]) => {
        if (!isCurrentEffect()) return;
        if (loaded.source === "future_schema") {
          if (!isCurrentEffect()) return;
          store.getState().set_conflict("future_schema");
          if (!isCurrentEffect()) return;
          store.getState().set_read_only(true);
          if (!isCurrentEffect()) return;
          store.getState().set_loading(false);
        } else {
          if (
            loaded.document === null
            || loaded.durable_revision === null
            || loaded.durable_token === null
          ) {
            throw new Error("backend returned an invalid workbench load result");
          }
          if (!isCurrentEffect()) return;
          if (!store.getState().adopt_durable_state({
            document: loaded.document,
            durable_revision: loaded.durable_revision,
            durable_token: loaded.durable_token,
          })) {
            throw new Error("backend returned an invalid workbench load result");
          }
          let captureMigrationEnvelope = false;
          let migrationEnvelope: WorkbenchPendingSaveEnvelope | null = null;
          if (loaded.source === "default") {
            let rawLegacyLayout: string | null = null;
            try {
              rawLegacyLayout = legacyStorageRef.current?.getItem("wardian-layout") ?? null;
            } catch {
              rawLegacyLayout = null;
            }
            const viewport = viewportRef.current?.() ?? {
              width: typeof window === "undefined" ? 0 : window.innerWidth,
              height: typeof window === "undefined" ? 0 : window.innerHeight,
            };
            const migration = readLegacyWorkbenchMigration(
              rawLegacyLayout,
              viewport,
              initialViewModeRef.current,
            );
            if (!isCurrentEffect()) return;
            const migrated = store.getState().apply_commands([
              { type: "update_shell", patch: migration.shell_patch },
              { type: "open_surface", surface: migration.initial_surface },
            ]);
            if (!migrated.accepted) {
              throw new Error("could not create the first migrated workbench document");
            }
            captureMigrationEnvelope = true;
          }
          if (!isCurrentEffect()) return;
          const queue = createWorkbenchSaveQueue({
            store,
            adapter: options.adapter,
            request_id: () => requestIdRef.current(),
            on_save_pending: (envelope) => {
              if (!captureMigrationEnvelope || migrationEnvelope !== null) return;
              migrationEnvelope = envelope;
              captureMigrationEnvelope = false;
            },
            on_save_durable: (envelope) => {
              if (migrationEnvelope === null || envelope !== migrationEnvelope) return;
              migrationEnvelope = null;
              try {
                legacyStorageRef.current?.removeItem("wardian-layout");
              } catch {
                // The workbench is already durable; legacy cleanup is best effort.
              }
            },
          });
          if (!isCurrentEffect()) {
            void queue.shutdown();
            return;
          }
          queueRef.current = queue;
          if (captureMigrationEnvelope) void queue.flush();
          const shellProjection = shellProjectionRef.current;
          if (shellProjection) {
            projectingShell = true;
            try {
              shellProjection.write(store.getState().document.shell);
            } finally {
              projectingShell = false;
            }
            unsubscribeShell = shellProjection.subscribe(() => {
              if (!isCurrentEffect() || projectingShell) return;
              const projectedShell = shellProjection.read();
              if (shellEquals(projectedShell, store.getState().document.shell)) return;
              if (!isCurrentEffect()) return;
              store.getState().apply_commands([
                { type: "update_shell", patch: projectedShell },
              ]);
            });
          }
        }

        const publishStoreStatus = (
          state: ReturnType<WorkbenchStore["getState"]>,
          previous: ReturnType<WorkbenchStore["getState"]>,
        ): void => {
          if (!isCurrentEffect()) return;
          const shellProjection = shellProjectionRef.current;
          if (
            shellProjection
            && state.document.shell !== previous.document.shell
            && !shellEquals(shellProjection.read(), state.document.shell)
          ) {
            projectingShell = true;
            try {
              shellProjection.write(state.document.shell);
            } finally {
              projectingShell = false;
            }
          }
          if (!isCurrentEffect()) return;
          setHookStatus((current) => ({
            ...current,
            conflict: state.conflict,
            save_error: state.save_error,
            is_dirty: state.is_dirty,
            save_pending: state.save_pending,
          }));
        };
        if (!isCurrentEffect()) return;
        unsubscribeStore = store.subscribe(publishStoreStatus);
        const state = store.getState();
        if (!isCurrentEffect()) return;
        setHookStatus({
          status: "ready",
          safe_mode: boot.safe_mode,
          source: loaded.source,
          notice: loaded.notice,
          conflict: state.conflict,
          save_error: state.save_error,
          is_dirty: state.is_dirty,
          save_pending: state.save_pending,
          resolving_conflict: false,
        });
      })
      .catch((error: unknown) => {
        if (!isCurrentEffect()) return;
        const message = error instanceof Error ? error.message : String(error);
        if (!isCurrentEffect()) return;
        store.getState().set_loading(false);
        if (!isCurrentEffect()) return;
        store.getState().set_save_error(message);
        if (!isCurrentEffect()) return;
        setHookStatus({
          status: "error",
          safe_mode: false,
          source: null,
          notice: message,
          conflict: store.getState().conflict,
          save_error: message,
          is_dirty: store.getState().is_dirty,
          save_pending: store.getState().save_pending,
          resolving_conflict: false,
        });
      });

    return () => {
      invalidateEffect();
      unsubscribeStore?.();
      unsubscribeShell?.();
      if (queueRef.current !== null) void queueRef.current.shutdown();
      queueRef.current = null;
    };
  }, [isCurrentGeneration, options.adapter, options.enabled, store]);

  const flush = useCallback(async (): Promise<void> => {
    await queueRef.current?.flush();
  }, []);
  const reset = useCallback(
    (expectedTransactionVersion?: number) => queueRef.current?.reset(
      expectedTransactionVersion,
    ) ?? Promise.resolve(false),
    [],
  );
  const useDisk = useCallback(async (): Promise<boolean> => {
    const generation = effectGenerationRef.current;
    if (
      !options.enabled
      || !isCurrentGeneration(generation)
      || resolutionInFlightRef.current !== null
      || store.getState().conflict !== "revision_conflict"
    ) return false;
    resolutionInFlightRef.current = generation;
    if (!isCurrentGeneration(generation)) return false;
    setHookStatus((current) => ({ ...current, resolving_conflict: true }));
    try {
      let loaded: WorkbenchLoadResult;
      try {
        loaded = await options.adapter.load();
      } catch (error: unknown) {
        if (!isCurrentGeneration(generation)) return false;
        store.getState().set_save_error(
          error instanceof Error ? error.message : String(error),
        );
        return false;
      }
      if (!isCurrentGeneration(generation)) return false;
      if (store.getState().conflict !== "revision_conflict") return false;
      if (loaded.source === "future_schema") {
        if (!isCurrentGeneration(generation)) return false;
        store.getState().set_conflict("future_schema");
        if (!isCurrentGeneration(generation)) return false;
        store.getState().set_read_only(true);
        if (!isCurrentGeneration(generation)) return false;
        setHookStatus((current) => ({
          ...current,
          source: loaded.source,
          notice: loaded.notice,
          conflict: "future_schema",
        }));
        return false;
      }
      if (
        loaded.document === null
        || loaded.durable_revision === null
        || loaded.durable_token === null
      ) return false;
      if (!isCurrentGeneration(generation)) return false;
      if (!store.getState().adopt_durable_state({
        document: loaded.document,
        durable_revision: loaded.durable_revision,
        durable_token: loaded.durable_token,
      })) return false;
      if (!isCurrentGeneration(generation)) return false;
      setHookStatus((current) => ({
        ...current,
        source: loaded.source,
        notice: loaded.notice,
        conflict: null,
        save_error: null,
      }));
      return true;
    } finally {
      if (resolutionInFlightRef.current === generation) {
        resolutionInFlightRef.current = null;
        if (isCurrentGeneration(generation)) {
          setHookStatus((current) => ({ ...current, resolving_conflict: false }));
        }
      }
    }
  }, [isCurrentGeneration, options.adapter, options.enabled, store]);
  const replaceDisk = useCallback(async (): Promise<boolean> => {
    const generation = effectGenerationRef.current;
    if (
      !options.enabled
      || !isCurrentGeneration(generation)
      || resolutionInFlightRef.current !== null
      || store.getState().conflict !== "revision_conflict"
    ) return false;
    resolutionInFlightRef.current = generation;
    if (!isCurrentGeneration(generation)) return false;
    setHookStatus((current) => ({ ...current, resolving_conflict: true }));
    try {
      let loaded: WorkbenchLoadResult;
      try {
        loaded = await options.adapter.load();
      } catch (error: unknown) {
        if (!isCurrentGeneration(generation)) return false;
        store.getState().set_save_error(
          error instanceof Error ? error.message : String(error),
        );
        return false;
      }
      if (!isCurrentGeneration(generation)) return false;
      if (store.getState().conflict !== "revision_conflict") return false;
      if (loaded.source === "future_schema") {
        if (!isCurrentGeneration(generation)) return false;
        store.getState().set_conflict("future_schema");
        if (!isCurrentGeneration(generation)) return false;
        store.getState().set_read_only(true);
        if (!isCurrentGeneration(generation)) return false;
        setHookStatus((current) => ({
          ...current,
          source: loaded.source,
          notice: loaded.notice,
          conflict: "future_schema",
        }));
        return false;
      }
      if (
        loaded.document === null
        || loaded.durable_revision === null
        || loaded.durable_token === null
      ) return false;
      if (!isCurrentGeneration(generation)) return false;
      if (!store.getState().rebase_working_onto_durable({
        document: loaded.document,
        durable_revision: loaded.durable_revision,
        durable_token: loaded.durable_token,
      })) return false;
      if (!isCurrentGeneration(generation)) return false;
      setHookStatus((current) => ({
        ...current,
        source: loaded.source,
        notice: loaded.notice,
        conflict: null,
        save_error: null,
      }));
      if (!isCurrentGeneration(generation)) return false;
      const queue = queueRef.current;
      if (queue === null || queueRef.current !== queue) return false;
      const accepted = await queue.flush();
      if (!isCurrentGeneration(generation)) return false;
      return accepted;
    } finally {
      if (resolutionInFlightRef.current === generation) {
        resolutionInFlightRef.current = null;
        if (isCurrentGeneration(generation)) {
          setHookStatus((current) => ({ ...current, resolving_conflict: false }));
        }
      }
    }
  }, [isCurrentGeneration, options.adapter, options.enabled, store]);
  const exportLocalJson = useCallback((): WorkbenchLocalExport => ({
    filename: "wardian-workbench-local.json",
    mime_type: "application/json",
    json: JSON.stringify(store.getState().document, null, 2),
  }), [store]);

  return {
    store,
    ...hookStatus,
    flush,
    reset,
    use_disk: useDisk,
    replace_disk: replaceDisk,
    export_local_json: exportLocalJson,
  };
}
