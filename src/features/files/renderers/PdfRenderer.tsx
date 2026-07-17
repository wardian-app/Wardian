import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { PDFDocumentProxy, PDFPageProxy, RenderTask } from "pdfjs-dist";

import type { FileRendererProps } from "../rendererRegistry";
import { configurePdfWorker } from "./pdfWorker";

const PDF_MAX_SIZE_BYTES = 256 * 1024 * 1024;
const PDF_RANGE_CHUNK_BYTES = 65_536;
const PDF_RENDER_WINDOW = 1;
const PDF_SEARCH_DEBOUNCE_MS = 250;
const PDF_SEARCH_PAGE_BUDGET = 128;
const PDF_SEARCH_TIME_BUDGET_MS = 2_000;
const PDF_MAX_CSS_DIMENSION = 4_096;
const PDF_MAX_CSS_PIXELS = 16_000_000;
const PDF_MAX_CANVAS_DIMENSION = 8_192;
const PDF_MAX_CANVAS_PIXELS = 32_000_000;
const PDF_MAX_DPR = 2;
const PDF_ESTIMATED_PAGE_HEIGHT = 960;
const PDF_PAGE_GAP = 16;
const PDF_MAX_VIRTUAL_HEIGHT = 16_000_000;
let pdfLeaseSequence = 0;

type PdfSearchResult = {
  matches: number;
  searched_pages: number;
  total_pages: number;
  partial: boolean;
};

function nextLeaseId(resourceId: string, revision: number) {
  pdfLeaseSequence += 1;
  return `pdf:${resourceId}@${revision}:${pdfLeaseSequence}`;
}

function errorMessage(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}

function releaseLease(
  client: FileRendererProps["client"],
  snapshot: FileRendererProps["snapshot"],
  leaseId: string,
) {
  try {
    void client.closeRendererLease(snapshot, leaseId).catch(() => undefined);
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

function virtualHeight(pageCount: number, pageHeight: number) {
  const requested = pageCount * (pageHeight + PDF_PAGE_GAP);
  return Math.max(pageHeight, Math.min(PDF_MAX_VIRTUAL_HEIGHT, requested));
}

function virtualPageTop(pageNumber: number, pageCount: number, totalHeight: number, pageHeight: number) {
  if (pageCount <= 1) return 0;
  return ((pageNumber - 1) / (pageCount - 1)) * Math.max(0, totalHeight - pageHeight);
}

function virtualWindowPageTop(
  pageNumber: number,
  centerPage: number,
  pageCount: number,
  totalHeight: number,
  pageHeight: number,
) {
  const anchor = virtualPageTop(centerPage, pageCount, totalHeight, pageHeight);
  const requested = anchor + (pageNumber - centerPage) * (pageHeight + PDF_PAGE_GAP);
  return Math.max(0, Math.min(totalHeight - pageHeight, requested));
}

function pageAtScroll(scrollTop: number, clientHeight: number, pageCount: number, totalHeight: number) {
  if (pageCount <= 1) return 1;
  const maximumScroll = Math.max(1, totalHeight - clientHeight);
  const clampedScroll = Math.max(0, Math.min(maximumScroll, scrollTop));
  const fraction = clampedScroll / maximumScroll;
  return Math.max(1, Math.min(pageCount, Math.round(fraction * (pageCount - 1)) + 1));
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

function PdfPage({ document, pageNumber, scale, onFatalError, onMeasuredHeight, top }: {
  document: PDFDocumentProxy;
  pageNumber: number;
  scale: number;
  onFatalError: (cause: unknown) => void;
  onMeasuredHeight: (height: number) => void;
  top: number;
}) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
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
      onMeasuredHeight(geometry.cssHeight + 32);
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
  }, [document, onFatalError, onMeasuredHeight, pageNumber, scale]);

  return (
    <figure className="files-pdf-page" data-page-number={pageNumber} style={{ top: `${Math.round(top)}px` }}>
      <figcaption>Page {pageNumber}</figcaption>
      {error ? <div role="alert">{error}</div> : <canvas ref={canvasRef} aria-label={`PDF page ${pageNumber}`} />}
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
  const [searchResult, setSearchResult] = useState<PdfSearchResult | null>(null);
  const [searching, setSearching] = useState(false);
  const [centerPage, setCenterPage] = useState(1);
  const [estimatedPageHeight, setEstimatedPageHeight] = useState(PDF_ESTIMATED_PAGE_HEIGHT);
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
      releaseLease(client, snapshot, leaseId);
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
    setSearchResult(null);
    setSearching(false);
    setCenterPage(1);
    setEstimatedPageHeight(PDF_ESTIMATED_PAGE_HEIGHT);

    void (async () => {
      try {
        const ticket = await client.issueTicket(snapshot, leaseId);
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
    const normalizedQuery = query.trim();
    if (!document || !normalizedQuery) {
      setSearchResult(normalizedQuery && document ? {
        matches: 0,
        searched_pages: 0,
        total_pages: document.numPages,
        partial: document.numPages > 0,
      } : null);
      setSearching(false);
      return;
    }
    let cancelled = false;
    setSearchResult(null);
    setSearching(true);
    const timer = globalThis.setTimeout(() => {
      void (async () => {
        try {
          const needle = normalizedQuery.toLocaleLowerCase();
          let count = 0;
          let searchedPages = 0;
          const startedAt = performance.now();
          const pageLimit = Math.min(document.numPages, PDF_SEARCH_PAGE_BUDGET);
          for (let pageNumber = 1; pageNumber <= pageLimit; pageNumber += 1) {
            if (cancelled) return;
            if (performance.now() - startedAt >= PDF_SEARCH_TIME_BUDGET_MS) break;
            const page = await document.getPage(pageNumber);
            if (cancelled) return;
            const content = await page.getTextContent();
            if (cancelled) return;
            searchedPages += 1;
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
          if (!cancelled) {
            setSearchResult({
              matches: count,
              searched_pages: searchedPages,
              total_pages: document.numPages,
              partial: searchedPages < document.numPages,
            });
            setSearching(false);
          }
        } catch (cause) {
          if (!cancelled) {
            setSearching(false);
            failPdf(cause);
          }
        }
      })();
    }, PDF_SEARCH_DEBOUNCE_MS);
    return () => {
      cancelled = true;
      globalThis.clearTimeout(timer);
    };
  }, [document, failPdf, query]);

  const retry = useCallback(() => setRetryToken((value) => value + 1), []);
  const totalVirtualHeight = document
    ? virtualHeight(document.numPages, estimatedPageHeight)
    : estimatedPageHeight;
  const pages = useMemo(
    () => document
      ? [...renderWindow(centerPage, document.numPages)].sort((left, right) => left - right)
      : [],
    [centerPage, document],
  );
  const onPdfScroll = useCallback(() => {
    const viewport = viewportRef.current;
    if (!viewport || !document) return;
    const centerPage = pageAtScroll(
      viewport.scrollTop,
      viewport.clientHeight,
      document.numPages,
      totalVirtualHeight,
    );
    setCenterPage(centerPage);
  }, [document, totalVirtualHeight]);
  const onMeasuredHeight = useCallback((height: number) => {
    if (!Number.isFinite(height) || height <= 0) return;
    const measured = Math.max(200, Math.min(PDF_MAX_CSS_DIMENSION + 32, Math.round(height)));
    setEstimatedPageHeight((current) => Math.max(current, measured));
  }, []);
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
        {searching ? <output aria-live="polite">Searching…</output> : null}
        {!searching && searchResult ? (
          <output aria-live="polite">
            {searchResult.matches} {searchResult.matches === 1 ? "match" : "matches"}
            {searchResult.partial
              ? ` in ${searchResult.searched_pages} of ${searchResult.total_pages} pages (search limited)`
              : ""}
          </output>
        ) : null}
        <button type="button" aria-label="Zoom out" onClick={() => setScale((value) => Math.max(0.5, value - 0.25))}>−</button>
        <output aria-label="PDF zoom">{Math.round(scale * 100)}%</output>
        <button type="button" aria-label="Zoom in" onClick={() => setScale((value) => Math.min(4, value + 0.25))}>+</button>
      </div>
      <div ref={viewportRef} className="files-pdf-viewport" onScroll={onPdfScroll}>
        {document
          ? (
            <div className="files-pdf-virtual-spacer" style={{ height: `${Math.round(totalVirtualHeight)}px` }}>
              {pages.map((pageNumber) => (
                <PdfPage
                  key={pageNumber}
                  document={document}
                  pageNumber={pageNumber}
                  scale={scale}
                  onFatalError={failPdf}
                  onMeasuredHeight={onMeasuredHeight}
                  top={virtualWindowPageTop(
                    pageNumber,
                    centerPage,
                    document.numPages,
                    totalVirtualHeight,
                    estimatedPageHeight,
                  )}
                />
              ))}
            </div>
          )
          : <div className="files-resource-state" role="status">Loading PDF…</div>}
      </div>
    </section>
  );
}
