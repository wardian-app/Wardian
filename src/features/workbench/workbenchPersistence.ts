import type {
  WorkbenchDocumentV1,
  WorkbenchShellV1,
  WorkbenchSurfaceV1,
} from "../../types";
import type {
  ReadonlyWorkbenchDocumentV1,
  WorkbenchStore,
} from "./useWorkbenchStore";
import { validateWorkbenchDocument } from "./workbenchModel";

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

export class WorkbenchPersistenceAdapterError extends Error {
  constructor(command: "boot" | "load" | "save" | "reset", detail: string) {
    super(`Invalid workbench ${command} response: ${detail}`);
    this.name = "WorkbenchPersistenceAdapterError";
  }
}

type DecodeCommand = "boot" | "load" | "save" | "reset";

function decodeRecord(
  value: unknown,
  command: DecodeCommand,
  requiredKeys: readonly string[],
  optionalKeys: readonly string[] = [],
): Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw new WorkbenchPersistenceAdapterError(command, "expected a plain object");
  }
  const prototype = Object.getPrototypeOf(value);
  if (prototype !== Object.prototype && prototype !== null) {
    throw new WorkbenchPersistenceAdapterError(command, "expected a plain object prototype");
  }
  const record = value as Record<string, unknown>;
  const allowed = new Set([...requiredKeys, ...optionalKeys]);
  const ownKeys = Reflect.ownKeys(record);
  if (ownKeys.some((key) => typeof key !== "string")) {
    throw new WorkbenchPersistenceAdapterError(command, "contained a symbol field");
  }
  const keys = ownKeys as string[];
  if (keys.some((key) => {
    const descriptor = Object.getOwnPropertyDescriptor(record, key);
    return descriptor === undefined
      || descriptor.enumerable !== true
      || !("value" in descriptor);
  })) {
    throw new WorkbenchPersistenceAdapterError(
      command,
      "fields must be enumerable data properties",
    );
  }
  if (!requiredKeys.every((key) => Object.prototype.hasOwnProperty.call(record, key))) {
    throw new WorkbenchPersistenceAdapterError(command, "missing a required field");
  }
  if (keys.some((key) => !allowed.has(key))) {
    throw new WorkbenchPersistenceAdapterError(command, "contained an unexpected field");
  }
  return record;
}

function decodeRevision(value: unknown, command: DecodeCommand): number {
  if (!Number.isSafeInteger(value) || (value as number) < 0 || Object.is(value, -0)) {
    throw new WorkbenchPersistenceAdapterError(
      command,
      "durable_revision must be a nonnegative safe integer",
    );
  }
  return value as number;
}

function decodeToken(value: unknown, command: DecodeCommand): string {
  if (typeof value !== "string" || value.length === 0) {
    throw new WorkbenchPersistenceAdapterError(command, "durable_token must be nonempty");
  }
  return value;
}

function decodeRequestId(value: unknown, command: DecodeCommand): string {
  if (typeof value !== "string" || value.length === 0) {
    throw new WorkbenchPersistenceAdapterError(command, "request_id must be nonempty");
  }
  return value;
}

function decodeDocument(value: unknown, command: DecodeCommand): WorkbenchDocumentV1 {
  const validation = validateWorkbenchDocument(value as WorkbenchDocumentV1);
  if (!validation.valid) {
    throw new WorkbenchPersistenceAdapterError(command, "document failed V1 validation");
  }
  return validation.document;
}

function decodeBootResult(value: unknown): WorkbenchBootConfig {
  const record = decodeRecord(value, "boot", ["safe_mode"]);
  if (typeof record.safe_mode !== "boolean") {
    throw new WorkbenchPersistenceAdapterError("boot", "safe_mode must be boolean");
  }
  return { safe_mode: record.safe_mode };
}

function decodeLoadResult(value: unknown): WorkbenchLoadResult {
  const record = decodeRecord(value, "load", [
    "source",
    "document",
    "notice",
    "durable_revision",
    "durable_token",
  ]);
  const source = record.source;
  if (
    source !== "primary"
    && source !== "backup"
    && source !== "default"
    && source !== "future_schema"
  ) {
    throw new WorkbenchPersistenceAdapterError("load", "source was unknown");
  }
  if (record.notice !== null && typeof record.notice !== "string") {
    throw new WorkbenchPersistenceAdapterError("load", "notice must be string or null");
  }
  if (source === "future_schema") {
    if (
      record.document !== null
      || record.durable_revision !== null
      || record.durable_token !== null
    ) {
      throw new WorkbenchPersistenceAdapterError(
        "load",
        "future_schema must not expose a V1 document or durable identity",
      );
    }
    return {
      source,
      document: null,
      notice: record.notice,
      durable_revision: null,
      durable_token: null,
    };
  }
  const document = decodeDocument(record.document, "load");
  const durableRevision = decodeRevision(record.durable_revision, "load");
  if (document.revision !== durableRevision) {
    throw new WorkbenchPersistenceAdapterError(
      "load",
      "document revision did not match durable_revision",
    );
  }
  return {
    source,
    document,
    notice: record.notice,
    durable_revision: durableRevision,
    durable_token: decodeToken(record.durable_token, "load"),
  };
}

function decodeOutcome(value: unknown, command: "save" | "reset"): WorkbenchPersistenceOutcome {
  if (value !== "saved" && value !== "revision_conflict" && value !== "future_schema") {
    throw new WorkbenchPersistenceAdapterError(command, "outcome was unknown");
  }
  return value;
}

function decodePersistenceIdentity(
  record: Record<string, unknown>,
  command: "save" | "reset",
  outcome: WorkbenchPersistenceOutcome,
): { durable_revision: number | null; durable_token: string | null } {
  if (outcome === "future_schema") {
    if (record.durable_revision !== null || record.durable_token !== null) {
      throw new WorkbenchPersistenceAdapterError(
        command,
        "future_schema must not expose a durable V1 identity",
      );
    }
    return { durable_revision: null, durable_token: null };
  }
  return {
    durable_revision: decodeRevision(record.durable_revision, command),
    durable_token: decodeToken(record.durable_token, command),
  };
}

function decodeSaveResult(
  value: unknown,
  request: WorkbenchSaveRequest,
): WorkbenchSaveResult {
  const record = decodeRecord(value, "save", [
    "outcome",
    "durable_revision",
    "durable_token",
    "request_id",
  ]);
  const outcome = decodeOutcome(record.outcome, "save");
  const requestId = decodeRequestId(record.request_id, "save");
  if (requestId !== request.request_id) {
    throw new WorkbenchPersistenceAdapterError("save", "request_id did not echo the request");
  }
  const identity = decodePersistenceIdentity(record, "save", outcome);
  if (outcome === "saved" && identity.durable_revision !== request.document.revision) {
    throw new WorkbenchPersistenceAdapterError(
      "save",
      "saved revision did not match the proposed document",
    );
  }
  return { outcome, ...identity, request_id: requestId };
}

function decodeResetResult(
  value: unknown,
  request: WorkbenchResetRequest,
): WorkbenchResetResult {
  const record = decodeRecord(value, "reset", [
    "outcome",
    "durable_revision",
    "durable_token",
    "request_id",
  ], ["document"]);
  const outcome = decodeOutcome(record.outcome, "reset");
  const requestId = decodeRequestId(record.request_id, "reset");
  if (requestId !== request.request_id) {
    throw new WorkbenchPersistenceAdapterError("reset", "request_id did not echo the request");
  }
  const identity = decodePersistenceIdentity(record, "reset", outcome);
  if (outcome !== "saved") {
    if (Object.prototype.hasOwnProperty.call(record, "document")) {
      throw new WorkbenchPersistenceAdapterError(
        "reset",
        "non-saved reset must not include a document",
      );
    }
    return { outcome, ...identity, request_id: requestId };
  }
  if (!Object.prototype.hasOwnProperty.call(record, "document")) {
    throw new WorkbenchPersistenceAdapterError("reset", "saved reset omitted its document");
  }
  const document = decodeDocument(record.document, "reset");
  if (
    identity.durable_revision !== request.expected_revision + 1
    || document.revision !== identity.durable_revision
  ) {
    throw new WorkbenchPersistenceAdapterError(
      "reset",
      "saved reset document did not match the successor revision",
    );
  }
  return { outcome, ...identity, request_id: requestId, document };
}

/** Thin, snake_case-preserving bridge to the Rust-owned persistence commands. */
export function createWorkbenchInvokeAdapter(
  invoke: WorkbenchInvoke,
): WorkbenchPersistenceAdapter {
  return {
    boot: async () => decodeBootResult(await invoke("get_workbench_boot_config")),
    load: async () => decodeLoadResult(await invoke("load_workbench_state")),
    save: async (request) => decodeSaveResult(
      await invoke("save_workbench_state", { ...request }),
      request,
    ),
    reset: async (request) => decodeResetResult(
      await invoke("reset_workbench_state", { ...request }),
      request,
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
  if (result.outcome !== "saved") return { type: "ignore" };
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
  on_save_pending?: (envelope: WorkbenchPendingSaveEnvelope) => void;
  on_save_durable?: (
    envelope: WorkbenchPendingSaveEnvelope,
    result: {
    request_id: string;
    durable_revision: number;
    durable_token: string;
    },
  ) => void;
};

export type WorkbenchPendingSaveEnvelope = {
  request_id: string;
  revision: number;
  pending_transaction_version: number | null;
  pending_document: ReadonlyWorkbenchDocumentV1;
};

export type WorkbenchSaveQueue = {
  /** Resolves true only when an existing or newly-created save was submitted. */
  flush: () => Promise<boolean>;
  reset: (expected_transaction_version?: number) => Promise<boolean>;
  shutdown: () => Promise<void>;
};

type ActiveSave = {
  request: WorkbenchSaveRequest;
  envelope: WorkbenchPendingSaveEnvelope;
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
  let shuttingDown = false;
  let disposed = false;
  let shutdownPromise: Promise<void> | null = null;

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
    if (
      disposed
      || shuttingDown
      || timer !== null
      || active !== null
      || resetInFlight
      || !eligible()
    ) return;
    timer = setTimer(() => {
      timer = null;
      void flush();
    }, delay);
  };

  const armRetry = (pending: ActiveSave): void => {
    if (disposed || shuttingDown || timer !== null || active !== pending) return;
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
    options.on_save_durable?.(
      pending.envelope,
      {
        request_id: decision.request_id,
        durable_revision: decision.durable_revision,
        durable_token: decision.durable_token,
      },
    );
    finishActive(pending);
    if (!shuttingDown && eligible()) void flush();
  };

  sendActive = (pending): Promise<void> => {
    if (pending.promise !== null) return pending.promise;
    pending.promise = options.adapter.save(pending.request)
      .then((result) => {
        handleResult(pending, result);
      })
      .catch((error: unknown) => {
        if (active !== pending) return;
        if (error instanceof WorkbenchPersistenceAdapterError) {
          options.store.getState().fail_pending_save(
            pending.request.request_id,
            error.message,
          );
          finishActive(pending);
          return;
        }
        options.store.getState().set_save_error(errorMessage(error));
        armRetry(pending);
      })
      .finally(() => {
        pending.promise = null;
      });
    return pending.promise;
  };

  const flush = async (allowDuringShutdown = false): Promise<boolean> => {
    cancelTimer();
    if (resetInFlight || (shuttingDown && !allowDuringShutdown)) return false;
    if (active !== null) {
      await sendActive(active);
      return true;
    }
    if (disposed || !eligible()) return false;
    const requestId = createRequestId();
    if (!options.store.getState().begin_pending_save(requestId)) return false;
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
      return false;
    }
    const envelope: WorkbenchPendingSaveEnvelope = {
      request_id: requestId,
      revision: pendingRevision,
      pending_transaction_version: state.pending_transaction_version,
      pending_document: pendingDocument,
    };
    active = {
      request: {
        document: structuredClone(pendingDocument) as WorkbenchDocumentV1,
        expected_revision: state.durable_revision,
        expected_token: expectedToken,
        request_id: requestId,
      },
      envelope,
      pending_transaction_version: state.pending_transaction_version,
      pending_expected_token: expectedToken,
      promise: null,
    };
    options.on_save_pending?.(envelope);
    await sendActive(active);
    return true;
  };

  const unsubscribe = options.store.subscribe(() => {
    arm(saveDelay);
  });
  arm(saveDelay);

  return {
    flush,
    reset: async (expectedTransactionVersion) => {
      cancelTimer();
      if (resetInFlight || shuttingDown || disposed) return false;
      if (active !== null) {
        await sendActive(active);
        if (active !== null) return false;
      }
      const before = options.store.getState();
      if (
        (expectedTransactionVersion !== undefined
          && before.transaction_version !== expectedTransactionVersion)
        || before.loading
        || before.read_only
        || before.conflict !== null
        || before.durable_token === null
        || before.durable_token.length === 0
      ) return false;
      resetInFlight = true;
      cancelTimer();
      const requestId = createRequestId();
      if (!options.store.getState().begin_pending_reset(
        requestId,
        before.transaction_version,
      )) {
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
            if (error instanceof WorkbenchPersistenceAdapterError) break;
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
          return true;
        }
        options.store.getState().fail_pending_save(requestId, failure);
        return false;
      } finally {
        resetInFlight = false;
        if (!disposed && !shuttingDown && eligible()) void flush();
      }
    },
    shutdown: () => {
      if (shutdownPromise !== null) return shutdownPromise;
      if (disposed) return Promise.resolve();
      shuttingDown = true;
      cancelTimer();
      shutdownPromise = (async () => {
        try {
          const activeAtShutdown = active;
          if (activeAtShutdown !== null) {
            await sendActive(activeAtShutdown);
            if (active === activeAtShutdown) {
              options.store.getState().fail_pending_save(
                activeAtShutdown.request.request_id,
                options.store.getState().save_error
                  ?? "workbench shutdown save was not acknowledged",
              );
              finishActive(activeAtShutdown);
              return;
            }
            const afterActive = options.store.getState();
            if (
              afterActive.conflict !== null
              || afterActive.read_only
              || afterActive.save_error !== null
            ) return;
          } else {
            const current = options.store.getState();
            if (
              current.conflict !== null
              || current.read_only
              || current.save_error !== null
            ) return;
          }

          if (!eligible()) return;
          await flush(true);
          if (active !== null) {
            const unacknowledged = active;
            options.store.getState().fail_pending_save(
              unacknowledged.request.request_id,
              options.store.getState().save_error
                ?? "workbench shutdown drain was not acknowledged",
            );
            finishActive(unacknowledged);
          }
        } finally {
          cancelTimer();
          unsubscribe();
          disposed = true;
          shuttingDown = false;
        }
      })();
      return shutdownPromise;
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
  queue: "inbox",
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
      ? {
          mode: "auto",
          focused_agent_id: null,
          search_query: "",
          status_filter: [],
        }
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
