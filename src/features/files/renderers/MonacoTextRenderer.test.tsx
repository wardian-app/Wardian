import { act, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { FileResourceSnapshotV1 } from "../../../types";
import { FileEditorController } from "../fileEditorController";
import type { FileResourceClient } from "../fileResourceClient";
import MonacoTextRenderer from "./MonacoTextRenderer";

const createModel = vi.fn();
const createEditor = vi.fn();
const setTheme = vi.fn();
const setModelLanguage = vi.fn();
const getModel = vi.fn();
const uriFrom = vi.fn((parts: { path: string }) => ({ path: parts.path }));

vi.mock("monaco-editor", () => ({
  KeyCode: { KeyS: 49 },
  KeyMod: { CtrlCmd: 2048 },
  Uri: { from: uriFrom },
  editor: { create: createEditor, createModel, getModel, setModelLanguage, setTheme },
}));

vi.mock("monaco-editor/esm/vs/editor/editor.worker.js?worker", () => ({
  default: class TestEditorWorker {},
}));

type FakeModel = {
  value: string;
  uri: { path: string };
  dispose: ReturnType<typeof vi.fn>;
  getValue: () => string;
  setValue: (value: string) => void;
  onDidChangeContent: (listener: () => void) => { dispose: () => void };
};

const controllers: FileEditorController[] = [];

function fakeModel(text: string, uri: { path: string }): FakeModel {
  const listeners = new Set<() => void>();
  const model: FakeModel = {
    value: text,
    uri,
    dispose: vi.fn(),
    getValue: () => model.value,
    setValue: (value) => {
      model.value = value;
      for (const listener of listeners) listener();
    },
    onDidChangeContent: (listener) => {
      listeners.add(listener);
      return { dispose: () => listeners.delete(listener) };
    },
  };
  return model;
}

function snapshot(revision = 4): FileResourceSnapshotV1 {
  return {
    resource_id: "file:C:/work/src/main.ts",
    subscription_id: "subscription-1",
    revision,
    descriptor: {
      schema: 1,
      canonical_path: "C:/work/src/main.ts",
      display_name: "main.ts",
      extension: "ts",
      mime_type: "text/plain",
      encoding: "utf-8",
      renderer_kind: "text",
      size_bytes: 24,
      line_count: 2,
      content_hash: `hash-${revision}`,
      modified_at_ms: revision,
      capabilities: { preview: true, changes: true, draft: true, stream: false },
      unavailable_reason: null,
    },
  };
}

async function editorHarness() {
  const client = {
    saveText: vi.fn().mockResolvedValue({
      status: "saved",
      revision: 5,
      content_hash: "hash-5",
    }),
    checkpointRecovery: vi.fn(),
    listRecoveries: vi.fn().mockResolvedValue([]),
    getRecovery: vi.fn(),
    discardRecovery: vi.fn(),
  } as unknown as FileResourceClient;
  const controller = new FileEditorController(snapshot().resource_id, client, {
    checkpoint_debounce_ms: 60_000,
  });
  controllers.push(controller);
  await controller.initialize({ owner: snapshot(), text: "const answer = 42;\n" });
  const props = (surfaceId: string, revision = 4) => ({
    snapshot: snapshot(revision),
    client,
    lifecycle: { visible: true },
    surface_id: surfaceId,
    editor_controller: controller,
    buffer_snapshot: Object.freeze({
      resource_id: snapshot().resource_id,
      revision,
      buffer_generation: controller.getSnapshot().buffer_generation,
      text: controller.getSnapshot().working_text,
      dirty: controller.getSnapshot().dirty,
    }),
    editor_language: "typescript",
    on_open_file: vi.fn(),
    on_open_with: vi.fn(),
    on_reveal: vi.fn(),
  });
  return { client, controller, props };
}

describe("MonacoTextRenderer", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    getModel.mockReturnValue(null);
    createModel.mockImplementation((text, _language, uri) => fakeModel(text, uri));
    createEditor.mockImplementation(() => ({
      addCommand: vi.fn(),
      dispose: vi.fn(),
      layout: vi.fn(),
    }));
  });

  afterEach(() => {
    for (const controller of controllers.splice(0)) controller.dispose();
    vi.unstubAllGlobals();
  });

  it("shares canonical resource models and disposes only after the last pane", async () => {
    const { controller, props } = await editorHarness();
    const first = render(<MonacoTextRenderer {...props("files-a")} />);
    const second = render(<MonacoTextRenderer {...props("files-b")} />);

    await waitFor(() => expect(createEditor).toHaveBeenCalledTimes(2));
    expect(createModel).toHaveBeenCalledTimes(1);
    expect(createModel.mock.calls[0]?.[1]).toBe("typescript");
    expect(uriFrom).toHaveBeenCalledWith(expect.objectContaining({
      path: `/${encodeURIComponent(snapshot().resource_id)}`,
    }));
    expect(createEditor).toHaveBeenCalledWith(expect.any(HTMLElement), expect.objectContaining({
      model: createModel.mock.results[0]?.value,
      readOnly: false,
    }));

    const model = createModel.mock.results[0]?.value as FakeModel;
    act(() => model.setValue("const answer = 43;\n"));
    expect(controller.getSnapshot()).toMatchObject({
      working_text: "const answer = 43;\n",
      dirty: true,
      buffer_generation: 1,
    });

    first.unmount();
    second.unmount();
    expect(model.dispose).not.toHaveBeenCalled();
    controller.dispose();
    expect(model.dispose).toHaveBeenCalledOnce();
  });

  it("applies controller text without a model feedback mutation", async () => {
    const { controller, props } = await editorHarness();
    render(<MonacoTextRenderer {...props("files-a")} />);
    await waitFor(() => expect(createEditor).toHaveBeenCalledOnce());
    const model = createModel.mock.results[0]?.value as FakeModel;

    act(() => { controller.mutate("changed from another pane\n"); });
    await waitFor(() => expect(model.getValue()).toBe("changed from another pane\n"));
    expect(controller.getSnapshot().buffer_generation).toBe(1);
  });

  it("routes Ctrl/Cmd+S through the active resource session", async () => {
    const { client, controller, props } = await editorHarness();
    act(() => { controller.mutate("save me\n"); });
    render(<MonacoTextRenderer {...props("files-a")} />);
    await waitFor(() => expect(createEditor).toHaveBeenCalledOnce());
    const editor = createEditor.mock.results[0]?.value as {
      addCommand: ReturnType<typeof vi.fn>;
    };
    expect(editor.addCommand).toHaveBeenCalledWith(2048 | 49, expect.any(Function));
    await act(async () => {
      await editor.addCommand.mock.calls[0]?.[1]();
    });
    expect(client.saveText).toHaveBeenCalledWith(expect.objectContaining({ text: "save me\n" }));
    expect(controller.getSnapshot().dirty).toBe(false);
  });

  it("contains rejected keyboard saves without replacing or detaching the editor", async () => {
    const { client, controller, props } = await editorHarness();
    const saveText = vi.mocked(client.saveText);
    saveText.mockRejectedValueOnce(new Error("write access revoked"));
    act(() => { controller.mutate("unsaved text\n"); });
    const view = render(<MonacoTextRenderer {...props("files-a")} />);
    await waitFor(() => expect(createEditor).toHaveBeenCalledOnce());
    const editor = createEditor.mock.results[0]?.value as {
      addCommand: ReturnType<typeof vi.fn>;
    };
    const saveCommand = editor.addCommand.mock.calls[0]?.[1] as () => Promise<void>;
    const unhandled = vi.fn();
    window.addEventListener("unhandledrejection", unhandled);

    await expect(act(async () => saveCommand())).resolves.toBeUndefined();

    expect(await screen.findByRole("alert")).toHaveTextContent(
      /save failed: write access revoked/i,
    );
    expect(view.getByTestId("monaco-text-renderer")).toBeVisible();
    expect(createModel).toHaveBeenCalledOnce();
    expect(createEditor).toHaveBeenCalledOnce();
    expect(controller.getSnapshot()).toMatchObject({
      working_text: "unsaved text\n",
      dirty: true,
    });
    expect(unhandled).not.toHaveBeenCalled();

    saveText.mockResolvedValueOnce({
      status: "saved",
      revision: 5,
      content_hash: "hash-5",
    });
    await expect(act(async () => saveCommand())).resolves.toBeUndefined();
    await waitFor(() => expect(screen.queryByRole("alert")).toBeNull());
    expect(createModel).toHaveBeenCalledOnce();
    expect(createEditor).toHaveBeenCalledOnce();
    window.removeEventListener("unhandledrejection", unhandled);
  });

  it("does not recreate the model or editor for a newer snapshot revision", async () => {
    const { props } = await editorHarness();
    const view = render(<MonacoTextRenderer {...props("files-a")} />);
    await waitFor(() => expect(createEditor).toHaveBeenCalledOnce());
    view.rerender(<MonacoTextRenderer {...props("files-a", 5)} />);
    expect(createModel).toHaveBeenCalledOnce();
    expect(createEditor).toHaveBeenCalledOnce();
  });

  it("lays out from real ResizeObserver size changes without repeated jitter", async () => {
    let callback: ResizeObserverCallback | undefined;
    const disconnect = vi.fn();
    vi.stubGlobal("ResizeObserver", class TestResizeObserver {
      constructor(next: ResizeObserverCallback) { callback = next; }
      observe() {}
      unobserve() {}
      disconnect() { disconnect(); }
    });
    const editor = { addCommand: vi.fn(), dispose: vi.fn(), layout: vi.fn() };
    createEditor.mockReturnValue(editor);
    const { props } = await editorHarness();
    const view = render(<MonacoTextRenderer {...props("files-a")} />);
    await waitFor(() => expect(createEditor).toHaveBeenCalledOnce());

    const target = view.getByTestId("monaco-text-renderer");
    const entry = {
      target,
      contentRect: { width: 640, height: 360 },
    } as unknown as ResizeObserverEntry;
    act(() => callback?.([entry], {} as ResizeObserver));
    act(() => callback?.([entry], {} as ResizeObserver));
    expect(editor.layout).toHaveBeenCalledTimes(1);
    expect(editor.layout).toHaveBeenCalledWith({ width: 640, height: 360 });
    view.unmount();
    expect(disconnect).toHaveBeenCalledOnce();
  });

  it("follows live Wardian data-theme changes without recreating the editor", async () => {
    document.documentElement.setAttribute("data-theme", "dark");
    const { props } = await editorHarness();
    const view = render(<MonacoTextRenderer {...props("files-a")} />);
    await waitFor(() => expect(createEditor).toHaveBeenCalledOnce());
    expect(createEditor).toHaveBeenCalledWith(expect.any(HTMLElement), expect.objectContaining({
      theme: "vs-dark",
    }));
    document.documentElement.setAttribute("data-theme", "light");
    await waitFor(() => expect(setTheme).toHaveBeenCalledWith("vs"));
    expect(createEditor).toHaveBeenCalledOnce();
    view.unmount();
  });
});
