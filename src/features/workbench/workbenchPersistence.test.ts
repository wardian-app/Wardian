import { describe, expect, it, vi } from "vitest";

import { createWorkbenchStore } from "./useWorkbenchStore";
import { makeSingleGroupDocument, makeSurface } from "./workbenchTestUtils";
import {
  createWorkbenchInvokeAdapter,
  createWorkbenchSaveQueue,
  readLegacyWorkbenchMigration,
  decideWorkbenchSaveResponse,
  type WorkbenchPersistenceAdapter,
  type WorkbenchResetResult,
  type WorkbenchSaveResult,
} from "./workbenchPersistence";

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, resolve, reject };
}

function adapterWithSave(
  save: WorkbenchPersistenceAdapter["save"],
): WorkbenchPersistenceAdapter {
  return {
    boot: vi.fn().mockResolvedValue({ safe_mode: false }),
    load: vi.fn(),
    save,
    reset: vi.fn(),
  };
}

describe("createWorkbenchInvokeAdapter", () => {
  it("loads the backend document and keeps its opaque durable token", async () => {
    const invoke = vi.fn().mockResolvedValue({
      source: "default",
      document: makeSingleGroupDocument(),
      notice: null,
      durable_revision: 0,
      durable_token: "opaque-token-from-rust",
    });

    const result = await createWorkbenchInvokeAdapter(invoke).load();

    expect(invoke).toHaveBeenCalledWith("load_workbench_state");
    expect(result.durable_token).toBe("opaque-token-from-rust");
  });

  it("passes exact snake_case save/reset payloads and reads boot safe mode separately", async () => {
    const invoke = vi.fn()
      .mockResolvedValueOnce({ safe_mode: true })
      .mockResolvedValueOnce({
        outcome: "saved",
        durable_revision: 1,
        durable_token: "opaque-next",
        request_id: "request-1",
      })
      .mockResolvedValueOnce({
        outcome: "saved",
        durable_revision: 2,
        durable_token: "opaque-reset",
        request_id: "request-2",
        document: { ...makeSingleGroupDocument(), revision: 2 },
      });
    const adapter = createWorkbenchInvokeAdapter(invoke);
    const document = { ...makeSingleGroupDocument(), revision: 1 };

    await expect(adapter.boot()).resolves.toEqual({ safe_mode: true });
    await adapter.save({
      document,
      expected_revision: 0,
      expected_token: "opaque-zero",
      request_id: "request-1",
    });
    await adapter.reset({
      expected_revision: 1,
      expected_token: "opaque-next",
      request_id: "request-2",
    });

    expect(invoke).toHaveBeenNthCalledWith(1, "get_workbench_boot_config");
    expect(invoke).toHaveBeenNthCalledWith(2, "save_workbench_state", {
      document,
      expected_revision: 0,
      expected_token: "opaque-zero",
      request_id: "request-1",
    });
    expect(invoke).toHaveBeenNthCalledWith(3, "reset_workbench_state", {
      expected_revision: 1,
      expected_token: "opaque-next",
      request_id: "request-2",
    });
  });

  it.each([
    ["non-object", null],
    ["missing field", {}],
    ["extra field", { safe_mode: false, extra: true }],
    ["wrong type", { safe_mode: "yes" }],
    ["non-plain object", Object.assign(new Date(0), { safe_mode: false })],
  ])("rejects a malformed boot response: %s", async (_label, response) => {
    const adapter = createWorkbenchInvokeAdapter(vi.fn().mockResolvedValue(response));

    await expect(adapter.boot()).rejects.toThrow(/boot.*response/i);
  });

  it.each([
    ["missing field", {
      source: "primary",
      document: makeSingleGroupDocument(),
      notice: null,
      durable_revision: 0,
    }],
    ["extra field", {
      source: "primary",
      document: makeSingleGroupDocument(),
      notice: null,
      durable_revision: 0,
      durable_token: "opaque-zero",
      extra: true,
    }],
    ["unknown source", {
      source: "mystery",
      document: makeSingleGroupDocument(),
      notice: null,
      durable_revision: 0,
      durable_token: "opaque-zero",
    }],
    ["invalid document", {
      source: "primary",
      document: { ...makeSingleGroupDocument(), revision: -1 },
      notice: null,
      durable_revision: 0,
      durable_token: "opaque-zero",
    }],
    ["normal source without document", {
      source: "primary",
      document: null,
      notice: null,
      durable_revision: 0,
      durable_token: "opaque-zero",
    }],
    ["wrong notice type", {
      source: "primary",
      document: makeSingleGroupDocument(),
      notice: false,
      durable_revision: 0,
      durable_token: "opaque-zero",
    }],
    ["unsafe revision", {
      source: "primary",
      document: makeSingleGroupDocument(),
      notice: null,
      durable_revision: Number.MAX_SAFE_INTEGER + 1,
      durable_token: "opaque-zero",
    }],
    ["empty token", {
      source: "primary",
      document: makeSingleGroupDocument(),
      notice: null,
      durable_revision: 0,
      durable_token: "",
    }],
    ["mismatched revision", {
      source: "primary",
      document: makeSingleGroupDocument(),
      notice: null,
      durable_revision: 1,
      durable_token: "opaque-one",
    }],
    ["future schema with V1 identity", {
      source: "future_schema",
      document: makeSingleGroupDocument(),
      notice: "Upgrade Wardian.",
      durable_revision: 0,
      durable_token: "opaque-zero",
    }],
  ])("rejects a malformed load response: %s", async (_label, response) => {
    const adapter = createWorkbenchInvokeAdapter(vi.fn().mockResolvedValue(response));

    await expect(adapter.load()).rejects.toThrow(/load.*response/i);
  });

  it.each([
    ["missing field", {
      outcome: "saved",
      durable_revision: 1,
      request_id: "request-1",
    }],
    ["extra field", {
      outcome: "saved",
      durable_revision: 1,
      durable_token: "opaque-one",
      request_id: "request-1",
      extra: true,
    }],
    ["unknown outcome", {
      outcome: "mystery_outcome",
      durable_revision: 1,
      durable_token: "opaque-one",
      request_id: "request-1",
    }],
    ["wrong revision type", {
      outcome: "saved",
      durable_revision: "1",
      durable_token: "opaque-one",
      request_id: "request-1",
    }],
    ["empty token", {
      outcome: "saved",
      durable_revision: 1,
      durable_token: "",
      request_id: "request-1",
    }],
    ["wrong request id", {
      outcome: "saved",
      durable_revision: 1,
      durable_token: "opaque-one",
      request_id: "other-request",
    }],
    ["wrong saved revision", {
      outcome: "saved",
      durable_revision: 2,
      durable_token: "opaque-two",
      request_id: "request-1",
    }],
    ["future schema with V1 identity", {
      outcome: "future_schema",
      durable_revision: 1,
      durable_token: "opaque-one",
      request_id: "request-1",
    }],
    ["conflict without identity", {
      outcome: "revision_conflict",
      durable_revision: null,
      durable_token: null,
      request_id: "request-1",
    }],
  ])("rejects a malformed save response: %s", async (_label, response) => {
    const adapter = createWorkbenchInvokeAdapter(vi.fn().mockResolvedValue(response));

    await expect(adapter.save({
      document: { ...makeSingleGroupDocument(), revision: 1 },
      expected_revision: 0,
      expected_token: "opaque-zero",
      request_id: "request-1",
    })).rejects.toThrow(/save.*response/i);
  });

  it.each([
    ["missing field", {
      outcome: "saved",
      durable_revision: 1,
      request_id: "reset-1",
      document: { ...makeSingleGroupDocument(), revision: 1 },
    }],
    ["extra field", {
      outcome: "saved",
      durable_revision: 1,
      durable_token: "opaque-reset",
      request_id: "reset-1",
      document: { ...makeSingleGroupDocument(), revision: 1 },
      extra: true,
    }],
    ["unknown outcome", {
      outcome: "mystery_outcome",
      durable_revision: 1,
      durable_token: "opaque-reset",
      request_id: "reset-1",
    }],
    ["wrong request id", {
      outcome: "saved",
      durable_revision: 1,
      durable_token: "opaque-reset",
      request_id: "other-reset",
      document: { ...makeSingleGroupDocument(), revision: 1 },
    }],
    ["saved without document", {
      outcome: "saved",
      durable_revision: 1,
      durable_token: "opaque-reset",
      request_id: "reset-1",
    }],
    ["saved with null document", {
      outcome: "saved",
      durable_revision: 1,
      durable_token: "opaque-reset",
      request_id: "reset-1",
      document: null,
    }],
    ["saved with invalid document", {
      outcome: "saved",
      durable_revision: 1,
      durable_token: "opaque-reset",
      request_id: "reset-1",
      document: { ...makeSingleGroupDocument(), revision: -1 },
    }],
    ["saved with wrong successor revision", {
      outcome: "saved",
      durable_revision: 2,
      durable_token: "opaque-reset",
      request_id: "reset-1",
      document: { ...makeSingleGroupDocument(), revision: 2 },
    }],
    ["conflict with document", {
      outcome: "revision_conflict",
      durable_revision: 1,
      durable_token: "opaque-one",
      request_id: "reset-1",
      document: { ...makeSingleGroupDocument(), revision: 1 },
    }],
    ["future schema with V1 identity", {
      outcome: "future_schema",
      durable_revision: 1,
      durable_token: "opaque-one",
      request_id: "reset-1",
    }],
  ])("rejects a malformed reset response: %s", async (_label, response) => {
    const adapter = createWorkbenchInvokeAdapter(vi.fn().mockResolvedValue(response));

    await expect(adapter.reset({
      expected_revision: 0,
      expected_token: "opaque-zero",
      request_id: "reset-1",
    })).rejects.toThrow(/reset.*response/i);
  });
});

describe("decideWorkbenchSaveResponse", () => {
  it("acknowledges only the matching request and proposed revision", () => {
    const pending = { request_id: "request-1", revision: 1 };
    const saved = {
      outcome: "saved" as const,
      durable_revision: 1,
      durable_token: "rust-token",
      request_id: "request-1",
    };

    expect(decideWorkbenchSaveResponse(pending, saved)).toEqual({
      type: "acknowledge",
      request_id: "request-1",
      durable_revision: 1,
      durable_token: "rust-token",
    });
    expect(decideWorkbenchSaveResponse(pending, {
      ...saved,
      request_id: "stale-request",
    })).toEqual({ type: "ignore" });
    expect(decideWorkbenchSaveResponse(pending, {
      ...saved,
      durable_revision: 2,
    })).toEqual({ type: "ignore" });
  });
});

describe("createWorkbenchSaveQueue", () => {
  it("sends revision zero as an exact R to R+1 save within 250 ms", async () => {
    vi.useFakeTimers();
    try {
      const store = createWorkbenchStore({
        initial_document: makeSingleGroupDocument(),
        durable_token: "opaque-zero",
      });
      const save = vi.fn().mockResolvedValue({
        outcome: "saved",
        durable_revision: 1,
        durable_token: "opaque-one",
        request_id: "request-1",
      } satisfies WorkbenchSaveResult);
      createWorkbenchSaveQueue({
        store,
        adapter: adapterWithSave(save),
        request_id: () => "request-1",
      });

      store.getState().apply_commands([
        { type: "open_surface", surface: makeSurface("surface-1") },
      ]);
      await vi.advanceTimersByTimeAsync(249);
      expect(save).not.toHaveBeenCalled();
      await vi.advanceTimersByTimeAsync(1);

      expect(save).toHaveBeenCalledWith({
        document: expect.objectContaining({ revision: 1 }),
        expected_revision: 0,
        expected_token: "opaque-zero",
        request_id: "request-1",
      });
      expect(store.getState().durable_revision).toBe(1);
      expect(store.getState().durable_token).toBe("opaque-one");
      expect(store.getState().pending_request_id).toBeNull();
    } finally {
      vi.useRealTimers();
    }
  });

  it("coalesces edits made during a write and saves the newest draft next", async () => {
    vi.useFakeTimers();
    try {
      const first = deferred<WorkbenchSaveResult>();
      const save = vi.fn()
        .mockImplementationOnce(() => first.promise)
        .mockResolvedValueOnce({
          outcome: "saved",
          durable_revision: 2,
          durable_token: "opaque-two",
          request_id: "request-2",
        } satisfies WorkbenchSaveResult);
      const requestIds = ["request-1", "request-2"];
      const store = createWorkbenchStore({
        initial_document: makeSingleGroupDocument(),
        durable_token: "opaque-zero",
      });
      createWorkbenchSaveQueue({
        store,
        adapter: adapterWithSave(save),
        request_id: () => requestIds.shift() ?? "unexpected-request",
      });

      store.getState().apply_commands([
        { type: "open_surface", surface: makeSurface("surface-1") },
      ]);
      await vi.advanceTimersByTimeAsync(250);
      store.getState().apply_commands([
        { type: "open_surface", surface: makeSurface("surface-2") },
      ]);
      await vi.advanceTimersByTimeAsync(500);
      expect(save).toHaveBeenCalledTimes(1);

      first.resolve({
        outcome: "saved",
        durable_revision: 1,
        durable_token: "opaque-one",
        request_id: "request-1",
      });
      await vi.runAllTimersAsync();

      expect(save).toHaveBeenCalledTimes(2);
      expect(save.mock.calls[1]?.[0]).toMatchObject({
        document: {
          revision: 2,
          surfaces: {
            "surface-1": expect.any(Object),
            "surface-2": expect.any(Object),
          },
        },
        expected_revision: 1,
        expected_token: "opaque-one",
        request_id: "request-2",
      });
      expect(store.getState().durable_revision).toBe(2);
      expect(store.getState().is_dirty).toBe(false);
    } finally {
      vi.useRealTimers();
    }
  });

  it("retries a lost response with the identical pending request", async () => {
    vi.useFakeTimers();
    try {
      const save = vi.fn()
        .mockRejectedValueOnce(new Error("response lost"))
        .mockResolvedValueOnce({
          outcome: "saved",
          durable_revision: 1,
          durable_token: "opaque-one",
          request_id: "request-1",
        } satisfies WorkbenchSaveResult);
      const store = createWorkbenchStore({
        initial_document: makeSingleGroupDocument(),
        durable_token: "opaque-zero",
      });
      createWorkbenchSaveQueue({
        store,
        adapter: adapterWithSave(save),
        request_id: () => "request-1",
      });
      store.getState().apply_commands([
        { type: "open_surface", surface: makeSurface("surface-1") },
      ]);

      await vi.advanceTimersByTimeAsync(250);
      expect(store.getState().pending_request_id).toBe("request-1");
      await vi.advanceTimersByTimeAsync(250);

      expect(save).toHaveBeenCalledTimes(2);
      expect(save.mock.calls[1]?.[0]).toBe(save.mock.calls[0]?.[0]);
      expect(store.getState().durable_revision).toBe(1);
    } finally {
      vi.useRealTimers();
    }
  });

  it("ignores a wrong-request acknowledgement and retries the same request", async () => {
    vi.useFakeTimers();
    try {
      const save = vi.fn()
        .mockResolvedValueOnce({
          outcome: "saved",
          durable_revision: 1,
          durable_token: "stale-token",
          request_id: "wrong-request",
        } satisfies WorkbenchSaveResult)
        .mockResolvedValueOnce({
          outcome: "saved",
          durable_revision: 1,
          durable_token: "opaque-one",
          request_id: "request-1",
        } satisfies WorkbenchSaveResult);
      const store = createWorkbenchStore({
        initial_document: makeSingleGroupDocument(),
        durable_token: "opaque-zero",
      });
      createWorkbenchSaveQueue({
        store,
        adapter: adapterWithSave(save),
        request_id: () => "request-1",
      });
      store.getState().apply_commands([
        { type: "open_surface", surface: makeSurface("surface-1") },
      ]);

      await vi.advanceTimersByTimeAsync(500);

      expect(save).toHaveBeenCalledTimes(2);
      expect(save.mock.calls[1]?.[0]).toBe(save.mock.calls[0]?.[0]);
      expect(store.getState().durable_token).toBe("opaque-one");
    } finally {
      vi.useRealTimers();
    }
  });

  it("freezes writes on a CAS conflict while preserving the full local draft", async () => {
    vi.useFakeTimers();
    try {
      const save = vi.fn().mockResolvedValue({
        outcome: "revision_conflict",
        durable_revision: 4,
        durable_token: "disk-token",
        request_id: "request-1",
      } satisfies WorkbenchSaveResult);
      const store = createWorkbenchStore({
        initial_document: makeSingleGroupDocument(),
        durable_token: "opaque-zero",
      });
      createWorkbenchSaveQueue({
        store,
        adapter: adapterWithSave(save),
        request_id: () => "request-1",
      });
      store.getState().apply_commands([
        { type: "open_surface", surface: makeSurface("surface-1") },
      ]);

      await vi.advanceTimersByTimeAsync(250);
      expect(store.getState().conflict).toBe("revision_conflict");
      expect(store.getState().document.surfaces["surface-1"]).toBeDefined();
      expect(store.getState().is_dirty).toBe(true);

      store.getState().apply_commands([
        { type: "open_surface", surface: makeSurface("surface-2") },
      ]);
      await vi.advanceTimersByTimeAsync(1_000);
      expect(save).toHaveBeenCalledTimes(1);
      expect(store.getState().document.surfaces["surface-2"]).toBeDefined();
    } finally {
      vi.useRealTimers();
    }
  });

  it("retries a lost reset response with the identical request without stranding pending state", async () => {
    const resetDocument = { ...makeSingleGroupDocument(), revision: 1 };
    const reset = vi.fn()
      .mockRejectedValueOnce(new Error("reset response lost"))
      .mockResolvedValueOnce({
        outcome: "saved",
        durable_revision: 1,
        durable_token: "reset-token",
        request_id: "reset-request",
        document: resetDocument,
      });
    const store = createWorkbenchStore({
      initial_document: makeSingleGroupDocument([makeSurface("before-reset")]),
      durable_token: "opaque-zero",
    });
    const queue = createWorkbenchSaveQueue({
      store,
      adapter: {
        save: vi.fn(),
        reset,
      },
      request_id: () => "reset-request",
    });

    await expect(queue.reset()).resolves.toBe(true);

    expect(reset).toHaveBeenCalledTimes(2);
    expect(reset.mock.calls[1]?.[0]).toBe(reset.mock.calls[0]?.[0]);
    expect(store.getState().pending_request_id).toBeNull();
    expect(store.getState().durable_revision).toBe(1);
    expect(store.getState().durable_token).toBe("reset-token");
  });

  it("leaves the working document intact when durable reset conflicts or fails", async () => {
    for (const reset of [
      vi.fn().mockResolvedValue({
        outcome: "revision_conflict",
        durable_revision: 2,
        durable_token: "disk-token",
        request_id: "reset-request",
      } satisfies WorkbenchResetResult),
      vi.fn().mockRejectedValue(new Error("reset unavailable")),
    ]) {
      const store = createWorkbenchStore({
        initial_document: makeSingleGroupDocument([makeSurface("before-reset")]),
        durable_token: "opaque-zero",
      });
      const before = store.getState().document;
      const queue = createWorkbenchSaveQueue({
        store,
        adapter: { save: vi.fn(), reset },
        request_id: () => "reset-request",
      });

      await expect(queue.reset(store.getState().transaction_version)).resolves.toBe(false);

      expect(store.getState().document).toBe(before);
      expect(store.getState().document.surfaces["before-reset"]).toBeDefined();
      expect(store.getState().durable_revision).toBe(0);
    }
  });

  it("does not reset state that changed while an active save was draining", async () => {
    const activeSave = deferred<WorkbenchSaveResult>();
    const reset = vi.fn();
    const store = createWorkbenchStore({
      initial_document: makeSingleGroupDocument(),
      durable_token: "opaque-zero",
    });
    const queue = createWorkbenchSaveQueue({
      store,
      adapter: {
        save: vi.fn(() => activeSave.promise),
        reset,
      },
      request_id: () => "request-1",
    });
    store.getState().apply_commands([
      { type: "open_surface", surface: makeSurface("surface-a") },
    ]);
    void queue.flush();
    const expectedTransactionVersion = store.getState().transaction_version;
    const resetting = queue.reset(expectedTransactionVersion);

    store.getState().apply_commands([
      { type: "open_surface", surface: makeSurface("surface-b") },
    ]);
    activeSave.resolve({
      outcome: "saved",
      durable_revision: 1,
      durable_token: "opaque-one",
      request_id: "request-1",
    });

    await expect(resetting).resolves.toBe(false);
    expect(reset).not.toHaveBeenCalled();
    expect(store.getState().document.surfaces["surface-a"]).toBeDefined();
    expect(store.getState().document.surfaces["surface-b"]).toBeDefined();
  });

  it("drains one newest snapshot after shutdown waits for an active save", async () => {
    const first = deferred<WorkbenchSaveResult>();
    const save = vi.fn()
      .mockImplementationOnce(() => first.promise)
      .mockResolvedValueOnce({
        outcome: "saved",
        durable_revision: 2,
        durable_token: "opaque-two",
        request_id: "request-2",
      } satisfies WorkbenchSaveResult);
    const requestIds = ["request-1", "request-2"];
    const store = createWorkbenchStore({
      initial_document: makeSingleGroupDocument(),
      durable_token: "opaque-zero",
    });
    const queue = createWorkbenchSaveQueue({
      store,
      adapter: adapterWithSave(save),
      request_id: () => requestIds.shift() ?? "unexpected-request",
    });

    store.getState().apply_commands([
      { type: "open_surface", surface: makeSurface("surface-a") },
    ]);
    void queue.flush();
    expect(save).toHaveBeenCalledTimes(1);
    store.getState().apply_commands([
      { type: "open_surface", surface: makeSurface("surface-b") },
    ]);
    const shutdown = queue.shutdown();
    expect(save).toHaveBeenCalledTimes(1);

    first.resolve({
      outcome: "saved",
      durable_revision: 1,
      durable_token: "opaque-one",
      request_id: "request-1",
    });
    await shutdown;

    expect(save).toHaveBeenCalledTimes(2);
    expect(save.mock.calls[1]?.[0]).toMatchObject({
      document: {
        revision: 2,
        surfaces: {
          "surface-a": expect.any(Object),
          "surface-b": expect.any(Object),
        },
      },
      expected_revision: 1,
      expected_token: "opaque-one",
      request_id: "request-2",
    });
  });
});

describe("readLegacyWorkbenchMigration", () => {
  it("imports only allowlisted shell fields from a real Zustand payload and clamps them", () => {
    const payload = JSON.stringify({
      state: {
        layout: { column_tracks: [0.2, 0.8], row_height: 999 },
        leftSidebarWidth: 50,
        rightSidebarWidth: 900,
        userTerminalOpen: true,
        userTerminalHeight: 9_000,
        settingsOpen: true,
        gridStacked: true,
        previousColumnTracks: [0.4, 0.6],
        libraryDetailWidth: 777,
      },
      version: 0,
    });

    expect(readLegacyWorkbenchMigration(payload, {
      width: 1_000,
      height: 800,
    })).toEqual({
      shell_patch: {
        left_sidebar_width: 200,
        right_sidebar_width: 400,
        bottom_terminal_open: true,
        bottom_terminal_height: 560,
      },
      initial_surface: {
        surface_id: "initial-agents-overview",
        surface_type: "agents-overview",
        state_schema_version: 1,
        state: {
          mode: "auto",
          focused_agent_id: null,
          search_query: "",
          status_filter: [],
        },
      },
    });
  });

  it("uses shell defaults and Agents Overview for corrupt or missing legacy state", () => {
    const expected = readLegacyWorkbenchMigration(null, { width: 1_200, height: 900 });

    expect(expected.shell_patch).toEqual({
      left_sidebar_width: 240,
      right_sidebar_width: 240,
      bottom_terminal_open: false,
      bottom_terminal_height: 360,
    });
    expect(expected.initial_surface.surface_type).toBe("agents-overview");
    expect(readLegacyWorkbenchMigration("{broken", {
      width: 1_200,
      height: 900,
    })).toEqual(expected);
  });

  it("can seed the same-run old view without treating it as historical state", () => {
    expect(readLegacyWorkbenchMigration(null, {
      width: 1_200,
      height: 900,
    }, "library").initial_surface).toMatchObject({
      surface_id: "initial-library",
      surface_type: "library",
    });
  });
});
