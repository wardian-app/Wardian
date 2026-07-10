import type {
  WorkbenchDocumentV1,
  WorkbenchShellV1,
  WorkbenchSurfaceV1,
} from "../../types";
import type { WorkbenchStore } from "./useWorkbenchStore";

export type WorkbenchLoadSource = "primary" | "backup" | "default" | "future_schema";

export type WorkbenchPersistenceOutcome =
  | "saved"
  | "revision_conflict"
  | "future_schema";

export type WorkbenchBootConfig = {
  safe_mode: boolean;
};

export type WorkbenchLoadResult = {
  source: WorkbenchLoadSource;
  document: WorkbenchDocumentV1 | null;
  notice: string | null;
  durable_revision: number | null;
  durable_token: string | null;
};

export type WorkbenchSaveRequest = {
  document: WorkbenchDocumentV1;
  expected_revision: number;
  expected_token: string;
  request_id: string;
};

export type WorkbenchSaveResult = {
  outcome: WorkbenchPersistenceOutcome;
  durable_revision: number | null;
  durable_token: string | null;
  request_id: string;
};

export type WorkbenchResetRequest = {
  expected_revision: number;
  expected_token: string;
  request_id: string;
};

export type WorkbenchResetResult = WorkbenchSaveResult & {
  document?: WorkbenchDocumentV1 | null;
};

export type WorkbenchInvoke = (
  command: string,
  args?: Record<string, unknown>,
) => Promise<unknown>;

export type WorkbenchPersistenceAdapter = {
  boot: () => Promise<WorkbenchBootConfig>;
  load: () => Promise<WorkbenchLoadResult>;
  save: (request: WorkbenchSaveRequest) => Promise<WorkbenchSaveResult>;
  reset: (request: WorkbenchResetRequest) => Promise<WorkbenchResetResult>;
};

function invokeResult<TResult>(result: Promise<unknown>): Promise<TResult> {
  return result as Promise<TResult>;
}

/** Thin, snake_case-preserving bridge to the Rust-owned persistence commands. */
export function createWorkbenchInvokeAdapter(
  invoke: WorkbenchInvoke,
): WorkbenchPersistenceAdapter {
  return {
    boot: () => invokeResult<WorkbenchBootConfig>(invoke("get_workbench_boot_config")),
    load: () => invokeResult<WorkbenchLoadResult>(invoke("load_workbench_state")),
    save: (request) => invokeResult<WorkbenchSaveResult>(
      invoke("save_workbench_state", { ...request }),
    ),
    reset: (request) => invokeResult<WorkbenchResetResult>(
      invoke("reset_workbench_state", { ...request }),
    ),
  };
}

export type PendingWorkbenchSave = {
  request_id: string;
  revision: number;
};

export type WorkbenchSaveResponseDecision =
  | {
      type: "acknowledge";
      request_id: string;
      durable_revision: number;
      durable_token: string;
    }
  | { type: "revision_conflict"; request_id: string }
  | { type: "future_schema"; request_id: string }
  | { type: "ignore" };

/**
 * Pure response gate for the serialized queue. Only the exact request and
 * proposed successor revision may advance the durable base.
 */
export function decideWorkbenchSaveResponse(
  pending: PendingWorkbenchSave,
  result: WorkbenchSaveResult,
): WorkbenchSaveResponseDecision {
  if (result.request_id !== pending.request_id) return { type: "ignore" };
  if (result.outcome === "revision_conflict") {
    return { type: "revision_conflict", request_id: pending.request_id };
  }
  if (result.outcome === "future_schema") {
    return { type: "future_schema", request_id: pending.request_id };
  }
  if (
    result.durable_revision !== pending.revision
    || typeof result.durable_token !== "string"
    || result.durable_token.length === 0
  ) {
    return { type: "ignore" };
  }
  return {
    type: "acknowledge",
    request_id: pending.request_id,
    durable_revision: result.durable_revision,
    durable_token: result.durable_token,
  };
}

export type WorkbenchSaveQueueOptions = {
  store: WorkbenchStore;
  adapter: Pick<WorkbenchPersistenceAdapter, "save" | "reset">;
  request_id?: () => string;
  save_delay_ms?: number;
  retry_delay_ms?: number;
  set_timeout?: (callback: () => void, delay: number) => ReturnType<typeof setTimeout>;
  clear_timeout?: (timer: ReturnType<typeof setTimeout>) => void;
  on_durable?: (result: {
    request_id: string;
    durable_revision: number;
    durable_token: string;
  }) => void;
};

export type WorkbenchSaveQueue = {
  flush: () => Promise<void>;
  reset: () => Promise<boolean>;
  shutdown: () => void;
};

type ActiveSave = {
  request: WorkbenchSaveRequest;
  pending_transaction_version: number | null;
  pending_expected_token: string;
  promise: Promise<void> | null;
};

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/**
 * Serializes one canonical Task 3 store into Task 4 CAS saves. Network failures
 * retain the exact pending envelope so a lost response is retried idempotently.
 */
export function createWorkbenchSaveQueue(
  options: WorkbenchSaveQueueOptions,
): WorkbenchSaveQueue {
  const saveDelay = options.save_delay_ms ?? 250;
  const retryDelay = options.retry_delay_ms ?? 250;
  const createRequestId = options.request_id ?? (() => crypto.randomUUID());
  const setTimer = options.set_timeout ?? ((callback, delay) => setTimeout(callback, delay));
  const clearTimer = options.clear_timeout ?? ((timer) => clearTimeout(timer));
  let timer: ReturnType<typeof setTimeout> | null = null;
  let active: ActiveSave | null = null;
  let resetInFlight = false;
  let disposed = false;

  const cancelTimer = (): void => {
    if (timer === null) return;
    clearTimer(timer);
    timer = null;
  };

  const eligible = (): boolean => {
    const state = options.store.getState();
    return state.is_dirty
      && state.pending_request_id === null
      && state.conflict === null
      && !state.loading
      && !state.read_only;
  };

  let sendActive: (pending: ActiveSave) => Promise<void>;

  const arm = (delay: number): void => {
    if (disposed || timer !== null || active !== null || resetInFlight || !eligible()) return;
    timer = setTimer(() => {
      timer = null;
      void flush();
    }, delay);
  };

  const armRetry = (pending: ActiveSave): void => {
    if (disposed || timer !== null || active !== pending) return;
    timer = setTimer(() => {
      timer = null;
      void sendActive(pending);
    }, retryDelay);
  };

  const finishActive = (pending: ActiveSave): void => {
    if (active === pending) active = null;
  };

  const handleResult = (pending: ActiveSave, result: WorkbenchSaveResult): void => {
    if (active !== pending) return;
    const decision = decideWorkbenchSaveResponse(
      { request_id: pending.request.request_id, revision: pending.request.document.revision },
      result,
    );
    if (decision.type === "ignore") {
      armRetry(pending);
      return;
    }
    if (decision.type === "revision_conflict") {
      options.store.getState().reject_pending_save(
        pending.request.request_id,
        "revision_conflict",
      );
      finishActive(pending);
      return;
    }
    if (decision.type === "future_schema") {
      options.store.getState().reject_pending_save(
        pending.request.request_id,
        "future_schema",
      );
      finishActive(pending);
      return;
    }
    const acknowledged = options.store.getState().acknowledge_pending_save(
      decision.request_id,
      decision.durable_revision,
      pending.pending_transaction_version,
      pending.pending_expected_token,
      decision.durable_token,
    );
    if (!acknowledged) {
      armRetry(pending);
      return;
    }
    options.on_durable?.({
      request_id: decision.request_id,
      durable_revision: decision.durable_revision,
      durable_token: decision.durable_token,
    });
    finishActive(pending);
    if (eligible()) void flush();
  };

  sendActive = (pending): Promise<void> => {
    if (pending.promise !== null) return pending.promise;
    pending.promise = options.adapter.save(pending.request)
      .then((result) => {
        handleResult(pending, result);
      })
      .catch((error: unknown) => {
        if (active !== pending) return;
        options.store.getState().set_save_error(errorMessage(error));
        armRetry(pending);
      })
      .finally(() => {
        pending.promise = null;
      });
    return pending.promise;
  };

  const flush = async (): Promise<void> => {
    cancelTimer();
    if (resetInFlight) return;
    if (active !== null) {
      await sendActive(active);
      return;
    }
    if (disposed || !eligible()) return;
    const requestId = createRequestId();
    if (!options.store.getState().begin_pending_save(requestId)) return;
    const state = options.store.getState();
    const pendingDocument = state.pending_document;
    const pendingRevision = state.pending_revision;
    const expectedToken = state.pending_expected_token;
    if (
      pendingDocument === null
      || pendingRevision === null
      || expectedToken === null
      || expectedToken.length === 0
      || pendingRevision !== state.durable_revision + 1
    ) {
      options.store.getState().fail_pending_save(
        requestId,
        "invalid pending workbench save envelope",
      );
      return;
    }
    active = {
      request: {
        document: structuredClone(pendingDocument) as WorkbenchDocumentV1,
        expected_revision: state.durable_revision,
        expected_token: expectedToken,
        request_id: requestId,
      },
      pending_transaction_version: state.pending_transaction_version,
      pending_expected_token: expectedToken,
      promise: null,
    };
    await sendActive(active);
  };

  const unsubscribe = options.store.subscribe(() => {
    arm(saveDelay);
  });
  arm(saveDelay);

  return {
    flush,
    reset: async () => {
      cancelTimer();
      if (resetInFlight || disposed) return false;
      if (active !== null) {
        await sendActive(active);
        if (active !== null) return false;
      }
      const before = options.store.getState();
      if (
        before.loading
        || before.read_only
        || before.conflict !== null
        || before.durable_token === null
        || before.durable_token.length === 0
      ) return false;
      resetInFlight = true;
      const resetResult = before.reset_document();
      if (!resetResult.accepted) {
        resetInFlight = false;
        return false;
      }
      cancelTimer();
      const requestId = createRequestId();
      if (!options.store.getState().begin_pending_save(requestId)) {
        resetInFlight = false;
        return false;
      }
      const pending = options.store.getState();
      if (
        pending.pending_revision !== before.durable_revision + 1
        || pending.pending_transaction_version === null
      ) {
        options.store.getState().fail_pending_save(
          requestId,
          "invalid pending workbench reset envelope",
        );
        resetInFlight = false;
        if (eligible()) void flush();
        return false;
      }
      const request: WorkbenchResetRequest = {
        expected_revision: before.durable_revision,
        expected_token: before.durable_token,
        request_id: requestId,
      };
      let failure = "workbench reset acknowledgement did not match the pending request";
      try {
        for (let attempt = 0; attempt < 2; attempt += 1) {
          let result: WorkbenchResetResult;
          try {
            result = await options.adapter.reset(request);
          } catch (error) {
            failure = errorMessage(error);
            options.store.getState().set_save_error(failure);
            continue;
          }
          const decision = decideWorkbenchSaveResponse(
            { request_id: requestId, revision: pending.pending_revision },
            result,
          );
          if (decision.type === "revision_conflict" || decision.type === "future_schema") {
            options.store.getState().reject_pending_save(requestId, decision.type);
            return false;
          }
          if (decision.type !== "acknowledge" || result.document == null) continue;
          const acknowledged = options.store.getState().acknowledge_pending_reset(
            decision.request_id,
            decision.durable_revision,
            pending.pending_transaction_version,
            before.durable_token,
            decision.durable_token,
            result.document,
          );
          if (!acknowledged) continue;
          options.on_durable?.({
            request_id: decision.request_id,
            durable_revision: decision.durable_revision,
            durable_token: decision.durable_token,
          });
          return true;
        }
        options.store.getState().fail_pending_save(requestId, failure);
        return false;
      } finally {
        resetInFlight = false;
        if (!disposed && eligible()) void flush();
      }
    },
    shutdown: () => {
      if (disposed) return;
      cancelTimer();
      if (active !== null) {
        void sendActive(active);
      } else if (eligible()) {
        void flush();
      }
      disposed = true;
      unsubscribe();
    },
  };
}

export type LegacyWorkbenchViewport = {
  width: number;
  height: number;
};

export type LegacyWorkbenchViewMode =
  | "grid"
  | "dashboard"
  | "queue"
  | "library"
  | "workflows"
  | "graph"
  | "garden";

export type LegacyWorkbenchMigration = {
  shell_patch: Pick<
    WorkbenchShellV1,
    | "left_sidebar_width"
    | "right_sidebar_width"
    | "bottom_terminal_open"
    | "bottom_terminal_height"
  >;
  initial_surface: WorkbenchSurfaceV1;
};

const INITIAL_SURFACE_TYPE_BY_VIEW: Record<LegacyWorkbenchViewMode, string> = {
  grid: "agents-overview",
  dashboard: "dashboard",
  queue: "queue",
  library: "library",
  workflows: "workflows",
  graph: "graph",
  garden: "garden",
};

function recordValue(value: unknown): Record<string, unknown> | null {
  return typeof value === "object" && value !== null && !Array.isArray(value)
    ? value as Record<string, unknown>
    : null;
}

function finiteNumber(value: unknown, fallback: number): number {
  return typeof value === "number" && Number.isFinite(value) ? value : fallback;
}

function clampRounded(value: number, minimum: number, maximum: number): number {
  return Math.max(minimum, Math.min(maximum, Math.round(value)));
}

function initialSurface(viewMode: LegacyWorkbenchViewMode | undefined): WorkbenchSurfaceV1 {
  const surfaceType = INITIAL_SURFACE_TYPE_BY_VIEW[viewMode ?? "grid"];
  return {
    surface_id: `initial-${surfaceType}`,
    surface_type: surfaceType,
    state_schema_version: 1,
    state: surfaceType === "agents-overview"
      ? { focused_agent_id: null, presentation_mode: "auto" }
      : {},
  };
}

/** Reads the one-time Zustand blob without importing unrelated layout state. */
export function readLegacyWorkbenchMigration(
  raw: string | null,
  viewport: LegacyWorkbenchViewport,
  viewMode?: LegacyWorkbenchViewMode,
): LegacyWorkbenchMigration {
  let state: Record<string, unknown> = {};
  if (raw !== null) {
    try {
      const root = recordValue(JSON.parse(raw));
      state = recordValue(root?.state) ?? {};
    } catch {
      state = {};
    }
  }
  const viewportWidth = Number.isFinite(viewport.width) ? Math.max(0, viewport.width) : 0;
  const viewportHeight = Number.isFinite(viewport.height) ? Math.max(0, viewport.height) : 0;
  const maximumSidebarWidth = Math.max(200, Math.floor(viewportWidth * 0.4));
  const maximumTerminalHeight = Math.max(180, Math.floor(viewportHeight * 0.7));
  return {
    shell_patch: {
      left_sidebar_width: clampRounded(
        finiteNumber(state.leftSidebarWidth, 240),
        200,
        maximumSidebarWidth,
      ),
      right_sidebar_width: clampRounded(
        finiteNumber(state.rightSidebarWidth, 240),
        200,
        maximumSidebarWidth,
      ),
      bottom_terminal_open: typeof state.userTerminalOpen === "boolean"
        ? state.userTerminalOpen
        : false,
      bottom_terminal_height: clampRounded(
        finiteNumber(state.userTerminalHeight, 360),
        180,
        maximumTerminalHeight,
      ),
    },
    initial_surface: initialSurface(viewMode),
  };
}
