import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { PDFDocumentProxy, PDFPageProxy, RenderTask } from "pdfjs-dist";

import type { FileRendererProps } from "../rendererRegistry";
import { configurePdfWorker } from "./pdfWorker";

const PDF_MAX_SIZE_BYTES = 256 * 1024 * 1024;
const PDF_RANGE_CHUNK_BYTES = 65_536;
const PDF_RENDER_WINDOW = 1;
const PDF_SEARCH_DEBOUNCE_MS = 250;
const PDF_MAX_CSS_DIMENSION = 4_096;
const PDF_MAX_CSS_PIXELS = 16_000_000;
const PDF_MAX_CANVAS_DIMENSION = 8_192;
const PDF_MAX_CANVAS_PIXELS = 32_000_000;
const PDF_MAX_DPR = 2;
let pdfLeaseSequence = 0;

function nextLeaseId(resourceId: string, revision: number) {
  pdfLeaseSequence += 1;
  return `pdf:${resourceId}@${revision}:${pdfLeaseSequence}`;
}

function errorMessage(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}

function releaseLease(client: FileRendererProps["client"], resourceId: string, leaseId: string) {
  try {
    void client.closeRendererLease(resourceId, leaseId).catch(() => undefined);
  } catch {
    // Subscription teardown is the final capability revocation boundary.
  }
}

function ignoreCleanup(cleanup: (() => unknown) | undefined) {
  if (!cleanup) return;
  try {
    void Promise.resolve(cleanup()).catch(() => undefined);
  } catch {
    // Cleanup must never escape the resource-local renderer boundary.
  }
}

function renderWindow(center: number, pageCount: number) {
  const pages = new Set<number>();
  for (
    let page = Math.max(1, center - PDF_RENDER_WINDOW);
    page <= Math.min(pageCount, center + PDF_RENDER_WINDOW);
    page += 1
  ) pages.add(page);
  return pages;
}

function boundedPageGeometry(page: PDFPageProxy, requestedScale: number) {
  const base = page.getViewport({ scale: 1 });
  if (
    !Number.isFinite(base.width)
    || !Number.isFinite(base.height)
    || base.width <= 0
    || base.height <= 0
  ) throw new Error("PDF page geometry is invalid");
  const dimensionScale = PDF_MAX_CSS_DIMENSION / Math.max(base.width, base.height);
  const areaScale = Math.sqrt(PDF_MAX_CSS_PIXELS / (base.width * base.height));
  const safeScale = Math.min(requestedScale, dimensionScale, areaScale);
  if (!Number.isFinite(safeScale) || safeScale <= 0) {
    throw new Error("PDF page geometry exceeds safe preview limits");
  }
  const viewport = page.getViewport({ scale: safeScale });
  const cssWidth = Math.max(1, Math.min(PDF_MAX_CSS_DIMENSION, Math.floor(viewport.width)));
  const cssHeight = Math.max(1, Math.min(PDF_MAX_CSS_DIMENSION, Math.floor(viewport.height)));
  const requestedDpr = Math.max(1, Math.min(PDF_MAX_DPR, globalThis.devicePixelRatio || 1));
  const dimensionDpr = PDF_MAX_CANVAS_DIMENSION / Math.max(cssWidth, cssHeight);
  const areaDpr = Math.sqrt(PDF_MAX_CANVAS_PIXELS / (cssWidth * cssHeight));
  const outputScale = Math.max(0.01, Math.min(requestedDpr, dimensionDpr, areaDpr));
  return { viewport, cssWidth, cssHeight, outputScale };
}

function PdfPage({ document, pageNumber, scale, active, onFatalError }: {
  document: PDFDocumentProxy;
  pageNumber: number;
  scale: number;
  active: boolean;
  onFatalError: (cause: unknown) => void;
}) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!active) return;
    let cancelled = false;
    let renderTask: RenderTask | null = null;
    setError(null);
    void document.getPage(pageNumber).then((page) => {
      if (cancelled) return;
      const canvas = canvasRef.current;
      const context = canvas?.getContext("2d");
      if (!canvas || !context) throw new Error("Canvas rendering is unavailable");
      const geometry = boundedPageGeometry(page, scale);
      canvas.width = Math.max(1, Math.floor(geometry.cssWidth * geometry.outputScale));
      canvas.height = Math.max(1, Math.floor(geometry.cssHeight * geometry.outputScale));
      canvas.style.width = `${geometry.cssWidth}px`;
      canvas.style.height = `${geometry.cssHeight}px`;
      renderTask = page.render({
        canvas,
        canvasContext: context,
        transform: geometry.outputScale === 1
          ? undefined
          : [geometry.outputScale, 0, 0, geometry.outputScale, 0, 0],
        viewport: geometry.viewport,
      });
      return renderTask.promise;
    }).catch((cause) => {
      if (!cancelled && (cause as { name?: string }).name !== "RenderingCancelledException") {
        setError(errorMessage(cause));
        onFatalError(cause);
      }
    });
    return () => {
      cancelled = true;
      renderTask?.cancel();
    };
  }, [active, document, onFatalError, pageNumber, scale]);

  return (
    <figure className="files-pdf-page" data-page-number={pageNumber}>
      <figcaption>Page {pageNumber}</figcaption>
      {error
        ? <div role="alert">{error}</div>
        : active
          ? <canvas ref={canvasRef} aria-label={`PDF page ${pageNumber}`} />
          : <div className="files-pdf-page-placeholder" aria-hidden="true" />}
    </figure>
  );
}

export default function PdfRenderer({ snapshot, client, lifecycle }: FileRendererProps) {
  const viewportRef = useRef<HTMLDivElement>(null);
  const disposeAttemptRef = useRef<() => void>(() => undefined);
  const [document, setDocument] = useState<PDFDocumentProxy | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [retryToken, setRetryToken] = useState(0);
  const [scale, setScale] = useState(1);
  const [query, setQuery] = useState("");
  const [matches, setMatches] = useState<number | null>(null);
  const [activePages, setActivePages] = useState<ReadonlySet<number>>(() => new Set([1, 2]));
  const descriptor = snapshot.descriptor;
  const mime = descriptor.mime_type.trim().toLowerCase();
  const allowed = (descriptor.renderer_kind === "pdf" || mime === "application/pdf")
    && descriptor.capabilities.preview
    && descriptor.capabilities.stream
    && descriptor.unavailable_reason === null
    && descriptor.size_bytes <= PDF_MAX_SIZE_BYTES;
  const failPdf = useCallback((cause: unknown) => {
    disposeAttemptRef.current();
    setDocument(null);
    setError(errorMessage(cause));
  }, []);

  useEffect(() => {
    if (!lifecycle.visible || !allowed) return;
    const leaseId = nextLeaseId(snapshot.resource_id, snapshot.revision);
    let cancelled = false;
    let issued = false;
    let released = false;
    let loadingTask: ReturnType<typeof import("pdfjs-dist")["getDocument"]> | null = null;
    let loadedDocument: PDFDocumentProxy | null = null;
    let disposed = false;
    const release = () => {
      if (!issued || released) return;
      released = true;
      releaseLease(client, snapshot.resource_id, leaseId);
    };
    const disposeAttempt = () => {
      if (!disposed) {
        disposed = true;
        if (loadedDocument) ignoreCleanup(() => loadedDocument?.destroy());
        else if (loadingTask) ignoreCleanup(() => loadingTask?.destroy());
      }
      release();
    };
    disposeAttemptRef.current = disposeAttempt;
    setDocument(null);
    setError(null);
    setMatches(null);
    setActivePages(new Set([1, 2]));

    void (async () => {
      try {
        const ticket = await client.issueTicket(snapshot.resource_id, snapshot.revision, leaseId);
        issued = true;
        if (cancelled || ticket.revision !== snapshot.revision) {
          disposeAttempt();
          return;
        }
        const pdfjs = await import("pdfjs-dist");
        if (cancelled) {
          disposeAttempt();
          return;
        }
        configurePdfWorker(pdfjs);
        loadingTask = pdfjs.getDocument({
          disableAutoFetch: false,
          disableRange: false,
          disableStream: false,
          rangeChunkSize: PDF_RANGE_CHUNK_BYTES,
          url: ticket.url,
        });
        loadedDocument = await loadingTask.promise;
        if (cancelled) {
          disposeAttempt();
          return;
        }
        setDocument(loadedDocument);
      } catch (cause) {
        disposeAttempt();
        if (!cancelled) setError(errorMessage(cause));
      }
    })();
    return () => {
      cancelled = true;
      disposeAttempt();
      if (disposeAttemptRef.current === disposeAttempt) {
        disposeAttemptRef.current = () => undefined;
      }
    };
  }, [allowed, client, lifecycle.visible, retryToken, snapshot.resource_id, snapshot.revision]);

  useEffect(() => {
    const viewport = viewportRef.current;
    if (!document || !viewport || typeof IntersectionObserver === "undefined") return;
    const observer = new IntersectionObserver((entries) => {
      const firstVisible = entries
        .filter((entry) => entry.isIntersecting)
        .map((entry) => Number((entry.target as HTMLElement).dataset.pageNumber))
        .filter(Number.isFinite)
        .sort((left, right) => left - right)[0];
      if (firstVisible) setActivePages(renderWindow(firstVisible, document.numPages));
    }, { root: viewport, rootMargin: "75% 0px", threshold: 0.01 });
    viewport.querySelectorAll<HTMLElement>("[data-page-number]").forEach((page) => observer.observe(page));
    return () => observer.disconnect();
  }, [document]);

  useEffect(() => {
    const normalizedQuery = query.trim();
    if (!document || !normalizedQuery) {
      setMatches(normalizedQuery ? 0 : null);
      return;
    }
    let cancelled = false;
    const timer = globalThis.setTimeout(() => {
      void (async () => {
        try {
          const needle = normalizedQuery.toLocaleLowerCase();
          let count = 0;
          for (let pageNumber = 1; pageNumber <= document.numPages; pageNumber += 1) {
            if (cancelled) return;
            const page = await document.getPage(pageNumber);
            if (cancelled) return;
            const content = await page.getTextContent();
            if (cancelled) return;
            const text = content.items
              .map((item) => "str" in item ? item.str : "")
              .join(" ")
              .toLocaleLowerCase();
            let from = 0;
            while ((from = text.indexOf(needle, from)) >= 0) {
              count += 1;
              from += Math.max(1, needle.length);
            }
          }
          if (!cancelled) setMatches(count);
        } catch (cause) {
          if (!cancelled) failPdf(cause);
        }
      })();
    }, PDF_SEARCH_DEBOUNCE_MS);
    return () => {
      cancelled = true;
      globalThis.clearTimeout(timer);
    };
  }, [document, failPdf, query]);

  const retry = useCallback(() => setRetryToken((value) => value + 1), []);
  const pages = useMemo(
    () => document ? Array.from({ length: document.numPages }, (_, index) => index + 1) : [],
    [document],
  );
  if (!lifecycle.visible) {
    return <div className="files-resource-state" role="status">PDF preview suspended.</div>;
  }
  if (!allowed) {
    return <div className="files-resource-state" role="status">{descriptor.unavailable_reason ?? "pdf_preview_unavailable"}</div>;
  }
  if (error) {
    return (
      <section className="files-resource-state" role="alert">
        <h2>PDF preview unavailable</h2>
        <p>{error}</p>
        <button type="button" onClick={retry}>Retry</button>
      </section>
    );
  }
  return (
    <section className="files-binary-renderer" aria-label="PDF preview">
      <div className="files-renderer-toolbar" role="toolbar" aria-label="PDF controls">
        <label>
          <span className="files-visually-hidden">Search PDF</span>
          <input aria-label="Search PDF" type="search" value={query} onChange={(event) => setQuery(event.target.value)} />
        </label>
        {matches !== null ? <output aria-live="polite">{matches} {matches === 1 ? "match" : "matches"}</output> : null}
        <button type="button" aria-label="Zoom out" onClick={() => setScale((value) => Math.max(0.5, value - 0.25))}>−</button>
        <output aria-label="PDF zoom">{Math.round(scale * 100)}%</output>
        <button type="button" aria-label="Zoom in" onClick={() => setScale((value) => Math.min(4, value + 0.25))}>+</button>
      </div>
      <div ref={viewportRef} className="files-pdf-viewport">
        {document
          ? pages.map((pageNumber) => (
            <PdfPage
              key={pageNumber}
              document={document}
              pageNumber={pageNumber}
              scale={scale}
              active={activePages.has(pageNumber)}
              onFatalError={failPdf}
            />
          ))
          : <div className="files-resource-state" role="status">Loading PDF…</div>}
      </div>
    </section>
  );
}
