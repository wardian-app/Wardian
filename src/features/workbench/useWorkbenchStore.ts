import { createStore, type StoreApi } from "zustand/vanilla";

import type {
  WorkbenchCommandResult,
  WorkbenchDocumentV1,
  WorkbenchValidationError,
} from "../../types";
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
};

export type WorkbenchStoreState = {
  document: WorkbenchDocumentV1;
  zoomed_group_id: string | null;
  launcher_open: boolean;
  durable_revision: number;
  durable_token: string | null;
  save_pending: boolean;
  save_error: string | null;
  apply_commands: (commands: readonly WorkbenchCommand[]) => WorkbenchCommandResult;
  reset_document: () => WorkbenchCommandResult;
  set_zoomed_group_id: (groupId: string | null) => void;
  set_launcher_open: (open: boolean) => void;
  acknowledge_durable: (revision: number, token: string) => void;
  set_save_error: (error: string | null) => void;
};

export type WorkbenchStore = Pick<
  StoreApi<WorkbenchStoreState>,
  "getState" | "getInitialState" | "subscribe"
>;

function rejected(
  document: WorkbenchDocumentV1,
  errors: WorkbenchValidationError[],
): WorkbenchCommandResult {
  return { accepted: false, document, errors };
}

function nextRevision(durableRevision: number): number | null {
  return Number.isSafeInteger(durableRevision) && durableRevision < Number.MAX_SAFE_INTEGER
    ? durableRevision + 1
    : null;
}

export function createWorkbenchStore(
  options: WorkbenchStoreOptions = {},
): WorkbenchStore {
  const initialDocument = options.initial_document ?? createDefaultWorkbenchDocument();
  const validation = validateWorkbenchDocument(initialDocument);
  if (!validation.valid) {
    throw new Error(
      `initial workbench document is invalid: ${validation.errors
        .map((error) => `${error.path} ${error.message}`)
        .join(", ")}`,
    );
  }

  const initialDurableRevision = options.durable_revision ?? initialDocument.revision;
  if (
    !Number.isSafeInteger(initialDurableRevision)
    || initialDurableRevision < 0
    || initialDurableRevision > initialDocument.revision
  ) {
    throw new Error("durable_revision must be within 0..document.revision");
  }
  const now = options.now ?? (() => new Date().toISOString());

  const zustandStore = createStore<WorkbenchStoreState>((set, get) => {
    const publish = (document: WorkbenchDocumentV1): WorkbenchCommandResult => {
      set({ document, save_pending: true, save_error: null });
      return { accepted: true, document };
    };

    const stagedBase = (): WorkbenchDocumentV1 | WorkbenchCommandResult => {
      const current = get();
      const revision = nextRevision(current.durable_revision);
      if (revision === null) {
        return rejected(current.document, [{
          path: "$.revision",
          message: "durable revision cannot be incremented safely",
        }]);
      }
      return {
        ...current.document,
        revision,
        saved_at: now(),
      };
    };

    return {
      document: initialDocument,
      zoomed_group_id: null,
      launcher_open: false,
      durable_revision: initialDurableRevision,
      durable_token: options.durable_token ?? null,
      save_pending: initialDocument.revision > initialDurableRevision,
      save_error: null,

      apply_commands: (commands) => {
        const original = get().document;
        if (commands.length === 0) {
          return rejected(original, [{
            path: "$.command",
            message: "a workbench transaction requires at least one command",
          }]);
        }
        const base = stagedBase();
        if ("accepted" in base) return base;

        let candidate = base;
        for (const command of commands) {
          const result = applyWorkbenchCommand(candidate, command);
          if (!result.accepted) return rejected(original, result.errors);
          candidate = result.document;
        }
        return publish(candidate);
      },

      reset_document: () => {
        const original = get().document;
        const revision = nextRevision(get().durable_revision);
        if (revision === null) {
          return rejected(original, [{
            path: "$.revision",
            message: "durable revision cannot be incremented safely",
          }]);
        }
        const candidate: WorkbenchDocumentV1 = {
          ...createDefaultWorkbenchDocument(),
          revision,
          saved_at: now(),
        };
        const result = applyWorkbenchCommand(candidate, {
          type: "update_shell",
          patch: {},
        });
        if (!result.accepted) return rejected(original, result.errors);
        set({ zoomed_group_id: null, launcher_open: false });
        return publish(result.document);
      },

      set_zoomed_group_id: (groupId) => {
        if (groupId !== null && !(groupId in get().document.groups)) {
          throw new Error(`group ${groupId} does not exist`);
        }
        set({ zoomed_group_id: groupId });
      },

      set_launcher_open: (open) => set({ launcher_open: open }),

      acknowledge_durable: (revision, token) => {
        const current = get();
        if (
          !Number.isSafeInteger(revision)
          || revision < current.durable_revision
          || revision > current.document.revision
        ) {
          throw new Error("durable acknowledgement revision is outside the working range");
        }
        set({
          durable_revision: revision,
          durable_token: token,
          save_pending: current.document.revision > revision,
          save_error: null,
        });
      },

      set_save_error: (error) => set({ save_error: error }),
    };
  });

  return {
    getState: zustandStore.getState,
    getInitialState: zustandStore.getInitialState,
    subscribe: zustandStore.subscribe,
  };
}
