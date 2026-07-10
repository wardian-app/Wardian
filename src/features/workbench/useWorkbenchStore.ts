import { createStore, type StoreApi } from "zustand/vanilla";

import type { WorkbenchDocumentV1, WorkbenchValidationError } from "../../types";
import {
  applyWorkbenchCommand,
  createDefaultWorkbenchDocument,
  type WorkbenchCommand,
  validateWorkbenchDocument,
} from "./workbenchModel";

export type WorkbenchStoreOptions = {
  initial_document?: WorkbenchDocumentV1;
  durable_revision?: number;
  durable_token?: string | null;
  now?: () => string;
  create_default_document?: () => WorkbenchDocumentV1;
};

export type WorkbenchMutationResult =
  | {
      accepted: true;
      document: WorkbenchDocumentV1;
      stale: false;
    }
  | {
      accepted: false;
      document: WorkbenchDocumentV1;
      errors: WorkbenchValidationError[];
      stale: boolean;
    };

export type WorkbenchStoreState = {
  document: WorkbenchDocumentV1;
  durable_document: WorkbenchDocumentV1;
  zoomed_group_id: string | null;
  launcher_open: boolean;
  transaction_version: number;
  durable_revision: number;
  durable_token: string | null;
  is_dirty: boolean;
  save_pending: boolean;
  pending_request_id: string | null;
  pending_revision: number | null;
  pending_document: WorkbenchDocumentV1 | null;
  pending_expected_token: string | null;
  pending_transaction_version: number | null;
  conflict: string | null;
  loading: boolean;
  read_only: boolean;
  save_error: string | null;
  apply_commands: (commands: readonly WorkbenchCommand[]) => WorkbenchMutationResult;
  compare_and_apply_commands: (
    expectedTransactionVersion: number,
    commands: readonly WorkbenchCommand[],
  ) => WorkbenchMutationResult;
  reset_document: () => WorkbenchMutationResult;
  compare_and_reset_document: (
    expectedTransactionVersion: number,
  ) => WorkbenchMutationResult;
  set_zoomed_group_id: (groupId: string | null) => void;
  set_launcher_open: (open: boolean) => void;
  begin_pending_save: (requestId: string) => boolean;
  acknowledge_pending_save: (
    requestId: string,
    revision: number,
    durableToken: string,
  ) => boolean;
  fail_pending_save: (requestId: string, error: string) => boolean;
  set_conflict: (conflict: string | null) => void;
  set_loading: (loading: boolean) => void;
  set_read_only: (readOnly: boolean) => void;
  set_save_error: (error: string | null) => void;
};

export type WorkbenchStore = Pick<
  StoreApi<WorkbenchStoreState>,
  "getState" | "getInitialState" | "subscribe"
>;

function rejected(
  document: WorkbenchDocumentV1,
  errors: WorkbenchValidationError[],
  stale = false,
): WorkbenchMutationResult {
  return { accepted: false, document, errors, stale };
}

function accepted(document: WorkbenchDocumentV1): WorkbenchMutationResult {
  return { accepted: true, document, stale: false };
}

function nextRevision(durableRevision: number): number | null {
  return Number.isSafeInteger(durableRevision) && durableRevision < Number.MAX_SAFE_INTEGER
    ? durableRevision + 1
    : null;
}

function deepFreeze<T>(value: T, seen = new WeakSet<object>()): T {
  if (typeof value !== "object" || value === null || seen.has(value)) return value;
  seen.add(value);
  for (const key of Reflect.ownKeys(value)) {
    const descriptor = Object.getOwnPropertyDescriptor(value, key);
    if (descriptor && "value" in descriptor) deepFreeze(descriptor.value, seen);
  }
  return Object.freeze(value);
}

function cloneAndFreezeDocument(document: WorkbenchDocumentV1): WorkbenchDocumentV1 {
  return deepFreeze(structuredClone(document));
}

export function createWorkbenchStore(
  options: WorkbenchStoreOptions = {},
): WorkbenchStore {
  const suppliedInitialDocument = options.initial_document ?? createDefaultWorkbenchDocument();
  const validation = validateWorkbenchDocument(suppliedInitialDocument);
  if (!validation.valid) {
    throw new Error(
      `initial workbench document is invalid: ${validation.errors
        .map((error) => `${error.path} ${error.message}`)
        .join(", ")}`,
    );
  }
  const initialDocument = cloneAndFreezeDocument(suppliedInitialDocument);

  const initialDurableRevision = options.durable_revision ?? initialDocument.revision;
  if (
    !Number.isSafeInteger(initialDurableRevision)
    || initialDurableRevision < 0
    || initialDurableRevision !== initialDocument.revision
  ) {
    throw new Error("durable_revision must equal document.revision at initialization");
  }
  const now = options.now ?? (() => new Date().toISOString());
  const createDefaultDocument = options.create_default_document ?? createDefaultWorkbenchDocument;

  const zustandStore = createStore<WorkbenchStoreState>((set, get) => {
    const reduceCommands = (
      current: WorkbenchStoreState,
      commands: readonly WorkbenchCommand[],
    ): WorkbenchMutationResult => {
      if (current.loading || current.read_only) {
        return rejected(current.document, [{
          path: "$.durability",
          message: current.loading ? "workbench is loading" : "workbench is read-only",
        }]);
      }
      if (commands.length === 0) {
        return rejected(current.document, [{
          path: "$.command",
          message: "a workbench transaction requires at least one command",
        }]);
      }
      const revision = current.is_dirty
        ? current.document.revision
        : nextRevision(current.durable_revision);
      if (revision === null) {
        return rejected(current.document, [{
          path: "$.revision",
          message: "durable revision cannot be incremented safely",
        }]);
      }

      let candidate: WorkbenchDocumentV1 = {
        ...current.document,
        revision,
        saved_at: now(),
      };
      for (const command of commands) {
        const result = applyWorkbenchCommand(candidate, command);
        if (!result.accepted) return rejected(current.document, result.errors);
        candidate = result.document;
      }
      return accepted(cloneAndFreezeDocument(candidate));
    };

    const compareAndApply = (
      expectedTransactionVersion: number,
      commands: readonly WorkbenchCommand[],
    ): WorkbenchMutationResult => {
      let outcome: WorkbenchMutationResult | undefined;
      set((current) => {
        if (current.transaction_version !== expectedTransactionVersion) {
          outcome = rejected(current.document, [{
            path: "$.transaction_version",
            message: "workbench transaction is stale",
          }], true);
          return current;
        }
        outcome = reduceCommands(current, commands);
        if (!outcome.accepted) return current;
        return {
          ...current,
          document: outcome.document,
          is_dirty: true,
          save_error: null,
          transaction_version: current.transaction_version + 1,
        };
      });
      if (!outcome) throw new Error("workbench command transaction did not run");
      return outcome;
    };

    const compareAndReset = (
      expectedTransactionVersion: number,
    ): WorkbenchMutationResult => {
      let outcome: WorkbenchMutationResult | undefined;
      set((current) => {
        if (current.transaction_version !== expectedTransactionVersion) {
          outcome = rejected(current.document, [{
            path: "$.transaction_version",
            message: "workbench reset is stale",
          }], true);
          return current;
        }
        if (current.loading || current.read_only) {
          outcome = rejected(current.document, [{
            path: "$.durability",
            message: current.loading ? "workbench is loading" : "workbench is read-only",
          }]);
          return current;
        }
        const revision = current.is_dirty
          ? current.document.revision
          : nextRevision(current.durable_revision);
        if (revision === null) {
          outcome = rejected(current.document, [{
            path: "$.revision",
            message: "durable revision cannot be incremented safely",
          }]);
          return current;
        }
        let defaultDocument: WorkbenchDocumentV1;
        try {
          defaultDocument = createDefaultDocument();
        } catch (error) {
          outcome = rejected(current.document, [{
            path: "$.default_document",
            message: error instanceof Error ? error.message : String(error),
          }]);
          return current;
        }
        const defaultValidation = validateWorkbenchDocument(defaultDocument);
        if (!defaultValidation.valid) {
          outcome = rejected(current.document, defaultValidation.errors);
          return current;
        }
        const candidate: WorkbenchDocumentV1 = {
          ...cloneAndFreezeDocument(defaultDocument),
          revision,
          saved_at: now(),
        };
        const result = applyWorkbenchCommand(candidate, {
          type: "update_shell",
          patch: {},
        });
        if (!result.accepted) {
          outcome = rejected(current.document, result.errors);
          return current;
        }
        const document = cloneAndFreezeDocument(result.document);
        outcome = accepted(document);
        return {
          ...current,
          document,
          zoomed_group_id: null,
          launcher_open: false,
          is_dirty: true,
          save_error: null,
          transaction_version: current.transaction_version + 1,
        };
      });
      if (!outcome) throw new Error("workbench reset transaction did not run");
      return outcome;
    };

    return {
      document: initialDocument,
      durable_document: initialDocument,
      zoomed_group_id: null,
      launcher_open: false,
      transaction_version: 0,
      durable_revision: initialDurableRevision,
      durable_token: options.durable_token ?? null,
      is_dirty: false,
      save_pending: false,
      pending_request_id: null,
      pending_revision: null,
      pending_document: null,
      pending_expected_token: null,
      pending_transaction_version: null,
      conflict: null,
      loading: false,
      read_only: false,
      save_error: null,

      apply_commands: (commands) => compareAndApply(get().transaction_version, commands),
      compare_and_apply_commands: compareAndApply,
      reset_document: () => compareAndReset(get().transaction_version),
      compare_and_reset_document: compareAndReset,

      set_zoomed_group_id: (groupId) => {
        if (groupId !== null && !(groupId in get().document.groups)) {
          throw new Error(`group ${groupId} does not exist`);
        }
        set((current) => current.zoomed_group_id === groupId
          ? current
          : {
              ...current,
              zoomed_group_id: groupId,
              transaction_version: current.transaction_version + 1,
            });
      },

      set_launcher_open: (open) => set((current) => current.launcher_open === open
        ? current
        : {
            ...current,
            launcher_open: open,
            transaction_version: current.transaction_version + 1,
          }),

      begin_pending_save: (requestId) => {
        let begun = false;
        set((current) => {
          if (
            requestId.length === 0
            || !current.is_dirty
            || current.pending_request_id !== null
            || current.loading
            || current.read_only
            || current.conflict !== null
          ) return current;
          begun = true;
          return {
            ...current,
            save_pending: true,
            pending_request_id: requestId,
            pending_revision: current.document.revision,
            pending_document: current.document,
            pending_expected_token: current.durable_token,
            pending_transaction_version: current.transaction_version,
            save_error: null,
            transaction_version: current.transaction_version + 1,
          };
        });
        return begun;
      },

      acknowledge_pending_save: (requestId, revision, durableToken) => {
        let acknowledged = false;
        set((current) => {
          if (
            current.pending_request_id !== requestId
            || current.pending_revision !== revision
            || current.pending_document === null
          ) return current;

          const durableDocument = current.pending_document;
          const hasNewerWorkingDocument = current.document !== durableDocument;
          let workingDocument = durableDocument;
          if (hasNewerWorkingDocument) {
            const workingRevision = nextRevision(revision);
            if (workingRevision === null) return current;
            const candidate: WorkbenchDocumentV1 = {
              ...current.document,
              revision: workingRevision,
              saved_at: now(),
            };
            const result = applyWorkbenchCommand(candidate, {
              type: "update_shell",
              patch: {},
            });
            if (!result.accepted) return current;
            workingDocument = cloneAndFreezeDocument(result.document);
          }

          acknowledged = true;
          return {
            ...current,
            document: workingDocument,
            durable_document: durableDocument,
            durable_revision: revision,
            durable_token: durableToken,
            is_dirty: hasNewerWorkingDocument,
            save_pending: false,
            pending_request_id: null,
            pending_revision: null,
            pending_document: null,
            pending_expected_token: null,
            pending_transaction_version: null,
            conflict: null,
            save_error: null,
            transaction_version: current.transaction_version + 1,
          };
        });
        return acknowledged;
      },

      fail_pending_save: (requestId, error) => {
        let failed = false;
        set((current) => {
          if (current.pending_request_id !== requestId) return current;
          failed = true;
          return {
            ...current,
            save_pending: false,
            pending_request_id: null,
            pending_revision: null,
            pending_document: null,
            pending_expected_token: null,
            pending_transaction_version: null,
            save_error: error,
            transaction_version: current.transaction_version + 1,
          };
        });
        return failed;
      },

      set_conflict: (conflict) => set((current) => current.conflict === conflict
        ? current
        : {
            ...current,
            conflict,
            transaction_version: current.transaction_version + 1,
          }),

      set_loading: (loading) => set((current) => current.loading === loading
        ? current
        : {
            ...current,
            loading,
            transaction_version: current.transaction_version + 1,
          }),

      set_read_only: (readOnly) => set((current) => current.read_only === readOnly
        ? current
        : {
            ...current,
            read_only: readOnly,
            transaction_version: current.transaction_version + 1,
          }),

      set_save_error: (error) => set((current) => current.save_error === error
        ? current
        : {
            ...current,
            save_error: error,
            transaction_version: current.transaction_version + 1,
          }),
    };
  });

  return {
    getState: zustandStore.getState,
    getInitialState: zustandStore.getInitialState,
    subscribe: zustandStore.subscribe,
  };
}
