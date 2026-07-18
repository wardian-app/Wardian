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
import { fileDiffForController } from "./fileDiffModel";

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
  let recoveries = [recoverySummary()];
  return {
    saveText: vi.fn().mockResolvedValue({
      status: "saved",
      revision: 5,
      content_hash: "hash-saved",
    }),
    readText: vi.fn().mockImplementation(async (request) => ({
      schema: 1,
      resource_id: request.resource_id,
      revision: request.revision,
      text: "disk\n",
    })),
    checkpointRecovery: vi.fn().mockResolvedValue(checkpoint(1)),
    listRecoveries: vi.fn().mockImplementation(async () => [...recoveries]),
    getRecovery: vi.fn().mockResolvedValue(recovery()),
    discardRecovery: vi.fn().mockImplementation(async (request) => {
      recoveries = recoveries.filter((candidate) => candidate.recovery_id !== request.recovery_id);
    }),
    mergeRecovery: vi.fn().mockResolvedValue({
      status: "clean",
      recovery_revision: 1,
      current_revision: 6,
      current_content_hash: "hash-disk",
      disk_changed: true,
      merged_text: "merged local edit\n",
    }),
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
    vi.mocked(client.checkpointRecovery).mockResolvedValue(checkpoint(1, {
      recovery_id: "recovery-current",
    }));
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

    expect(client.getRecovery).toHaveBeenCalledOnce();
    expect(controller.getSnapshot()).toMatchObject({
      working_text: "typed before discovery\n",
      dirty: true,
      buffer_generation: 1,
      recovery_discovery: "conflict",
      recovery: {
        status: "conflict",
        conflicting_recovery_id: "recovery-1",
        current_recovery_id: "recovery-current",
        current_durable: true,
      },
    });
    await expect(controller.save()).rejects.toThrow(
      "Recovery conflict must be resolved before Save.",
    );
    expect(client.saveText).not.toHaveBeenCalled();
    expect(client.checkpointRecovery).toHaveBeenCalledWith(expect.objectContaining({
      recovery_id: null,
      buffer: "typed before discovery\n",
    }));

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

  it("blocks both recovery conflict choices while the current checkpoint is pending", async () => {
    const discovery = deferred<FileRecoverySummaryV1[]>();
    const pendingCheckpoint = deferred<FileRecoveryCheckpointV1>();
    const client = fakeClient();
    vi.mocked(client.listRecoveries).mockReturnValue(discovery.promise);
    vi.mocked(client.checkpointRecovery).mockReturnValue(pendingCheckpoint.promise);
    const controller = new FileEditorControllerRegistry(client).forResource(owner.resource_id);

    const initialization = controller.initialize({
      owner,
      text: "base\n",
      discover_recovery: true,
    });
    controller.mutate("newer current edit\n");
    discovery.resolve([recoverySummary()]);
    await vi.waitFor(() => expect(controller.getSnapshot().recovery).toMatchObject({
      status: "conflict",
      current_durable: false,
      buffer_generation: 1,
    }));

    await expect(controller.resolveRecoveryConflict("keep_current"))
      .rejects.toThrow("Current edits must be durable before resolving the recovery conflict.");
    await expect(controller.resolveRecoveryConflict("use_recovered"))
      .rejects.toThrow("Current edits must be durable before resolving the recovery conflict.");
    expect(controller.getSnapshot().working_text).toBe("newer current edit\n");

    pendingCheckpoint.resolve(checkpoint(1, { recovery_id: "recovery-current" }));
    await initialization;
    await controller.resolveRecoveryConflict("use_recovered");
    expect(controller.getSnapshot()).toMatchObject({
      working_text: "recovered edit\n",
      recovery: { status: "durable", recovery_id: "recovery-1" },
    });
    await Promise.resolve();
    expect(controller.getSnapshot()).toMatchObject({
      working_text: "recovered edit\n",
      recovery: { status: "durable", recovery_id: "recovery-1" },
    });
  });

  it("blocks both recovery conflict choices after the current checkpoint fails", async () => {
    const discovery = deferred<FileRecoverySummaryV1[]>();
    const client = fakeClient();
    vi.mocked(client.listRecoveries).mockReturnValue(discovery.promise);
    vi.mocked(client.checkpointRecovery).mockRejectedValue(new Error("checkpoint unavailable"));
    const controller = new FileEditorControllerRegistry(client).forResource(owner.resource_id);

    const initialization = controller.initialize({
      owner,
      text: "base\n",
      discover_recovery: true,
    });
    controller.mutate("newer current edit\n");
    discovery.resolve([recoverySummary()]);
    await expect(initialization).rejects.toThrow("checkpoint unavailable");

    expect(controller.getSnapshot()).toMatchObject({
      working_text: "newer current edit\n",
      recovery_discovery: "conflict",
      recovery: {
        status: "conflict",
        current_durable: false,
      },
    });
    await expect(controller.resolveRecoveryConflict("keep_current"))
      .rejects.toThrow("Current edits must be durable before resolving the recovery conflict.");
    await expect(controller.resolveRecoveryConflict("use_recovered"))
      .rejects.toThrow("Current edits must be durable before resolving the recovery conflict.");
    expect(client.discardRecovery).not.toHaveBeenCalled();
  });

  it("invalidates conflict durability immediately when the current buffer advances", async () => {
    const discovery = deferred<FileRecoverySummaryV1[]>();
    const client = fakeClient();
    vi.mocked(client.listRecoveries).mockReturnValue(discovery.promise);
    vi.mocked(client.checkpointRecovery)
      .mockResolvedValueOnce(checkpoint(1, { recovery_id: "recovery-current" }))
      .mockResolvedValueOnce(checkpoint(2, { recovery_id: "recovery-current" }));
    const controller = new FileEditorControllerRegistry(client, {
      checkpoint_debounce_ms: 60_000,
    }).forResource(owner.resource_id);

    const initialization = controller.initialize({
      owner,
      text: "base\n",
      discover_recovery: true,
    });
    controller.mutate("first current edit\n");
    discovery.resolve([recoverySummary()]);
    await initialization;
    expect(controller.getSnapshot().recovery).toMatchObject({
      status: "conflict",
      current_durable: true,
      buffer_generation: 1,
    });

    controller.mutate("second current edit\n");
    expect(controller.getSnapshot().recovery).toMatchObject({
      status: "conflict",
      current_durable: false,
      buffer_generation: 2,
    });
    await expect(controller.resolveRecoveryConflict("use_recovered"))
      .rejects.toThrow("Current edits must be durable before resolving the recovery conflict.");

    await controller.flushRecovery();
    expect(controller.getSnapshot().recovery).toMatchObject({
      status: "conflict",
      current_durable: true,
      buffer_generation: 2,
    });
  });

  it("pins an attached transient presentation when later recovery discovery restores dirty text", async () => {
    const discovery = deferred<FileRecoverySummaryV1[]>();
    const client = fakeClient();
    vi.mocked(client.listRecoveries).mockReturnValue(discovery.promise);
    const controller = new FileEditorControllerRegistry(client).forResource(owner.resource_id);
    const pin = vi.fn();
    controller.attachPresentation("surface-a", { on_pin: pin });

    const initialization = controller.initialize({
      owner,
      text: "base\n",
      discover_recovery: true,
    });
    expect(pin).not.toHaveBeenCalled();
    discovery.resolve([recoverySummary()]);
    await initialization;

    expect(controller.getSnapshot()).toMatchObject({ dirty: true });
    expect(pin).toHaveBeenCalledOnce();
  });

  it.each(["list", "get"] as const)(
    "retries an incomplete %s recovery discovery without replacing the ready controller",
    async (failurePoint) => {
      const client = fakeClient();
      if (failurePoint === "list") {
        vi.mocked(client.listRecoveries)
          .mockRejectedValueOnce(new Error("recovery list unavailable"))
          .mockResolvedValueOnce([recoverySummary()]);
      } else {
        vi.mocked(client.listRecoveries).mockResolvedValue([recoverySummary()]);
        vi.mocked(client.getRecovery)
          .mockRejectedValueOnce(new Error("recovery read unavailable"))
          .mockResolvedValueOnce(recovery());
      }
      const registry = new FileEditorControllerRegistry(client);
      const controller = registry.forResource(owner.resource_id);
      const pin = vi.fn();
      controller.attachPresentation("surface-a", { on_pin: pin });
      const read = vi.fn().mockResolvedValue({
        schema: 1 as const,
        resource_id: owner.resource_id,
        revision: owner.revision,
        text: "base\n",
      });

      await expect(
        registry.synchronizeAuthoritative(owner, read, "surface-a"),
      ).rejects.toThrow(failurePoint === "list" ? "list unavailable" : "read unavailable");
      expect(controller.getSnapshot()).toMatchObject({
        status: "ready",
        working_text: "base\n",
        dirty: false,
      });

      await registry.synchronizeAuthoritative(owner, read, "surface-a");

      expect(controller.getSnapshot()).toMatchObject({
        working_text: "recovered edit\n",
        dirty: true,
        recovery: { status: "durable", recovery_id: "recovery-1" },
      });
      expect(pin).toHaveBeenCalledOnce();
      expect(client.listRecoveries).toHaveBeenCalledTimes(2);
      expect(client.getRecovery).toHaveBeenCalledTimes(failurePoint === "list" ? 1 : 2);
    },
  );

  it("checkpoints the exact dirty generation when Retry confirms no older recovery exists", async () => {
    const client = fakeClient();
    vi.mocked(client.listRecoveries)
      .mockRejectedValueOnce(new Error("recovery list unavailable"))
      .mockResolvedValueOnce([]);
    vi.mocked(client.checkpointRecovery).mockResolvedValue(checkpoint(1, {
      recovery_id: "recovery-current",
    }));
    const registry = new FileEditorControllerRegistry(client, {
      checkpoint_debounce_ms: 60_000,
    });
    const controller = registry.forResource(owner.resource_id);
    controller.attachPresentation("surface-a", {});
    const read = vi.fn().mockResolvedValue({
      schema: 1 as const,
      resource_id: owner.resource_id,
      revision: owner.revision,
      text: "base\n",
    });
    await expect(
      registry.synchronizeAuthoritative(owner, read, "surface-a"),
    ).rejects.toThrow("recovery list unavailable");
    controller.mutate("exact current edit\n");

    await controller.retryRecovery();

    expect(client.checkpointRecovery).toHaveBeenCalledWith(expect.objectContaining({
      recovery_id: null,
      expected_recovery_revision: null,
      base: "base\n",
      buffer: "exact current edit\n",
    }));
    expect(controller.getSnapshot()).toMatchObject({
      working_text: "exact current edit\n",
      dirty: true,
      recovery_discovery: "complete",
      recovery: {
        status: "durable",
        recovery_id: "recovery-current",
        buffer_generation: 1,
      },
      last_error: null,
    });
  });

  it("durably preserves current dirty bytes beside a distinct discovered recovery conflict", async () => {
    const client = fakeClient();
    vi.mocked(client.listRecoveries)
      .mockRejectedValueOnce(new Error("recovery list unavailable"))
      .mockResolvedValueOnce([recoverySummary({ recovery_id: "recovery-older" })]);
    vi.mocked(client.getRecovery).mockResolvedValue(recovery({
      recovery_id: "recovery-older",
      buffer: "older recovered edit\n",
    }));
    vi.mocked(client.checkpointRecovery).mockResolvedValue(checkpoint(1, {
      recovery_id: "recovery-current",
    }));
    const registry = new FileEditorControllerRegistry(client, {
      checkpoint_debounce_ms: 60_000,
    });
    const controller = registry.forResource(owner.resource_id);
    controller.attachPresentation("surface-a", {});
    const read = vi.fn().mockResolvedValue({
      schema: 1 as const,
      resource_id: owner.resource_id,
      revision: owner.revision,
      text: "base\n",
    });
    await expect(
      registry.synchronizeAuthoritative(owner, read, "surface-a"),
    ).rejects.toThrow("recovery list unavailable");
    controller.mutate("newer exact current edit\n");

    await controller.retryRecovery();

    expect(client.checkpointRecovery).toHaveBeenCalledWith(expect.objectContaining({
      recovery_id: null,
      expected_recovery_revision: null,
      base: "base\n",
      buffer: "newer exact current edit\n",
    }));
    expect(client.discardRecovery).not.toHaveBeenCalled();
    expect(controller.getSnapshot()).toMatchObject({
      working_text: "newer exact current edit\n",
      dirty: true,
      recovery_discovery: "conflict",
      recovery: {
        status: "conflict",
        conflicting_recovery_id: "recovery-older",
        current_recovery_id: "recovery-current",
        current_durable: true,
        buffer_generation: 1,
      },
    });
    await expect(controller.save()).rejects.toThrow(
      "Recovery conflict must be resolved before Save.",
    );

    await controller.resolveRecoveryConflict("keep_current");
    expect(controller.getSnapshot()).toMatchObject({
      working_text: "newer exact current edit\n",
      recovery_discovery: "complete",
      recovery: {
        status: "durable",
        recovery_id: "recovery-current",
      },
    });
    expect(client.discardRecovery).toHaveBeenCalledWith({
      recovery_id: "recovery-older",
      expected_recovery_revision: 2,
      resource_key: owner.resource_id,
    });
    vi.mocked(client.listRecoveries).mockResolvedValue([]);
    await expect(controller.save()).resolves.toMatchObject({ status: "saved" });
    expect(client.saveText).toHaveBeenCalledWith(expect.objectContaining({
      text: "newer exact current edit\n",
      recovery_cleanup: {
        recovery_id: "recovery-current",
        expected_recovery_revision: 1,
      },
    }));
  });

  it("durably rejects the current checkpoint before using and saving the older recovery", async () => {
    const client = fakeClient();
    vi.mocked(client.listRecoveries)
      .mockRejectedValueOnce(new Error("recovery list unavailable"))
      .mockResolvedValueOnce([recoverySummary({ recovery_id: "recovery-older" })]);
    vi.mocked(client.getRecovery).mockResolvedValue(recovery({
      recovery_id: "recovery-older",
      base: "older base\n",
      buffer: "older recovered edit\n",
    }));
    vi.mocked(client.checkpointRecovery).mockResolvedValue(checkpoint(1, {
      recovery_id: "recovery-current",
    }));
    const registry = new FileEditorControllerRegistry(client, {
      checkpoint_debounce_ms: 60_000,
    });
    const controller = registry.forResource(owner.resource_id);
    controller.attachPresentation("surface-a", {});
    const read = vi.fn().mockResolvedValue({
      schema: 1 as const,
      resource_id: owner.resource_id,
      revision: owner.revision,
      text: "base\n",
    });
    await expect(
      registry.synchronizeAuthoritative(owner, read, "surface-a"),
    ).rejects.toThrow("recovery list unavailable");
    controller.mutate("newer exact current edit\n");
    await controller.retryRecovery();

    await controller.resolveRecoveryConflict("use_recovered");

    expect(controller.getSnapshot()).toMatchObject({
      saved_text: "older base\n",
      working_text: "older recovered edit\n",
      dirty: true,
      recovery_discovery: "complete",
      recovery: {
        status: "durable",
        recovery_id: "recovery-older",
      },
    });
    expect(client.checkpointRecovery).toHaveBeenCalledWith(expect.objectContaining({
      recovery_id: null,
      buffer: "newer exact current edit\n",
    }));
    expect(client.discardRecovery).toHaveBeenCalledWith({
      recovery_id: "recovery-current",
      expected_recovery_revision: 1,
      resource_key: owner.resource_id,
    });

    vi.mocked(client.listRecoveries).mockResolvedValue([]);
    await expect(controller.save()).resolves.toMatchObject({ status: "saved" });
    expect(client.saveText).toHaveBeenCalledWith(expect.objectContaining({
      text: "older recovered edit\n",
      recovery_cleanup: {
        recovery_id: "recovery-older",
        expected_recovery_revision: 2,
      },
    }));
  });

  it("does not implicitly delete either recovery when conflict edits return to the saved text", async () => {
    const client = fakeClient();
    vi.mocked(client.listRecoveries)
      .mockRejectedValueOnce(new Error("recovery list unavailable"))
      .mockResolvedValueOnce([recoverySummary({ recovery_id: "recovery-older" })]);
    vi.mocked(client.getRecovery).mockResolvedValue(recovery({
      recovery_id: "recovery-older",
      buffer: "older recovered edit\n",
    }));
    vi.mocked(client.checkpointRecovery).mockResolvedValue(checkpoint(1, {
      recovery_id: "recovery-current",
    }));
    const registry = new FileEditorControllerRegistry(client, {
      checkpoint_debounce_ms: 60_000,
    });
    const controller = registry.forResource(owner.resource_id);
    controller.attachPresentation("surface-a", {});
    const read = vi.fn().mockResolvedValue({
      schema: 1 as const,
      resource_id: owner.resource_id,
      revision: owner.revision,
      text: "base\n",
    });
    await expect(
      registry.synchronizeAuthoritative(owner, read, "surface-a"),
    ).rejects.toThrow("recovery list unavailable");
    controller.mutate("newer exact current edit\n");
    await controller.retryRecovery();

    controller.mutate("base\n");
    controller.mutate("base\n");

    expect(controller.getSnapshot()).toMatchObject({
      working_text: "base\n",
      dirty: false,
      recovery_discovery: "conflict",
      recovery: {
        status: "conflict",
        conflicting_recovery_id: "recovery-older",
        current_recovery_id: "recovery-current",
        current_durable: true,
      },
    });
    expect(client.discardRecovery).not.toHaveBeenCalled();

    await controller.resolveRecoveryConflict("keep_current");
    expect(controller.getSnapshot()).toMatchObject({
      working_text: "base\n",
      dirty: false,
      recovery_discovery: "complete",
      recovery: { status: "none" },
    });
    expect(client.discardRecovery).toHaveBeenCalledWith({
      recovery_id: "recovery-older",
      expected_recovery_revision: 2,
      resource_key: owner.resource_id,
    });
  });

  it("coalesces exact authoritative reads and discovers recovery only for an uninitialized controller", async () => {
    const client = fakeClient();
    vi.mocked(client.listRecoveries).mockResolvedValue([]);
    const registry = new FileEditorControllerRegistry(client);
    registry.forResource(owner.resource_id).attachPresentation("surface-a", {});
    const readInitial = vi.fn().mockResolvedValue({
      schema: 1 as const,
      resource_id: owner.resource_id,
      revision: owner.revision,
      text: "base\n",
    });

    const [first, duplicate] = await Promise.all([
      registry.synchronizeAuthoritative(owner, readInitial),
      registry.synchronizeAuthoritative(owner, readInitial),
    ]);

    expect(first).toBe(duplicate);
    expect(readInitial).toHaveBeenCalledOnce();
    expect(client.listRecoveries).toHaveBeenCalledOnce();

    first.mutate("newer in-memory edit\n");
    const externalOwner = {
      ...owner,
      revision: 5,
      descriptor: { ...descriptor, content_hash: "hash-external" },
    };
    const readExternal = vi.fn().mockResolvedValue({
      schema: 1 as const,
      resource_id: owner.resource_id,
      revision: 5,
      text: "external disk text\n",
    });
    await registry.synchronizeAuthoritative(externalOwner, readExternal);

    expect(readExternal).toHaveBeenCalledOnce();
    expect(client.listRecoveries).toHaveBeenCalledOnce();
    expect(first.getSnapshot()).toMatchObject({
      working_text: "newer in-memory edit\n",
      dirty: true,
      stale: true,
      disk_head_revision: 5,
      disk_head_hash: "hash-external",
    });
  });

  it("adopts the exact newer disk head when stale edits return to the old saved text", async () => {
    const client = fakeClient();
    vi.mocked(client.listRecoveries).mockResolvedValue([]);
    const registry = new FileEditorControllerRegistry(client, { checkpoint_debounce_ms: 60_000 });
    const controller = registry.forResource(owner.resource_id);
    await controller.initialize({ owner, text: "base\n", discover_recovery: true });

    controller.mutate("local edit\n");
    controller.applyAuthoritative({
      ...owner,
      revision: 5,
      descriptor: { ...descriptor, content_hash: "hash-external" },
    }, "external disk text\n");
    expect(controller.getSnapshot()).toMatchObject({
      working_text: "local edit\n",
      dirty: true,
      stale: true,
    });

    controller.mutate("base\n");

    expect(controller.getSnapshot()).toMatchObject({
      base_revision: 5,
      buffer_base_hash: "hash-external",
      disk_head_revision: 5,
      disk_head_hash: "hash-external",
      saved_text: "external disk text\n",
      working_text: "external disk text\n",
      dirty: false,
      stale: false,
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
    expect(controller.canReleaseAfterPostcommit(0)).toBe(false);
    pendingDiscard.resolve();
    await vi.waitFor(() => expect(controller.getSnapshot().recovery.status).toBe("none"));
    expect(controller.canReleaseAfterPostcommit(0)).toBe(true);
  });

  it("retires a stale clean recovery without replacing the authoritative disk text", async () => {
    const pendingDiscard = deferred<void>();
    const client = fakeClient();
    vi.mocked(client.getRecovery).mockResolvedValue(recovery({
      base_content_hash: "hash-stale-recovery",
      base: "obsolete clean recovery\n",
      buffer: "obsolete clean recovery\n",
    }));
    vi.mocked(client.discardRecovery).mockReturnValue(pendingDiscard.promise);
    const controller = new FileEditorControllerRegistry(client).forResource(owner.resource_id);

    await controller.initialize({ owner, text: "current authoritative disk\n", discover_recovery: true });
    expect(controller.getSnapshot()).toMatchObject({
      saved_text: "current authoritative disk\n",
      working_text: "current authoritative disk\n",
      buffer_base_hash: "hash-base",
      disk_head_hash: "hash-base",
      base_revision: 4,
      dirty: false,
      stale: false,
      recovery: { status: "durable", recovery_id: "recovery-1" },
    });

    pendingDiscard.resolve();
    await vi.waitFor(() => expect(controller.getSnapshot().recovery.status).toBe("none"));
    expect(controller.getSnapshot()).toMatchObject({
      saved_text: "current authoritative disk\n",
      working_text: "current authoritative disk\n",
      buffer_base_hash: "hash-base",
      disk_head_hash: "hash-base",
      dirty: false,
      stale: false,
    });
  });

  it("retires a lingering recovery whose buffer is already the authoritative disk text", async () => {
    const client = fakeClient();
    vi.mocked(client.getRecovery).mockResolvedValue(recovery({
      base_content_hash: "hash-older-base",
      base: "older base\n",
      buffer: "already saved on disk\n",
    }));
    const controller = new FileEditorControllerRegistry(client).forResource(owner.resource_id);

    await controller.initialize({
      owner,
      text: "already saved on disk\n",
      discover_recovery: true,
    });
    await vi.waitFor(() => expect(controller.getSnapshot().recovery.status).toBe("none"));

    expect(controller.getSnapshot()).toMatchObject({
      saved_text: "already saved on disk\n",
      working_text: "already saved on disk\n",
      buffer_base_hash: "hash-base",
      disk_head_hash: "hash-base",
      dirty: false,
      stale: false,
    });
    expect(client.discardRecovery).toHaveBeenCalledWith({
      recovery_id: "recovery-1",
      expected_recovery_revision: 2,
      resource_key: owner.resource_id,
    });
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

  it("rebinds a dirty matching base to a new authorized subscription incarnation", async () => {
    const client = fakeClient();
    const controller = new FileEditorControllerRegistry(client, {
      checkpoint_debounce_ms: 60_000,
    }).forResource(owner.resource_id);
    await controller.initialize({ owner, text: "base\n", discover_recovery: false });
    controller.mutate("local edit\n");

    controller.applyAuthoritative({
      ...owner,
      subscription_id: "subscription-reopened",
      revision: 1,
    }, "base\n");
    await controller.save();

    expect(client.saveText).toHaveBeenCalledWith(expect.objectContaining({
      resource_id: owner.resource_id,
      subscription_id: "subscription-reopened",
      expected_revision: 1,
      buffer_base_hash: "hash-base",
      text: "local edit\n",
    }));
    expect(controller.getSnapshot()).toMatchObject({
      subscription_id: "subscription-reopened",
      base_revision: 5,
      dirty: false,
      stale: false,
    });
  });

  it("rebinds Save and recovery to a surviving presentation owner when an alias closes", async () => {
    const client = fakeClient();
    vi.mocked(client.listRecoveries).mockResolvedValue([]);
    vi.mocked(client.checkpointRecovery).mockResolvedValue(checkpoint(1, {
      recovery_id: "recovery-survivor",
    }));
    const registry = new FileEditorControllerRegistry(client, {
      checkpoint_debounce_ms: 60_000,
    });
    const controller = registry.forResource(owner.resource_id);
    const firstMembership = controller.attachPresentation("surface-a", {});
    const secondMembership = controller.attachPresentation("surface-b", {});
    const firstOwner = { ...owner, subscription_id: "subscription-a" };
    const secondOwner = { ...owner, subscription_id: "subscription-b" };
    const read = (candidate: FileResourceSnapshotV1) => vi.fn().mockResolvedValue({
      schema: 1 as const,
      resource_id: candidate.resource_id,
      revision: candidate.revision,
      text: "base\n",
    });

    await registry.synchronizeAuthoritative(firstOwner, read(firstOwner), "surface-a");
    await registry.synchronizeAuthoritative(secondOwner, read(secondOwner), "surface-b");
    expect(controller.getSnapshot().subscription_id).toBe("subscription-b");

    secondMembership.detach();
    expect(controller.getSnapshot().subscription_id).toBe("subscription-a");
    controller.mutate("surviving edit\n");
    await controller.flushRecovery();
    await controller.save("surface-a");

    expect(client.checkpointRecovery).toHaveBeenCalledWith(expect.objectContaining({
      subscription_id: "subscription-a",
      buffer: "surviving edit\n",
    }));
    expect(client.saveText).toHaveBeenCalledWith(expect.objectContaining({
      subscription_id: "subscription-a",
      text: "surviving edit\n",
    }));
    firstMembership.detach();
  });

  it("promotes a surviving alias when the current owner closes during first recovery discovery", async () => {
    const pendingDiscovery = deferred<FileRecoverySummaryV1[]>();
    const client = fakeClient();
    vi.mocked(client.listRecoveries).mockReturnValue(pendingDiscovery.promise);
    const registry = new FileEditorControllerRegistry(client, {
      checkpoint_debounce_ms: 60_000,
    });
    const controller = registry.forResource(owner.resource_id);
    const firstMembership = controller.attachPresentation("surface-a", {});
    const secondMembership = controller.attachPresentation("surface-b", {});
    const firstOwner = { ...owner, subscription_id: "subscription-a" };
    const secondOwner = { ...owner, subscription_id: "subscription-b" };
    const read = (candidate: FileResourceSnapshotV1) => vi.fn().mockResolvedValue({
      schema: 1 as const,
      resource_id: candidate.resource_id,
      revision: candidate.revision,
      text: "base\n",
    });

    const firstSync = registry.synchronizeAuthoritative(
      firstOwner,
      read(firstOwner),
      "surface-a",
    );
    await vi.waitFor(() => expect(controller.getSnapshot().subscription_id)
      .toBe("subscription-a"));
    const secondSync = registry.synchronizeAuthoritative(
      secondOwner,
      read(secondOwner),
      "surface-b",
    );
    await vi.waitFor(() => expect(controller.getSnapshot().subscription_id)
      .toBe("subscription-b"));

    secondMembership.detach();
    expect(controller.getSnapshot().subscription_id).toBe("subscription-a");
    pendingDiscovery.resolve([]);
    await Promise.all([firstSync, secondSync]);
    expect(controller.getSnapshot().subscription_id).toBe("subscription-a");

    controller.mutate("survivor after pending discovery\n");
    await controller.flushRecovery();
    await controller.save("surface-a");
    expect(client.checkpointRecovery).toHaveBeenCalledWith(expect.objectContaining({
      subscription_id: "subscription-a",
    }));
    expect(client.saveText).toHaveBeenCalledWith(expect.objectContaining({
      subscription_id: "subscription-a",
    }));
    firstMembership.detach();
  });

  it("does not apply an alias owner whose presentation closes before its exact read resolves", async () => {
    const pendingRead = deferred<{
      schema: 1;
      resource_id: string;
      revision: number;
      text: string;
    }>();
    const client = fakeClient();
    vi.mocked(client.listRecoveries).mockResolvedValue([]);
    const registry = new FileEditorControllerRegistry(client, {
      checkpoint_debounce_ms: 60_000,
    });
    const controller = registry.forResource(owner.resource_id);
    const firstMembership = controller.attachPresentation("surface-a", {});
    const firstOwner = { ...owner, subscription_id: "subscription-a" };
    await registry.synchronizeAuthoritative(firstOwner, async () => ({
      schema: 1,
      resource_id: firstOwner.resource_id,
      revision: firstOwner.revision,
      text: "base\n",
    }), "surface-a");
    const secondMembership = controller.attachPresentation("surface-b", {});
    const secondOwner = { ...owner, subscription_id: "subscription-b" };

    const secondSync = registry.synchronizeAuthoritative(
      secondOwner,
      () => pendingRead.promise,
      "surface-b",
    );
    secondMembership.detach();
    pendingRead.resolve({
      schema: 1,
      resource_id: secondOwner.resource_id,
      revision: secondOwner.revision,
      text: "base\n",
    });
    await secondSync;

    expect(controller.getSnapshot()).toMatchObject({
      subscription_id: "subscription-a",
      recovery_discovery: "complete",
    });
    controller.mutate("survivor after pending read\n");
    await controller.flushRecovery();
    expect(client.checkpointRecovery).toHaveBeenCalledWith(expect.objectContaining({
      subscription_id: "subscription-a",
    }));
    firstMembership.detach();
  });

  it("blocks guarded writes while a promoted fallback has not discovered recovery", async () => {
    const client = fakeClient();
    const controller = new FileEditorControllerRegistry(client, {
      checkpoint_debounce_ms: 60_000,
    }).forResource(owner.resource_id);
    controller.attachPresentation("surface-a", {});
    controller.bindPresentationOwner("surface-a", owner, "base\n");
    controller.mutate("edit before discovery\n");

    expect(controller.getSnapshot().recovery_discovery).toBe("not_started");
    await expect(controller.save()).rejects.toThrow(
      "Recovery discovery must complete before Save.",
    );
    await expect(controller.flushRecovery()).rejects.toThrow(
      "Recovery discovery must complete before checkpointing newer edits.",
    );
    expect(client.saveText).not.toHaveBeenCalled();
    expect(client.checkpointRecovery).not.toHaveBeenCalled();
  });

  it("deduplicates queued Saves of one generation after the first Save succeeds", async () => {
    const pendingSave = deferred<{
      status: "saved";
      revision: number;
      content_hash: string;
    }>();
    const client = fakeClient();
    vi.mocked(client.saveText)
      .mockReturnValueOnce(pendingSave.promise)
      .mockResolvedValueOnce({
        status: "stale_conflict",
        revision: 6,
        content_hash: "hash-self-conflict",
      });
    const controller = new FileEditorControllerRegistry(client, {
      checkpoint_debounce_ms: 60_000,
    }).forResource(owner.resource_id);
    await controller.initialize({ owner, text: "base\n", discover_recovery: false });
    controller.mutate("save once\n");

    const first = controller.save();
    const duplicate = controller.save();
    pendingSave.resolve({ status: "saved", revision: 5, content_hash: "hash-saved" });

    await expect(first).resolves.toMatchObject({ status: "saved" });
    await expect(duplicate).resolves.toMatchObject({
      status: "unchanged",
      revision: 5,
      content_hash: "hash-saved",
    });
    expect(client.saveText).toHaveBeenCalledOnce();
    expect(controller.getSnapshot()).toMatchObject({
      working_text: "save once\n",
      dirty: false,
      stale: false,
      save_state: "idle",
    });
  });

  it("rebases a queued newer-generation Save onto the preceding Save and stays saving", async () => {
    const firstSave = deferred<{
      status: "saved";
      revision: number;
      content_hash: string;
    }>();
    const secondSave = deferred<{
      status: "saved";
      revision: number;
      content_hash: string;
    }>();
    const client = fakeClient();
    vi.mocked(client.saveText)
      .mockReturnValueOnce(firstSave.promise)
      .mockReturnValueOnce(secondSave.promise);
    const controller = new FileEditorControllerRegistry(client, {
      checkpoint_debounce_ms: 60_000,
    }).forResource(owner.resource_id);
    await controller.initialize({ owner, text: "base\n", discover_recovery: false });
    controller.mutate("first save\n");
    const first = controller.save();
    controller.mutate("second save\n");
    const second = controller.save();

    firstSave.resolve({ status: "saved", revision: 5, content_hash: "hash-first" });
    await expect(first).resolves.toMatchObject({ status: "saved" });
    expect(controller.getSnapshot().save_state).toBe("saving");
    await vi.waitFor(() => expect(client.saveText).toHaveBeenNthCalledWith(
      2,
      expect.objectContaining({
        subscription_id: owner.subscription_id,
        expected_revision: 5,
        buffer_base_hash: "hash-first",
        text: "second save\n",
      }),
    ));
    secondSave.resolve({ status: "saved", revision: 6, content_hash: "hash-second" });
    await expect(second).resolves.toMatchObject({ status: "saved" });

    expect(controller.getSnapshot()).toMatchObject({
      saved_text: "second save\n",
      working_text: "second save\n",
      base_revision: 6,
      buffer_base_hash: "hash-second",
      dirty: false,
      stale: false,
      save_state: "idle",
    });
  });

  it("does not let a duplicate queued Save stale-mark a newer in-memory edit", async () => {
    const pendingSave = deferred<{
      status: "saved";
      revision: number;
      content_hash: string;
    }>();
    const client = fakeClient();
    vi.mocked(client.saveText).mockReturnValueOnce(pendingSave.promise);
    const controller = new FileEditorControllerRegistry(client, {
      checkpoint_debounce_ms: 60_000,
    }).forResource(owner.resource_id);
    await controller.initialize({ owner, text: "base\n", discover_recovery: false });
    controller.mutate("save snapshot\n");

    const first = controller.save();
    const duplicate = controller.save();
    controller.mutate("newer local edit\n");
    pendingSave.resolve({ status: "saved", revision: 5, content_hash: "hash-saved" });

    await expect(first).resolves.toMatchObject({ status: "saved" });
    await expect(duplicate).resolves.toMatchObject({ status: "unchanged" });
    expect(client.saveText).toHaveBeenCalledOnce();
    expect(controller.getSnapshot()).toMatchObject({
      saved_text: "save snapshot\n",
      working_text: "newer local edit\n",
      dirty: true,
      stale: false,
      save_state: "idle",
    });
  });

  it("does not apply an old subscription Save completion to a new owner incarnation", async () => {
    const pendingSave = deferred<{
      status: "saved";
      revision: number;
      content_hash: string;
    }>();
    const client = fakeClient();
    vi.mocked(client.saveText).mockReturnValue(pendingSave.promise);
    const controller = new FileEditorControllerRegistry(client, {
      checkpoint_debounce_ms: 60_000,
    }).forResource(owner.resource_id);
    await controller.initialize({ owner, text: "base\n", discover_recovery: false });
    controller.mutate("local edit\n");
    const saving = controller.save();
    await vi.waitFor(() => expect(client.saveText).toHaveBeenCalledOnce());

    controller.applyAuthoritative({
      ...owner,
      subscription_id: "subscription-new-owner",
      revision: 1,
    }, "base\n");
    pendingSave.resolve({ status: "saved", revision: 5, content_hash: "hash-old-owner-save" });
    await saving;

    expect(controller.getSnapshot()).toMatchObject({
      subscription_id: "subscription-new-owner",
      base_revision: 1,
      buffer_base_hash: "hash-base",
      saved_text: "base\n",
      working_text: "local edit\n",
      dirty: true,
      stale: false,
      save_state: "idle",
    });
  });

  it("does not let a late authorization failure revoke a restored owner", async () => {
    const pendingSave = deferred<{
      status: "saved";
      revision: number;
      content_hash: string;
    }>();
    const client = fakeClient();
    vi.mocked(client.saveText).mockReturnValue(pendingSave.promise);
    const controller = new FileEditorControllerRegistry(client, {
      checkpoint_debounce_ms: 60_000,
    }).forResource(owner.resource_id);
    await controller.initialize({ owner, text: "base\n", discover_recovery: false });
    controller.mutate("local edit\n");
    const saving = controller.save();
    await vi.waitFor(() => expect(client.saveText).toHaveBeenCalledOnce());

    controller.applyAuthoritative({
      ...owner,
      subscription_id: "subscription-restored",
      revision: 1,
    }, "base\n");
    pendingSave.reject({
      code: "unauthorized_path",
      message: "old subscription was revoked",
    });
    await expect(saving).rejects.toMatchObject({ code: "unauthorized_path" });

    expect(controller.getSnapshot()).toMatchObject({
      subscription_id: "subscription-restored",
      working_text: "local edit\n",
      dirty: true,
      authorization: { status: "available" },
    });
  });

  it("rebases recovery after cleanup failure and prevents old bytes from resurrecting", async () => {
    let persistedRecoveries = [recoverySummary()];
    let discardAttempts = 0;
    const client = fakeClient();
    vi.mocked(client.listRecoveries).mockImplementation(async () => [...persistedRecoveries]);
    vi.mocked(client.discardRecovery).mockImplementation(async (request) => {
      discardAttempts += 1;
      if (discardAttempts === 1) throw new Error("cleanup unavailable");
      persistedRecoveries = persistedRecoveries.filter((candidate) => (
        candidate.recovery_id !== request.recovery_id
      ));
    });
    vi.mocked(client.checkpointRecovery).mockImplementation(async (request) => {
      const committed = checkpoint(3, {
        recovery_id: request.recovery_id ?? "recovery-fresh",
        base_content_hash: request.base_content_hash,
      });
      persistedRecoveries = [{
        ...recoverySummary(),
        recovery_id: committed.recovery_id,
        recovery_revision: committed.recovery_revision,
        base_content_hash: committed.base_content_hash,
      }];
      return committed;
    });
    vi.mocked(client.saveText)
      .mockResolvedValueOnce({ status: "saved", revision: 5, content_hash: "hash-saved" })
      .mockResolvedValueOnce({ status: "saved", revision: 6, content_hash: "hash-second" });
    const registry = new FileEditorControllerRegistry(client);
    const controller = registry.forResource(owner.resource_id);
    await controller.initialize({ owner, text: "base\n", discover_recovery: true });

    await expect(controller.save()).rejects.toThrow("cleanup unavailable");
    expect(controller.getSnapshot()).toMatchObject({
      saved_text: "recovered edit\n",
      working_text: "recovered edit\n",
      dirty: false,
      save_state: "error",
      recovery: {
        status: "error",
        recovery_id: "recovery-1",
        recovery_revision: 2,
        retryable: true,
      },
    });
    expect(controller.canReleaseAfterPostcommit(0)).toBe(false);

    controller.mutate("new edit after cleanup failure\n");
    await controller.flushRecovery();
    expect(client.checkpointRecovery).toHaveBeenCalledWith(expect.objectContaining({
      recovery_id: "recovery-1",
      expected_recovery_revision: 2,
      base_content_hash: "hash-saved",
      base: "recovered edit\n",
      buffer: "new edit after cleanup failure\n",
    }));
    expect(controller.getSnapshot().recovery).toMatchObject({
      status: "durable",
      recovery_id: "recovery-1",
      recovery_revision: 3,
    });

    await controller.save();
    expect(discardAttempts).toBe(2);
    expect(persistedRecoveries).toEqual([]);
    expect(controller.getSnapshot()).toMatchObject({
      saved_text: "new edit after cleanup failure\n",
      working_text: "new edit after cleanup failure\n",
      buffer_base_hash: "hash-second",
      dirty: false,
      recovery: { status: "none" },
    });
    expect(registry.releaseAfterPostcommit(owner.resource_id, 0)).toBe(true);

    const restartedOwner = {
      ...owner,
      revision: 6,
      descriptor: { ...descriptor, content_hash: "hash-second" },
    };
    const restarted = new FileEditorControllerRegistry(client).forResource(owner.resource_id);
    await restarted.initialize({
      owner: restartedOwner,
      text: "new edit after cleanup failure\n",
      discover_recovery: true,
    });
    expect(restarted.getSnapshot()).toMatchObject({
      saved_text: "new edit after cleanup failure\n",
      working_text: "new edit after cleanup failure\n",
      buffer_base_hash: "hash-second",
      dirty: false,
      stale: false,
      recovery: { status: "none" },
    });
  });

  it("retries clean recovery cleanup without requiring another edit", async () => {
    let persistedRecoveries = [recoverySummary()];
    let discardAttempts = 0;
    const client = fakeClient();
    vi.mocked(client.listRecoveries).mockImplementation(async () => [...persistedRecoveries]);
    vi.mocked(client.discardRecovery).mockImplementation(async (request) => {
      discardAttempts += 1;
      if (discardAttempts === 1) throw new Error("cleanup unavailable");
      persistedRecoveries = persistedRecoveries.filter((candidate) => (
        candidate.recovery_id !== request.recovery_id
      ));
    });
    const controller = new FileEditorControllerRegistry(client).forResource(owner.resource_id);
    await controller.initialize({ owner, text: "base\n", discover_recovery: true });

    await expect(controller.save()).rejects.toThrow("cleanup unavailable");
    expect(controller.getSnapshot()).toMatchObject({
      dirty: false,
      recovery: { status: "error", recovery_id: "recovery-1" },
    });
    expect(controller.canReleaseAfterPostcommit(0)).toBe(false);

    await expect(controller.save()).resolves.toEqual({
      status: "unchanged",
      revision: 5,
      content_hash: "hash-saved",
    });
    expect(client.saveText).toHaveBeenCalledOnce();
    expect(discardAttempts).toBe(2);
    expect(controller.getSnapshot()).toMatchObject({
      dirty: false,
      save_state: "idle",
      recovery: { status: "none" },
      last_error: null,
    });
    expect(controller.canReleaseAfterPostcommit(0)).toBe(true);
  });

  it("creates a fresh recovery after Save deleted the old record but verification failed", async () => {
    let persistedRecoveries = [recoverySummary()];
    let failVerification = false;
    const client = fakeClient();
    vi.mocked(client.listRecoveries).mockImplementation(async () => {
      if (failVerification) {
        failVerification = false;
        throw new Error("verification unavailable");
      }
      return [...persistedRecoveries];
    });
    vi.mocked(client.getRecovery).mockImplementation(async (request) => {
      const summary = persistedRecoveries.find((candidate) => (
        candidate.recovery_id === request.recovery_id
      ));
      if (!summary) throw new Error("not found");
      return recovery({
        ...summary,
        base: summary.base_content_hash === "hash-saved" ? "recovered edit\n" : "base\n",
        buffer: summary.recovery_id === "recovery-fresh"
          ? "new edit after ambiguous cleanup\n"
          : "recovered edit\n",
      });
    });
    vi.mocked(client.saveText).mockImplementation(async () => {
      persistedRecoveries = [];
      failVerification = true;
      return { status: "saved", revision: 5, content_hash: "hash-saved" };
    });
    vi.mocked(client.checkpointRecovery).mockImplementation(async (request) => {
      const committed = checkpoint(1, {
        recovery_id: request.recovery_id ?? "recovery-fresh",
        base_content_hash: request.base_content_hash,
      });
      persistedRecoveries = [{
        ...recoverySummary(),
        recovery_id: committed.recovery_id,
        recovery_revision: committed.recovery_revision,
        base_content_hash: committed.base_content_hash,
      }];
      return committed;
    });
    const controller = new FileEditorControllerRegistry(client, {
      checkpoint_debounce_ms: 60_000,
    }).forResource(owner.resource_id);
    await controller.initialize({ owner, text: "base\n", discover_recovery: true });

    await expect(controller.save()).rejects.toThrow("verification unavailable");
    controller.mutate("new edit after ambiguous cleanup\n");
    await controller.retryRecovery();

    expect(client.checkpointRecovery).toHaveBeenCalledWith(expect.objectContaining({
      recovery_id: null,
      expected_recovery_revision: null,
      base_content_hash: "hash-saved",
      base: "recovered edit\n",
      buffer: "new edit after ambiguous cleanup\n",
    }));
    expect(controller.getSnapshot().recovery).toMatchObject({
      status: "durable",
      recovery_id: "recovery-fresh",
    });

    const restartedOwner = {
      ...owner,
      revision: 5,
      descriptor: { ...descriptor, content_hash: "hash-saved" },
    };
    const restarted = new FileEditorControllerRegistry(client).forResource(owner.resource_id);
    await restarted.initialize({
      owner: restartedOwner,
      text: "recovered edit\n",
      discover_recovery: true,
    });
    expect(restarted.getSnapshot()).toMatchObject({
      saved_text: "recovered edit\n",
      working_text: "new edit after ambiguous cleanup\n",
      dirty: true,
      recovery: { status: "durable", recovery_id: "recovery-fresh" },
    });
  });

  it("preserves an ambiguous recovery and refuses to checkpoint over an advanced revision", async () => {
    let verificationFailed = false;
    const client = fakeClient();
    vi.mocked(client.listRecoveries).mockImplementation(async () => {
      if (verificationFailed) {
        verificationFailed = false;
        throw new Error("verification unavailable");
      }
      return [recoverySummary({ recovery_revision: 3 })];
    });
    vi.mocked(client.saveText).mockImplementation(async () => {
      verificationFailed = true;
      return { status: "saved", revision: 5, content_hash: "hash-saved" };
    });
    const controller = new FileEditorControllerRegistry(client, {
      checkpoint_debounce_ms: 60_000,
    }).forResource(owner.resource_id);
    vi.mocked(client.listRecoveries).mockResolvedValueOnce([recoverySummary()]);
    await controller.initialize({ owner, text: "base\n", discover_recovery: true });

    await expect(controller.save()).rejects.toThrow("verification unavailable");
    controller.mutate("edit after advanced recovery\n");
    await expect(controller.retryRecovery()).rejects.toThrow(
      "Recovery advanced while its previous outcome was being reconciled.",
    );

    expect(client.checkpointRecovery).not.toHaveBeenCalled();
    expect(controller.getSnapshot()).toMatchObject({
      working_text: "edit after advanced recovery\n",
      dirty: true,
      recovery: {
        status: "error",
        recovery_id: "recovery-1",
        recovery_revision: 2,
        retryable: true,
      },
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
      authorization: { status: "available" },
      recovery: {
        status: "error",
        buffer_generation: generation,
        retryable: true,
      },
    });
  });

  it("becomes read-only on revoked Save while advancing an existing recovery CAS", async () => {
    const client = fakeClient();
    vi.mocked(client.saveText).mockRejectedValue({
      code: "unauthorized_path",
      message: "the agent root no longer authorizes this file",
    });
    vi.mocked(client.checkpointRecovery).mockResolvedValue(checkpoint(3));
    const controller = new FileEditorControllerRegistry(client, {
      checkpoint_debounce_ms: 60_000,
    }).forResource(owner.resource_id);
    await controller.initialize({ owner, text: "base\n", discover_recovery: true });
    controller.mutate("latest revoked edit\n");

    await expect(controller.save()).rejects.toMatchObject({ code: "unauthorized_path" });
    await vi.waitFor(() => expect(client.checkpointRecovery).toHaveBeenCalledWith(
      expect.objectContaining({
        recovery_id: "recovery-1",
        expected_recovery_revision: 2,
        buffer: "latest revoked edit\n",
      }),
    ));
    await vi.waitFor(() => expect(controller.getSnapshot().recovery).toMatchObject({
      status: "durable",
      recovery_id: "recovery-1",
      recovery_revision: 3,
      buffer_generation: 2,
    }));
    expect(controller.getSnapshot()).toMatchObject({
      working_text: "latest revoked edit\n",
      dirty: true,
      authorization: {
        status: "unavailable",
        code: "unauthorized_path",
      },
    });

    const generation = controller.getSnapshot().buffer_generation;
    expect(controller.mutate("must be rejected while read-only\n")).toBe(generation);
    expect(controller.getSnapshot().working_text).toBe("latest revoked edit\n");

    controller.applyAuthoritative({
      ...owner,
      subscription_id: "subscription-restored",
      revision: 5,
    }, "base\n");
    expect(controller.getSnapshot()).toMatchObject({
      working_text: "latest revoked edit\n",
      dirty: true,
      authorization: { status: "available" },
    });
  });

  it("does not invent first-recovery authority after a revoked checkpoint", async () => {
    const client = fakeClient();
    vi.mocked(client.checkpointRecovery).mockRejectedValue({
      code: "unauthorized_resource",
      message: "the live subscription is no longer authorized",
    });
    const controller = new FileEditorControllerRegistry(client, {
      checkpoint_debounce_ms: 60_000,
    }).forResource(owner.resource_id);
    await controller.initialize({ owner, text: "base\n", discover_recovery: false });
    controller.mutate("not yet recoverable\n");

    await expect(controller.flushRecovery()).rejects.toMatchObject({
      code: "unauthorized_resource",
    });
    expect(client.checkpointRecovery).toHaveBeenCalledWith(expect.objectContaining({
      recovery_id: null,
      expected_recovery_revision: null,
    }));
    expect(controller.getSnapshot()).toMatchObject({
      working_text: "not yet recoverable\n",
      dirty: true,
      authorization: {
        status: "unavailable",
        code: "unauthorized_resource",
      },
      recovery: { status: "error", recovery_id: null },
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

  it("keeps a stale working buffer as a pure no-op", async () => {
    const controller = new FileEditorControllerRegistry(fakeClient())
      .forResource(owner.resource_id);
    await controller.initialize({ owner, text: "base\n", discover_recovery: false });
    controller.mutate("local\n");
    controller.applyAuthoritative({
      ...owner,
      revision: 6,
      descriptor: { ...descriptor, content_hash: "hash-disk" },
    }, "disk\n");
    const before = controller.getSnapshot();

    controller.keepWorkingBuffer();

    expect(controller.getSnapshot()).toBe(before);
  });

  it("shares one diff result per saved-base and buffer generation", async () => {
    const controller = new FileEditorControllerRegistry(fakeClient())
      .forResource(owner.resource_id);
    await controller.initialize({ owner, text: "base\n", discover_recovery: false });
    const clean = fileDiffForController(controller);
    expect(fileDiffForController(controller)).toBe(clean);

    controller.mutate("local\n");
    const dirty = fileDiffForController(controller);
    expect(dirty).not.toBe(clean);
    expect(fileDiffForController(controller)).toBe(dirty);
  });

  it("reloads only an exact authorized disk head and retires recovery before adopting it", async () => {
    const client = fakeClient();
    const controller = new FileEditorControllerRegistry(client, { checkpoint_debounce_ms: 60_000 })
      .forResource(owner.resource_id);
    await controller.initialize({ owner, text: "base\n", discover_recovery: false });
    controller.mutate("local\n");
    await controller.flushRecovery();
    controller.applyAuthoritative({
      ...owner,
      revision: 6,
      descriptor: { ...descriptor, content_hash: "hash-disk" },
    }, "disk\n");

    await controller.reloadFromDisk();

    expect(client.readText).toHaveBeenCalledWith({
      resource_id: owner.resource_id,
      subscription_id: owner.subscription_id,
      revision: 6,
    });
    expect(client.discardRecovery).toHaveBeenCalledWith({
      recovery_id: "recovery-1",
      expected_recovery_revision: 1,
      resource_key: owner.resource_id,
    });
    expect(client.saveText).not.toHaveBeenCalled();
    expect(controller.getSnapshot()).toMatchObject({
      base_revision: 6,
      buffer_base_hash: "hash-disk",
      saved_text: "disk\n",
      working_text: "disk\n",
      dirty: false,
      stale: false,
      recovery: { status: "none" },
    });
  });

  it("fails a disk reload closed when the working generation changes during the exact read", async () => {
    const pendingRead = deferred<{
      schema: 1;
      resource_id: string;
      revision: number;
      text: string;
    }>();
    const client = fakeClient();
    vi.mocked(client.readText).mockReturnValue(pendingRead.promise);
    const controller = new FileEditorControllerRegistry(client, { checkpoint_debounce_ms: 60_000 })
      .forResource(owner.resource_id);
    await controller.initialize({ owner, text: "base\n", discover_recovery: false });
    controller.mutate("local\n");
    controller.applyAuthoritative({
      ...owner,
      revision: 6,
      descriptor: { ...descriptor, content_hash: "hash-disk" },
    }, "disk\n");
    const reloading = controller.reloadFromDisk();
    controller.mutate("newer local\n");
    pendingRead.resolve({
      schema: 1,
      resource_id: owner.resource_id,
      revision: 6,
      text: "disk\n",
    });

    await expect(reloading).rejects.toThrow(/editor or disk head changed/i);
    expect(client.discardRecovery).not.toHaveBeenCalled();
    expect(controller.getSnapshot()).toMatchObject({
      working_text: "newer local\n",
      dirty: true,
      stale: true,
    });
  });

  it("rebases a clean three-way merge locally and checkpoints it without writing", async () => {
    const client = fakeClient();
    const controller = new FileEditorControllerRegistry(client, { checkpoint_debounce_ms: 60_000 })
      .forResource(owner.resource_id);
    await controller.initialize({ owner, text: "base\n", discover_recovery: false });
    controller.mutate("local\n");
    controller.applyAuthoritative({
      ...owner,
      revision: 6,
      descriptor: { ...descriptor, content_hash: "hash-disk" },
    }, "disk\n");

    await expect(controller.mergeStaleBuffer()).resolves.toMatchObject({ status: "clean" });

    expect(client.mergeRecovery).toHaveBeenCalledWith({
      recovery_id: "recovery-1",
      expected_recovery_revision: 1,
      resource_key: owner.resource_id,
      resource_id: owner.resource_id,
      subscription_id: owner.subscription_id,
    });
    expect(client.saveText).not.toHaveBeenCalled();
    expect(controller.getSnapshot()).toMatchObject({
      base_revision: 6,
      buffer_base_hash: "hash-disk",
      saved_text: "disk\n",
      working_text: "merged local edit\n",
      dirty: true,
      stale: false,
    });
    expect(client.checkpointRecovery).toHaveBeenLastCalledWith(expect.objectContaining({
      base_content_hash: "hash-disk",
      base: "disk\n",
      buffer: "merged local edit\n",
    }));
  });

  it("preserves the exact buffer and base when a merge conflicts", async () => {
    const client = fakeClient();
    vi.mocked(client.mergeRecovery).mockResolvedValue({
      status: "conflicted",
      recovery_revision: 1,
      current_revision: 6,
      current_content_hash: "hash-disk",
      disk_changed: true,
      merged_text: "<<<<<<< ours\nlocal\n=======\ndisk\n>>>>>>> theirs\n",
    });
    const controller = new FileEditorControllerRegistry(client, { checkpoint_debounce_ms: 60_000 })
      .forResource(owner.resource_id);
    await controller.initialize({ owner, text: "base\n", discover_recovery: false });
    controller.mutate("local\n");
    controller.applyAuthoritative({
      ...owner,
      revision: 6,
      descriptor: { ...descriptor, content_hash: "hash-disk" },
    }, "disk\n");
    const before = controller.getSnapshot();

    await expect(controller.mergeStaleBuffer()).resolves.toMatchObject({ status: "conflicted" });

    expect(controller.getSnapshot()).toMatchObject({
      saved_text: before.saved_text,
      working_text: before.working_text,
      buffer_base_hash: before.buffer_base_hash,
      dirty: true,
      stale: true,
    });
    expect(controller.getSnapshot().last_error).toMatch(/overlapping changes/i);
    expect(client.saveText).not.toHaveBeenCalled();
  });

  it("rejects a clean merge result if the working generation changes in flight", async () => {
    const pendingMerge = deferred<{
      status: "clean";
      recovery_revision: number;
      current_revision: number;
      current_content_hash: string;
      disk_changed: boolean;
      merged_text: string;
    }>();
    const client = fakeClient();
    vi.mocked(client.mergeRecovery).mockReturnValue(pendingMerge.promise);
    const controller = new FileEditorControllerRegistry(client, { checkpoint_debounce_ms: 60_000 })
      .forResource(owner.resource_id);
    await controller.initialize({ owner, text: "base\n", discover_recovery: false });
    controller.mutate("local\n");
    controller.applyAuthoritative({
      ...owner,
      revision: 6,
      descriptor: { ...descriptor, content_hash: "hash-disk" },
    }, "disk\n");
    const merging = controller.mergeStaleBuffer();
    await vi.waitFor(() => expect(client.mergeRecovery).toHaveBeenCalledOnce());
    controller.mutate("newer local\n");
    pendingMerge.resolve({
      status: "clean",
      recovery_revision: 1,
      current_revision: 6,
      current_content_hash: "hash-disk",
      disk_changed: true,
      merged_text: "merged local edit\n",
    });

    await expect(merging).rejects.toThrow(/editor or disk head changed/i);
    expect(controller.getSnapshot()).toMatchObject({
      saved_text: "base\n",
      working_text: "newer local\n",
      dirty: true,
      stale: true,
    });
    expect(client.saveText).not.toHaveBeenCalled();
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
        base: "base\n",
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
    expect(controller.canReleaseAfterPostcommit(0)).toBe(false);
    pendingDiscard.resolve();
    await vi.waitFor(() => expect(controller.getSnapshot().recovery.status).toBe("none"));

    expect(controller.canReleaseAfterPostcommit(0)).toBe(true);
    expect(registry.releaseAfterPostcommit(owner.resource_id, 0)).toBe(true);
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
      request.base_content_hash === "hash-base" ? firstCheckpoint.promise : freshCheckpoint.promise
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
      base_content_hash: "hash-saved",
      base: "save snapshot\n",
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

  it("keeps one canonical controller while an exact authoritative read or recovery discovery is pending", async () => {
    const pendingRead = deferred<{
      schema: 1;
      resource_id: string;
      revision: number;
      text: string;
    }>();
    const pendingDiscovery = deferred<FileRecoverySummaryV1[]>();
    const client = fakeClient();
    vi.mocked(client.listRecoveries).mockReturnValue(pendingDiscovery.promise);
    const registry = new FileEditorControllerRegistry(client);
    const controller = registry.forResource(owner.resource_id);
    const synchronizing = registry.synchronizeAuthoritative(owner, () => pendingRead.promise);

    expect(registry.releaseAfterPostcommit(owner.resource_id, 0)).toBe(false);
    expect(registry.forResource(owner.resource_id)).toBe(controller);
    pendingRead.resolve({
      schema: 1,
      resource_id: owner.resource_id,
      revision: owner.revision,
      text: "base\n",
    });
    await vi.waitFor(() => expect(controller.getSnapshot().status).toBe("ready"));
    expect(registry.releaseAfterPostcommit(owner.resource_id, 0)).toBe(false);
    expect(registry.forResource(owner.resource_id)).toBe(controller);

    pendingDiscovery.resolve([]);
    await synchronizing;
    await vi.waitFor(() => expect(registry.getExisting(owner.resource_id)).toBeUndefined());
  });
});
