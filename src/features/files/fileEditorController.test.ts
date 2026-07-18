import { describe, expect, it, vi } from "vitest";
import type {
  FileContentDescriptorV1,
  FileRecoveryCheckpointV1,
  FileRecoverySummaryV1,
  FileRecoveryV1,
  FileResourceSnapshotV1,
} from "../../types";
import {
  FileEditorControllerRegistry,
  type FileEditorResourceClient,
} from "./fileEditorController";

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, resolve, reject };
}

const descriptor: FileContentDescriptorV1 = {
  schema: 1,
  canonical_path: "C:/work/Notes.md",
  display_name: "Notes.md",
  extension: "md",
  mime_type: "text/markdown",
  encoding: "utf-8",
  renderer_kind: "markdown",
  size_bytes: 5,
  line_count: 1,
  content_hash: "hash-base",
  modified_at_ms: 100,
  capabilities: { preview: true, changes: true, draft: true, stream: false },
  unavailable_reason: null,
};

const owner: FileResourceSnapshotV1 = {
  resource_id: "file:C:/work/Notes.md",
  subscription_id: "subscription-1",
  revision: 4,
  descriptor,
};

function recoverySummary(overrides: Partial<FileRecoverySummaryV1> = {}): FileRecoverySummaryV1 {
  return {
    schema: 1,
    recovery_id: "recovery-1",
    resource_key: owner.resource_id,
    display_name: "Notes.md",
    extension: "md",
    mime_type: "text/markdown",
    base_content_hash: "hash-base",
    base_opaque_revision: "opaque-base",
    recovery_revision: 2,
    created_at_ms: 10,
    updated_at_ms: 20,
    ...overrides,
  };
}

function recovery(overrides: Partial<FileRecoveryV1> = {}): FileRecoveryV1 {
  return {
    ...recoverySummary(),
    base: "base\n",
    buffer: "recovered edit\n",
    ...overrides,
  };
}

function checkpoint(
  recovery_revision: number,
  overrides: Partial<FileRecoveryCheckpointV1> = {},
): FileRecoveryCheckpointV1 {
  return {
    schema: 1,
    recovery_id: "recovery-1",
    resource_key: owner.resource_id,
    base_content_hash: "hash-base",
    base_opaque_revision: "opaque-base",
    recovery_revision,
    created_at_ms: 10,
    updated_at_ms: 20 + recovery_revision,
    ...overrides,
  };
}

function fakeClient(): FileEditorResourceClient {
  return {
    saveText: vi.fn().mockResolvedValue({
      status: "saved",
      revision: 5,
      content_hash: "hash-saved",
    }),
    checkpointRecovery: vi.fn().mockResolvedValue(checkpoint(1)),
    listRecoveries: vi.fn().mockResolvedValue([recoverySummary()]),
    getRecovery: vi.fn().mockResolvedValue(recovery()),
    discardRecovery: vi.fn().mockResolvedValue(undefined),
  };
}

describe("FileEditorController", () => {
  it("shares one resource-owned buffer across surfaces and survives detach/reattach", async () => {
    const client = fakeClient();
    const registry = new FileEditorControllerRegistry(client, { checkpoint_debounce_ms: 60_000 });
    const first = registry.forResource(owner.resource_id);
    const second = registry.forResource(owner.resource_id);
    const firstPin = vi.fn();
    const secondPin = vi.fn();
    const firstMembership = first.attachPresentation("surface-a", { on_pin: firstPin });
    second.attachPresentation("surface-b", { on_pin: secondPin });

    expect(first).toBe(second);
    await first.initialize({ owner, text: "base\n", discover_recovery: false });
    first.mutate("edited once\n");
    first.mutate("edited twice\n");

    expect(first.getSnapshot()).toMatchObject({
      working_text: "edited twice\n",
      dirty: true,
      buffer_generation: 2,
      presentation_ids: ["surface-a", "surface-b"],
    });
    expect(firstPin).toHaveBeenCalledOnce();
    expect(secondPin).toHaveBeenCalledOnce();
    firstMembership.detach();
    expect(first.getSnapshot().working_text).toBe("edited twice\n");
    const reattached = first.attachPresentation("surface-a", { on_pin: firstPin });
    expect(firstPin).toHaveBeenCalledOnce();
    expect(first.observePresentation("surface-a")).toEqual({
      attached: true,
      generation: reattached.generation,
    });
    expect(first.getSnapshot().dirty).toBe(true);
  });

  it("restores the newest discovered recovery without letting a late read replace newer edits", async () => {
    const discovery = deferred<FileRecoverySummaryV1[]>();
    const client = fakeClient();
    vi.mocked(client.listRecoveries).mockReturnValue(discovery.promise);
    const controller = new FileEditorControllerRegistry(client).forResource(owner.resource_id);

    const initialization = controller.initialize({
      owner,
      text: "base\n",
      discover_recovery: true,
    });
    expect(controller.getSnapshot().working_text).toBe("base\n");
    controller.mutate("typed before discovery\n");
    discovery.resolve([recoverySummary()]);
    await initialization;

    expect(client.getRecovery).not.toHaveBeenCalled();
    expect(controller.getSnapshot()).toMatchObject({
      working_text: "typed before discovery\n",
      dirty: true,
      buffer_generation: 1,
    });

    const restored = new FileEditorControllerRegistry(fakeClient())
      .forResource(owner.resource_id);
    await restored.initialize({ owner, text: "base\n", discover_recovery: true });
    expect(restored.getSnapshot()).toMatchObject({
      saved_text: "base\n",
      working_text: "recovered edit\n",
      dirty: true,
      stale: false,
      recovery: { status: "durable", recovery_revision: 2 },
    });
  });

  it("keeps recovered bytes stale when the reopened disk head differs from their base", async () => {
    const client = fakeClient();
    vi.mocked(client.getRecovery).mockResolvedValue(recovery({
      base_content_hash: "hash-older-base",
      base: "older base\n",
      buffer: "recovered local edit\n",
    }));
    const controller = new FileEditorControllerRegistry(client).forResource(owner.resource_id);

    await controller.initialize({ owner, text: "current disk\n", discover_recovery: true });

    expect(controller.getSnapshot()).toMatchObject({
      saved_text: "older base\n",
      working_text: "recovered local edit\n",
      buffer_base_hash: "hash-older-base",
      disk_head_hash: "hash-base",
      disk_head_revision: 4,
      dirty: true,
      stale: true,
    });
  });

  it("retires a discovered recovery whose buffer already equals its base", async () => {
    const pendingDiscard = deferred<void>();
    const client = fakeClient();
    vi.mocked(client.getRecovery).mockResolvedValue(recovery({
      base: "already clean\n",
      buffer: "already clean\n",
    }));
    vi.mocked(client.discardRecovery).mockReturnValue(pendingDiscard.promise);
    const controller = new FileEditorControllerRegistry(client).forResource(owner.resource_id);

    await controller.initialize({ owner, text: "disk\n", discover_recovery: true });
    expect(controller.getSnapshot().dirty).toBe(false);
    expect(controller.canReleaseAfterPostcommit()).toBe(false);
    pendingDiscard.resolve();
    await vi.waitFor(() => expect(controller.getSnapshot().recovery.status).toBe("none"));
    expect(controller.canReleaseAfterPostcommit()).toBe(true);
  });

  it("saves an exact snapshot and leaves a newer edit dirty", async () => {
    const pendingSave = deferred<{
      status: "saved";
      revision: number;
      content_hash: string;
    }>();
    const client = fakeClient();
    vi.mocked(client.saveText).mockReturnValue(pendingSave.promise);
    const controller = new FileEditorControllerRegistry(client, { checkpoint_debounce_ms: 60_000 })
      .forResource(owner.resource_id);
    await controller.initialize({ owner, text: "base\n", discover_recovery: false });
    controller.mutate("save this\n");

    const saving = controller.save("surface-a");
    await vi.waitFor(() => expect(client.saveText).toHaveBeenCalledWith({
        resource_id: owner.resource_id,
        subscription_id: owner.subscription_id,
        expected_revision: 4,
        buffer_base_hash: "hash-base",
        text: "save this\n",
        recovery_cleanup: null,
      }));
    controller.mutate("newer edit\n");
    pendingSave.resolve({ status: "saved", revision: 5, content_hash: "hash-saved" });
    await saving;

    expect(controller.getSnapshot()).toMatchObject({
      saved_text: "save this\n",
      working_text: "newer edit\n",
      buffer_base_hash: "hash-saved",
      base_revision: 5,
      dirty: true,
      buffer_generation: 2,
      save_state: "idle",
    });
  });

  it("cleans an exact durable recovery on save and recreates recovery for an edit raced with cleanup", async () => {
    const pendingSave = deferred<{
      status: "saved";
      revision: number;
      content_hash: string;
    }>();
    const client = fakeClient();
    vi.mocked(client.saveText).mockReturnValue(pendingSave.promise);
    vi.mocked(client.checkpointRecovery).mockResolvedValue(checkpoint(1, {
      recovery_id: "recovery-new",
    }));
    const controller = new FileEditorControllerRegistry(client, { checkpoint_debounce_ms: 60_000 })
      .forResource(owner.resource_id);
    await controller.initialize({ owner, text: "base\n", discover_recovery: true });

    const saving = controller.save("surface-a");
    await vi.waitFor(() => expect(client.saveText).toHaveBeenCalledWith(expect.objectContaining({
        text: "recovered edit\n",
        recovery_cleanup: {
          recovery_id: "recovery-1",
          expected_recovery_revision: 2,
        },
      })));
    controller.mutate("edit while saving\n");
    pendingSave.resolve({ status: "saved", revision: 5, content_hash: "hash-saved" });
    await saving;
    await controller.flushRecovery();

    expect(client.checkpointRecovery).toHaveBeenCalledWith(expect.objectContaining({
      recovery_id: null,
      expected_recovery_revision: null,
      base_revision: 5,
      base_content_hash: "hash-saved",
      buffer: "edit while saving\n",
    }));
    expect(controller.getSnapshot()).toMatchObject({
      working_text: "edit while saving\n",
      dirty: true,
      recovery: {
        status: "durable",
        recovery_id: "recovery-new",
        recovery_revision: 1,
      },
    });
  });

  it("keeps exact dirty bytes and exposes retryable state after save or checkpoint failure", async () => {
    const client = fakeClient();
    vi.mocked(client.saveText).mockRejectedValue(new Error("save unavailable"));
    vi.mocked(client.checkpointRecovery).mockRejectedValue(new Error("checkpoint unavailable"));
    const controller = new FileEditorControllerRegistry(client, { checkpoint_debounce_ms: 60_000 })
      .forResource(owner.resource_id);
    await controller.initialize({ owner, text: "base\n", discover_recovery: false });
    controller.mutate("must survive\n");
    const generation = controller.getSnapshot().buffer_generation;

    await expect(controller.save()).rejects.toThrow("save unavailable");
    expect(controller.getSnapshot()).toMatchObject({
      working_text: "must survive\n",
      buffer_generation: generation,
      dirty: true,
      save_state: "error",
      last_error: "save unavailable",
    });
    await expect(controller.flushRecovery()).rejects.toThrow("checkpoint unavailable");
    expect(controller.getSnapshot()).toMatchObject({
      working_text: "must survive\n",
      buffer_generation: generation,
      dirty: true,
      recovery: {
        status: "error",
        buffer_generation: generation,
        retryable: true,
      },
    });
  });

  it("preserves a stale save buffer and asks the initiating presentation to compare", async () => {
    const client = fakeClient();
    vi.mocked(client.saveText).mockResolvedValue({
      status: "stale_conflict",
      revision: 6,
      content_hash: "hash-disk",
    });
    const openComparison = vi.fn();
    const controller = new FileEditorControllerRegistry(client).forResource(owner.resource_id);
    controller.attachPresentation("surface-a", { on_open_comparison: openComparison });
    await controller.initialize({ owner, text: "base\n", discover_recovery: false });
    controller.mutate("local\n");

    await expect(controller.save("surface-a")).resolves.toMatchObject({
      status: "stale_conflict",
    });
    expect(controller.getSnapshot()).toMatchObject({
      working_text: "local\n",
      dirty: true,
      stale: true,
      disk_head_hash: "hash-disk",
      disk_head_revision: 6,
    });
    expect(openComparison).toHaveBeenCalledOnce();
  });

  it("serializes recovery checkpoints so an older completion cannot win over the final mutation", async () => {
    const first = deferred<FileRecoveryCheckpointV1>();
    const second = deferred<FileRecoveryCheckpointV1>();
    const client = fakeClient();
    vi.mocked(client.checkpointRecovery)
      .mockReturnValueOnce(first.promise)
      .mockReturnValueOnce(second.promise);
    const controller = new FileEditorControllerRegistry(client, { checkpoint_debounce_ms: 60_000 })
      .forResource(owner.resource_id);
    await controller.initialize({ owner, text: "base\n", discover_recovery: false });
    controller.mutate("older\n");
    const flushing = controller.flushRecovery();
    await vi.waitFor(() => expect(client.checkpointRecovery).toHaveBeenCalledWith(
      expect.objectContaining({
        recovery_id: null,
        expected_recovery_revision: null,
        buffer: "older\n",
      }),
    ));
    controller.mutate("newest\n");
    first.resolve(checkpoint(1));
    await vi.waitFor(() => {
      expect(client.checkpointRecovery).toHaveBeenLastCalledWith(expect.objectContaining({
        recovery_id: "recovery-1",
        expected_recovery_revision: 1,
        buffer: "newest\n",
      }));
    });
    second.resolve(checkpoint(2));
    await flushing;

    expect(controller.getSnapshot()).toMatchObject({
      working_text: "newest\n",
      dirty: true,
      recovery: {
        status: "durable",
        recovery_revision: 2,
        buffer_generation: 2,
      },
    });
  });

  it("clears an exact discarded generation only after durable discard succeeds", async () => {
    const pendingDiscard = deferred<void>();
    const client = fakeClient();
    vi.mocked(client.discardRecovery).mockReturnValue(pendingDiscard.promise);
    const controller = new FileEditorControllerRegistry(client).forResource(owner.resource_id);
    await controller.initialize({ owner, text: "base\n", discover_recovery: true });
    const generation = controller.getSnapshot().buffer_generation;

    const discarding = controller.discard(generation);
    expect(controller.getSnapshot().dirty).toBe(true);
    pendingDiscard.resolve();
    await discarding;
    expect(controller.getSnapshot()).toMatchObject({
      working_text: "base\n",
      dirty: false,
      recovery: { status: "none" },
    });
    expect(client.discardRecovery).toHaveBeenCalledWith({
      recovery_id: "recovery-1",
      expected_recovery_revision: 2,
      resource_key: owner.resource_id,
    });
  });

  it("does not clear or replace a newer generation after save, discard, or disk snapshots", async () => {
    const pendingDiscard = deferred<void>();
    const client = fakeClient();
    vi.mocked(client.discardRecovery).mockReturnValue(pendingDiscard.promise);
    const controller = new FileEditorControllerRegistry(client).forResource(owner.resource_id);
    await controller.initialize({ owner, text: "base\n", discover_recovery: true });
    const discardedGeneration = controller.getSnapshot().buffer_generation;
    const discarding = controller.discard(discardedGeneration);
    controller.mutate("newer than discard\n");
    controller.applyAuthoritative({
      ...owner,
      revision: 7,
      descriptor: { ...descriptor, content_hash: "hash-external" },
    }, "external\n");
    pendingDiscard.resolve();
    await discarding;

    expect(controller.getSnapshot()).toMatchObject({
      working_text: "newer than discard\n",
      dirty: true,
      stale: true,
      disk_head_hash: "hash-external",
      disk_head_revision: 7,
    });
  });

  it("retires durable recovery before a reverted clean session can be released", async () => {
    const pendingDiscard = deferred<void>();
    const client = fakeClient();
    vi.mocked(client.discardRecovery).mockReturnValue(pendingDiscard.promise);
    const registry = new FileEditorControllerRegistry(client);
    const controller = registry.forResource(owner.resource_id);
    await controller.initialize({ owner, text: "base\n", discover_recovery: true });

    controller.mutate("base\n");
    expect(controller.getSnapshot().dirty).toBe(false);
    expect(controller.canReleaseAfterPostcommit()).toBe(false);
    pendingDiscard.resolve();
    await vi.waitFor(() => expect(controller.getSnapshot().recovery.status).toBe("none"));

    expect(controller.canReleaseAfterPostcommit()).toBe(true);
    expect(registry.releaseAfterPostcommit(owner.resource_id)).toBe(true);
    expect(client.discardRecovery).toHaveBeenCalledWith({
      recovery_id: "recovery-1",
      expected_recovery_revision: 2,
      resource_key: owner.resource_id,
    });
  });

  it("orders an in-flight checkpoint before save cleanup and recreates a raced newer recovery", async () => {
    const firstCheckpoint = deferred<FileRecoveryCheckpointV1>();
    const freshCheckpoint = deferred<FileRecoveryCheckpointV1>();
    const pendingSave = deferred<{
      status: "saved";
      revision: number;
      content_hash: string;
    }>();
    const client = fakeClient();
    vi.mocked(client.listRecoveries).mockResolvedValue([]);
    vi.mocked(client.checkpointRecovery).mockImplementation((request) => (
      request.base_revision === 4 ? firstCheckpoint.promise : freshCheckpoint.promise
    ));
    vi.mocked(client.saveText).mockReturnValue(pendingSave.promise);
    const controller = new FileEditorControllerRegistry(client, { checkpoint_debounce_ms: 60_000 })
      .forResource(owner.resource_id);
    await controller.initialize({ owner, text: "base\n", discover_recovery: false });
    controller.mutate("save snapshot\n");
    const flushing = controller.flushRecovery();
    const saving = controller.save();
    controller.mutate("newer raced edit\n");

    expect(client.saveText).not.toHaveBeenCalled();
    firstCheckpoint.resolve(checkpoint(1));
    await vi.waitFor(() => expect(client.saveText).toHaveBeenCalledWith(expect.objectContaining({
      text: "save snapshot\n",
      recovery_cleanup: {
        recovery_id: "recovery-1",
        expected_recovery_revision: 1,
      },
    })));
    pendingSave.resolve({ status: "saved", revision: 5, content_hash: "hash-saved" });
    await saving;
    await vi.waitFor(() => expect(client.checkpointRecovery).toHaveBeenCalledTimes(2));
    expect(client.checkpointRecovery).toHaveBeenLastCalledWith(expect.objectContaining({
      recovery_id: null,
      expected_recovery_revision: null,
      base_revision: 5,
      base_content_hash: "hash-saved",
      buffer: "newer raced edit\n",
    }));
    freshCheckpoint.resolve(checkpoint(1, { recovery_id: "recovery-fresh" }));
    await flushing;
    await vi.waitFor(() => expect(controller.getSnapshot()).toMatchObject({
        working_text: "newer raced edit\n",
        dirty: true,
        recovery: {
          status: "durable",
          recovery_id: "recovery-fresh",
          recovery_revision: 1,
        },
      }));
  });

  it("orders discard after an in-flight checkpoint without leaving a late recovery", async () => {
    const pendingCheckpoint = deferred<FileRecoveryCheckpointV1>();
    const pendingDiscard = deferred<void>();
    const client = fakeClient();
    vi.mocked(client.listRecoveries).mockResolvedValue([]);
    vi.mocked(client.checkpointRecovery).mockReturnValue(pendingCheckpoint.promise);
    vi.mocked(client.discardRecovery).mockReturnValue(pendingDiscard.promise);
    const controller = new FileEditorControllerRegistry(client, { checkpoint_debounce_ms: 60_000 })
      .forResource(owner.resource_id);
    await controller.initialize({ owner, text: "base\n", discover_recovery: false });
    controller.mutate("discard me\n");
    const generation = controller.getSnapshot().buffer_generation;
    const flushing = controller.flushRecovery();
    const discarding = controller.discard(generation);

    expect(client.discardRecovery).not.toHaveBeenCalled();
    pendingCheckpoint.resolve(checkpoint(1));
    await vi.waitFor(() => expect(client.discardRecovery).toHaveBeenCalledWith({
      recovery_id: "recovery-1",
      expected_recovery_revision: 1,
      resource_key: owner.resource_id,
    }));
    pendingDiscard.resolve();
    await expect(discarding).resolves.toBe(true);
    await flushing;
    expect(controller.getSnapshot()).toMatchObject({
      working_text: "base\n",
      dirty: false,
      recovery: { status: "none" },
    });
    expect(client.checkpointRecovery).toHaveBeenCalledOnce();
  });

  it("allows Don't Save after an earlier checkpoint fails", async () => {
    const pendingCheckpoint = deferred<FileRecoveryCheckpointV1>();
    const client = fakeClient();
    vi.mocked(client.listRecoveries).mockResolvedValue([]);
    vi.mocked(client.checkpointRecovery).mockReturnValue(pendingCheckpoint.promise);
    const controller = new FileEditorControllerRegistry(client, { checkpoint_debounce_ms: 60_000 })
      .forResource(owner.resource_id);
    await controller.initialize({ owner, text: "base\n", discover_recovery: false });
    controller.mutate("discard despite failure\n");
    const generation = controller.getSnapshot().buffer_generation;
    const flushing = controller.flushRecovery();
    const discarding = controller.discard(generation);
    pendingCheckpoint.reject(new Error("recovery unavailable"));

    await expect(flushing).rejects.toThrow("recovery unavailable");
    await expect(discarding).resolves.toBe(true);
    expect(client.discardRecovery).not.toHaveBeenCalled();
    expect(controller.getSnapshot()).toMatchObject({
      working_text: "base\n",
      dirty: false,
      recovery: { status: "none" },
    });
  });

  it("rejects cross-resource owners and ignores older revisions from the same subscription", async () => {
    const controller = new FileEditorControllerRegistry(fakeClient())
      .forResource(owner.resource_id);
    expect(() => controller.applyAuthoritative({
      ...owner,
      resource_id: "file:C:/work/Other.md",
    }, "wrong\n")).toThrow("does not match");
    await controller.initialize({ owner, text: "base\n", discover_recovery: false });
    controller.applyAuthoritative({
      ...owner,
      revision: 6,
      descriptor: { ...descriptor, content_hash: "hash-new" },
    }, "new\n");
    controller.applyAuthoritative({
      ...owner,
      revision: 5,
      descriptor: { ...descriptor, content_hash: "hash-old" },
    }, "old\n");
    controller.applyAuthoritative({
      ...owner,
      revision: 6,
      descriptor: { ...descriptor, content_hash: "hash-same-revision-replay" },
    }, "same revision replay\n");

    expect(controller.getSnapshot()).toMatchObject({
      working_text: "new\n",
      saved_text: "new\n",
      disk_head_revision: 6,
      disk_head_hash: "hash-new",
    });
  });

  it("publishes immutable tear-free snapshots and explicit membership generations", async () => {
    const controller = new FileEditorControllerRegistry(fakeClient())
      .forResource(owner.resource_id);
    const observed: unknown[] = [];
    const unsubscribe = controller.subscribe(() => observed.push(controller.getSnapshot()));
    const before = controller.getSnapshot();
    expect(controller.getSnapshot()).toBe(before);
    const membership = controller.attachPresentation("surface-a", {});
    expect(controller.getSnapshot()).not.toBe(before);
    expect(controller.getSnapshot()).toBe(controller.getSnapshot());
    const attachedGeneration = membership.generation;
    membership.detach();
    expect(controller.observePresentation("surface-a")).toEqual({
      attached: false,
      generation: attachedGeneration + 1,
    });
    expect(observed.length).toBeGreaterThanOrEqual(2);
    unsubscribe();
  });
});
