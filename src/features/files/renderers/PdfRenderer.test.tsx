import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

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

function snapshot(revision = 3, subscriptionId = "subscription-pdf"): FileResourceSnapshotV1 {
  return {
    resource_id: "file:C:/work/report.pdf",
    subscription_id: subscriptionId,
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

function props(client: FileResourceClient, revision = 3, subscriptionId = "subscription-pdf") {
  return {
    snapshot: snapshot(revision, subscriptionId),
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

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("uses the bundled worker and a range-capable ticket with pages, search, and zoom", async () => {
    const client = {
      issueTicket: vi.fn().mockResolvedValue({
        schema: 1,
        ticket_id: "ticket-pdf",
        url: "http://wardian-resource.localhost/ticket-pdf",
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
      url: "http://wardian-resource.localhost/ticket-pdf",
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

  it("rolls the renderer lease when the subscription changes at the same revision", async () => {
    const issuedLeases: string[] = [];
    const client = {
      issueTicket: vi.fn().mockImplementation(async (
        owner: FileResourceSnapshotV1,
        lease: string,
      ) => {
        issuedLeases.push(lease);
        return {
          schema: 1,
          ticket_id: `${owner.subscription_id}:${lease}`,
          url: `wardian-resource://localhost/${owner.subscription_id}`,
          resource_id: owner.resource_id,
          revision: owner.revision,
          renderer_lease_id: lease,
          expires_at_ms: Date.now() + 60_000,
        };
      }),
      closeRendererLease: vi.fn().mockResolvedValue(undefined),
    } as unknown as FileResourceClient;
    const firstSnapshot = snapshot(3, "subscription-a");
    const secondSnapshot = snapshot(3, "subscription-b");
    const view = render(<PdfRenderer {...props(client, 3, "subscription-a")} />);

    await screen.findByText("Page 1");
    view.rerender(<PdfRenderer {...props(client, 3, "subscription-b")} />);

    await waitFor(() => expect(client.issueTicket).toHaveBeenCalledTimes(2));
    expect(client.closeRendererLease).toHaveBeenCalledWith(firstSnapshot, issuedLeases[0]);
    expect(client.issueTicket).toHaveBeenLastCalledWith(secondSnapshot, issuedLeases[1]);
    view.unmount();
    await waitFor(() => expect(client.closeRendererLease).toHaveBeenCalledWith(
      secondSnapshot,
      issuedLeases[1],
    ));
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
      issueTicket: vi.fn().mockImplementation((owner: FileResourceSnapshotV1, lease: string) => {
        issuedLeases.push(lease);
        if (owner.revision === 3) return firstTicket;
        return Promise.resolve({
          schema: 1,
          ticket_id: "ticket-current",
          url: "wardian-resource://localhost/ticket-current",
          resource_id: snapshot().resource_id,
          revision: owner.revision,
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
      expect.objectContaining({
        resource_id: snapshot().resource_id,
        subscription_id: snapshot().subscription_id,
        revision: 3,
      }),
      issuedLeases[0],
    ));
    expect(getDocument).not.toHaveBeenCalledWith(expect.objectContaining({
      url: "wardian-resource://localhost/ticket-stale",
    }));
  });

  it("mounts a fixed page window for huge documents and changes it on scroll", async () => {
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
      promise: Promise.resolve({ numPages: 1_000_000, destroy: vi.fn(), getPage }),
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

    await screen.findByText("Page 1");
    expect(screen.getAllByRole("figure").length).toBeLessThanOrEqual(3);
    expect(document.querySelectorAll("[data-page-number]").length).toBeLessThanOrEqual(3);
    await waitFor(() => expect(getPage.mock.calls.length).toBeLessThanOrEqual(3));
    const viewport = document.querySelector<HTMLElement>(".files-pdf-viewport")!;
    Object.defineProperty(viewport, "clientHeight", { value: 800, configurable: true });
    Object.defineProperty(viewport, "scrollHeight", { value: 16_000_000, configurable: true });
    viewport.scrollTop = 0;
    fireEvent.scroll(viewport);
    await waitFor(() => expect(screen.getByText("Page 1")).toBeInTheDocument());
    expect(screen.getAllByRole("figure").length).toBeLessThanOrEqual(3);

    let callsBeforeWindow = getPage.mock.calls.length;
    viewport.scrollTop = 8_000_000;
    fireEvent.scroll(viewport);
    await waitFor(() => expect(getPage).toHaveBeenCalledWith(expect.any(Number)));
    const visiblePages = screen.getAllByText(/^Page /).map((node) => Number(node.textContent?.replace("Page ", "")));
    expect(visiblePages.some((page) => page > 400_000 && page < 600_000)).toBe(true);
    expect(screen.getAllByRole("figure").length).toBeLessThanOrEqual(3);
    expect(getPage.mock.calls.length - callsBeforeWindow).toBeLessThanOrEqual(3);
    const pageTops = screen.getAllByRole("figure")
      .map((figure) => Number((figure as HTMLElement).style.top.replace("px", "")))
      .sort((left, right) => left - right);
    expect(pageTops[1]! - pageTops[0]!).toBeGreaterThan(500);

    callsBeforeWindow = getPage.mock.calls.length;
    viewport.scrollTop = 16_000_000 - 800;
    fireEvent.scroll(viewport);
    await waitFor(() => expect(screen.getByText("Page 1000000")).toBeInTheDocument());
    expect(screen.getAllByRole("figure").length).toBeLessThanOrEqual(3);
    expect(getPage.mock.calls.length - callsBeforeWindow).toBeLessThanOrEqual(3);

    callsBeforeWindow = getPage.mock.calls.length;
    viewport.scrollTop = 0;
    fireEvent.scroll(viewport);
    await waitFor(() => expect(screen.getByText("Page 1")).toBeInTheDocument());
    expect(screen.getAllByRole("figure").length).toBeLessThanOrEqual(3);
    expect(getPage.mock.calls.length - callsBeforeWindow).toBeLessThanOrEqual(3);
    expect(renderTasks.some((task) => task.cancel.mock.calls.length > 0)).toBe(true);
  });

  it("bounds search work and reports partial results for million-page documents", async () => {
    const getTextContent = vi.fn().mockResolvedValue({ items: [{ str: "needle" }] });
    const getPage = vi.fn().mockImplementation(async () => ({
      getViewport: ({ scale }: { scale: number }) => ({ width: 400 * scale, height: 600 * scale }),
      getTextContent,
      render: vi.fn().mockReturnValue({ promise: Promise.resolve(), cancel: vi.fn() }),
    }));
    getDocument.mockReturnValue({
      promise: Promise.resolve({ numPages: 1_000_000, destroy: vi.fn(), getPage }),
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

    await screen.findByText("Page 1");
    fireEvent.change(screen.getByRole("searchbox", { name: "Search PDF" }), {
      target: { value: "needle" },
    });
    expect(await screen.findByText(
      "128 matches in 128 of 1000000 pages (search limited)",
      {},
      { timeout: 2_000 },
    )).toBeInTheDocument();
    expect(getTextContent).toHaveBeenCalledTimes(128);
    expect(screen.getAllByRole("figure")).toHaveLength(2);
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
  });

  it("keeps a later-page anchor mounted while zooming to 400% and back down", async () => {
    const getPage = vi.fn().mockImplementation(async (pageNumber: number) => ({
      getViewport: ({ scale }: { scale: number }) => ({ width: 400 * scale, height: 600 * scale }),
      getTextContent: vi.fn().mockResolvedValue({ items: [{ str: `page ${pageNumber}` }] }),
      render: vi.fn().mockImplementation(() => {
        const task = { promise: Promise.resolve(), cancel: vi.fn() };
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
        schema: 1, ticket_id: "ticket", url: "http://wardian-resource.localhost/ticket",
        resource_id: snapshot().resource_id, revision: 3, renderer_lease_id: "lease",
        expires_at_ms: Date.now() + 60_000,
      }),
      closeRendererLease: vi.fn().mockResolvedValue(undefined),
    } as unknown as FileResourceClient;
    render(<PdfRenderer {...props(client)} />);

    await screen.findByText("Page 1");
    const viewport = screen.getByRole("region", { name: "PDF document viewport" });
    expect(viewport).toHaveAttribute("tabindex", "0");
    Object.defineProperty(viewport, "clientHeight", { value: 500, configurable: true });
    viewport.scrollTop = 39_000;
    fireEvent.scroll(viewport);
    await waitFor(() => {
      const pageNumbers = screen.getAllByRole("figure")
        .map((figure) => Number(figure.getAttribute("data-page-number")));
      expect(pageNumbers.some((page) => page >= 59 && page <= 62)).toBe(true);
    });
    const anchoredPage = Number(screen.getAllByRole("figure")[1]?.getAttribute("data-page-number"));

    for (let step = 0; step < 12; step += 1) {
      fireEvent.click(screen.getByRole("button", { name: "Zoom in" }));
    }
    await waitFor(() => expect(screen.getByLabelText("PDF zoom")).toHaveTextContent("400%"));
    const zoomedHeight = Number((document.querySelector(".files-pdf-virtual-spacer") as HTMLElement)
      .style.height.replace("px", ""));
    await waitFor(() => {
      const pages = screen.getAllByRole("figure")
        .map((figure) => Number(figure.getAttribute("data-page-number")));
      expect(pages.some((page) => Math.abs(page - anchoredPage) <= 1)).toBe(true);
      expect(screen.getAllByLabelText(/^PDF page /)).not.toHaveLength(0);
    });

    for (let step = 0; step < 14; step += 1) {
      fireEvent.click(screen.getByRole("button", { name: "Zoom out" }));
    }
    await waitFor(() => expect(screen.getByLabelText("PDF zoom")).toHaveTextContent("50%"));
    await waitFor(() => {
      const pages = screen.getAllByRole("figure")
        .map((figure) => Number(figure.getAttribute("data-page-number")));
      expect(pages.some((page) => Math.abs(page - anchoredPage) <= 1)).toBe(true);
      expect(screen.getAllByRole("figure").every((figure) => (
        Number.isFinite(Number((figure as HTMLElement).style.top.replace("px", "")))
      ))).toBe(true);
    });
    const zoomedOutHeight = Number((document.querySelector(".files-pdf-virtual-spacer") as HTMLElement)
      .style.height.replace("px", ""));
    expect(zoomedOutHeight).toBeLessThan(zoomedHeight);
    expect(renderTasks.some((task) => task.cancel.mock.calls.length > 0)).toBe(true);
  });

  it("uses per-page measurements to place short, tall, and short pages without overlap", async () => {
    const pageResolvers = new Map<number, (page: object) => void>();
    const getPage = vi.fn().mockImplementation((pageNumber: number) => new Promise((resolvePage) => {
      pageResolvers.set(pageNumber, resolvePage);
    }));
    getDocument.mockReturnValue({
      promise: Promise.resolve({ numPages: 3, destroy: vi.fn(), getPage }),
      destroy: vi.fn().mockResolvedValue(undefined),
    });
    const client = {
      issueTicket: vi.fn().mockResolvedValue({
        schema: 1, ticket_id: "ticket", url: "http://wardian-resource.localhost/ticket",
        resource_id: snapshot().resource_id, revision: 3, renderer_lease_id: "lease",
        expires_at_ms: Date.now() + 60_000,
      }),
      closeRendererLease: vi.fn().mockResolvedValue(undefined),
    } as unknown as FileResourceClient;
    render(<PdfRenderer {...props(client)} />);

    await screen.findByText("Page 1");
    const viewport = screen.getByRole("region", { name: "PDF document viewport" });
    Object.defineProperty(viewport, "clientHeight", { value: 2_000, configurable: true });
    fireEvent.scroll(viewport);
    await waitFor(() => expect(
      pageResolvers.has(1) && pageResolvers.has(2) && pageResolvers.has(3),
    ).toBe(true));
    const page = (height: number) => ({
      getViewport: ({ scale }: { scale: number }) => ({ width: 400 * scale, height: height * scale }),
      getTextContent: vi.fn().mockResolvedValue({ items: [] }),
      render: vi.fn().mockReturnValue({ promise: Promise.resolve(), cancel: vi.fn() }),
    });
    pageResolvers.get(2)?.(page(900));
    pageResolvers.get(3)?.(page(200));
    pageResolvers.get(1)?.(page(200));
    await waitFor(() => expect(screen.getAllByLabelText(/^PDF page /)).toHaveLength(3));

    const figures = [1, 2, 3].map((pageNumber) => (
      document.querySelector<HTMLElement>(`[data-page-number="${pageNumber}"]`)!
    ));
    const tops = figures.map((figure) => Number(figure.style.top.replace("px", "")));
    expect(tops[1]! - tops[0]!).toBe(248);
    expect(tops[2]! - tops[1]!).toBe(948);
    expect(tops[0]).toBeGreaterThanOrEqual(0);
    expect(tops[2]! + 232).toBeLessThanOrEqual(Number(
      (document.querySelector(".files-pdf-virtual-spacer") as HTMLElement).style.height
        .replace("px", ""),
    ));
  });

  it("tracks pane resize and keeps toolbar controls contained at narrow widths", async () => {
    let resize: (() => void) | undefined;
    const disconnect = vi.fn();
    vi.stubGlobal("ResizeObserver", class {
      constructor(callback: () => void) { resize = callback; }
      observe() { /* test drives the observer explicitly */ }
      disconnect() { disconnect(); }
    });
    const getPage = vi.fn().mockImplementation(async (pageNumber: number) => ({
      getViewport: ({ scale }: { scale: number }) => ({ width: 400 * scale, height: 600 * scale }),
      getTextContent: vi.fn().mockResolvedValue({ items: [{ str: `page ${pageNumber}` }] }),
      render: vi.fn().mockReturnValue({ promise: Promise.resolve(), cancel: vi.fn() }),
    }));
    getDocument.mockReturnValue({
      promise: Promise.resolve({ numPages: 20, destroy: vi.fn(), getPage }),
      destroy: vi.fn().mockResolvedValue(undefined),
    });
    const client = {
      issueTicket: vi.fn().mockResolvedValue({
        schema: 1, ticket_id: "ticket", url: "http://wardian-resource.localhost/ticket",
        resource_id: snapshot().resource_id, revision: 3, renderer_lease_id: "lease",
        expires_at_ms: Date.now() + 60_000,
      }),
      closeRendererLease: vi.fn().mockResolvedValue(undefined),
    } as unknown as FileResourceClient;
    const view = render(<PdfRenderer {...props(client)} />);
    await screen.findByText("Page 1");
    const viewport = screen.getByRole("region", { name: "PDF document viewport" });
    Object.defineProperty(viewport, "clientHeight", { value: 2_000, configurable: true });
    resize?.();
    await waitFor(() => expect(screen.getAllByRole("figure").length).toBeGreaterThan(2));
    expect(viewport.dispatchEvent(new KeyboardEvent("keydown", {
      key: "PageDown", bubbles: true, cancelable: true,
    }))).toBe(true);
    expect(screen.getByRole("toolbar", { name: "PDF controls" })).toHaveClass("files-pdf-toolbar");
    expect(screen.getByRole("searchbox", { name: "Search PDF" }).parentElement)
      .toHaveClass("files-pdf-search");

    const css = readFileSync(resolve(process.cwd(), "src/features/files/FilesSurface.css"), "utf8");
    expect(css).toMatch(/\.files-pdf-toolbar\s*\{[^}]*flex-wrap:\s*wrap/s);
    expect(css).toMatch(/\.files-pdf-search\s*\{[^}]*min-width:\s*0[^}]*flex:/s);
    expect(css).toMatch(/@container \(max-width: 440px\)[\s\S]*\.files-pdf-search\s*\{[^}]*flex-basis:\s*100%/);
    view.unmount();
    expect(disconnect).toHaveBeenCalledOnce();
  });

  it("debounces search, stops stale generations between pages, and releases failed attempts", async () => {
    let rejectLoad: ((cause: Error) => void) | undefined;
    const destroy = vi.fn().mockResolvedValue(undefined);
    getDocument.mockReturnValueOnce({
      promise: new Promise((_resolve, reject) => { rejectLoad = reject; }),
      destroy,
    });
    const issueTicket = vi.fn().mockImplementation(async (
      owner: FileResourceSnapshotV1,
      lease: string,
    ) => ({
        schema: 1, ticket_id: lease, url: `wardian-resource://localhost/${lease}`,
        resource_id: owner.resource_id, revision: owner.revision, renderer_lease_id: lease,
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
    await screen.findByText("Page 1");
    const search = screen.getByRole("searchbox", { name: "Search PDF" });
    fireEvent.change(search, { target: { value: "first" } });
    await waitFor(() => expect(searchGetPage.mock.calls.length).toBeGreaterThan(2), { timeout: 1000 });
    expect(searchGetPage.mock.calls.length).toBeLessThanOrEqual(4);
    fireEvent.change(search, { target: { value: "second" } });
    pageResolvers.splice(0).forEach((resolve) => resolve());
    await new Promise((resolve) => setTimeout(resolve, 300));
    expect(searchGetPage.mock.calls.length).toBeLessThan(10);
    expect(new Set(issueTicket.mock.calls.map((call) => call[1])).size).toBe(2);
  });
});
