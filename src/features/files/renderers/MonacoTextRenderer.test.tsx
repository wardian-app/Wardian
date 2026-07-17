import { act, render, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { FileResourceSnapshotV1 } from "../../../types";
import type { FileResourceClient } from "../fileResourceClient";
import MonacoTextRenderer from "./MonacoTextRenderer";

const createModel = vi.fn();
const createEditor = vi.fn();
const setTheme = vi.fn();
const getModel = vi.fn();
const uriFrom = vi.fn((parts: { path: string }) => ({ path: parts.path }));

vi.mock("monaco-editor", () => ({
  Uri: { from: uriFrom },
  editor: { create: createEditor, createModel, getModel, setTheme },
}));

vi.mock("monaco-editor/esm/vs/editor/editor.worker.js?worker", () => ({
  default: class TestEditorWorker {},
}));

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

function props(client: FileResourceClient, revision = 4) {
  return {
    snapshot: snapshot(revision),
    client,
    lifecycle: { visible: true },
    on_open_file: vi.fn(),
    on_open_with: vi.fn(),
    on_reveal: vi.fn(),
  };
}

describe("MonacoTextRenderer", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    getModel.mockReturnValue(null);
    createModel.mockImplementation((_text, _language, uri) => ({ uri, dispose: vi.fn() }));
    createEditor.mockImplementation(() => ({ dispose: vi.fn(), layout: vi.fn() }));
  });

  it("shares revision-keyed models and disposes only after the last pane", async () => {
    const client = {
      readText: vi.fn().mockResolvedValue({
        schema: 1,
        resource_id: snapshot().resource_id,
        revision: 4,
        text: "const answer = 42;\n",
      }),
    } as unknown as FileResourceClient;

    const first = render(<MonacoTextRenderer {...props(client)} />);
    const second = render(<MonacoTextRenderer {...props(client)} />);

    await waitFor(() => expect(createEditor).toHaveBeenCalledTimes(2));
    expect(createModel).toHaveBeenCalledTimes(1);
    expect(createModel.mock.calls[0]?.[1]).toBe("typescript");
    expect(uriFrom).toHaveBeenCalledWith(expect.objectContaining({
      path: `/${snapshot().resource_id}@4`,
    }));
    expect(createEditor).toHaveBeenCalledWith(expect.any(HTMLElement), expect.objectContaining({
      automaticLayout: false,
      readOnly: true,
    }));

    const model = createModel.mock.results[0]?.value as { dispose: ReturnType<typeof vi.fn> };
    first.unmount();
    expect(model.dispose).not.toHaveBeenCalled();
    second.unmount();
    expect(model.dispose).toHaveBeenCalledOnce();
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
    const editor = { dispose: vi.fn(), layout: vi.fn() };
    createEditor.mockReturnValue(editor);
    const client = {
      readText: vi.fn().mockResolvedValue({
        schema: 1,
        resource_id: snapshot().resource_id,
        revision: 4,
        text: "hello",
      }),
    } as unknown as FileResourceClient;
    const view = render(<MonacoTextRenderer {...props(client)} />);
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
    expect(editor.dispose).toHaveBeenCalledOnce();
    vi.unstubAllGlobals();
  });

  it("does not read active HTML or an unavailable text descriptor", () => {
    const client = { readText: vi.fn() } as unknown as FileResourceClient;
    const activeHtml = snapshot();
    activeHtml.descriptor.mime_type = "text/html";
    const first = render(<MonacoTextRenderer {...props(client)} snapshot={activeHtml} />);
    expect(client.readText).not.toHaveBeenCalled();
    first.unmount();
    const unavailable = snapshot();
    unavailable.descriptor.capabilities.preview = false;
    unavailable.descriptor.unavailable_reason = "monaco_size_limit_exceeded";
    render(<MonacoTextRenderer {...props(client)} snapshot={unavailable} />);
    expect(client.readText).not.toHaveBeenCalled();
  });

  it("accepts the registry's validated text MIME fallback", async () => {
    const fallback = snapshot();
    fallback.descriptor.renderer_kind = "unsupported";
    const client = { readText: vi.fn().mockResolvedValue({
      schema: 1, resource_id: fallback.resource_id, revision: fallback.revision, text: "fallback",
    }) } as unknown as FileResourceClient;
    render(<MonacoTextRenderer {...props(client)} snapshot={fallback} />);
    await waitFor(() => expect(client.readText).toHaveBeenCalledOnce());
  });

  it("follows live Wardian data-theme changes without recreating the editor", async () => {
    document.documentElement.setAttribute("data-theme", "dark");
    const client = {
      readText: vi.fn().mockResolvedValue({
        schema: 1, resource_id: snapshot().resource_id, revision: 4, text: "hello",
      }),
    } as unknown as FileResourceClient;
    const view = render(<MonacoTextRenderer {...props(client)} />);
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
