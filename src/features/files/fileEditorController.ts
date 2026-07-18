import type {
  CheckpointFileRecoveryRequestV1,
  DiscardFileRecoveryRequestV1,
  FileRecoveryCheckpointV1,
  FileRecoverySummaryV1,
  FileRecoveryV1,
  FileResourceSaveResultV1,
  FileResourceSnapshotV1,
  ListFileRecoveriesRequestV1,
  SaveFileResourceTextRequestV1,
} from "../../types";
import type { FileResourceClient } from "./fileResourceClient";

export type FileEditorResourceClient = Pick<
  FileResourceClient,
  | "saveText"
  | "checkpointRecovery"
  | "listRecoveries"
  | "getRecovery"
  | "discardRecovery"
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
  readonly save_state: "idle" | "saving" | "error";
  readonly recovery: FileEditorRecoveryState;
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
};

type PresentationEntry = {
  generation: number;
  callbacks: FileEditorPresentationCallbacks;
};

const DEFAULT_CHECKPOINT_DEBOUNCE_MS = 750;

function errorMessage(error: unknown): string {
  if (error instanceof Error) return error.message;
  if (typeof error === "object" && error !== null && "message" in error) {
    return String(error.message);
  }
  return String(error);
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
  readonly #presentations = new Map<string, PresentationEntry>();
  readonly #presentationObservations = new Map<string, number>();
  readonly #pinnedPresentations = new Set<string>();
  #snapshot: FileEditorSnapshot;
  #owner: FileResourceSnapshotV1 | null = null;
  #baseRevision: number | null = null;
  #initializationGeneration = 0;
  #recoveryMetadata: RecoveryMetadata | null = null;
  #checkpointTimer: ReturnType<typeof setTimeout> | null = null;
  #checkpointRequestedGeneration: number | null = null;
  #checkpointLoop: Promise<void> | null = null;
  readonly #durableQueue: Array<() => void> = [];
  #durableBusy = false;
  #durableOperationCount = 0;

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
      save_state: "idle",
      recovery: Object.freeze({ status: "none" }),
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

  async initialize(input: FileEditorInitialization): Promise<void> {
    const initializationGeneration = ++this.#initializationGeneration;
    const alreadyDirty = this.#snapshot.dirty;
    this.applyAuthoritative(input.owner, input.text);
    const bufferGeneration = this.#snapshot.buffer_generation;
    if (input.discover_recovery === false || alreadyDirty) return;

    const recoveries = await this.#client.listRecoveries({
      resource_key: this.#resourceKey,
    } satisfies ListFileRecoveriesRequestV1);
    if (
      initializationGeneration !== this.#initializationGeneration
      || bufferGeneration !== this.#snapshot.buffer_generation
    ) return;
    const newest = newestRecovery(recoveries);
    if (!newest) return;
    const recovered = await this.#client.getRecovery({
      recovery_id: newest.recovery_id,
      resource_key: this.#resourceKey,
    });
    if (
      initializationGeneration !== this.#initializationGeneration
      || bufferGeneration !== this.#snapshot.buffer_generation
    ) return;
    this.#applyRecovery(recovered);
  }

  applyAuthoritative(owner: FileResourceSnapshotV1, text: string): void {
    if (owner.resource_id !== this.#resourceKey) {
      throw new Error("The authoritative file does not match this editor resource.");
    }
    if (
      this.#owner?.subscription_id === owner.subscription_id
      && this.#snapshot.disk_head_revision !== null
      && owner.revision <= this.#snapshot.disk_head_revision
    ) return;
    this.#owner = owner;
    const diskHeadHash = owner.descriptor.content_hash;
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
        save_state: "idle",
        last_error: null,
      });
      return;
    }
    this.#publish({
      resource_id: owner.resource_id,
      subscription_id: owner.subscription_id,
      disk_head_revision: owner.revision,
      disk_head_hash: diskHeadHash,
      stale: diskHeadHash !== this.#snapshot.buffer_base_hash,
    });
  }

  mutate(text: string): number {
    if (this.#snapshot.status !== "ready") {
      throw new Error("The file editor is not initialized.");
    }
    if (text === this.#snapshot.working_text) {
      if (!this.#snapshot.dirty && this.#snapshot.recovery.status !== "none") {
        void this.#retireRecoveryForCleanGeneration(this.#snapshot.buffer_generation)
          .catch(() => undefined);
      }
      return this.#snapshot.buffer_generation;
    }
    const bufferGeneration = this.#snapshot.buffer_generation + 1;
    const dirty = text !== this.#snapshot.saved_text;
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
      this.#scheduleCheckpoint(bufferGeneration);
    } else {
      this.#clearCheckpointTimer();
      void this.#retireRecoveryForCleanGeneration(bufferGeneration).catch(() => undefined);
    }
    return bufferGeneration;
  }

  async save(presentationId?: string): Promise<FileResourceSaveResultV1> {
    const owner = this.#owner;
    const baseRevision = this.#baseRevision;
    const baseHash = this.#snapshot.buffer_base_hash;
    if (!owner || baseRevision === null || baseHash === null) {
      throw new Error("The file editor has no live authorized subscription.");
    }
    const bufferGeneration = this.#snapshot.buffer_generation;
    const submittedText = this.#snapshot.working_text;
    this.#clearCheckpointTimer();
    this.#publish({ save_state: "saving", last_error: null });
    return this.#enqueueDurable(async () => {
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
      let result: FileResourceSaveResultV1;
      try {
        result = await this.#client.saveText(request);
      } catch (error) {
        this.#publish({ save_state: "error", last_error: errorMessage(error) });
        throw error;
      }

      if (result.status === "stale_conflict") {
        this.#publish({
          save_state: "idle",
          stale: true,
          disk_head_revision: result.revision,
          disk_head_hash: result.content_hash,
          last_error: null,
        });
        if (presentationId) {
          this.#presentations.get(presentationId)?.callbacks.on_open_comparison?.();
        }
        return result;
      }

      this.#baseRevision = result.revision;
      if (
        cleanup
        && this.#recoveryMetadata?.recovery_id === cleanup.recovery_id
        && this.#recoveryMetadata.recovery_revision === cleanup.expected_recovery_revision
      ) {
        this.#recoveryMetadata = null;
      }
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
        save_state: "idle",
        recovery: cleanup && this.#recoveryMetadata === null
          ? Object.freeze({ status: "none" as const })
          : this.#snapshot.recovery,
        last_error: null,
      });
      if (dirty) {
        this.#checkpointRequestedGeneration = this.#snapshot.buffer_generation;
        void this.#ensureCheckpointLoop().catch(() => undefined);
      }
      return result;
    });
  }

  async flushRecovery(): Promise<void> {
    if (!this.#snapshot.dirty) return;
    this.#clearCheckpointTimer();
    this.#checkpointRequestedGeneration = this.#snapshot.buffer_generation;
    await this.#ensureCheckpointLoop();
  }

  async retryRecovery(): Promise<void> {
    if (this.#snapshot.dirty) {
      await this.flushRecovery();
      return;
    }
    await this.#retireRecoveryForCleanGeneration(this.#snapshot.buffer_generation);
  }

  async discard(expectedBufferGeneration: number): Promise<boolean> {
    if (expectedBufferGeneration !== this.#snapshot.buffer_generation) return false;
    this.#clearCheckpointTimer();
    return this.#enqueueDurable(async () => {
      if (expectedBufferGeneration !== this.#snapshot.buffer_generation) return false;
      const recovery = this.#recoveryMetadata;
      if (recovery) {
        const request: DiscardFileRecoveryRequestV1 = {
          recovery_id: recovery.recovery_id,
          expected_recovery_revision: recovery.recovery_revision,
          resource_key: this.#resourceKey,
        };
        await this.#client.discardRecovery(request);
        if (
          this.#recoveryMetadata?.recovery_id === recovery.recovery_id
          && this.#recoveryMetadata.recovery_revision === recovery.recovery_revision
        ) this.#recoveryMetadata = null;
      }
      if (expectedBufferGeneration !== this.#snapshot.buffer_generation) {
        this.#publish({ recovery: Object.freeze({ status: "none" }) });
        this.#checkpointRequestedGeneration = this.#snapshot.buffer_generation;
        void this.#ensureCheckpointLoop().catch(() => undefined);
        return false;
      }
      this.#publish({
        working_text: this.#snapshot.saved_text,
        dirty: false,
        stale: false,
        recovery: Object.freeze({ status: "none" }),
        last_error: null,
      });
      return true;
    });
  }

  canReleaseAfterPostcommit(expectedPresentationGeneration?: number): boolean {
    return this.#presentations.size === 0
      && !this.#snapshot.dirty
      && this.#checkpointLoop === null
      && this.#durableOperationCount === 0
      && this.#snapshot.recovery.status === "none"
      && (
        expectedPresentationGeneration === undefined
        || expectedPresentationGeneration === this.#snapshot.presentation_generation
      );
  }

  #applyRecovery(recovered: FileRecoveryV1): void {
    if (recovered.resource_key !== this.#resourceKey) {
      throw new Error("The recovered buffer does not match this editor resource.");
    }
    const bufferGeneration = this.#snapshot.buffer_generation + 1;
    this.#recoveryMetadata = {
      recovery_id: recovered.recovery_id,
      recovery_revision: recovered.recovery_revision,
      buffer_generation: bufferGeneration,
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
    if (!this.#snapshot.dirty) {
      void this.#retireRecoveryForCleanGeneration(bufferGeneration).catch(() => undefined);
    }
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
        this.#publish({});
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
    const owner = this.#owner;
    const baseRevision = this.#baseRevision;
    const baseHash = this.#snapshot.buffer_base_hash;
    if (!owner || baseRevision === null || baseHash === null) {
      throw new Error("Dirty recovery requires a live authorized subscription.");
    }
    const bufferGeneration = this.#snapshot.buffer_generation;
    const recovery = this.#recoveryMetadata;
    const request: CheckpointFileRecoveryRequestV1 = {
      recovery_id: recovery?.recovery_id ?? null,
      expected_recovery_revision: recovery?.recovery_revision ?? null,
      resource_id: owner.resource_id,
      subscription_id: owner.subscription_id,
      base_revision: baseRevision,
      base_content_hash: baseHash,
      resource_key: this.#resourceKey,
      buffer: this.#snapshot.working_text,
    };
    this.#publish({
      recovery: Object.freeze({
        status: "checkpointing",
        recovery_id: recovery?.recovery_id ?? null,
        recovery_revision: recovery?.recovery_revision ?? null,
        buffer_generation: bufferGeneration,
      }),
    });
    let committed: FileRecoveryCheckpointV1;
    try {
      committed = await this.#client.checkpointRecovery(request);
    } catch (error) {
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
    if (committed.resource_key !== this.#resourceKey) {
      throw new Error("The recovery checkpoint does not match this editor resource.");
    }
    this.#recoveryMetadata = {
      recovery_id: committed.recovery_id,
      recovery_revision: committed.recovery_revision,
      buffer_generation: bufferGeneration,
    };
    this.#publish({
      recovery: Object.freeze({
        status: "durable",
        recovery_id: committed.recovery_id,
        recovery_revision: committed.recovery_revision,
        buffer_generation: bufferGeneration,
      }),
      last_error: null,
    });
    return bufferGeneration;
  }

  #retireRecoveryForCleanGeneration(bufferGeneration: number): Promise<void> {
    return this.#enqueueDurable(async () => {
      if (this.#snapshot.dirty || this.#snapshot.buffer_generation !== bufferGeneration) return;
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
    });
  }

  #enqueueDurable<T>(operation: () => Promise<T>): Promise<T> {
    this.#durableOperationCount += 1;
    return new Promise<T>((resolve, reject) => {
      const run = () => {
        this.#durableBusy = true;
        void (async () => {
          try {
            resolve(await operation());
          } catch (error) {
            reject(error);
          } finally {
            this.#durableOperationCount = Math.max(0, this.#durableOperationCount - 1);
            const next = this.#durableQueue.shift();
            if (next) {
              next();
            } else {
              this.#durableBusy = false;
              this.#publish({});
            }
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

  releaseAfterPostcommit(
    resourceKey: string,
    expectedPresentationGeneration?: number,
  ): boolean {
    const session = this.#sessions.get(resourceKey);
    if (!session?.canReleaseAfterPostcommit(expectedPresentationGeneration)) return false;
    this.#sessions.delete(resourceKey);
    return true;
  }
}
