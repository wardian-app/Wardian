import { act, renderHook, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { createWorkbenchStore } from "./useWorkbenchStore";
import { useWorkbenchPersistence } from "./useWorkbenchPersistence";
import { makeSingleGroupDocument, makeSurface } from "./workbenchTestUtils";
import { WorkbenchPersistenceAdapterError } from "./workbenchPersistence";
import type {
  WorkbenchLoadResult,
  WorkbenchPersistenceAdapter,
  WorkbenchResetResult,
  WorkbenchSaveResult,
} from "./workbenchPersistence";

function primaryLoad(): WorkbenchLoadResult {
  return {
    source: "primary",
    document: makeSingleGroupDocument(),
    notice: null,
    durable_revision: 0,
    durable_token: "opaque-zero",
  };
}

function createAdapter(
  overrides: Partial<WorkbenchPersistenceAdapter> = {},
): WorkbenchPersistenceAdapter {
  return {
    boot: vi.fn().mockResolvedValue({ safe_mode: false }),
    load: vi.fn().mockResolvedValue(primaryLoad()),
    save: vi.fn().mockResolvedValue({
      outcome: "saved",
      durable_revision: 1,
      durable_token: "opaque-one",
      request_id: "request-1",
    } satisfies WorkbenchSaveResult),
    reset: vi.fn().mockResolvedValue({
      outcome: "saved",
      durable_revision: 1,
      durable_token: "opaque-reset",
      request_id: "request-1",
      document: { ...makeSingleGroupDocument(), revision: 1 },
    } satisfies WorkbenchResetResult),
    ...overrides,
  };
}

afterEach(() => {
  vi.useRealTimers();
});

describe("useWorkbenchPersistence", () => {
  it("loads boot state and adopts the backend document into the stable store", async () => {
    const loaded = {
      ...makeSingleGroupDocument([makeSurface("disk-surface")]),
      revision: 3,
    };
    const adapter = createAdapter({
      boot: vi.fn().mockResolvedValue({ safe_mode: true }),
      load: vi.fn().mockResolvedValue({
        source: "backup",
        document: loaded,
        notice: "Recovered from backup.",
        durable_revision: 3,
        durable_token: "opaque-three",
      }),
    });
    const store = createWorkbenchStore({ loading: true });

    const { result } = renderHook(() => useWorkbenchPersistence({
      enabled: true,
      adapter,
      store,
      request_id: () => "request-1",
    }));

    await waitFor(() => expect(result.current.status).toBe("ready"));
    expect(result.current.store).toBe(store);
    expect(result.current.safe_mode).toBe(true);
    expect(result.current.notice).toBe("Recovered from backup.");
    expect(store.getState().document).toEqual(loaded);
    expect(store.getState().durable_revision).toBe(3);
    expect(store.getState().durable_token).toBe("opaque-three");
  });

  it("queues the first exact pending snapshot within 250 ms", async () => {
    const save = vi.fn().mockResolvedValue({
      outcome: "saved",
      durable_revision: 1,
      durable_token: "opaque-one",
      request_id: "request-1",
    } satisfies WorkbenchSaveResult);
    const adapter = createAdapter({ save });
    const store = createWorkbenchStore({ loading: true });
    const { result } = renderHook(() => useWorkbenchPersistence({
      enabled: true,
      adapter,
      store,
      request_id: () => "request-1",
    }));
    await waitFor(() => expect(result.current.status).toBe("ready"));
    vi.useFakeTimers();

    act(() => {
      store.getState().apply_commands([
        { type: "open_surface", surface: makeSurface("local-surface") },
      ]);
    });
    await vi.advanceTimersByTimeAsync(249);
    expect(save).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(1);

    expect(save).toHaveBeenCalledWith({
      document: expect.objectContaining({
        revision: 1,
        surfaces: { "local-surface": expect.any(Object) },
      }),
      expected_revision: 0,
      expected_token: "opaque-zero",
      request_id: "request-1",
    });
    expect(store.getState().durable_revision).toBe(1);
  });

  it("keeps a future-schema load read-only and exportable without save or reset", async () => {
    const adapter = createAdapter({
      load: vi.fn().mockResolvedValue({
        source: "future_schema",
        document: null,
        notice: "Upgrade required.",
        durable_revision: null,
        durable_token: null,
      }),
    });
    const store = createWorkbenchStore({ loading: true });
    const { result } = renderHook(() => useWorkbenchPersistence({
      enabled: true,
      adapter,
      store,
      request_id: () => "request-1",
    }));

    await waitFor(() => expect(result.current.status).toBe("ready"));
    expect(store.getState().read_only).toBe(true);
    expect(result.current.conflict).toBe("future_schema");
    expect(result.current.export_local_json().json).toContain('"schema_version": 1');
    await act(async () => {
      await result.current.reset();
    });
    expect(adapter.save).not.toHaveBeenCalled();
    expect(adapter.reset).not.toHaveBeenCalled();
  });

  it("flushes reset immediately through the reset command", async () => {
    const adapter = createAdapter();
    const store = createWorkbenchStore({ loading: true });
    const { result } = renderHook(() => useWorkbenchPersistence({
      enabled: true,
      adapter,
      store,
      request_id: () => "request-1",
    }));
    await waitFor(() => expect(result.current.status).toBe("ready"));

    await act(async () => {
      await result.current.reset();
    });

    expect(adapter.reset).toHaveBeenCalledWith({
      expected_revision: 0,
      expected_token: "opaque-zero",
      request_id: "request-1",
    });
    expect(adapter.save).not.toHaveBeenCalled();
    expect(store.getState().durable_revision).toBe(1);
    expect(store.getState().durable_token).toBe("opaque-reset");
  });

  it("requests a best-effort immediate flush when unmounted", async () => {
    const save = vi.fn().mockResolvedValue({
      outcome: "saved",
      durable_revision: 1,
      durable_token: "opaque-one",
      request_id: "request-1",
    } satisfies WorkbenchSaveResult);
    const adapter = createAdapter({ save });
    const store = createWorkbenchStore({ loading: true });
    const { result, unmount } = renderHook(() => useWorkbenchPersistence({
      enabled: true,
      adapter,
      store,
      request_id: () => "request-1",
    }));
    await waitFor(() => expect(result.current.status).toBe("ready"));
    act(() => {
      store.getState().apply_commands([
        { type: "open_surface", surface: makeSurface("local-surface") },
      ]);
    });

    unmount();
    await waitFor(() => expect(save).toHaveBeenCalledTimes(1));
  });

  it("uses freshly reloaded disk state only after explicit Use Disk", async () => {
    const diskDocument = {
      ...makeSingleGroupDocument([makeSurface("disk-surface")]),
      revision: 4,
    };
    const load = vi.fn()
      .mockResolvedValueOnce(primaryLoad())
      .mockResolvedValueOnce({
        source: "primary",
        document: diskDocument,
        notice: null,
        durable_revision: 4,
        durable_token: "disk-token",
      } satisfies WorkbenchLoadResult);
    const adapter = createAdapter({
      load,
      save: vi.fn().mockResolvedValue({
        outcome: "revision_conflict",
        durable_revision: 4,
        durable_token: "disk-token",
        request_id: "request-1",
      } satisfies WorkbenchSaveResult),
    });
    const store = createWorkbenchStore({ loading: true });
    const { result } = renderHook(() => useWorkbenchPersistence({
      enabled: true,
      adapter,
      store,
      request_id: () => "request-1",
    }));
    await waitFor(() => expect(result.current.status).toBe("ready"));
    vi.useFakeTimers();
    act(() => {
      store.getState().apply_commands([
        { type: "open_surface", surface: makeSurface("local-surface") },
      ]);
    });
    await vi.advanceTimersByTimeAsync(250);
    expect(store.getState().document.surfaces["local-surface"]).toBeDefined();
    expect(store.getState().conflict).toBe("revision_conflict");

    await act(async () => {
      expect(await result.current.use_disk()).toBe(true);
    });

    expect(load).toHaveBeenCalledTimes(2);
    expect(result.current.store).toBe(store);
    expect(store.getState().document).toEqual(diskDocument);
    expect(store.getState().document.surfaces["local-surface"]).toBeUndefined();
    expect(store.getState().conflict).toBeNull();
  });

  it("rebases the complete local draft onto fresh disk without structural merge", async () => {
    const diskDocument = {
      ...makeSingleGroupDocument([makeSurface("disk-only")]),
      revision: 3,
    };
    const load = vi.fn()
      .mockResolvedValueOnce(primaryLoad())
      .mockResolvedValueOnce({
        source: "primary",
        document: diskDocument,
        notice: null,
        durable_revision: 3,
        durable_token: "disk-token",
      } satisfies WorkbenchLoadResult);
    const save = vi.fn()
      .mockResolvedValueOnce({
        outcome: "revision_conflict",
        durable_revision: 3,
        durable_token: "disk-token",
        request_id: "request-1",
      } satisfies WorkbenchSaveResult)
      .mockResolvedValueOnce({
        outcome: "saved",
        durable_revision: 4,
        durable_token: "replacement-token",
        request_id: "request-2",
      } satisfies WorkbenchSaveResult);
    const requestIds = ["request-1", "request-2"];
    const adapter = createAdapter({ load, save });
    const store = createWorkbenchStore({ loading: true });
    const { result } = renderHook(() => useWorkbenchPersistence({
      enabled: true,
      adapter,
      store,
      request_id: () => requestIds.shift() ?? "unexpected-request",
    }));
    await waitFor(() => expect(result.current.status).toBe("ready"));
    vi.useFakeTimers();
    act(() => {
      store.getState().apply_commands([
        { type: "open_surface", surface: makeSurface("local-only") },
      ]);
    });
    await vi.advanceTimersByTimeAsync(250);

    await act(async () => {
      expect(await result.current.replace_disk()).toBe(true);
    });

    expect(save).toHaveBeenCalledTimes(2);
    expect(save.mock.calls[1]?.[0]).toMatchObject({
      document: {
        revision: 4,
        surfaces: { "local-only": expect.any(Object) },
      },
      expected_revision: 3,
      expected_token: "disk-token",
      request_id: "request-2",
    });
    expect(save.mock.calls[1]?.[0].document.surfaces["disk-only"]).toBeUndefined();
    expect(store.getState().durable_token).toBe("replacement-token");
    expect(store.getState().conflict).toBeNull();
  });

  it("removes wardian-layout only after the migrated document is durably acknowledged", async () => {
    let resolveSave!: (result: WorkbenchSaveResult) => void;
    const save = vi.fn(() => new Promise<WorkbenchSaveResult>((resolve) => {
      resolveSave = resolve;
    }));
    const adapter = createAdapter({
      load: vi.fn().mockResolvedValue({
        source: "default",
        document: makeSingleGroupDocument(),
        notice: null,
        durable_revision: 0,
        durable_token: "opaque-zero",
      } satisfies WorkbenchLoadResult),
      save,
    });
    const legacyStorage = {
      getItem: vi.fn().mockReturnValue(JSON.stringify({
        state: {
          leftSidebarWidth: 260,
          rightSidebarWidth: 310,
          userTerminalOpen: true,
          userTerminalHeight: 420,
          settingsOpen: true,
          gridStacked: true,
          libraryDetailWidth: 700,
        },
        version: 0,
      })),
      removeItem: vi.fn(),
    };
    const store = createWorkbenchStore({ loading: true });
    const { result } = renderHook(() => useWorkbenchPersistence({
      enabled: true,
      adapter,
      store,
      legacy_storage: legacyStorage,
      viewport: () => ({ width: 1_200, height: 900 }),
      request_id: () => "migration-request",
    }));
    await waitFor(() => expect(result.current.status).toBe("ready"));

    expect(store.getState().document.shell).toMatchObject({
      left_sidebar_width: 260,
      right_sidebar_width: 310,
      bottom_terminal_open: true,
      bottom_terminal_height: 420,
    });
    expect(store.getState().document.surfaces["initial-agents-overview"]).toBeDefined();
    let flushPromise!: Promise<void>;
    act(() => {
      flushPromise = result.current.flush();
    });
    await waitFor(() => expect(save).toHaveBeenCalledTimes(1));
    expect(legacyStorage.removeItem).not.toHaveBeenCalled();

    await act(async () => {
      resolveSave({
        outcome: "saved",
        durable_revision: 1,
        durable_token: "migration-token",
        request_id: "migration-request",
      });
      await flushPromise;
    });

    expect(legacyStorage.removeItem).toHaveBeenCalledWith("wardian-layout");
    expect(store.getState().durable_revision).toBe(1);
  });

  it("serializes conflict resolutions so a second choice cannot overlap the first", async () => {
    let resolveReload!: (result: WorkbenchLoadResult) => void;
    const reload = new Promise<WorkbenchLoadResult>((resolve) => {
      resolveReload = resolve;
    });
    const load = vi.fn()
      .mockResolvedValueOnce(primaryLoad())
      .mockImplementationOnce(() => reload);
    const adapter = createAdapter({ load });
    const store = createWorkbenchStore({ loading: true });
    const { result } = renderHook(() => useWorkbenchPersistence({
      enabled: true,
      adapter,
      store,
    }));
    await waitFor(() => expect(result.current.status).toBe("ready"));
    act(() => store.getState().set_conflict("revision_conflict"));

    let firstResolution!: Promise<boolean>;
    act(() => {
      firstResolution = result.current.use_disk();
    });
    await waitFor(() => expect(result.current.resolving_conflict).toBe(true));
    await expect(result.current.replace_disk()).resolves.toBe(false);
    expect(load).toHaveBeenCalledTimes(2);

    await act(async () => {
      resolveReload({
        source: "primary",
        document: { ...makeSingleGroupDocument(), revision: 2 },
        notice: null,
        durable_revision: 2,
        durable_token: "disk-token",
      });
      await expect(firstResolution).resolves.toBe(true);
    });
    expect(result.current.resolving_conflict).toBe(false);
  });

  it("projects restored shell state and persists later shell mutations through the document", async () => {
    const loadedDocument = {
      ...makeSingleGroupDocument(),
      shell: {
        ...makeSingleGroupDocument().shell,
        left_sidebar_width: 330,
        bottom_terminal_open: true,
      },
    };
    let projected = { ...makeSingleGroupDocument().shell };
    const listeners = new Set<() => void>();
    const shellProjection = {
      read: () => projected,
      write: vi.fn((shell: typeof projected) => {
        projected = { ...shell };
      }),
      subscribe: (listener: () => void) => {
        listeners.add(listener);
        return () => listeners.delete(listener);
      },
    };
    const save = vi.fn().mockResolvedValue({
      outcome: "saved",
      durable_revision: 1,
      durable_token: "opaque-one",
      request_id: "request-1",
    } satisfies WorkbenchSaveResult);
    const adapter = createAdapter({
      load: vi.fn().mockResolvedValue({
        ...primaryLoad(),
        document: loadedDocument,
      }),
      save,
    });
    const store = createWorkbenchStore({ loading: true });
    const { result } = renderHook(() => useWorkbenchPersistence({
      enabled: true,
      adapter,
      store,
      shell_projection: shellProjection,
      request_id: () => "request-1",
    }));
    await waitFor(() => expect(result.current.status).toBe("ready"));
    expect(projected.left_sidebar_width).toBe(330);
    expect(projected.bottom_terminal_open).toBe(true);

    projected = { ...projected, right_sidebar_width: 350 };
    act(() => listeners.forEach((listener) => listener()));
    await act(async () => {
      await result.current.flush();
    });

    expect(save).toHaveBeenCalledWith(expect.objectContaining({
      document: expect.objectContaining({
        shell: expect.objectContaining({ right_sidebar_width: 350 }),
      }),
    }));
  });

  it("does not delete wardian-layout when reset becomes durable before the migration snapshot", async () => {
    const requestIds = ["migration-request", "reset-before-migration"];
    const legacyStorage = {
      getItem: vi.fn().mockReturnValue(JSON.stringify({
        state: { leftSidebarWidth: 300 },
        version: 0,
      })),
      removeItem: vi.fn(),
    };
    const adapter = createAdapter({
      load: vi.fn().mockResolvedValue({
        source: "default",
        document: makeSingleGroupDocument(),
        notice: null,
        durable_revision: 0,
        durable_token: "opaque-zero",
      } satisfies WorkbenchLoadResult),
      save: vi.fn().mockRejectedValue(new WorkbenchPersistenceAdapterError(
        "save",
        "malformed migration acknowledgement",
      )),
      reset: vi.fn().mockResolvedValue({
        outcome: "saved",
        durable_revision: 1,
        durable_token: "reset-token",
        request_id: "reset-before-migration",
        document: { ...makeSingleGroupDocument(), revision: 1 },
      } satisfies WorkbenchResetResult),
    });
    const { result } = renderHook(() => useWorkbenchPersistence({
      enabled: true,
      adapter,
      legacy_storage: legacyStorage,
      request_id: () => requestIds.shift() ?? "unexpected-request",
    }));
    await waitFor(() => expect(result.current.status).toBe("ready"));

    await act(async () => {
      await expect(result.current.reset()).resolves.toBe(true);
    });

    expect(legacyStorage.removeItem).not.toHaveBeenCalled();
  });

  it("does not delete wardian-layout for a later replacement acknowledgement", async () => {
    const requestIds = ["migration-request", "replacement-request"];
    const legacyStorage = {
      getItem: vi.fn().mockReturnValue(JSON.stringify({
        state: { rightSidebarWidth: 320 },
        version: 0,
      })),
      removeItem: vi.fn(),
    };
    const load = vi.fn()
      .mockResolvedValueOnce({
        source: "default",
        document: makeSingleGroupDocument(),
        notice: null,
        durable_revision: 0,
        durable_token: "opaque-zero",
      } satisfies WorkbenchLoadResult)
      .mockResolvedValueOnce({
        source: "primary",
        document: { ...makeSingleGroupDocument(), revision: 2 },
        notice: null,
        durable_revision: 2,
        durable_token: "disk-token",
      } satisfies WorkbenchLoadResult);
    const save = vi.fn()
      .mockResolvedValueOnce({
        outcome: "revision_conflict",
        durable_revision: 2,
        durable_token: "disk-token",
        request_id: "migration-request",
      } satisfies WorkbenchSaveResult)
      .mockResolvedValueOnce({
        outcome: "saved",
        durable_revision: 3,
        durable_token: "replacement-token",
        request_id: "replacement-request",
      } satisfies WorkbenchSaveResult);
    const adapter = createAdapter({ load, save });
    const { result } = renderHook(() => useWorkbenchPersistence({
      enabled: true,
      adapter,
      legacy_storage: legacyStorage,
      request_id: () => requestIds.shift() ?? "unexpected-request",
    }));
    await waitFor(() => expect(result.current.status).toBe("ready"));
    await act(async () => {
      await result.current.flush();
    });
    await waitFor(() => expect(result.current.conflict).toBe("revision_conflict"));

    await act(async () => {
      await expect(result.current.replace_disk()).resolves.toBe(true);
    });

    expect(legacyStorage.removeItem).not.toHaveBeenCalled();
  });
});
