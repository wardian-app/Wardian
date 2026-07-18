import { describe, expect, it, vi } from "vitest";

import type {
  FileContentDescriptorV1,
  FileRecoveryCheckpointV1,
  FileRecoverySummaryV1,
  FileRecoveryV1,
  FileResourceSnapshotV1,
  FilesSurfaceStateV2,
} from "../../types";
import { createCoreWorkbenchSurfaceRegistry } from "../workbench/coreSurfaceRegistry";
import { createWorkbenchNavigationService } from "../workbench/navigationService";
import type { DirtySurfacePrompt } from "../workbench/surfaces/dirtySurfaceGuards";
import { createWorkbenchStore } from "../workbench/useWorkbenchStore";
import { makeSingleGroupDocument, makeSurface } from "../workbench/workbenchTestUtils";
import {
  FileEditorControllerRegistry,
  type FileEditorResourceClient,
} from "./fileEditorController";
import { createFilesCloseAdapter } from "./filesCloseAdapter";

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
  canonical_path: "/work/report.md",
  display_name: "report.md",
  extension: "md",
  mime_type: "text/markdown",
  encoding: "utf-8",
  renderer_kind: "markdown",
  size_bytes: 5,
  line_count: 1,
  content_hash: "hash-base",
  modified_at_ms: 1,
  capabilities: { preview: true, changes: true, draft: true, stream: false },
  unavailable_reason: null,
};

const owner: FileResourceSnapshotV1 = {
  resource_id: "file:/work/report.md",
  subscription_id: "subscription-1",
  revision: 4,
  descriptor,
};

const recoverySummary: FileRecoverySummaryV1 = {
  schema: 1,
  recovery_id: "recovery-1",
  resource_key: owner.resource_id,
  display_name: "report.md",
  extension: "md",
  mime_type: "text/markdown",
  base_content_hash: "hash-base",
  base_opaque_revision: "opaque-base",
  recovery_revision: 2,
  created_at_ms: 1,
  updated_at_ms: 2,
};

const recovery: FileRecoveryV1 = {
  ...recoverySummary,
  base: "base\n",
  buffer: "recovered edit\n",
};

const filesState: FilesSurfaceStateV2 = {
  resource_kind: "file",
  transient_preview: false,
  presentation: "editor",
  comparison_open: false,
  comparison_layout_preference: "auto",
  comparison_baseline: null,
  review_drawer_open: false,
  selected_version_id: null,
  optional_checkpoint_id: null,
};

function fakeClient(): FileEditorResourceClient {
  return {
    saveText: vi.fn().mockResolvedValue({
      status: "saved",
      revision: 5,
      content_hash: "hash-saved",
    }),
    checkpointRecovery: vi.fn().mockResolvedValue({
      schema: 1,
      recovery_id: "recovery-1",
      resource_key: owner.resource_id,
      base_content_hash: "hash-base",
      base_opaque_revision: "opaque-base",
      recovery_revision: 3,
      created_at_ms: 1,
      updated_at_ms: 3,
    } satisfies FileRecoveryCheckpointV1),
    listRecoveries: vi.fn().mockResolvedValue([recoverySummary]),
    getRecovery: vi.fn().mockResolvedValue(recovery),
    discardRecovery: vi.fn().mockResolvedValue(undefined),
  };
}

function filesSurface(surfaceId: string) {
  return makeSurface(surfaceId, {
    surface_type: "files",
    resource_key: owner.resource_id,
    state_schema_version: 2,
    state: filesState,
  });
}

async function recoveredController(client = fakeClient()) {
  const sessions = new FileEditorControllerRegistry(client, {
    checkpoint_debounce_ms: 60_000,
  });
  const controller = sessions.forResource(owner.resource_id);
  await controller.initialize({ owner, text: "base\n", discover_recovery: true });
  return { client, sessions, controller };
}

describe("Files resource-owned close adapter", () => {
  it("does not create a phantom editor session while observing Workbench surfaces", () => {
    const sessions = new FileEditorControllerRegistry(fakeClient());
    const prompt = vi.fn<DirtySurfacePrompt>();
    const adapter = createFilesCloseAdapter(sessions, prompt);

    expect(adapter.observe(filesSurface("files-a"))).toBeNull();
    expect(sessions.getExisting(owner.resource_id)).toBeUndefined();
    expect(prompt).not.toHaveBeenCalled();
  });

  it("guards a dirty canonical controller while Workbench still holds a restored alias", async () => {
    const { sessions, controller } = await recoveredController();
    controller.attachPresentation("files-alias", {});
    const prompt = vi.fn<DirtySurfacePrompt>(() => "discard");
    const registry = createCoreWorkbenchSurfaceRegistry({
      files_close_adapter: createFilesCloseAdapter(sessions, prompt),
    });
    const aliasedSurface = makeSurface("files-alias", {
      surface_type: "files",
      resource_key: "file:/work/restored-alias.md",
      state_schema_version: 2,
      state: filesState,
    });
    const store = createWorkbenchStore({
      initial_document: makeSingleGroupDocument([aliasedSurface]),
    });

    await expect(createWorkbenchNavigationService({ registry, store }).close("files-alias"))
      .resolves.toBe("allow");
    expect(prompt).toHaveBeenCalledOnce();
    expect(controller.getSnapshot().dirty).toBe(false);
  });

  it("skips a nonfinal duplicate close and prompts once when the final presentation closes", async () => {
    const { client, sessions, controller } = await recoveredController();
    const firstMembership = controller.attachPresentation("files-a", {});
    const finalMembership = controller.attachPresentation("files-b", {});
    const prompt = vi.fn<DirtySurfacePrompt>(() => "discard");
    const registry = createCoreWorkbenchSurfaceRegistry({
      files_close_adapter: createFilesCloseAdapter(sessions, prompt),
    });
    const store = createWorkbenchStore({
      initial_document: makeSingleGroupDocument([
        filesSurface("files-a"),
        filesSurface("files-b"),
      ]),
    });
    const navigation = createWorkbenchNavigationService({ registry, store });

    await expect(navigation.close("files-a")).resolves.toBe("allow");
    expect(prompt).not.toHaveBeenCalled();
    expect(controller.getSnapshot().dirty).toBe(true);
    firstMembership.detach();
    expect(sessions.releaseAfterPostcommit(
      owner.resource_id,
      controller.getSnapshot().presentation_generation,
    )).toBe(false);

    await expect(navigation.close("files-b")).resolves.toBe("allow");
    expect(prompt).toHaveBeenCalledOnce();
    expect(prompt).toHaveBeenCalledWith(expect.objectContaining({
      surface_type: "files",
      discard_label: "Don't Save",
    }));
    expect(client.discardRecovery).toHaveBeenCalledOnce();
    expect(controller.getSnapshot().dirty).toBe(false);
    expect(sessions.getExisting(owner.resource_id)).toBe(controller);

    finalMembership.detach();
    expect(sessions.releaseAfterPostcommit(
      owner.resource_id,
      controller.getSnapshot().presentation_generation,
    )).toBe(true);
    expect(sessions.getExisting(owner.resource_id)).toBeUndefined();
  });

  it("collects one decision for a dirty canonical resource presented in two closing panes", async () => {
    const { client, sessions, controller } = await recoveredController();
    controller.attachPresentation("files-a", {});
    controller.attachPresentation("files-b", {});
    const prompt = vi.fn<DirtySurfacePrompt>(() => "discard");
    const registry = createCoreWorkbenchSurfaceRegistry({
      files_close_adapter: createFilesCloseAdapter(sessions, prompt),
    });
    const store = createWorkbenchStore({
      initial_document: makeSingleGroupDocument([
        filesSurface("files-a"),
        filesSurface("files-b"),
      ]),
    });
    const navigation = createWorkbenchNavigationService({ registry, store });

    await expect(navigation.close_group("group-1")).resolves.toBe("allow");

    expect(prompt).toHaveBeenCalledOnce();
    expect(client.discardRecovery).toHaveBeenCalledOnce();
  });

  it("cancels final close when Save fails or a newer edit races the saved snapshot", async () => {
    const failedClient = fakeClient();
    vi.mocked(failedClient.saveText).mockRejectedValue(new Error("save unavailable"));
    const failedSessions = new FileEditorControllerRegistry(failedClient, {
      checkpoint_debounce_ms: 60_000,
    });
    const failedController = failedSessions.forResource(owner.resource_id);
    await failedController.initialize({ owner, text: "base\n", discover_recovery: false });
    failedController.attachPresentation("files-failed", {});
    failedController.mutate("dirty\n");
    const failedRegistry = createCoreWorkbenchSurfaceRegistry({
      files_close_adapter: createFilesCloseAdapter(failedSessions, () => "save"),
    });
    const failedStore = createWorkbenchStore({
      initial_document: makeSingleGroupDocument([filesSurface("files-failed")]),
    });

    await expect(createWorkbenchNavigationService({
      registry: failedRegistry,
      store: failedStore,
    }).close("files-failed")).resolves.toBe("cancel");
    expect(failedStore.getState().document.surfaces["files-failed"]).toBeDefined();
    expect(failedController.getSnapshot().dirty).toBe(true);

    const pendingSave = deferred<{
      status: "saved";
      revision: number;
      content_hash: string;
    }>();
    const racedClient = fakeClient();
    vi.mocked(racedClient.saveText).mockReturnValue(pendingSave.promise);
    const racedSessions = new FileEditorControllerRegistry(racedClient, {
      checkpoint_debounce_ms: 60_000,
    });
    const racedController = racedSessions.forResource(owner.resource_id);
    await racedController.initialize({ owner, text: "base\n", discover_recovery: false });
    racedController.attachPresentation("files-raced", {});
    racedController.mutate("save snapshot\n");
    const racedRegistry = createCoreWorkbenchSurfaceRegistry({
      files_close_adapter: createFilesCloseAdapter(racedSessions, () => "save"),
    });
    const racedStore = createWorkbenchStore({
      initial_document: makeSingleGroupDocument([filesSurface("files-raced")]),
    });
    const closing = createWorkbenchNavigationService({
      registry: racedRegistry,
      store: racedStore,
    }).close("files-raced");
    await vi.waitFor(() => expect(racedClient.saveText).toHaveBeenCalledOnce());
    racedController.mutate("newer edit\n");
    pendingSave.resolve({ status: "saved", revision: 5, content_hash: "hash-saved" });

    await expect(closing).resolves.toBe("cancel");
    expect(racedStore.getState().document.surfaces["files-raced"]).toBeDefined();
    expect(racedController.getSnapshot()).toMatchObject({
      working_text: "newer edit\n",
      dirty: true,
    });
  });

  it("releases a durably checkpointed postcommit edit and rehydrates it on reopen", async () => {
    const { client, sessions, controller } = await recoveredController();
    const membership = controller.attachPresentation("files-a", {});
    const registry = createCoreWorkbenchSurfaceRegistry({
      files_close_adapter: createFilesCloseAdapter(sessions, () => "discard"),
    });
    const store = createWorkbenchStore({
      initial_document: makeSingleGroupDocument([filesSurface("files-a")]),
    });
    const unsubscribe = store.subscribe((state, previous) => {
      if (state.document !== previous.document) controller.mutate("postcommit raced edit\n");
    });

    await expect(createWorkbenchNavigationService({ registry, store }).close("files-a"))
      .resolves.toBe("allow");
    unsubscribe();

    expect(client.discardRecovery).not.toHaveBeenCalled();
    expect(controller.getSnapshot()).toMatchObject({
      working_text: "postcommit raced edit\n",
      dirty: true,
      recovery: { recovery_id: "recovery-1" },
    });
    membership.detach();
    expect(sessions.releaseAfterPostcommit(
      owner.resource_id,
      controller.getSnapshot().presentation_generation,
    )).toBe(false);

    await controller.flushRecovery();
    expect(client.checkpointRecovery).toHaveBeenCalledWith(expect.objectContaining({
      recovery_id: "recovery-1",
      expected_recovery_revision: 2,
      buffer: "postcommit raced edit\n",
    }));
    expect(controller.getSnapshot().recovery).toMatchObject({
      status: "durable",
      recovery_id: "recovery-1",
      recovery_revision: 3,
    });
    await vi.waitFor(() => expect(sessions.getExisting(owner.resource_id)).toBeUndefined());

    vi.mocked(client.getRecovery).mockResolvedValue({
      ...recovery,
      recovery_revision: 3,
      updated_at_ms: 3,
      buffer: "postcommit raced edit\n",
    });
    const reopened = sessions.forResource(owner.resource_id);
    await reopened.initialize({ owner, text: "base\n", discover_recovery: true });
    expect(reopened.getSnapshot()).toMatchObject({
      working_text: "postcommit raced edit\n",
      dirty: true,
      recovery: {
        status: "durable",
        recovery_id: "recovery-1",
        recovery_revision: 3,
      },
    });
  });
});
