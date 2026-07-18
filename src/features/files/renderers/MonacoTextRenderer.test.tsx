import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type * as Monaco from "monaco-editor";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { FilesComparisonBaseline, FileResourceSnapshotV1 } from "../../../types";
import { FileEditorController } from "../fileEditorController";
import type { FileResourceClient } from "../fileResourceClient";
import MonacoTextRenderer from "./MonacoTextRenderer";

const createModel = vi.fn();
const createEditor = vi.fn();
const setTheme = vi.fn();
const setModelLanguage = vi.fn();
const getModel = vi.fn();
const uriFrom = vi.fn((parts: { path: string }) => ({ path: parts.path }));
const addZone = vi.fn();
const removeZone = vi.fn();
const layoutZone = vi.fn();

vi.mock("monaco-editor", () => ({
  KeyCode: { F7: 65, KeyS: 49 },
  KeyMod: { CtrlCmd: 2048, Shift: 1024 },
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
  deltaDecorations: ReturnType<typeof vi.fn>;
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
    deltaDecorations: vi.fn().mockImplementation((_previous, decorations: unknown[]) => (
      decorations.map((_, index) => `decoration-${index}`)
    )),
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

async function editorHarness(initialText = "const answer = 42;\n") {
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
  await controller.initialize({ owner: snapshot(), text: initialText });
  const props = (
    surfaceId: string,
    revision = 4,
    comparisonBaseline: FilesComparisonBaseline | null = { kind: "saved_file" },
  ) => ({
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
      read_only: controller.getSnapshot().authorization.status === "unavailable",
    }),
    editor_language: "typescript",
    comparison_baseline: comparisonBaseline,
    on_open_file: vi.fn(),
    on_open_with: vi.fn(),
    on_reveal: vi.fn(),
  });
  return { client, controller, props };
}

describe("MonacoTextRenderer", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    let zoneIndex = 0;
    addZone.mockImplementation(() => `zone-${zoneIndex += 1}`);
    getModel.mockReturnValue(null);
    createModel.mockImplementation((text, _language, uri) => fakeModel(text, uri));
    createEditor.mockImplementation(() => ({
      addCommand: vi.fn(),
      changeViewZones: vi.fn((callback) => callback({ addZone, removeZone, layoutZone })),
      deltaDecorations: vi.fn().mockImplementation((_previous, decorations: unknown[]) => (
        decorations.map((_, index) => `view-decoration-${index}`)
      )),
      dispose: vi.fn(),
      focus: vi.fn(),
      layout: vi.fn(),
      revealLineInCenter: vi.fn(),
      setPosition: vi.fn(),
      updateOptions: vi.fn(),
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

  it("switches the shared model read-only in place and rejects a raced model edit", async () => {
    const { client, controller, props } = await editorHarness();
    const view = render(<MonacoTextRenderer {...props("files-a")} />);
    await waitFor(() => expect(createEditor).toHaveBeenCalledOnce());
    const editor = createEditor.mock.results[0]?.value as {
      updateOptions: ReturnType<typeof vi.fn>;
    };
    const model = createModel.mock.results[0]?.value as FakeModel;
    controller.mutate("dirty before revocation\n");
    vi.mocked(client.saveText).mockRejectedValue({
      code: "unauthorized_path",
      message: "access revoked",
    });

    await expect(controller.save()).rejects.toMatchObject({ code: "unauthorized_path" });
    view.rerender(<MonacoTextRenderer {...props("files-a")} />);
    await waitFor(() => expect(editor.updateOptions).toHaveBeenCalledWith({ readOnly: true }));
    expect(createEditor).toHaveBeenCalledOnce();
    expect(createModel).toHaveBeenCalledOnce();

    const generation = controller.getSnapshot().buffer_generation;
    act(() => model.setValue("raced edit after revocation\n"));
    await waitFor(() => expect(model.getValue()).toBe("dirty before revocation\n"));
    expect(controller.getSnapshot().buffer_generation).toBe(generation);
    expect(controller.getSnapshot().working_text).toBe("dirty before revocation\n");
  });

  it("updates saved-file decorations without replacing the canonical model", async () => {
    const { controller, props } = await editorHarness();
    render(<MonacoTextRenderer {...props("files-a")} />);
    await waitFor(() => expect(createEditor).toHaveBeenCalledOnce());
    const editor = createEditor.mock.results[0]?.value as {
      deltaDecorations: ReturnType<typeof vi.fn>;
    };
    const before = editor.deltaDecorations.mock.calls.length;

    act(() => { controller.mutate("const answer = 43;\n"); });
    await waitFor(() => expect(editor.deltaDecorations.mock.calls.length).toBeGreaterThan(before));
    expect(editor.deltaDecorations).toHaveBeenLastCalledWith(
      expect.any(Array),
      expect.arrayContaining([expect.objectContaining({
        options: expect.objectContaining({
          className: "files-diff-modified-line",
          glyphMarginClassName: "files-diff-modified-glyph",
        }),
      })]),
    );
    expect(createModel).toHaveBeenCalledOnce();
    expect(createEditor).toHaveBeenCalledOnce();
  });

  it("keeps saved-file annotations scoped to the presentation that selected them", async () => {
    const { controller, props } = await editorHarness();
    render(<MonacoTextRenderer {...props("files-saved", 4, { kind: "saved_file" })} />);
    render(<MonacoTextRenderer {...props("files-prompt", 4, {
      kind: "prompt_checkpoint",
      checkpoint_id: "checkpoint-1",
    })} />);
    await waitFor(() => expect(createEditor).toHaveBeenCalledTimes(2));
    const savedEditor = createEditor.mock.results[0]?.value as {
      deltaDecorations: ReturnType<typeof vi.fn>;
    };
    const promptEditor = createEditor.mock.results[1]?.value as {
      deltaDecorations: ReturnType<typeof vi.fn>;
    };

    act(() => controller.mutate("const answer = 43;\n"));
    await waitFor(() => expect(savedEditor.deltaDecorations).toHaveBeenLastCalledWith(
      expect.any(Array),
      expect.arrayContaining([expect.objectContaining({
        options: expect.objectContaining({ className: "files-diff-modified-line" }),
      })]),
    ));
    expect(promptEditor.deltaDecorations).toHaveBeenLastCalledWith(expect.any(Array), []);
    expect(createModel).toHaveBeenCalledOnce();
    expect((createModel.mock.results[0]?.value as FakeModel).deltaDecorations)
      .not.toHaveBeenCalled();
  });

  it("adds collapsible deleted-line zones and removes them with the editor view", async () => {
    const { controller, props } = await editorHarness("const answer = 42;");
    const view = render(<MonacoTextRenderer {...props("files-a")} />);
    await waitFor(() => expect(createEditor).toHaveBeenCalledOnce());

    act(() => { controller.mutate(""); });
    await waitFor(() => expect(addZone).toHaveBeenCalled());
    const zone = addZone.mock.calls[addZone.mock.calls.length - 1]?.[0] as Monaco.editor.IViewZone;
    const toggle = zone.domNode.querySelector("button");
    const content = zone.domNode.querySelector("pre");
    expect(zone.afterLineNumber).toBe(0);
    expect(toggle).toHaveAttribute("aria-expanded", "true");
    expect(toggle).toHaveAccessibleName(/collapse 1 deleted line/i);
    expect(content).toHaveTextContent("const answer = 42;");

    act(() => { toggle?.click(); });
    expect(toggle).toHaveAttribute("aria-expanded", "false");
    expect(content).toHaveAttribute("hidden");
    expect(zone.heightInLines).toBe(1);
    expect(layoutZone).toHaveBeenCalledWith(expect.stringMatching(/^zone-/));

    view.unmount();
    expect(removeZone).toHaveBeenCalledWith(expect.stringMatching(/^zone-/));
    expect(createModel).toHaveBeenCalledOnce();
  });

  it("places a deleted suffix zone after the surviving final line", async () => {
    const { controller, props } = await editorHarness("alpha\nbeta");
    render(<MonacoTextRenderer {...props("files-a")} />);
    await waitFor(() => expect(createEditor).toHaveBeenCalledOnce());

    act(() => controller.mutate("alpha"));
    await waitFor(() => expect(addZone).toHaveBeenCalled());
    const zone = addZone.mock.calls[addZone.mock.calls.length - 1]?.[0] as Monaco.editor.IViewZone;
    expect(zone.afterLineNumber).toBe(1);
    expect(zone.domNode).toHaveTextContent("beta");
  });

  it("repeats and wraps keyboard change navigation after focus moves into Monaco", async () => {
    const user = userEvent.setup();
    const { controller, props } = await editorHarness("alpha\nbeta\ngamma");
    render(<MonacoTextRenderer {...props("files-a")} />);
    await waitFor(() => expect(createEditor).toHaveBeenCalledOnce());
    const editor = createEditor.mock.results[0]?.value as {
      addCommand: ReturnType<typeof vi.fn>;
      focus: ReturnType<typeof vi.fn>;
      revealLineInCenter: ReturnType<typeof vi.fn>;
      setPosition: ReturnType<typeof vi.fn>;
    };
    act(() => controller.mutate("alpha\nBETA\ngamma\nadded"));
    const next = await screen.findByRole("button", { name: "Next Saved file change" });
    next.focus();
    await user.keyboard("{Enter}");

    expect(editor.revealLineInCenter).toHaveBeenCalledWith(2);
    expect(editor.setPosition).toHaveBeenCalledWith({ lineNumber: 2, column: 1 });
    expect(editor.focus).toHaveBeenCalledOnce();
    expect(screen.getByRole("status", { name: "Current file change" }))
      .toHaveTextContent(/modified change 1 of 2, line 2, against saved file/i);

    const nextCommand = editor.addCommand.mock.calls.find(([binding]) => binding === 65)?.[1];
    const previousCommand = editor.addCommand.mock.calls.find(
      ([binding]) => binding === (1024 | 65),
    )?.[1];
    expect(nextCommand).toBeTypeOf("function");
    expect(previousCommand).toBeTypeOf("function");
    if (!nextCommand || !previousCommand) throw new Error("Monaco change commands are unavailable");

    act(() => nextCommand());
    expect(editor.revealLineInCenter).toHaveBeenLastCalledWith(4);
    expect(screen.getByRole("status", { name: "Current file change" }))
      .toHaveTextContent(/added change 2 of 2, line 4, against saved file/i);

    act(() => nextCommand());
    expect(editor.revealLineInCenter).toHaveBeenLastCalledWith(2);
    expect(screen.getByRole("status", { name: "Current file change" }))
      .toHaveTextContent(/modified change 1 of 2, line 2, against saved file/i);

    act(() => previousCommand());
    expect(editor.revealLineInCenter).toHaveBeenLastCalledWith(4);
    expect(screen.getByRole("status", { name: "Current file change" }))
      .toHaveTextContent(/added change 2 of 2, line 4, against saved file/i);
    expect(editor.focus).toHaveBeenCalledTimes(4);
    expect(createEditor).toHaveBeenCalledOnce();
    expect(createModel).toHaveBeenCalledOnce();
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
    const saveCommand = editor.addCommand.mock.calls.find(
      ([binding]) => binding === (2048 | 49),
    )?.[1] as (() => Promise<void>) | undefined;
    if (!saveCommand) throw new Error("Monaco save command is unavailable");
    await act(async () => {
      await saveCommand();
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
    const host = view.getByTestId("monaco-text-renderer");
    const editorDom = document.createElement("div");
    editorDom.dataset.testid = "monaco-editor-dom";
    host.appendChild(editorDom);
    const saveCommand = editor.addCommand.mock.calls.find(
      ([binding]) => binding === (2048 | 49),
    )?.[1] as (() => Promise<void>) | undefined;
    if (!saveCommand) throw new Error("Monaco save command is unavailable");
    const unhandled = vi.fn();
    window.addEventListener("unhandledrejection", unhandled);

    await expect(act(async () => saveCommand())).resolves.toBeUndefined();

    expect(await screen.findByRole("alert")).toHaveTextContent(
      /save failed: write access revoked/i,
    );
    expect(view.getByTestId("monaco-text-renderer")).toBe(host);
    expect(host).toContainElement(editorDom);
    expect(host.parentElement?.firstElementChild).toBe(host);
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
    expect(view.getByTestId("monaco-text-renderer")).toBe(host);
    expect(host).toContainElement(editorDom);
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
    const editor = {
      addCommand: vi.fn(),
      changeViewZones: vi.fn((callback) => callback({ addZone, removeZone, layoutZone })),
      deltaDecorations: vi.fn().mockImplementation((_previous, decorations: unknown[]) => (
        decorations.map((_, index) => `view-decoration-${index}`)
      )),
      dispose: vi.fn(),
      layout: vi.fn(),
    };
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
