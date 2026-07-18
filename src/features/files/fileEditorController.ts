import type {
  CheckpointFileRecoveryRequestV1,
  DiscardFileRecoveryRequestV1,
  FileRecoveryCheckpointV1,
  FileRecoveryMergeResultV1,
  FileRecoverySummaryV1,
  FileRecoveryV1,
  FileResourceSaveResultV1,
  FileResourceSnapshotV1,
  FileResourceTextV1,
  ListFileRecoveriesRequestV1,
  SaveFileResourceTextRequestV1,
} from "../../types";
import type { FileResourceClient } from "./fileResourceClient";

export type FileEditorResourceClient = Pick<
  FileResourceClient,
  | "saveText"
  | "readText"
  | "checkpointRecovery"
  | "listRecoveries"
  | "getRecovery"
  | "discardRecovery"
  | "mergeRecovery"
>;

export type FileEditorRecoveryState =
  | { readonly status: "none" }
  | {
      readonly status: "checkpointing";
      readonly recovery_id: string | null;
      readonly recovery_revision: number | null;
      readonly buffer_generation: number;
    }
  | {
      readonly status: "durable";
      readonly recovery_id: string;
      readonly recovery_revision: number;
      readonly buffer_generation: number;
    }
  | {
      readonly status: "error";
      readonly recovery_id: string | null;
      readonly recovery_revision: number | null;
      readonly buffer_generation: number;
      readonly retryable: true;
      readonly message: string;
    }
  | {
      readonly status: "conflict";
      readonly conflicting_recovery_id: string;
      readonly conflicting_recovery_revision: number;
      readonly current_recovery_id: string | null;
      readonly current_recovery_revision: number | null;
      readonly current_durable: boolean;
      readonly buffer_generation: number;
      readonly message: string;
    };

export type FileEditorRecoveryConflictChoice = "keep_current" | "use_recovered";

export type FileEditorAuthorizationState =
  | { readonly status: "available" }
  | {
      readonly status: "unavailable";
      readonly code: "unauthorized_path" | "unauthorized_resource" | "resource_not_found";
      readonly message: string;
    };

export type FileEditorSnapshot = {
  readonly resource_key: string;
  readonly status: "uninitialized" | "ready";
  readonly resource_id: string | null;
  readonly subscription_id: string | null;
  readonly base_revision: number | null;
  readonly buffer_base_hash: string | null;
  readonly disk_head_revision: number | null;
  readonly disk_head_hash: string | null;
  readonly saved_text: string;
  readonly working_text: string;
  readonly dirty: boolean;
  readonly stale: boolean;
  readonly authorization: FileEditorAuthorizationState;
  readonly save_state: "idle" | "saving" | "error";
  readonly recovery: FileEditorRecoveryState;
  readonly recovery_discovery:
    | "not_started"
    | "discovering"
    | "complete"
    | "error"
    | "conflict";
  readonly buffer_generation: number;
  readonly presentation_generation: number;
  readonly presentation_ids: readonly string[];
  readonly last_error: string | null;
};

export type FileEditorPresentationCallbacks = {
  readonly on_pin?: () => void;
  readonly on_open_comparison?: () => void;
};

export type FileEditorPresentationMembership = {
  readonly generation: number;
  readonly detach: () => void;
};

export type FileEditorControllerOptions = {
  readonly checkpoint_debounce_ms?: number;
};

export type FileEditorInitialization = {
  readonly owner: FileResourceSnapshotV1;
  readonly text: string;
  readonly discover_recovery?: boolean;
};

type RecoveryMetadata = {
  recovery_id: string;
  recovery_revision: number;
  buffer_generation: number;
  needs_reconciliation: boolean;
};

type RecoveryCas = {
  recovery_id: string;
  expected_recovery_revision: number;
};

type PresentationEntry = {
  generation: number;
  callbacks: FileEditorPresentationCallbacks;
};

type PresentationOwnerCandidate = {
  owner: FileResourceSnapshotV1;
  text: string;
  order: number;
};

type ExactDiskHead = {
  owner_generation: number;
  resource_id: string;
  subscription_id: string;
  revision: number;
  content_hash: string;
  text: string;
};

type ExactDiskOperation = ExactDiskHead & {
  buffer_generation: number;
};

const DEFAULT_CHECKPOINT_DEBOUNCE_MS = 750;

function errorMessage(error: unknown): string {
  if (error instanceof Error) return error.message;
  if (typeof error === "object" && error !== null && "message" in error) {
    return String(error.message);
  }
  return String(error);
}

function errorCode(error: unknown): string | null {
  if (typeof error === "object" && error !== null && "code" in error) {
    return String(error.code);
  }
  return null;
}

function isAuthorizationUnavailable(error: unknown): boolean {
  return [
    "unauthorized_path",
    "unauthorized_resource",
    "resource_not_found",
  ].includes(errorCode(error) ?? "");
}

function unavailableAuthorization(error: unknown): FileEditorAuthorizationState | null {
  if (!isAuthorizationUnavailable(error)) return null;
  return Object.freeze({
    status: "unavailable" as const,
    code: errorCode(error) as "unauthorized_path" | "unauthorized_resource" | "resource_not_found",
    message: errorMessage(error),
  });
}

function newestRecovery(
  recoveries: readonly FileRecoverySummaryV1[],
): FileRecoverySummaryV1 | undefined {
  return [...recoveries].sort((left, right) => (
    right.updated_at_ms - left.updated_at_ms
    || right.recovery_revision - left.recovery_revision
    || right.recovery_id.localeCompare(left.recovery_id)
  ))[0];
}

/**
 * Resource-owned, framework-independent editor state. Its immutable snapshot
 * and synchronous subscription contract are safe for `useSyncExternalStore`.
 */
export class FileEditorController {
  readonly #resourceKey: string;
  readonly #client: FileEditorResourceClient;
  readonly #checkpointDebounceMs: number;
  readonly #listeners = new Set<() => void>();
  readonly #disposeListeners = new Set<() => void>();
  readonly #presentations = new Map<string, PresentationEntry>();
  readonly #presentationOwners = new Map<string, PresentationOwnerCandidate>();
  readonly #presentationObservations = new Map<string, number>();
  readonly #pinnedPresentations = new Set<string>();
  #snapshot: FileEditorSnapshot;
  #owner: FileResourceSnapshotV1 | null = null;
  #ownerPresentationId: string | null = null;
  #ownerCandidateOrder = 0;
  #ownerGeneration = 0;
  #exactDiskHead: ExactDiskHead | null = null;
  #baseRevision: number | null = null;
  #initializationGeneration = 0;
  #pendingInitializationCount = 0;
  #saveOperationCount = 0;
  #recoveryMetadata: RecoveryMetadata | null = null;
  #recoveryConflict: FileRecoveryV1 | null = null;
  #checkpointTimer: ReturnType<typeof setTimeout> | null = null;
  #checkpointRequestedGeneration: number | null = null;
  #checkpointLoop: Promise<void> | null = null;
  readonly #durableQueue: Array<() => void> = [];
  #durableBusy = false;
  #durableOperationCount = 0;
  #disposed = false;

  constructor(
    resourceKey: string,
    client: FileEditorResourceClient,
    options: FileEditorControllerOptions = {},
  ) {
    this.#resourceKey = resourceKey;
    this.#client = client;
    this.#checkpointDebounceMs = Math.max(
      0,
      options.checkpoint_debounce_ms ?? DEFAULT_CHECKPOINT_DEBOUNCE_MS,
    );
    this.#snapshot = Object.freeze({
      resource_key: resourceKey,
      status: "uninitialized",
      resource_id: null,
      subscription_id: null,
      base_revision: null,
      buffer_base_hash: null,
      disk_head_revision: null,
      disk_head_hash: null,
      saved_text: "",
      working_text: "",
      dirty: false,
      stale: false,
      authorization: Object.freeze({ status: "available" }),
      save_state: "idle",
      recovery: Object.freeze({ status: "none" }),
      recovery_discovery: "not_started",
      buffer_generation: 0,
      presentation_generation: 0,
      presentation_ids: Object.freeze([]),
      last_error: null,
    });
  }

  getSnapshot = (): FileEditorSnapshot => this.#snapshot;

  subscribe = (listener: () => void): (() => void) => {
    this.#listeners.add(listener);
    return () => this.#listeners.delete(listener);
  };

  /** Retains editor-model resources for exactly this controller's registry lifetime. */
  onDispose = (listener: () => void): (() => void) => {
    if (this.#disposed) {
      listener();
      return () => undefined;
    }
    this.#disposeListeners.add(listener);
    return () => this.#disposeListeners.delete(listener);
  };

  dispose(): void {
    if (this.#disposed) return;
    this.#disposed = true;
    this.#clearCheckpointTimer();
    for (const listener of this.#disposeListeners) listener();
    this.#disposeListeners.clear();
    this.#listeners.clear();
  }

  attachPresentation(
    surfaceId: string,
    callbacks: FileEditorPresentationCallbacks,
  ): FileEditorPresentationMembership {
    const generation = (this.#presentationObservations.get(surfaceId) ?? 0) + 1;
    this.#presentationObservations.set(surfaceId, generation);
    this.#presentations.set(surfaceId, { generation, callbacks });
    if (this.#snapshot.dirty && !this.#pinnedPresentations.has(surfaceId)) {
      this.#pinnedPresentations.add(surfaceId);
      callbacks.on_pin?.();
    }
    this.#publishMembership();
    let attached = true;
    return {
      generation,
      detach: () => {
        if (!attached) return;
        attached = false;
        const current = this.#presentations.get(surfaceId);
        if (current?.generation !== generation) return;
        this.#presentations.delete(surfaceId);
        this.#presentationOwners.delete(surfaceId);
        if (this.#ownerPresentationId === surfaceId) {
          this.#promotePresentationOwner();
        }
        this.#presentationObservations.set(surfaceId, generation + 1);
        this.#publishMembership();
      },
    };
  }

  observePresentation(surfaceId: string): { attached: boolean; generation: number } {
    const entry = this.#presentations.get(surfaceId);
    return {
      attached: entry !== undefined,
      generation: entry?.generation ?? this.#presentationObservations.get(surfaceId) ?? 0,
    };
  }

  /**
   * Retains one live authorization candidate per presenting pane. Detaching
   * the active candidate promotes the newest remaining pane before later
   * guarded writes or recovery checkpoints acquire their operation snapshot.
   */
  bindPresentationOwner(
    surfaceId: string,
    owner: FileResourceSnapshotV1,
    text: string,
    makeCurrent = true,
  ): void {
    if (!this.#presentations.has(surfaceId)) return;
    const candidate = { owner, text, order: ++this.#ownerCandidateOrder };
    this.#presentationOwners.set(surfaceId, candidate);
    if (!makeCurrent) {
      if (this.#ownerPresentationId === null) this.#promotePresentationOwner();
      return;
    }
    this.#ownerPresentationId = surfaceId;
    this.#applyAuthoritative(owner, text);
  }

  #promotePresentationOwner(): void {
    const candidate = [...this.#presentationOwners.entries()].sort((left, right) => (
      right[1].order - left[1].order || left[0].localeCompare(right[0])
    ))[0];
    if (!candidate) {
      this.#ownerPresentationId = null;
      return;
    }
    this.#ownerPresentationId = candidate[0];
    this.#applyAuthoritative(candidate[1].owner, candidate[1].text);
  }

  async initialize(input: FileEditorInitialization): Promise<void> {
    this.#pendingInitializationCount += 1;
    const initializationGeneration = ++this.#initializationGeneration;
    try {
      const alreadyDirty = this.#snapshot.dirty;
      this.#applyAuthoritative(input.owner, input.text);
      const bufferGeneration = this.#snapshot.buffer_generation;
      if (input.discover_recovery === false) {
        if (this.#snapshot.recovery_discovery === "not_started") {
          this.#publish({ recovery_discovery: "complete" });
        }
        return;
      }
      if (this.#snapshot.recovery_discovery === "complete") return;
      if (this.#snapshot.recovery_discovery === "conflict") {
        await this.flushRecovery();
        return;
      }
      this.#publish({ recovery_discovery: "discovering" });

      let recoveries: readonly FileRecoverySummaryV1[];
      try {
        recoveries = await this.#client.listRecoveries({
          resource_key: this.#resourceKey,
        } satisfies ListFileRecoveriesRequestV1);
      } catch (error) {
        if (initializationGeneration === this.#initializationGeneration) {
          this.#publish({
            recovery_discovery: "error",
            last_error: errorMessage(error),
          });
        }
        throw error;
      }
      if (initializationGeneration !== this.#initializationGeneration) return;
      const newest = newestRecovery(recoveries);
      if (!newest) {
        this.#publish({ recovery_discovery: "complete", last_error: null });
        if (this.#snapshot.dirty) await this.flushRecovery();
        return;
      }
      let recovered: FileRecoveryV1;
      try {
        recovered = await this.#client.getRecovery({
          recovery_id: newest.recovery_id,
          resource_key: this.#resourceKey,
        });
      } catch (error) {
        if (initializationGeneration === this.#initializationGeneration) {
          this.#publish({
            recovery_discovery: "error",
            last_error: errorMessage(error),
          });
        }
        throw error;
      }
      if (initializationGeneration !== this.#initializationGeneration) return;
      if (alreadyDirty || bufferGeneration !== this.#snapshot.buffer_generation) {
        await this.#enterRecoveryConflict(recovered);
        return;
      }
      this.#applyRecovery(recovered);
      this.#publish({ recovery_discovery: "complete" });
    } finally {
      this.#pendingInitializationCount = Math.max(0, this.#pendingInitializationCount - 1);
      this.#publish({});
    }
  }

  applyAuthoritative(owner: FileResourceSnapshotV1, text: string): void {
    this.#initializationGeneration += 1;
    this.#applyAuthoritative(owner, text);
  }

  #applyAuthoritative(owner: FileResourceSnapshotV1, text: string): void {
    if (owner.resource_id !== this.#resourceKey) {
      throw new Error("The authoritative file does not match this editor resource.");
    }
    if (
      this.#owner?.subscription_id === owner.subscription_id
      && this.#snapshot.disk_head_revision !== null
      && owner.revision <= this.#snapshot.disk_head_revision
    ) return;
    if (this.#owner?.subscription_id !== owner.subscription_id) {
      this.#ownerGeneration += 1;
    }
    this.#owner = owner;
    const diskHeadHash = owner.descriptor.content_hash;
    this.#exactDiskHead = {
      owner_generation: this.#ownerGeneration,
      resource_id: owner.resource_id,
      subscription_id: owner.subscription_id,
      revision: owner.revision,
      content_hash: diskHeadHash,
      text,
    };
    if (this.#snapshot.status === "uninitialized" || !this.#snapshot.dirty) {
      this.#baseRevision = owner.revision;
      this.#publish({
        status: "ready",
        resource_id: owner.resource_id,
        subscription_id: owner.subscription_id,
        base_revision: owner.revision,
        buffer_base_hash: diskHeadHash,
        disk_head_revision: owner.revision,
        disk_head_hash: diskHeadHash,
        saved_text: text,
        working_text: text,
        dirty: false,
        stale: false,
        authorization: Object.freeze({ status: "available" }),
        save_state: "idle",
        last_error: null,
      });
      return;
    }
    const matchesBufferBase = diskHeadHash === this.#snapshot.buffer_base_hash;
    if (matchesBufferBase) this.#baseRevision = owner.revision;
    this.#publish({
      resource_id: owner.resource_id,
      subscription_id: owner.subscription_id,
      ...(matchesBufferBase ? { base_revision: owner.revision } : {}),
      disk_head_revision: owner.revision,
      disk_head_hash: diskHeadHash,
      authorization: Object.freeze({ status: "available" }),
      stale: !matchesBufferBase,
    });
  }

  mutate(text: string): number {
    if (this.#snapshot.status !== "ready") {
      throw new Error("The file editor is not initialized.");
    }
    if (this.#snapshot.authorization.status === "unavailable") {
      // Monaco may deliver one final model event while its read-only option is
      // being applied. Re-publish the controller snapshot so the canonical
      // model rolls back to the last accepted bytes.
      this.#publish({});
      return this.#snapshot.buffer_generation;
    }
    if (text === this.#snapshot.working_text) {
      if (
        !this.#recoveryConflict
        && !this.#snapshot.dirty
        && this.#snapshot.recovery.status !== "none"
      ) {
        void this.#retireRecoveryForCleanGeneration(this.#snapshot.buffer_generation)
          .catch(() => undefined);
      }
      return this.#snapshot.buffer_generation;
    }
    const bufferGeneration = this.#snapshot.buffer_generation + 1;
    const dirty = text !== this.#snapshot.saved_text;
    const exactDiskHead = !dirty
      && !this.#recoveryConflict
      ? this.#newerExactDiskHead()
      : null;
    if (exactDiskHead) {
      this.#baseRevision = exactDiskHead.revision;
      this.#clearCheckpointTimer();
      this.#publish({
        base_revision: exactDiskHead.revision,
        buffer_base_hash: exactDiskHead.content_hash,
        disk_head_revision: exactDiskHead.revision,
        disk_head_hash: exactDiskHead.content_hash,
        saved_text: exactDiskHead.text,
        working_text: exactDiskHead.text,
        dirty: false,
        stale: false,
        buffer_generation: bufferGeneration,
        save_state: this.#snapshot.save_state === "error" ? "idle" : this.#snapshot.save_state,
        last_error: null,
      });
      void this.#retireRecoveryForCleanGeneration(bufferGeneration).catch(() => undefined);
      return bufferGeneration;
    }
    this.#publish({
      working_text: text,
      dirty,
      stale: dirty && this.#snapshot.disk_head_hash !== this.#snapshot.buffer_base_hash,
      buffer_generation: bufferGeneration,
      save_state: this.#snapshot.save_state === "error" ? "idle" : this.#snapshot.save_state,
      last_error: null,
    });
    if (dirty) {
      this.#pinPresentations();
      if (this.#recoveryConflict) {
        this.#publish({
          recovery: this.#recoveryConflictState(
            false,
            "Recovered changes conflict with newer in-memory edits. Both versions were preserved.",
          ),
        });
      }
      this.#scheduleCheckpoint(bufferGeneration);
    } else {
      this.#clearCheckpointTimer();
      if (this.#recoveryConflict) {
        this.#publish({
          recovery: this.#recoveryConflictState(
            this.#checkpointLoop === null,
            "Recovered changes conflict with newer in-memory edits. Both versions were preserved.",
          ),
        });
      } else {
        void this.#retireRecoveryForCleanGeneration(bufferGeneration).catch(() => undefined);
      }
    }
    return bufferGeneration;
  }

  /** Keeps the stale working buffer byte-for-byte; closing comparison is surface state only. */
  keepWorkingBuffer(): void {
    // Intentionally no publication: this choice must be a pure no-op.
  }

  /**
   * Reloads the exact currently authorized disk head and retires recovery before
   * adopting it. Every owner/buffer/recovery generation is revalidated after IO.
   */
  async reloadFromDisk(): Promise<void> {
    const operation = this.#captureExactDiskHead("Reload from disk");
    let exact: FileResourceTextV1;
    try {
      exact = await this.#client.readText({
        resource_id: operation.resource_id,
        subscription_id: operation.subscription_id,
        revision: operation.revision,
      });
    } catch (error) {
      this.#markAuthorizationUnavailable(error, operation.owner_generation);
      throw error;
    }
    if (
      exact.resource_id !== operation.resource_id
      || exact.revision !== operation.revision
    ) throw new Error("Reload from disk returned a different file revision.");

    await this.#enqueueDurable(async () => {
      this.#assertExactDiskHead(operation, "Reload from disk");
      const recovery = this.#recoveryMetadata;
      if (recovery) {
        await this.#client.discardRecovery({
          recovery_id: recovery.recovery_id,
          expected_recovery_revision: recovery.recovery_revision,
          resource_key: this.#resourceKey,
        });
        if (
          this.#recoveryMetadata?.recovery_id === recovery.recovery_id
          && this.#recoveryMetadata.recovery_revision === recovery.recovery_revision
        ) this.#recoveryMetadata = null;
      }
      try {
        this.#assertExactDiskHead(operation, "Reload from disk");
      } catch (error) {
        if (recovery && this.#recoveryMetadata === null && this.#snapshot.dirty) {
          this.#publish({ recovery: Object.freeze({ status: "none" }) });
          this.#checkpointRequestedGeneration = this.#snapshot.buffer_generation;
          void this.#ensureCheckpointLoop().catch(() => undefined);
        }
        throw error;
      }
      this.#baseRevision = operation.revision;
      this.#recoveryConflict = null;
      this.#clearCheckpointTimer();
      this.#publish({
        base_revision: operation.revision,
        buffer_base_hash: operation.content_hash,
        disk_head_revision: operation.revision,
        disk_head_hash: operation.content_hash,
        saved_text: exact.text,
        working_text: exact.text,
        dirty: false,
        stale: false,
        buffer_generation: this.#snapshot.buffer_generation + 1,
        recovery: Object.freeze({ status: "none" }),
        recovery_discovery: "complete",
        last_error: null,
      });
    });
  }

  /**
   * Rebases the exact durable working generation onto the exact authorized disk
   * head. A clean merge updates only the local editor base/buffer; Save remains
   * the sole filesystem write path.
   */
  async mergeStaleBuffer(): Promise<FileRecoveryMergeResultV1> {
    await this.flushRecovery();
    const operation = this.#captureExactDiskHead("Merge");
    const recovery = this.#recoveryMetadata;
    if (!recovery || recovery.buffer_generation !== operation.buffer_generation) {
      throw new Error("Merge requires an exact durable recovery of the current buffer.");
    }
    const result = await this.#enqueueDurable(async () => {
      this.#assertExactDiskHead(operation, "Merge");
      if (
        this.#recoveryMetadata?.recovery_id !== recovery.recovery_id
        || this.#recoveryMetadata.recovery_revision !== recovery.recovery_revision
        || this.#recoveryMetadata.buffer_generation !== recovery.buffer_generation
      ) throw new Error("The recovery changed before Merge could start.");
      let merged: FileRecoveryMergeResultV1;
      try {
        merged = await this.#client.mergeRecovery({
          recovery_id: recovery.recovery_id,
          expected_recovery_revision: recovery.recovery_revision,
          resource_key: this.#resourceKey,
          resource_id: operation.resource_id,
          subscription_id: operation.subscription_id,
        });
      } catch (error) {
        this.#markAuthorizationUnavailable(error, operation.owner_generation);
        throw error;
      }
      this.#assertExactDiskHead(operation, "Merge");
      if (
        this.#recoveryMetadata?.recovery_id !== recovery.recovery_id
        || this.#recoveryMetadata.recovery_revision !== recovery.recovery_revision
        || this.#recoveryMetadata.buffer_generation !== recovery.buffer_generation
      ) throw new Error("The recovery changed while Merge was in flight.");
      if (merged.recovery_revision !== recovery.recovery_revision) {
        throw new Error("Merge returned a different recovery generation.");
      }
      if (
        merged.current_revision !== operation.revision
        || merged.current_content_hash !== operation.content_hash
      ) throw new Error("The disk head changed while Merge was in flight.");
      if (merged.status === "conflicted") {
        this.#publish({
          last_error: "Merge has overlapping changes. The working buffer was preserved.",
        });
        return merged;
      }

      const nextGeneration = this.#snapshot.buffer_generation + 1;
      const dirty = merged.merged_text !== operation.text;
      this.#baseRevision = merged.current_revision;
      this.#publish({
        base_revision: merged.current_revision,
        buffer_base_hash: merged.current_content_hash,
        disk_head_revision: merged.current_revision,
        disk_head_hash: merged.current_content_hash,
        saved_text: operation.text,
        working_text: merged.merged_text,
        dirty,
        stale: false,
        buffer_generation: nextGeneration,
        last_error: null,
      });
      if (dirty) {
        this.#checkpointRequestedGeneration = nextGeneration;
      } else {
        void this.#retireRecoveryForCleanGeneration(nextGeneration).catch(() => undefined);
      }
      return merged;
    });
    if (result.status === "clean" && this.#snapshot.dirty) {
      await this.#ensureCheckpointLoop();
    }
    return result;
  }

  async save(presentationId?: string): Promise<FileResourceSaveResultV1> {
    if (!this.#owner || this.#baseRevision === null || this.#snapshot.buffer_base_hash === null) {
      throw new Error("The file editor has no live authorized subscription.");
    }
    if (this.#snapshot.authorization.status === "unavailable") {
      throw new Error("Restore file access before saving this buffer.");
    }
    if (
      this.#snapshot.recovery_discovery === "not_started"
      || this.#snapshot.recovery_discovery === "discovering"
      || this.#snapshot.recovery_discovery === "error"
    ) {
      throw new Error("Recovery discovery must complete before Save.");
    }
    if (this.#snapshot.recovery_discovery === "conflict") {
      throw new Error("Recovery conflict must be resolved before Save.");
    }
    const requestedOwnerGeneration = this.#ownerGeneration;
    const bufferGeneration = this.#snapshot.buffer_generation;
    const submittedText = this.#snapshot.working_text;
    this.#clearCheckpointTimer();
    this.#saveOperationCount += 1;
    this.#publish({ save_state: "saving", last_error: null });
    let failure: string | null = null;
    try {
      return await this.#enqueueDurable(async () => {
        const owner = this.#owner;
        const baseRevision = this.#baseRevision;
        const baseHash = this.#snapshot.buffer_base_hash;
        if (!owner || baseRevision === null || baseHash === null) {
          throw new Error("The file editor has no live authorized subscription.");
        }
        if (this.#ownerGeneration !== requestedOwnerGeneration) {
          throw new Error("The authorized file subscription changed before Save could start.");
        }
        if (this.#snapshot.saved_text === submittedText) {
          await this.#retireRecoveryForCleanGenerationInsideQueue(bufferGeneration);
          return {
            status: "unchanged",
            revision: baseRevision,
            content_hash: baseHash,
          } satisfies FileResourceSaveResultV1;
        }

        const operationOwnerGeneration = this.#ownerGeneration;
        const durableRecovery = this.#recoveryMetadata;
        const cleanup = durableRecovery
          && durableRecovery.buffer_generation <= bufferGeneration
          ? {
              recovery_id: durableRecovery.recovery_id,
              expected_recovery_revision: durableRecovery.recovery_revision,
            }
          : null;
        const request: SaveFileResourceTextRequestV1 = {
          resource_id: owner.resource_id,
          subscription_id: owner.subscription_id,
          expected_revision: baseRevision,
          buffer_base_hash: baseHash,
          text: submittedText,
          recovery_cleanup: cleanup,
        };
        const result = await this.#client.saveText(request);
        const ownerStillCurrent = operationOwnerGeneration === this.#ownerGeneration
          && this.#owner?.subscription_id === owner.subscription_id;

        if (result.status === "stale_conflict") {
          if (ownerStillCurrent) {
            const stale = this.#snapshot.dirty
              && result.content_hash !== this.#snapshot.buffer_base_hash;
            this.#publish({
              stale,
              disk_head_revision: result.revision,
              disk_head_hash: result.content_hash,
              last_error: null,
            });
            if (stale && presentationId) {
              this.#presentations.get(presentationId)?.callbacks.on_open_comparison?.();
            }
          }
          return result;
        }

        let cleanupFailure: Error | null = null;
        if (cleanup) {
          try {
            await this.#reconcileSavedRecovery(cleanup);
          } catch (error) {
            if (
              this.#recoveryMetadata?.recovery_id === cleanup.recovery_id
              && this.#recoveryMetadata.recovery_revision === cleanup.expected_recovery_revision
            ) this.#recoveryMetadata.needs_reconciliation = true;
            cleanupFailure = error instanceof Error ? error : new Error(errorMessage(error));
          }
        }

        if (ownerStillCurrent) {
          this.#baseRevision = result.revision;
          const dirty = this.#snapshot.buffer_generation !== bufferGeneration
            || this.#snapshot.working_text !== submittedText;
          this.#publish({
            base_revision: result.revision,
            buffer_base_hash: result.content_hash,
            disk_head_revision: result.revision,
            disk_head_hash: result.content_hash,
            saved_text: submittedText,
            dirty,
            stale: false,
            recovery: cleanupFailure && cleanup
              ? Object.freeze({
                  status: "error" as const,
                  recovery_id: cleanup.recovery_id,
                  recovery_revision: cleanup.expected_recovery_revision,
                  buffer_generation: this.#snapshot.buffer_generation,
                  retryable: true as const,
                  message: cleanupFailure.message,
                })
              : cleanup && this.#recoveryMetadata === null
                ? Object.freeze({ status: "none" as const })
                : this.#snapshot.recovery,
            last_error: cleanupFailure?.message ?? null,
          });
        }
        if (this.#snapshot.dirty) {
          this.#checkpointRequestedGeneration = this.#snapshot.buffer_generation;
          void this.#ensureCheckpointLoop().catch(() => undefined);
        }
        if (cleanupFailure) throw cleanupFailure;
        return result;
      });
    } catch (error) {
      failure = errorMessage(error);
      const authorization = unavailableAuthorization(error);
      if (authorization && this.#ownerGeneration === requestedOwnerGeneration) {
        this.#publish({ authorization });
        if (this.#snapshot.dirty && this.#recoveryMetadata) {
          this.#checkpointRequestedGeneration = this.#snapshot.buffer_generation;
          void this.#ensureCheckpointLoop().catch(() => undefined);
        }
      }
      throw error;
    } finally {
      this.#saveOperationCount = Math.max(0, this.#saveOperationCount - 1);
      this.#publish({
        save_state: this.#saveOperationCount > 0
          ? "saving"
          : failure
            ? "error"
            : "idle",
        last_error: failure ?? (this.#saveOperationCount > 0 ? this.#snapshot.last_error : null),
      });
    }
  }

  async flushRecovery(): Promise<void> {
    if (!this.#snapshot.dirty) return;
    this.#clearCheckpointTimer();
    this.#checkpointRequestedGeneration = this.#snapshot.buffer_generation;
    await this.#ensureCheckpointLoop();
  }

  async retryRecovery(): Promise<void> {
    if (
      this.#snapshot.recovery_discovery === "error"
      || this.#snapshot.recovery_discovery === "not_started"
    ) {
      const owner = this.#owner;
      if (!owner) throw new Error("Recovery discovery requires a live authorized subscription.");
      await this.initialize({
        owner,
        text: this.#snapshot.saved_text,
        discover_recovery: true,
      });
      return;
    }
    if (this.#snapshot.recovery_discovery === "conflict") {
      if (this.#snapshot.dirty) await this.flushRecovery();
      return;
    }
    if (this.#snapshot.dirty) {
      await this.flushRecovery();
      return;
    }
    await this.#retireRecoveryForCleanGeneration(this.#snapshot.buffer_generation);
  }

  async resolveRecoveryConflict(choice: FileEditorRecoveryConflictChoice): Promise<void> {
    const conflict = this.#recoveryConflict;
    if (!conflict || this.#snapshot.recovery.status !== "conflict") {
      throw new Error("The file editor has no recovery conflict to resolve.");
    }
    if (!this.#hasDurableCurrentConflictBuffer()) {
      throw new Error("Current edits must be durable before resolving the recovery conflict.");
    }
    const current = this.#recoveryMetadata;
    const bufferGeneration = this.#snapshot.buffer_generation;
    const discardTargets: RecoveryCas[] = choice === "keep_current"
      ? [{
          recovery_id: conflict.recovery_id,
          expected_recovery_revision: conflict.recovery_revision,
        }, ...(!this.#snapshot.dirty && current ? [{
          recovery_id: current.recovery_id,
          expected_recovery_revision: current.recovery_revision,
        }] : [])]
      : current
        ? [{
            recovery_id: current.recovery_id,
            expected_recovery_revision: current.recovery_revision,
          }]
        : [];
    if (!discardTargets.length) {
      throw new Error("The rejected recovery is no longer available.");
    }
    await this.#enqueueDurable(async () => {
      if (
        this.#recoveryConflict !== conflict
        || this.#snapshot.buffer_generation !== bufferGeneration
        || !this.#hasDurableCurrentConflictBuffer()
      ) {
        throw new Error("The recovery conflict changed before the choice could be committed.");
      }
      await this.#discardRecoveryTargets(discardTargets);
      if (
        this.#recoveryConflict !== conflict
        || this.#snapshot.buffer_generation !== bufferGeneration
      ) {
        throw new Error("The recovery conflict changed while the choice was being committed.");
      }
      if (choice === "keep_current") {
        this.#recoveryConflict = null;
        if (!this.#snapshot.dirty) this.#recoveryMetadata = null;
        this.#publish({
          recovery_discovery: "complete",
          recovery: current && this.#snapshot.dirty
            ? Object.freeze({
                status: "durable" as const,
                recovery_id: current.recovery_id,
                recovery_revision: current.recovery_revision,
                buffer_generation: current.buffer_generation,
              })
            : Object.freeze({ status: "none" as const }),
          last_error: null,
        });
        return;
      }
      this.#recoveryMetadata = null;
      this.#recoveryConflict = null;
      this.#applyChosenRecovery(conflict);
      this.#publish({ recovery_discovery: "complete", last_error: null });
    });
  }

  async discard(expectedBufferGeneration: number): Promise<boolean> {
    if (expectedBufferGeneration !== this.#snapshot.buffer_generation) return false;
    this.#clearCheckpointTimer();
    return this.#enqueueDurable(async () => {
      if (expectedBufferGeneration !== this.#snapshot.buffer_generation) return false;
      const recovery = this.#recoveryMetadata;
      const conflict = this.#recoveryConflict;
      await this.#discardRecoveryTargets([
        ...(recovery ? [{
          recovery_id: recovery.recovery_id,
          expected_recovery_revision: recovery.recovery_revision,
        }] : []),
        ...(conflict ? [{
          recovery_id: conflict.recovery_id,
          expected_recovery_revision: conflict.recovery_revision,
        }] : []),
      ]);
      this.#recoveryMetadata = null;
      this.#recoveryConflict = null;
      if (expectedBufferGeneration !== this.#snapshot.buffer_generation) {
        this.#publish({ recovery: Object.freeze({ status: "none" }) });
        this.#checkpointRequestedGeneration = this.#snapshot.buffer_generation;
        void this.#ensureCheckpointLoop().catch(() => undefined);
        return false;
      }
      const exactDiskHead = this.#newerExactDiskHead();
      if (exactDiskHead) {
        this.#baseRevision = exactDiskHead.revision;
        this.#publish({
          base_revision: exactDiskHead.revision,
          buffer_base_hash: exactDiskHead.content_hash,
          disk_head_revision: exactDiskHead.revision,
          disk_head_hash: exactDiskHead.content_hash,
          saved_text: exactDiskHead.text,
          working_text: exactDiskHead.text,
          dirty: false,
          stale: false,
          recovery_discovery: "complete",
          recovery: Object.freeze({ status: "none" }),
          last_error: null,
        });
      } else {
        this.#publish({
          working_text: this.#snapshot.saved_text,
          dirty: false,
          stale: false,
          recovery_discovery: "complete",
          recovery: Object.freeze({ status: "none" }),
          last_error: null,
        });
      }
      return true;
    });
  }

  canReleaseAfterPostcommit(expectedPresentationGeneration: number): boolean {
    const recoveryIsReleasable = this.#snapshot.dirty
      ? this.#snapshot.recovery.status === "durable"
        && this.#snapshot.recovery.buffer_generation === this.#snapshot.buffer_generation
        || this.#snapshot.recovery.status === "conflict"
        && this.#snapshot.recovery.current_durable
        && this.#snapshot.recovery.buffer_generation === this.#snapshot.buffer_generation
      : this.#snapshot.recovery.status === "none";
    return this.#presentations.size === 0
      && this.#pendingInitializationCount === 0
      && this.#saveOperationCount === 0
      && this.#checkpointLoop === null
      && this.#durableOperationCount === 0
      && recoveryIsReleasable
      && expectedPresentationGeneration === this.#snapshot.presentation_generation;
  }

  #applyRecovery(recovered: FileRecoveryV1): void {
    if (recovered.resource_key !== this.#resourceKey) {
      throw new Error("The recovered buffer does not match this editor resource.");
    }
    if (
      recovered.buffer === recovered.base
      || recovered.buffer === this.#snapshot.saved_text
    ) {
      const bufferGeneration = this.#snapshot.buffer_generation;
      this.#recoveryMetadata = {
        recovery_id: recovered.recovery_id,
        recovery_revision: recovered.recovery_revision,
        buffer_generation: bufferGeneration,
        needs_reconciliation: false,
      };
      this.#publish({
        recovery: Object.freeze({
          status: "durable",
          recovery_id: recovered.recovery_id,
          recovery_revision: recovered.recovery_revision,
          buffer_generation: bufferGeneration,
        }),
        last_error: null,
      });
      void this.#retireRecoveryForCleanGeneration(bufferGeneration).catch(() => undefined);
      return;
    }
    const bufferGeneration = this.#snapshot.buffer_generation + 1;
    this.#recoveryMetadata = {
      recovery_id: recovered.recovery_id,
      recovery_revision: recovered.recovery_revision,
      buffer_generation: bufferGeneration,
      needs_reconciliation: false,
    };
    const diskHeadHash = this.#snapshot.disk_head_hash;
    const stale = diskHeadHash !== recovered.base_content_hash;
    if (!stale) this.#baseRevision = this.#snapshot.disk_head_revision;
    this.#publish({
      base_revision: this.#baseRevision,
      buffer_base_hash: recovered.base_content_hash,
      saved_text: recovered.base,
      working_text: recovered.buffer,
      dirty: recovered.buffer !== recovered.base,
      stale,
      buffer_generation: bufferGeneration,
      recovery: Object.freeze({
        status: "durable",
        recovery_id: recovered.recovery_id,
        recovery_revision: recovered.recovery_revision,
        buffer_generation: bufferGeneration,
      }),
      last_error: null,
    });
    if (this.#snapshot.dirty) {
      this.#pinPresentations();
    } else {
      void this.#retireRecoveryForCleanGeneration(bufferGeneration).catch(() => undefined);
    }
  }

  async #enterRecoveryConflict(recovered: FileRecoveryV1): Promise<void> {
    if (recovered.resource_key !== this.#resourceKey) {
      throw new Error("The recovered buffer does not match this editor resource.");
    }
    this.#recoveryConflict = recovered;
    this.#clearCheckpointTimer();
    const message = "Recovered changes conflict with newer in-memory edits. Both versions were preserved.";
    this.#publish({
      recovery_discovery: "conflict",
      recovery: this.#recoveryConflictState(false, message),
      last_error: message,
    });
    this.#checkpointRequestedGeneration = this.#snapshot.buffer_generation;
    await this.#ensureCheckpointLoop();
  }

  #applyChosenRecovery(recovered: FileRecoveryV1): void {
    const bufferGeneration = this.#snapshot.buffer_generation + 1;
    this.#recoveryMetadata = {
      recovery_id: recovered.recovery_id,
      recovery_revision: recovered.recovery_revision,
      buffer_generation: bufferGeneration,
      needs_reconciliation: false,
    };
    const stale = this.#snapshot.disk_head_hash !== recovered.base_content_hash;
    if (!stale) this.#baseRevision = this.#snapshot.disk_head_revision;
    this.#publish({
      base_revision: this.#baseRevision,
      buffer_base_hash: recovered.base_content_hash,
      saved_text: recovered.base,
      working_text: recovered.buffer,
      dirty: recovered.buffer !== recovered.base,
      stale,
      buffer_generation: bufferGeneration,
      recovery: Object.freeze({
        status: "durable",
        recovery_id: recovered.recovery_id,
        recovery_revision: recovered.recovery_revision,
        buffer_generation: bufferGeneration,
      }),
    });
    if (this.#snapshot.dirty) this.#pinPresentations();
  }

  #recoveryConflictState(currentDurable: boolean, message: string): FileEditorRecoveryState {
    const conflict = this.#recoveryConflict;
    if (!conflict) throw new Error("The file editor has no recovery conflict.");
    return Object.freeze({
      status: "conflict" as const,
      conflicting_recovery_id: conflict.recovery_id,
      conflicting_recovery_revision: conflict.recovery_revision,
      current_recovery_id: this.#recoveryMetadata?.recovery_id ?? null,
      current_recovery_revision: this.#recoveryMetadata?.recovery_revision ?? null,
      current_durable: currentDurable,
      buffer_generation: this.#snapshot.buffer_generation,
      message,
    });
  }

  #hasDurableCurrentConflictBuffer(): boolean {
    const state = this.#snapshot.recovery;
    if (state.status !== "conflict" || !state.current_durable) return false;
    if (!this.#snapshot.dirty) return this.#checkpointLoop === null;
    const current = this.#recoveryMetadata;
    return current !== null
      && current.buffer_generation === this.#snapshot.buffer_generation
      && state.buffer_generation === this.#snapshot.buffer_generation
      && state.current_recovery_id === current.recovery_id
      && state.current_recovery_revision === current.recovery_revision;
  }

  #pinPresentations(): void {
    for (const [surfaceId, presentation] of this.#presentations) {
      if (this.#pinnedPresentations.has(surfaceId)) continue;
      this.#pinnedPresentations.add(surfaceId);
      presentation.callbacks.on_pin?.();
    }
  }

  #scheduleCheckpoint(bufferGeneration: number): void {
    if (this.#checkpointLoop) {
      this.#checkpointRequestedGeneration = bufferGeneration;
      return;
    }
    this.#clearCheckpointTimer();
    this.#checkpointTimer = setTimeout(() => {
      this.#checkpointTimer = null;
      this.#checkpointRequestedGeneration = this.#snapshot.buffer_generation;
      void this.#ensureCheckpointLoop().catch(() => undefined);
    }, this.#checkpointDebounceMs);
  }

  #clearCheckpointTimer(): void {
    if (this.#checkpointTimer === null) return;
    clearTimeout(this.#checkpointTimer);
    this.#checkpointTimer = null;
  }

  #ensureCheckpointLoop(): Promise<void> {
    if (this.#checkpointLoop) return this.#checkpointLoop;
    const wrapped = this.#runCheckpointLoop().finally(() => {
      if (this.#checkpointLoop === wrapped) {
        this.#checkpointLoop = null;
        this.#publish({
          ...(this.#recoveryConflict && !this.#snapshot.dirty
            ? {
                recovery: this.#recoveryConflictState(
                  true,
                  "Recovered changes conflict with newer in-memory edits. Both versions were preserved.",
                ),
              }
            : {}),
        });
      }
    });
    this.#checkpointLoop = wrapped;
    return wrapped;
  }

  async #runCheckpointLoop(): Promise<void> {
    while (this.#snapshot.dirty && this.#checkpointRequestedGeneration !== null) {
      this.#checkpointRequestedGeneration = null;
      const bufferGeneration = await this.#enqueueDurable(
        () => this.#checkpointCurrentInsideQueue(),
      );
      if (
        bufferGeneration !== null
        && this.#snapshot.dirty
        && this.#snapshot.buffer_generation !== bufferGeneration
      ) {
        this.#checkpointRequestedGeneration = this.#snapshot.buffer_generation;
      }
    }
  }

  async #checkpointCurrentInsideQueue(): Promise<number | null> {
    if (!this.#snapshot.dirty) return null;
    if (
      this.#snapshot.recovery_discovery === "not_started"
      ||
      this.#snapshot.recovery_discovery === "discovering"
      || this.#snapshot.recovery_discovery === "error"
    ) {
      throw new Error("Recovery discovery must complete before checkpointing newer edits.");
    }
    const owner = this.#owner;
    const ownerGeneration = this.#ownerGeneration;
    const baseHash = this.#snapshot.buffer_base_hash;
    if (!owner || this.#baseRevision === null || baseHash === null) {
      throw new Error("Dirty recovery requires a live authorized subscription.");
    }
    const bufferGeneration = this.#snapshot.buffer_generation;
    try {
      await this.#reconcileAmbiguousRecovery();
    } catch (error) {
      const recovery = this.#recoveryMetadata;
      this.#publish({
        recovery: Object.freeze({
          status: "error",
          recovery_id: recovery?.recovery_id ?? null,
          recovery_revision: recovery?.recovery_revision ?? null,
          buffer_generation: this.#snapshot.buffer_generation,
          retryable: true,
          message: errorMessage(error),
        }),
        last_error: errorMessage(error),
      });
      throw error;
    }
    const recovery = this.#recoveryMetadata;
    const request: CheckpointFileRecoveryRequestV1 = {
      recovery_id: recovery?.recovery_id ?? null,
      expected_recovery_revision: recovery?.recovery_revision ?? null,
      resource_id: owner.resource_id,
      subscription_id: owner.subscription_id,
      base_content_hash: baseHash,
      resource_key: this.#resourceKey,
      base: this.#snapshot.saved_text,
      buffer: this.#snapshot.working_text,
    };
    const conflictMessage = this.#recoveryConflict
      ? "Recovered changes conflict with newer in-memory edits. Both versions were preserved."
      : null;
    const checkpointConflict = this.#recoveryConflict;
    this.#publish({
      recovery: conflictMessage
        ? this.#recoveryConflictState(false, conflictMessage)
        : Object.freeze({
            status: "checkpointing" as const,
            recovery_id: recovery?.recovery_id ?? null,
            recovery_revision: recovery?.recovery_revision ?? null,
            buffer_generation: bufferGeneration,
          }),
    });
    let committed: FileRecoveryCheckpointV1;
    try {
      committed = await this.#client.checkpointRecovery(request);
    } catch (error) {
      if (this.#ownerGeneration !== ownerGeneration) {
        this.#checkpointRequestedGeneration = this.#snapshot.buffer_generation;
        return null;
      }
      const authorization = unavailableAuthorization(error);
      this.#publish({
        ...(authorization ? { authorization } : {}),
        recovery: this.#recoveryConflict
          ? this.#recoveryConflictState(false, errorMessage(error))
          : Object.freeze({
              status: "error" as const,
              recovery_id: recovery?.recovery_id ?? null,
              recovery_revision: recovery?.recovery_revision ?? null,
              buffer_generation: this.#snapshot.buffer_generation,
              retryable: true as const,
              message: errorMessage(error),
            }),
        last_error: errorMessage(error),
      });
      throw error;
    }
    if (committed.resource_key !== this.#resourceKey) {
      throw new Error("The recovery checkpoint does not match this editor resource.");
    }
    if (checkpointConflict && this.#recoveryConflict !== checkpointConflict) return null;
    const authorization = this.#ownerGeneration === ownerGeneration
      && committed.file_authorization_error
      ? unavailableAuthorization(committed.file_authorization_error)
      : null;
    this.#recoveryMetadata = {
      recovery_id: committed.recovery_id,
      recovery_revision: committed.recovery_revision,
      buffer_generation: bufferGeneration,
      needs_reconciliation: false,
    };
    this.#publish({
      ...(authorization ? { authorization } : {}),
      recovery: this.#recoveryConflict
        ? this.#recoveryConflictState(
            bufferGeneration === this.#snapshot.buffer_generation,
            "Recovered changes conflict with newer in-memory edits. Both versions were preserved.",
          )
        : Object.freeze({
            status: "durable" as const,
            recovery_id: committed.recovery_id,
            recovery_revision: committed.recovery_revision,
            buffer_generation: bufferGeneration,
          }),
      last_error: this.#recoveryConflict
        ? "Recovered changes conflict with newer in-memory edits. Both versions were preserved."
        : authorization?.status === "unavailable" ? authorization.message : null,
    });
    return bufferGeneration;
  }

  async #reconcileSavedRecovery(cleanup: {
    recovery_id: string;
    expected_recovery_revision: number;
  }): Promise<void> {
    const list = async () => this.#client.listRecoveries({
      resource_key: this.#resourceKey,
    });
    const before = (await list()).find((candidate) => (
      candidate.recovery_id === cleanup.recovery_id
    ));
    if (before) {
      if (before.recovery_revision !== cleanup.expected_recovery_revision) {
        throw new Error("Recovery advanced while Save cleanup was being verified.");
      }
      await this.#client.discardRecovery({
        recovery_id: cleanup.recovery_id,
        expected_recovery_revision: cleanup.expected_recovery_revision,
        resource_key: this.#resourceKey,
      });
      const remains = (await list()).some((candidate) => (
        candidate.recovery_id === cleanup.recovery_id
      ));
      if (remains) {
        throw new Error("Saved recovery cleanup could not be verified.");
      }
    }
    if (
      this.#recoveryMetadata?.recovery_id === cleanup.recovery_id
      && this.#recoveryMetadata.recovery_revision === cleanup.expected_recovery_revision
    ) this.#recoveryMetadata = null;
  }

  async #reconcileAmbiguousRecovery(): Promise<void> {
    const recovery = this.#recoveryMetadata;
    if (!recovery?.needs_reconciliation) return;
    const candidate = (await this.#client.listRecoveries({
      resource_key: this.#resourceKey,
    })).find((entry) => entry.recovery_id === recovery.recovery_id);
    if (!candidate) {
      if (this.#recoveryMetadata === recovery) this.#recoveryMetadata = null;
      return;
    }
    if (candidate.recovery_revision !== recovery.recovery_revision) {
      throw new Error("Recovery advanced while its previous outcome was being reconciled.");
    }
    if (this.#recoveryMetadata === recovery) recovery.needs_reconciliation = false;
  }

  #newerExactDiskHead(): ExactDiskHead | null {
    const exact = this.#exactDiskHead;
    const owner = this.#owner;
    if (
      !exact
      || !owner
      || exact.owner_generation !== this.#ownerGeneration
      || exact.resource_id !== owner.resource_id
      || exact.subscription_id !== owner.subscription_id
      || exact.revision !== this.#snapshot.disk_head_revision
      || exact.content_hash !== this.#snapshot.disk_head_hash
      || exact.content_hash === this.#snapshot.buffer_base_hash
    ) return null;
    return exact;
  }

  async #discardRecoveryTargets(targets: readonly RecoveryCas[]): Promise<void> {
    const exactById = new Map<string, RecoveryCas>();
    for (const target of targets) {
      const previous = exactById.get(target.recovery_id);
      if (
        !previous
        || target.expected_recovery_revision > previous.expected_recovery_revision
      ) exactById.set(target.recovery_id, target);
    }
    for (const target of exactById.values()) {
      const request: DiscardFileRecoveryRequestV1 = {
        ...target,
        resource_key: this.#resourceKey,
      };
      try {
        await this.#client.discardRecovery(request);
      } catch (error) {
        // Exact deletion is idempotent. A missing record already satisfies the
        // requested postcondition; stale CAS and scope failures still fail closed.
        if (errorCode(error) !== "recovery_not_found") throw error;
      }
    }
  }

  #retireRecoveryForCleanGeneration(bufferGeneration: number): Promise<void> {
    if (this.#recoveryConflict) return Promise.resolve();
    return this.#enqueueDurable(
      () => this.#retireRecoveryForCleanGenerationInsideQueue(bufferGeneration),
    );
  }

  async #retireRecoveryForCleanGenerationInsideQueue(bufferGeneration: number): Promise<void> {
    if (this.#snapshot.dirty || this.#snapshot.buffer_generation !== bufferGeneration) return;
    try {
      await this.#reconcileAmbiguousRecovery();
    } catch (error) {
      const recovery = this.#recoveryMetadata;
      this.#publish({
        recovery: Object.freeze({
          status: "error",
          recovery_id: recovery?.recovery_id ?? null,
          recovery_revision: recovery?.recovery_revision ?? null,
          buffer_generation: bufferGeneration,
          retryable: true,
          message: errorMessage(error),
        }),
        last_error: errorMessage(error),
      });
      throw error;
    }
    const recovery = this.#recoveryMetadata;
    if (recovery) {
      try {
        await this.#client.discardRecovery({
          recovery_id: recovery.recovery_id,
          expected_recovery_revision: recovery.recovery_revision,
          resource_key: this.#resourceKey,
        });
      } catch (error) {
        this.#publish({
          recovery: Object.freeze({
            status: "error",
            recovery_id: recovery.recovery_id,
            recovery_revision: recovery.recovery_revision,
            buffer_generation: bufferGeneration,
            retryable: true,
            message: errorMessage(error),
          }),
          last_error: errorMessage(error),
        });
        throw error;
      }
      if (
        this.#recoveryMetadata?.recovery_id === recovery.recovery_id
        && this.#recoveryMetadata.recovery_revision === recovery.recovery_revision
      ) this.#recoveryMetadata = null;
    }
    if (!this.#snapshot.dirty && this.#snapshot.buffer_generation === bufferGeneration) {
      this.#publish({ recovery: Object.freeze({ status: "none" }), last_error: null });
    }
  }

  #captureExactDiskHead(action: string): ExactDiskOperation {
    if (this.#snapshot.authorization.status === "unavailable") {
      throw new Error(`Restore file access before ${action.toLowerCase()}.`);
    }
    if (!this.#snapshot.dirty || !this.#snapshot.stale) {
      throw new Error(`${action} requires a stale dirty buffer.`);
    }
    if (this.#snapshot.recovery_discovery !== "complete" || this.#recoveryConflict) {
      throw new Error(`${action} requires recovery discovery to be complete.`);
    }
    const owner = this.#owner;
    const diskHead = this.#exactDiskHead;
    if (
      !owner
      || !diskHead
      || diskHead.owner_generation !== this.#ownerGeneration
      || diskHead.resource_id !== owner.resource_id
      || diskHead.subscription_id !== owner.subscription_id
      || diskHead.revision !== owner.revision
      || diskHead.content_hash !== owner.descriptor.content_hash
      || diskHead.revision !== this.#snapshot.disk_head_revision
      || diskHead.content_hash !== this.#snapshot.disk_head_hash
    ) {
      throw new Error(`${action} is waiting for the exact authorized disk head.`);
    }
    return Object.freeze({
      ...diskHead,
      buffer_generation: this.#snapshot.buffer_generation,
    });
  }

  #assertExactDiskHead(operation: ExactDiskOperation, action: string): void {
    const owner = this.#owner;
    const diskHead = this.#exactDiskHead;
    if (
      this.#snapshot.buffer_generation !== operation.buffer_generation
      || !this.#snapshot.dirty
      || !this.#snapshot.stale
      || this.#ownerGeneration !== operation.owner_generation
      || !owner
      || owner.resource_id !== operation.resource_id
      || owner.subscription_id !== operation.subscription_id
      || owner.revision !== operation.revision
      || owner.descriptor.content_hash !== operation.content_hash
      || !diskHead
      || diskHead.owner_generation !== operation.owner_generation
      || diskHead.revision !== operation.revision
      || diskHead.content_hash !== operation.content_hash
      || diskHead.text !== operation.text
      || this.#snapshot.disk_head_revision !== operation.revision
      || this.#snapshot.disk_head_hash !== operation.content_hash
    ) throw new Error(`${action} was cancelled because the editor or disk head changed.`);
  }

  #markAuthorizationUnavailable(error: unknown, ownerGeneration: number): void {
    const authorization = unavailableAuthorization(error);
    if (authorization && this.#ownerGeneration === ownerGeneration) {
      this.#publish({ authorization });
    }
  }

  #enqueueDurable<T>(operation: () => Promise<T>): Promise<T> {
    this.#durableOperationCount += 1;
    return new Promise<T>((resolve, reject) => {
      const run = () => {
        this.#durableBusy = true;
        void (async () => {
          const finalize = () => {
            this.#durableOperationCount = Math.max(0, this.#durableOperationCount - 1);
            const next = this.#durableQueue.shift();
            if (next) {
              next();
            } else {
              this.#durableBusy = false;
              this.#publish({});
            }
          };
          try {
            const result = await operation();
            finalize();
            resolve(result);
          } catch (error) {
            finalize();
            reject(error);
          }
        })();
      };
      if (this.#durableBusy) {
        this.#durableQueue.push(run);
      } else {
        run();
      }
    });
  }

  #publishMembership(): void {
    this.#publish({
      presentation_generation: this.#snapshot.presentation_generation + 1,
      presentation_ids: Object.freeze([...this.#presentations.keys()].sort()),
    });
  }

  #publish(patch: Partial<FileEditorSnapshot>): void {
    this.#snapshot = Object.freeze({ ...this.#snapshot, ...patch });
    for (const listener of this.#listeners) listener();
  }
}

/** Owns exactly one editor controller per canonical Files resource key. */
export class FileEditorControllerRegistry {
  readonly #sessions = new Map<string, FileEditorController>();
  readonly #releaseWaiters = new Map<string, {
    expectedPresentationGeneration: number;
    unsubscribe: () => void;
  }>();
  readonly #authoritativeSynchronizations = new Map<string, {
    key: string;
    generation: number;
    pending: boolean;
    resource: Promise<FileResourceTextV1>;
    promise: Promise<FileEditorController>;
  }>();
  readonly #authoritativeGenerations = new Map<string, number>();
  readonly #client: FileEditorResourceClient;
  readonly #options: FileEditorControllerOptions;

  constructor(client: FileEditorResourceClient, options: FileEditorControllerOptions = {}) {
    this.#client = client;
    this.#options = options;
  }

  forResource(resourceKey: string): FileEditorController {
    let session = this.#sessions.get(resourceKey);
    if (!session) {
      session = new FileEditorController(resourceKey, this.#client, this.#options);
      this.#sessions.set(resourceKey, session);
    }
    return session;
  }

  getExisting(resourceKey: string): FileEditorController | undefined {
    return this.#sessions.get(resourceKey);
  }

  getByPresentation(surfaceId: string): FileEditorController | undefined {
    let match: FileEditorController | undefined;
    for (const controller of this.#sessions.values()) {
      if (!controller.getSnapshot().presentation_ids.includes(surfaceId)) continue;
      if (match) return undefined;
      match = controller;
    }
    return match;
  }

  synchronizeAuthoritative(
    owner: FileResourceSnapshotV1,
    readExactText: () => Promise<FileResourceTextV1>,
    presentationId?: string,
  ): Promise<FileEditorController> {
    const resourceKey = owner.resource_id;
    const key = JSON.stringify([
      owner.subscription_id,
      owner.revision,
      owner.descriptor.content_hash,
    ]);
    const current = this.#authoritativeSynchronizations.get(resourceKey);
    if (current?.key === key) {
      return current.resource.then((resource) => {
        const latest = this.#authoritativeSynchronizations.get(resourceKey);
        if (presentationId) {
          this.forResource(resourceKey).bindPresentationOwner(
            presentationId,
            owner,
            resource.text,
            latest?.generation === current.generation,
          );
        }
        return current.promise;
      });
    }

    const generation = (this.#authoritativeGenerations.get(resourceKey) ?? 0) + 1;
    this.#authoritativeGenerations.set(resourceKey, generation);
    const controller = this.forResource(resourceKey);
    const resource = readExactText().then((exact) => {
      if (
        exact.resource_id !== owner.resource_id
        || exact.revision !== owner.revision
      ) {
        throw new Error("The text read did not match the authoritative file snapshot.");
      }
      return exact;
    });
    let synchronization: Promise<FileEditorController>;
    synchronization = (async () => {
      const exact = await resource;
      const latest = this.#authoritativeSynchronizations.get(resourceKey);
      const attached = presentationId === undefined
        || controller.observePresentation(presentationId).attached;
      if (!attached) {
        if (latest?.generation === generation) {
          this.#authoritativeSynchronizations.delete(resourceKey);
        }
        return controller;
      }
      const accepted = latest?.generation === generation || latest === undefined;
      if (presentationId) {
        controller.bindPresentationOwner(presentationId, owner, exact.text, accepted);
        if (!controller.observePresentation(presentationId).attached) {
          if (latest?.generation === generation) {
            this.#authoritativeSynchronizations.delete(resourceKey);
          }
          return controller;
        }
      }
      if (!accepted) return controller;
      await controller.initialize({
        owner,
        text: exact.text,
        discover_recovery: true,
      });
      return controller;
    })().catch((error) => {
      const latest = this.#authoritativeSynchronizations.get(resourceKey);
      if (latest?.generation === generation) {
        this.#authoritativeSynchronizations.delete(resourceKey);
      }
      throw error;
    }).finally(() => {
      const latest = this.#authoritativeSynchronizations.get(resourceKey);
      if (latest?.generation === generation) latest.pending = false;
      const session = this.#sessions.get(resourceKey);
      if (session) {
        this.releaseAfterPostcommit(
          resourceKey,
          session.getSnapshot().presentation_generation,
        );
      }
    });
    this.#authoritativeSynchronizations.set(resourceKey, {
      key,
      generation,
      pending: true,
      resource,
      promise: synchronization,
    });
    return synchronization;
  }

  releaseAfterPostcommit(
    resourceKey: string,
    expectedPresentationGeneration: number,
  ): boolean {
    const session = this.#sessions.get(resourceKey);
    if (!session) return false;
    if (this.#tryRelease(resourceKey, session, expectedPresentationGeneration)) return true;
    const snapshot = session.getSnapshot();
    if (
      snapshot.presentation_ids.length === 0
      && snapshot.presentation_generation === expectedPresentationGeneration
    ) this.#watchPendingRelease(resourceKey, session, expectedPresentationGeneration);
    return false;
  }

  #tryRelease(
    resourceKey: string,
    session: FileEditorController,
    expectedPresentationGeneration: number,
  ): boolean {
    if (this.#authoritativeSynchronizations.get(resourceKey)?.pending) return false;
    if (!session.canReleaseAfterPostcommit(expectedPresentationGeneration)) return false;
    this.#releaseWaiters.get(resourceKey)?.unsubscribe();
    this.#releaseWaiters.delete(resourceKey);
    this.#sessions.delete(resourceKey);
    session.dispose();
    this.#authoritativeSynchronizations.delete(resourceKey);
    this.#authoritativeGenerations.delete(resourceKey);
    return true;
  }

  #watchPendingRelease(
    resourceKey: string,
    session: FileEditorController,
    expectedPresentationGeneration: number,
  ): void {
    const existing = this.#releaseWaiters.get(resourceKey);
    if (existing?.expectedPresentationGeneration === expectedPresentationGeneration) return;
    existing?.unsubscribe();
    const unsubscribe = session.subscribe(() => {
      if (this.#sessions.get(resourceKey) !== session) {
        unsubscribe();
        this.#releaseWaiters.delete(resourceKey);
        return;
      }
      const snapshot = session.getSnapshot();
      if (snapshot.presentation_generation !== expectedPresentationGeneration) {
        unsubscribe();
        this.#releaseWaiters.delete(resourceKey);
        return;
      }
      this.#tryRelease(resourceKey, session, expectedPresentationGeneration);
    });
    this.#releaseWaiters.set(resourceKey, {
      expectedPresentationGeneration,
      unsubscribe,
    });
  }
}
