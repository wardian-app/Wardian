import { act, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { FilesComparisonBaseline } from "../../types";
import { FileEditorController } from "./fileEditorController";
import type { FileResourceClient } from "./fileResourceClient";
import { FileComparisonLens } from "./FileComparisonLens";

const createDiffEditor = vi.fn();
const createModel = vi.fn();
const getModel = vi.fn();
const uriFrom = vi.fn((parts: { path: string }) => ({ path: parts.path }));
const addCommand = vi.fn();

vi.mock("monaco-editor", () => ({
  KeyCode: { KeyS: 49 },
  KeyMod: { CtrlCmd: 2048 },
  Uri: { from: uriFrom },
  editor: {
    create: vi.fn(),
    createDiffEditor,
    createModel,
    getModel,
    setModelLanguage: vi.fn(),
    setTheme: vi.fn(),
  },
}));

vi.mock("monaco-editor/esm/vs/editor/editor.worker.js?worker", () => ({
  default: class TestEditorWorker {},
}));

type FakeModel = {
  value: string;
  dispose: ReturnType<typeof vi.fn>;
  getValue: () => string;
  setValue: (value: string) => void;
  onDidChangeContent: (listener: () => void) => { dispose: () => void };
  deltaDecorations: ReturnType<typeof vi.fn>;
};

function fakeModel(text: string): FakeModel {
  const listeners = new Set<() => void>();
  const model: FakeModel = {
    value: text,
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
    deltaDecorations: vi.fn().mockReturnValue([]),
  };
  return model;
}

const controllers: FileEditorController[] = [];

function controllerOwner(revision = 4, contentHash = "hash-base") {
  return {
    resource_id: "file:C:/work/notes.md",
    subscription_id: "subscription-1",
    revision,
    descriptor: {
      schema: 1 as const,
      canonical_path: "C:/work/notes.md",
      display_name: "notes.md",
      extension: "md",
      mime_type: "text/markdown",
      encoding: "utf-8" as const,
      renderer_kind: "markdown" as const,
      size_bytes: 12,
      line_count: 2,
      content_hash: contentHash,
      modified_at_ms: 1,
      capabilities: { preview: true, changes: true, draft: true, stream: false },
      unavailable_reason: null,
    },
  };
}

async function harness(baseline: FilesComparisonBaseline = { kind: "saved_file" }) {
  const client = {
    saveText: vi.fn(),
    readText: vi.fn(),
    checkpointRecovery: vi.fn(),
    listRecoveries: vi.fn().mockResolvedValue([]),
    getRecovery: vi.fn(),
    discardRecovery: vi.fn(),
    mergeRecovery: vi.fn(),
  } as unknown as FileResourceClient;
  const controller = new FileEditorController("file:C:/work/notes.md", client, {
    checkpoint_debounce_ms: 60_000,
  });
  controllers.push(controller);
  await controller.initialize({
    owner: controllerOwner(),
    text: "saved\ntext\n",
    discover_recovery: false,
  });
  controller.mutate("working\ntext\n");
  const props = {
    controller,
    surface_id: "files-1",
    baseline,
    layout_preference: "auto" as const,
    language: "markdown",
    lifecycle: { visible: true },
    on_close: vi.fn(),
    on_layout_preference_change: vi.fn(),
    on_reload_from_disk: vi.fn().mockResolvedValue(undefined),
    on_keep_working_buffer: vi.fn(),
    on_merge: vi.fn().mockResolvedValue(undefined),
  };
  return { client, controller, props };
}

describe("FileComparisonLens", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    getModel.mockReturnValue(null);
    createModel.mockImplementation((text) => fakeModel(text));
    createDiffEditor.mockImplementation(() => ({
      dispose: vi.fn(),
      getModifiedEditor: vi.fn(() => ({ addCommand })),
      layout: vi.fn(),
      setModel: vi.fn(),
      updateOptions: vi.fn(),
    }));
    vi.stubGlobal("ResizeObserver", class TestResizeObserver {
      constructor(private readonly callback: ResizeObserverCallback) {}
      observe(target: Element) {
        this.callback([{
          target,
          contentRect: { width: 800, height: 500 },
        } as unknown as ResizeObserverEntry], this as unknown as ResizeObserver);
      }
      unobserve() {}
      disconnect() {}
    });
  });

  afterEach(() => {
    for (const controller of controllers.splice(0)) controller.dispose();
    vi.unstubAllGlobals();
  });

  it("uses the saved baseline and the live canonical modified model", async () => {
    const { controller, props } = await harness();
    render(<FileComparisonLens {...props} />);
    await waitFor(() => expect(createDiffEditor).toHaveBeenCalledOnce());
    const diffEditor = createDiffEditor.mock.results[0]?.value as {
      setModel: ReturnType<typeof vi.fn>;
    };
    expect(diffEditor.setModel).toHaveBeenCalledOnce();
    const pair = diffEditor.setModel.mock.calls[0]?.[0] as {
      original: FakeModel;
      modified: FakeModel;
    };
    expect(pair.original.getValue()).toBe("saved\ntext\n");
    expect(pair.modified.getValue()).toBe("working\ntext\n");

    act(() => controller.mutate("newest\ntext\n"));
    await waitFor(() => expect(pair.modified.getValue()).toBe("newest\ntext\n"));
    expect(createModel).toHaveBeenCalledTimes(2);
  });

  it("renders unsupported historical baselines honestly instead of using saved bytes", async () => {
    const { props } = await harness({
      kind: "prompt_checkpoint",
      checkpoint_id: "checkpoint-1",
    });
    render(<FileComparisonLens {...props} />);
    expect(screen.getByText("Since last prompt")).toBeInTheDocument();
    expect(screen.getByRole("status")).toHaveTextContent("Comparison baseline unavailable");
    expect(screen.queryByLabelText("Changes summary")).toBeNull();
    expect(screen.queryByText("Saved file")).toBeNull();
    expect(createDiffEditor).not.toHaveBeenCalled();
  });

  it("degrades a forced side-by-side layout to unified below its hard minimum", async () => {
    vi.stubGlobal("ResizeObserver", class TestResizeObserver {
      constructor(private readonly callback: ResizeObserverCallback) {}
      observe(target: Element) {
        this.callback([{
          target,
          contentRect: { width: 500, height: 500 },
        } as unknown as ResizeObserverEntry], this as unknown as ResizeObserver);
      }
      unobserve() {}
      disconnect() {}
    });
    const { props } = await harness();
    render(<FileComparisonLens {...props} layout_preference="side_by_side" />);
    await waitFor(() => expect(createDiffEditor).toHaveBeenCalledOnce());
    expect(createDiffEditor).toHaveBeenCalledWith(expect.any(HTMLElement), expect.objectContaining({
      renderSideBySide: false,
    }));
  });

  it("updates layout at responsive thresholds without recreating either model or editor", async () => {
    const observed: Array<{
      callback: ResizeObserverCallback;
      targets: Set<Element>;
    }> = [];
    vi.stubGlobal("ResizeObserver", class TestResizeObserver {
      readonly targets = new Set<Element>();
      constructor(readonly callback: ResizeObserverCallback) {
        observed.push(this);
      }
      observe(target: Element) {
        this.targets.add(target);
        this.callback([{
          target,
          contentRect: { width: 800, height: 500 },
        } as unknown as ResizeObserverEntry], this as unknown as ResizeObserver);
      }
      unobserve() {}
      disconnect() {}
    });
    const { props } = await harness();
    render(<FileComparisonLens {...props} />);
    await waitFor(() => expect(createDiffEditor).toHaveBeenCalledOnce());
    const editor = createDiffEditor.mock.results[0]?.value as {
      updateOptions: ReturnType<typeof vi.fn>;
    };
    expect(createDiffEditor).toHaveBeenCalledWith(expect.any(HTMLElement), expect.objectContaining({
      renderSideBySide: true,
      useInlineViewWhenSpaceIsLimited: false,
    }));

    act(() => {
      for (const observer of observed) {
        for (const target of observer.targets) {
          observer.callback([{
            target,
            contentRect: { width: 650, height: 500 },
          } as unknown as ResizeObserverEntry], observer as unknown as ResizeObserver);
        }
      }
    });
    await waitFor(() => expect(editor.updateOptions).toHaveBeenCalledWith({
      renderSideBySide: false,
    }));
    expect(createDiffEditor).toHaveBeenCalledOnce();
    expect(createModel).toHaveBeenCalledTimes(2);
  });

  it("keeps the diff editor mounted across equivalent state and saved-file revisions", async () => {
    const { controller, props } = await harness();
    act(() => controller.mutate("saved\ntext\n"));
    const view = render(<FileComparisonLens {...props} />);
    await waitFor(() => expect(createDiffEditor).toHaveBeenCalledOnce());
    const host = view.getByTestId("file-comparison-editor");
    const editorDom = document.createElement("div");
    editorDom.dataset.diffIdentity = "preserved";
    host.appendChild(editorDom);
    const editor = createDiffEditor.mock.results[0]?.value as {
      setModel: ReturnType<typeof vi.fn>;
    };
    const firstOriginal = editor.setModel.mock.calls[0]?.[0]?.original as FakeModel;

    view.rerender(
      <FileComparisonLens
        {...props}
        baseline={{ kind: "saved_file" }}
        lifecycle={{ visible: true }}
      />,
    );
    await act(async () => Promise.resolve());
    expect(createDiffEditor).toHaveBeenCalledOnce();
    expect(host).toContainElement(editorDom);

    act(() => {
      controller.applyAuthoritative(
        controllerOwner(5, "hash-next"),
        "next saved\ntext\n",
      );
    });
    await waitFor(() => expect(firstOriginal.getValue()).toBe("next saved\ntext\n"));
    expect(createDiffEditor).toHaveBeenCalledOnce();
    expect(createModel).toHaveBeenCalledTimes(2);
    expect(editor.setModel).toHaveBeenCalledOnce();
    expect(view.getByTestId("file-comparison-editor")).toBe(host);
    expect(host).toContainElement(editorDom);
    expect(firstOriginal.dispose).not.toHaveBeenCalled();
  });

  it("contains modified-editor Save failures without replacing the lens host", async () => {
    const { controller, props } = await harness();
    vi.spyOn(controller, "save").mockRejectedValueOnce(new Error("write denied"));
    const view = render(<FileComparisonLens {...props} />);
    await waitFor(() => expect(createDiffEditor).toHaveBeenCalledOnce());
    expect(addCommand).toHaveBeenCalledWith(2048 | 49, expect.any(Function));
    const host = view.getByTestId("file-comparison-editor");
    const editorDom = document.createElement("div");
    host.appendChild(editorDom);
    const save = addCommand.mock.calls[0]?.[1] as () => Promise<void>;

    await expect(act(async () => save())).resolves.toBeUndefined();

    expect(await screen.findByRole("alert")).toHaveTextContent(/save failed: write denied/i);
    expect(view.getByTestId("file-comparison-editor")).toBe(host);
    expect(host).toContainElement(editorDom);
    expect(createDiffEditor).toHaveBeenCalledOnce();
    expect(createModel).toHaveBeenCalledTimes(2);
  });

  it("makes the modified side read-only in place after authorization is revoked", async () => {
    const { client, controller, props } = await harness();
    render(<FileComparisonLens {...props} />);
    await waitFor(() => expect(createDiffEditor).toHaveBeenCalledOnce());
    const editor = createDiffEditor.mock.results[0]?.value as {
      updateOptions: ReturnType<typeof vi.fn>;
    };
    controller.applyAuthoritative(controllerOwner(5, "hash-external"), "disk\ntext\n");
    vi.mocked(client.saveText).mockRejectedValue({
      code: "unauthorized_path",
      message: "access revoked",
    });

    await expect(controller.save()).rejects.toMatchObject({ code: "unauthorized_path" });
    await waitFor(() => expect(editor.updateOptions).toHaveBeenCalledWith({ readOnly: true }));
    expect(screen.getByRole("button", { name: "Merge" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Reload from disk" })).toBeDisabled();
    expect(createDiffEditor).toHaveBeenCalledOnce();
    expect(createModel).toHaveBeenCalledTimes(2);
  });
});
