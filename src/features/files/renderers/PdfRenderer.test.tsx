import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { FileResourceSnapshotV1 } from "../../../types";
import type { FileResourceClient } from "../fileResourceClient";
import PdfRenderer from "./PdfRenderer";

const renderTasks: Array<{ promise: Promise<void>; cancel: ReturnType<typeof vi.fn> }> = [];
const getDocument = vi.fn();
const GlobalWorkerOptions: { workerPort: Worker | null } = { workerPort: null };

vi.mock("pdfjs-dist", () => ({ GlobalWorkerOptions, getDocument }));
vi.mock("pdfjs-dist/build/pdf.worker.min.mjs?worker", () => ({
  default: class TestPdfWorker {},
}));

function snapshot(revision = 3): FileResourceSnapshotV1 {
  return {
    resource_id: "file:C:/work/report.pdf",
    subscription_id: "subscription-pdf",
    revision,
    descriptor: {
      schema: 1,
      canonical_path: "C:/work/report.pdf",
      display_name: "report.pdf",
      extension: "pdf",
      mime_type: "application/pdf",
      encoding: null,
      renderer_kind: "pdf",
      size_bytes: 4096,
      line_count: null,
      content_hash: `hash-${revision}`,
      modified_at_ms: revision,
      capabilities: { preview: true, changes: false, draft: false, stream: true },
      unavailable_reason: null,
    },
  };
}

function props(client: FileResourceClient, revision = 3) {
  return {
    snapshot: snapshot(revision),
    client,
    lifecycle: { visible: true },
    on_open_file: vi.fn(),
    on_open_with: vi.fn(),
    on_reveal: vi.fn(),
  };
}

describe("PdfRenderer", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    renderTasks.length = 0;
    vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue({} as CanvasRenderingContext2D);
    const document = {
      numPages: 2,
      destroy: vi.fn().mockResolvedValue(undefined),
      getPage: vi.fn().mockImplementation(async (pageNumber: number) => ({
        getViewport: ({ scale }: { scale: number }) => ({ width: 400 * scale, height: 600 * scale }),
        getTextContent: vi.fn().mockResolvedValue({ items: [{ str: `Wardian page ${pageNumber}` }] }),
        render: vi.fn().mockImplementation(() => {
          const task = { promise: Promise.resolve(), cancel: vi.fn() };
          renderTasks.push(task);
          return task;
        }),
      })),
    };
    getDocument.mockReturnValue({ promise: Promise.resolve(document), destroy: vi.fn().mockResolvedValue(undefined) });
  });

  it("uses the bundled worker and a range-capable ticket with pages, search, and zoom", async () => {
    const client = {
      issueTicket: vi.fn().mockResolvedValue({
        schema: 1,
        ticket_id: "ticket-pdf",
        url: "wardian-resource://localhost/ticket-pdf",
        resource_id: snapshot().resource_id,
        revision: 3,
        renderer_lease_id: "pdf-lease",
        expires_at_ms: Date.now() + 60_000,
      }),
      closeRendererLease: vi.fn().mockResolvedValue(undefined),
    } as unknown as FileResourceClient;
    const view = render(<PdfRenderer {...props(client)} />);

    expect(await screen.findByText("Page 1")).toBeInTheDocument();
    expect(screen.getByText("Page 2")).toBeInTheDocument();
    expect(GlobalWorkerOptions.workerPort).not.toBeNull();
    expect(getDocument).toHaveBeenCalledWith(expect.objectContaining({
      disableRange: false,
      disableStream: false,
      rangeChunkSize: 65_536,
      url: "wardian-resource://localhost/ticket-pdf",
    }));
    fireEvent.change(screen.getByRole("searchbox", { name: "Search PDF" }), {
      target: { value: "Wardian" },
    });
    await waitFor(() => expect(screen.getByText("2 matches")).toBeInTheDocument());
    fireEvent.click(screen.getByRole("button", { name: "Zoom in" }));
    await waitFor(() => expect(renderTasks.length).toBeGreaterThan(2));
    view.unmount();
    await waitFor(() => expect(client.closeRendererLease).toHaveBeenCalledOnce());
    expect(renderTasks.some((task) => task.cancel.mock.calls.length > 0)).toBe(true);
  });

  it("does not issue or load a PDF that is unavailable or over its limit", () => {
    const client = {
      issueTicket: vi.fn(),
      closeRendererLease: vi.fn(),
    } as unknown as FileResourceClient;
    const invalid = snapshot();
    invalid.descriptor.size_bytes = 256 * 1024 * 1024 + 1;
    invalid.descriptor.capabilities.preview = false;
    invalid.descriptor.capabilities.stream = false;
    invalid.descriptor.unavailable_reason = "pdf_size_limit_exceeded";
    render(<PdfRenderer {...props(client)} snapshot={invalid} />);
    expect(screen.getByRole("status")).toHaveTextContent("pdf_size_limit_exceeded");
    expect(client.issueTicket).not.toHaveBeenCalled();
    expect(getDocument).not.toHaveBeenCalled();
  });

  it("accepts the registry's validated PDF MIME fallback", async () => {
    const fallback = snapshot();
    fallback.descriptor.renderer_kind = "unsupported";
    const client = {
      issueTicket: vi.fn().mockResolvedValue({
        schema: 1, ticket_id: "ticket", url: "wardian-resource://localhost/ticket",
        resource_id: fallback.resource_id, revision: 3, renderer_lease_id: "lease",
        expires_at_ms: Date.now() + 60_000,
      }),
      closeRendererLease: vi.fn().mockResolvedValue(undefined),
    } as unknown as FileResourceClient;
    render(<PdfRenderer {...props(client)} snapshot={fallback} />);
    expect(await screen.findByText("Page 1")).toBeInTheDocument();
  });

  it("destroys a loaded document and releases its lease on page failure", async () => {
    const destroyDocument = vi.fn().mockResolvedValue(undefined);
    getDocument.mockReturnValue({
      promise: Promise.resolve({
        numPages: 1,
        destroy: destroyDocument,
        getPage: vi.fn().mockResolvedValue({
          getViewport: ({ scale }: { scale: number }) => ({ width: 100 * scale, height: 100 * scale }),
          render: vi.fn().mockImplementation(() => { throw new Error("render failed"); }),
        }),
      }),
      destroy: vi.fn().mockResolvedValue(undefined),
    });
    const client = {
      issueTicket: vi.fn().mockResolvedValue({
        schema: 1, ticket_id: "ticket", url: "wardian-resource://localhost/ticket",
        resource_id: snapshot().resource_id, revision: 3, renderer_lease_id: "lease",
        expires_at_ms: Date.now() + 60_000,
      }),
      closeRendererLease: vi.fn().mockResolvedValue(undefined),
    } as unknown as FileResourceClient;
    render(<PdfRenderer {...props(client)} />);
    expect(await screen.findByRole("alert")).toHaveTextContent("render failed");
    await waitFor(() => expect(destroyDocument).toHaveBeenCalledOnce());
    await waitFor(() => expect(client.closeRendererLease).toHaveBeenCalledOnce());
  });

  it("releases a delayed ticket from a stale revision without loading it", async () => {
    let resolveFirst: ((value: object) => void) | undefined;
    const firstTicket = new Promise<object>((resolve) => { resolveFirst = resolve; });
    const issuedLeases: string[] = [];
    const client = {
      issueTicket: vi.fn().mockImplementation((_resource, revision, lease) => {
        issuedLeases.push(lease);
        if (revision === 3) return firstTicket;
        return Promise.resolve({
          schema: 1,
          ticket_id: "ticket-current",
          url: "wardian-resource://localhost/ticket-current",
          resource_id: snapshot().resource_id,
          revision,
          renderer_lease_id: lease,
          expires_at_ms: Date.now() + 60_000,
        });
      }),
      closeRendererLease: vi.fn().mockResolvedValue(undefined),
    } as unknown as FileResourceClient;
    const view = render(<PdfRenderer {...props(client)} />);
    await waitFor(() => expect(client.issueTicket).toHaveBeenCalledTimes(1));
    view.rerender(<PdfRenderer {...props(client, 4)} />);
    await waitFor(() => expect(client.issueTicket).toHaveBeenCalledTimes(2));
    resolveFirst?.({
      schema: 1,
      ticket_id: "ticket-stale",
      url: "wardian-resource://localhost/ticket-stale",
      resource_id: snapshot().resource_id,
      revision: 3,
      renderer_lease_id: issuedLeases[0],
      expires_at_ms: Date.now() + 60_000,
    });
    await waitFor(() => expect(client.closeRendererLease).toHaveBeenCalledWith(
      snapshot().resource_id,
      issuedLeases[0],
    ));
    expect(getDocument).not.toHaveBeenCalledWith(expect.objectContaining({
      url: "wardian-resource://localhost/ticket-stale",
    }));
  });

  it("renders only a bounded visible page window and cancels pages leaving it", async () => {
    let intersectionCallback: IntersectionObserverCallback | undefined;
    vi.stubGlobal("IntersectionObserver", class TestIntersectionObserver {
      constructor(callback: IntersectionObserverCallback) { intersectionCallback = callback; }
      observe() {}
      unobserve() {}
      disconnect() {}
      takeRecords() { return []; }
      root = null;
      rootMargin = "0px";
      thresholds = [0];
    });
    const getPage = vi.fn().mockImplementation(async (pageNumber: number) => ({
      getViewport: ({ scale }: { scale: number }) => ({ width: 400 * scale, height: 600 * scale }),
      getTextContent: vi.fn().mockResolvedValue({ items: [{ str: `page ${pageNumber}` }] }),
      render: vi.fn().mockImplementation(() => {
        const task = { promise: new Promise<void>(() => undefined), cancel: vi.fn() };
        renderTasks.push(task);
        return task;
      }),
    }));
    getDocument.mockReturnValue({
      promise: Promise.resolve({ numPages: 100, destroy: vi.fn(), getPage }),
      destroy: vi.fn().mockResolvedValue(undefined),
    });
    const client = {
      issueTicket: vi.fn().mockResolvedValue({
        schema: 1, ticket_id: "ticket", url: "wardian-resource://localhost/ticket",
        resource_id: snapshot().resource_id, revision: 3, renderer_lease_id: "lease",
        expires_at_ms: Date.now() + 60_000,
      }),
      closeRendererLease: vi.fn().mockResolvedValue(undefined),
    } as unknown as FileResourceClient;
    render(<PdfRenderer {...props(client)} />);

    await screen.findByText("Page 100");
    await waitFor(() => expect(getPage.mock.calls.length).toBeLessThanOrEqual(3));
    const page50 = screen.getByText("Page 50").closest("figure")!;
    intersectionCallback?.([{
      target: page50,
      isIntersecting: true,
      intersectionRatio: 1,
    } as unknown as IntersectionObserverEntry], {} as IntersectionObserver);
    await waitFor(() => expect(getPage).toHaveBeenCalledWith(50));
    expect(getPage.mock.calls.length).toBeLessThanOrEqual(6);
    expect(renderTasks.some((task) => task.cancel.mock.calls.length > 0)).toBe(true);
    vi.unstubAllGlobals();
  });

  it("caps malicious page geometry, zoom, and device pixel ratio before canvas allocation", async () => {
    vi.stubGlobal("devicePixelRatio", 16);
    const pageRender = vi.fn().mockReturnValue({ promise: Promise.resolve(), cancel: vi.fn() });
    const getPage = vi.fn().mockResolvedValue({
      getViewport: ({ scale }: { scale: number }) => ({ width: 1_000_000 * scale, height: 800_000 * scale }),
      getTextContent: vi.fn().mockResolvedValue({ items: [] }),
      render: pageRender,
    });
    getDocument.mockReturnValue({
      promise: Promise.resolve({ numPages: 1, destroy: vi.fn(), getPage }),
      destroy: vi.fn().mockResolvedValue(undefined),
    });
    const client = {
      issueTicket: vi.fn().mockResolvedValue({
        schema: 1, ticket_id: "ticket", url: "wardian-resource://localhost/ticket",
        resource_id: snapshot().resource_id, revision: 3, renderer_lease_id: "lease",
        expires_at_ms: Date.now() + 60_000,
      }),
      closeRendererLease: vi.fn().mockResolvedValue(undefined),
    } as unknown as FileResourceClient;
    render(<PdfRenderer {...props(client)} />);
    const canvas = await screen.findByLabelText("PDF page 1") as HTMLCanvasElement;
    await waitFor(() => expect(pageRender).toHaveBeenCalled());
    expect(canvas.width).toBeLessThanOrEqual(8192);
    expect(canvas.height).toBeLessThanOrEqual(8192);
    expect(canvas.width * canvas.height).toBeLessThanOrEqual(32_000_000);
    fireEvent.click(screen.getByRole("button", { name: "Zoom in" }));
    await waitFor(() => expect(pageRender).toHaveBeenCalledTimes(2));
    expect(canvas.width * canvas.height).toBeLessThanOrEqual(32_000_000);
    vi.unstubAllGlobals();
  });

  it("debounces search, stops stale generations between pages, and releases failed attempts", async () => {
    let rejectLoad: ((cause: Error) => void) | undefined;
    const destroy = vi.fn().mockResolvedValue(undefined);
    getDocument.mockReturnValueOnce({
      promise: new Promise((_resolve, reject) => { rejectLoad = reject; }),
      destroy,
    });
    const issueTicket = vi.fn().mockImplementation(async (
      _resource: string,
      revision: number,
      lease: string,
    ) => ({
        schema: 1, ticket_id: lease, url: `wardian-resource://localhost/${lease}`,
        resource_id: snapshot().resource_id, revision, renderer_lease_id: lease,
        expires_at_ms: Date.now() + 60_000,
      }));
    const client = {
      issueTicket,
      closeRendererLease: vi.fn().mockResolvedValue(undefined),
    } as unknown as FileResourceClient;
    render(<PdfRenderer {...props(client)} />);
    await waitFor(() => expect(getDocument).toHaveBeenCalledOnce());
    rejectLoad?.(new Error("bad pdf"));
    expect(await screen.findByRole("alert")).toHaveTextContent("bad pdf");
    await waitFor(() => expect(destroy).toHaveBeenCalled());
    await waitFor(() => expect(client.closeRendererLease).toHaveBeenCalledOnce());

    const pageResolvers: Array<() => void> = [];
    const searchGetPage = vi.fn().mockImplementation(async (pageNumber: number) => ({
      getViewport: ({ scale }: { scale: number }) => ({ width: 100 * scale, height: 100 * scale }),
      render: vi.fn().mockReturnValue({ promise: Promise.resolve(), cancel: vi.fn() }),
      getTextContent: () => new Promise((resolve) => pageResolvers.push(() => resolve({ items: [{ str: `page ${pageNumber}` }] }))),
    }));
    getDocument.mockReturnValueOnce({
      promise: Promise.resolve({ numPages: 20, destroy: vi.fn(), getPage: searchGetPage }),
      destroy: vi.fn().mockResolvedValue(undefined),
    });
    fireEvent.click(screen.getByRole("button", { name: "Retry" }));
    await screen.findByText("Page 20");
    const search = screen.getByRole("searchbox", { name: "Search PDF" });
    fireEvent.change(search, { target: { value: "first" } });
    await waitFor(() => expect(searchGetPage.mock.calls.length).toBeGreaterThan(2), { timeout: 1000 });
    expect(searchGetPage.mock.calls.length).toBeLessThanOrEqual(4);
    fireEvent.change(search, { target: { value: "second" } });
    pageResolvers.splice(0).forEach((resolve) => resolve());
    await new Promise((resolve) => setTimeout(resolve, 300));
    expect(searchGetPage.mock.calls.length).toBeLessThan(10);
    expect(new Set(issueTicket.mock.calls.map((call) => call[2])).size).toBe(2);
  });
});
