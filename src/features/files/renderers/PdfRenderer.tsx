import { useCallback, useEffect, useRef, useState } from "react";
import type { PDFDocumentProxy, PDFPageProxy, RenderTask } from "pdfjs-dist";

import type { FileRendererProps } from "../rendererRegistry";
import { configurePdfWorker } from "./pdfWorker";

const PDF_MAX_SIZE_BYTES = 256 * 1024 * 1024;
const PDF_RANGE_CHUNK_BYTES = 65_536;
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
    // Closing the shared resource already revokes every renderer lease.
  }
}

function PdfPage({ document, pageNumber, scale }: {
  document: PDFDocumentProxy;
  pageNumber: number;
  scale: number;
}) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    let renderTask: RenderTask | null = null;
    void document.getPage(pageNumber).then((page: PDFPageProxy) => {
      if (cancelled) return;
      const canvas = canvasRef.current;
      const context = canvas?.getContext("2d");
      if (!canvas || !context) throw new Error("Canvas rendering is unavailable");
      const viewport = page.getViewport({ scale });
      const outputScale = Math.max(1, globalThis.devicePixelRatio || 1);
      canvas.width = Math.floor(viewport.width * outputScale);
      canvas.height = Math.floor(viewport.height * outputScale);
      canvas.style.width = `${Math.floor(viewport.width)}px`;
      canvas.style.height = `${Math.floor(viewport.height)}px`;
      renderTask = page.render({
        canvas,
        canvasContext: context,
        transform: outputScale === 1 ? undefined : [outputScale, 0, 0, outputScale, 0, 0],
        viewport,
      });
      return renderTask.promise;
    }).catch((cause) => {
      if (!cancelled && (cause as { name?: string }).name !== "RenderingCancelledException") {
        setError(errorMessage(cause));
      }
    });
    return () => {
      cancelled = true;
      renderTask?.cancel();
    };
  }, [document, pageNumber, scale]);

  return (
    <figure className="files-pdf-page">
      <figcaption>Page {pageNumber}</figcaption>
      {error ? <div role="alert">{error}</div> : <canvas ref={canvasRef} aria-label={`PDF page ${pageNumber}`} />}
    </figure>
  );
}

export default function PdfRenderer({ snapshot, client, lifecycle }: FileRendererProps) {
  const leaseIdRef = useRef(nextLeaseId(snapshot.resource_id, snapshot.revision));
  const [document, setDocument] = useState<PDFDocumentProxy | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [retryToken, setRetryToken] = useState(0);
  const [scale, setScale] = useState(1);
  const [query, setQuery] = useState("");
  const [matches, setMatches] = useState<number | null>(null);
  const descriptor = snapshot.descriptor;
  const allowed = descriptor.renderer_kind === "pdf"
    && descriptor.capabilities.preview
    && descriptor.capabilities.stream
    && descriptor.unavailable_reason === null
    && descriptor.size_bytes <= PDF_MAX_SIZE_BYTES;

  useEffect(() => {
    leaseIdRef.current = nextLeaseId(snapshot.resource_id, snapshot.revision);
  }, [snapshot.resource_id, snapshot.revision]);

  useEffect(() => {
    if (!lifecycle.visible || !allowed) return;
    const leaseId = leaseIdRef.current;
    let cancelled = false;
    let issued = false;
    let loadingTask: ReturnType<typeof import("pdfjs-dist")["getDocument"]> | null = null;
    let loadedDocument: PDFDocumentProxy | null = null;
    setDocument(null);
    setError(null);
    setMatches(null);

    void client.issueTicket(snapshot.resource_id, snapshot.revision, leaseId).then(async (ticket) => {
      issued = true;
      if (cancelled || ticket.revision !== snapshot.revision) {
        releaseLease(client, snapshot.resource_id, leaseId);
        return;
      }
      const pdfjs = await import("pdfjs-dist");
      if (cancelled) return;
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
        await loadedDocument.destroy();
        return;
      }
      setDocument(loadedDocument);
    }).catch((cause) => {
      if (!cancelled) setError(errorMessage(cause));
    });
    return () => {
      cancelled = true;
      void loadingTask?.destroy();
      if (loadedDocument && !loadingTask) void loadedDocument.destroy();
      if (issued) releaseLease(client, snapshot.resource_id, leaseId);
    };
  }, [allowed, client, lifecycle.visible, retryToken, snapshot.resource_id, snapshot.revision]);

  useEffect(() => {
    if (!document || !query.trim()) {
      setMatches(query.trim() ? 0 : null);
      return;
    }
    let cancelled = false;
    const needle = query.trim().toLocaleLowerCase();
    void Promise.all(Array.from({ length: document.numPages }, async (_, index) => {
      const page = await document.getPage(index + 1);
      const content = await page.getTextContent();
      const text = content.items.map((item) => "str" in item ? item.str : "").join(" ").toLocaleLowerCase();
      let count = 0;
      let from = 0;
      while ((from = text.indexOf(needle, from)) >= 0) {
        count += 1;
        from += Math.max(1, needle.length);
      }
      return count;
    })).then((counts) => {
      if (!cancelled) setMatches(counts.reduce((sum, count) => sum + count, 0));
    }).catch((cause) => {
      if (!cancelled) setError(errorMessage(cause));
    });
    return () => { cancelled = true; };
  }, [document, query]);

  const retry = useCallback(() => setRetryToken((value) => value + 1), []);
  if (!lifecycle.visible) {
    return <div className="files-resource-state" role="status">PDF preview suspended.</div>;
  }
  if (!allowed) {
    return (
      <div className="files-resource-state" role="status">
        {descriptor.unavailable_reason ?? "pdf_preview_unavailable"}
      </div>
    );
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
          <input
            aria-label="Search PDF"
            type="search"
            value={query}
            onChange={(event) => setQuery(event.target.value)}
          />
        </label>
        {matches !== null ? <output aria-live="polite">{matches} {matches === 1 ? "match" : "matches"}</output> : null}
        <button type="button" aria-label="Zoom out" onClick={() => setScale((value) => Math.max(0.5, value - 0.25))}>−</button>
        <output aria-label="PDF zoom">{Math.round(scale * 100)}%</output>
        <button type="button" aria-label="Zoom in" onClick={() => setScale((value) => Math.min(4, value + 0.25))}>+</button>
      </div>
      <div className="files-pdf-viewport">
        {document
          ? Array.from({ length: document.numPages }, (_, index) => (
            <PdfPage key={index + 1} document={document} pageNumber={index + 1} scale={scale} />
          ))
          : <div className="files-resource-state" role="status">Loading PDF…</div>}
      </div>
    </section>
  );
}
