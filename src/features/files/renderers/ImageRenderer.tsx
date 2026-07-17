import { useCallback, useEffect, useRef, useState, type PointerEvent } from "react";

import type { FileRendererProps } from "../rendererRegistry";

const IMAGE_MAX_SIZE_BYTES = 64 * 1024 * 1024;
let imageLeaseSequence = 0;

function nextLeaseId(resourceId: string, revision: number) {
  imageLeaseSequence += 1;
  return `image:${resourceId}@${revision}:${imageLeaseSequence}`;
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

export default function ImageRenderer({ snapshot, client, lifecycle }: FileRendererProps) {
  const viewportRef = useRef<HTMLDivElement>(null);
  const leaseIdRef = useRef(nextLeaseId(snapshot.resource_id, snapshot.revision));
  const [url, setUrl] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [fit, setFit] = useState(true);
  const [zoom, setZoom] = useState(1);
  const [retryToken, setRetryToken] = useState(0);
  const dragRef = useRef<{ x: number; y: number; left: number; top: number } | null>(null);
  const descriptor = snapshot.descriptor;
  const allowed = descriptor.renderer_kind === "image"
    && descriptor.capabilities.preview
    && descriptor.capabilities.stream
    && descriptor.unavailable_reason === null
    && descriptor.size_bytes <= IMAGE_MAX_SIZE_BYTES;

  useEffect(() => {
    leaseIdRef.current = nextLeaseId(snapshot.resource_id, snapshot.revision);
  }, [snapshot.resource_id, snapshot.revision]);

  useEffect(() => {
    if (!lifecycle.visible || !allowed) return;
    const leaseId = leaseIdRef.current;
    let cancelled = false;
    let issued = false;
    setUrl(null);
    setError(null);
    void client.issueTicket(snapshot.resource_id, snapshot.revision, leaseId).then((ticket) => {
      issued = true;
      if (cancelled) {
        releaseLease(client, snapshot.resource_id, leaseId);
        return undefined;
      }
      if (ticket.revision !== snapshot.revision) {
        releaseLease(client, snapshot.resource_id, leaseId);
        return undefined;
      }
      setUrl(ticket.url);
      return undefined;
    }).catch((cause) => {
      if (!cancelled) setError(errorMessage(cause));
    });
    return () => {
      cancelled = true;
      if (issued) releaseLease(client, snapshot.resource_id, leaseId);
    };
  }, [allowed, client, lifecycle.visible, retryToken, snapshot.resource_id, snapshot.revision]);

  const pointerDown = (event: PointerEvent<HTMLDivElement>) => {
    const viewport = viewportRef.current;
    if (!viewport) return;
    viewport.setPointerCapture(event.pointerId);
    dragRef.current = {
      x: event.clientX,
      y: event.clientY,
      left: viewport.scrollLeft,
      top: viewport.scrollTop,
    };
  };
  const pointerMove = (event: PointerEvent<HTMLDivElement>) => {
    const viewport = viewportRef.current;
    const drag = dragRef.current;
    if (!viewport || !drag) return;
    viewport.scrollLeft = drag.left - (event.clientX - drag.x);
    viewport.scrollTop = drag.top - (event.clientY - drag.y);
  };
  const retry = useCallback(() => setRetryToken((value) => value + 1), []);

  if (!lifecycle.visible) {
    return <div className="files-resource-state" role="status">Image preview suspended.</div>;
  }
  if (!allowed) {
    return (
      <div className="files-resource-state" role="status">
        {descriptor.unavailable_reason ?? "image_preview_unavailable"}
      </div>
    );
  }
  if (error) {
    return (
      <section className="files-resource-state" role="alert">
        <h2>Image preview unavailable</h2>
        <p>{error}</p>
        <button type="button" onClick={retry}>Retry</button>
      </section>
    );
  }
  return (
    <section className="files-binary-renderer" aria-label="Image preview">
      <div className="files-renderer-toolbar" role="toolbar" aria-label="Image controls">
        <button type="button" aria-pressed={fit} onClick={() => setFit(true)}>Fit</button>
        <button type="button" aria-pressed={!fit && zoom === 1} onClick={() => { setFit(false); setZoom(1); }}>100%</button>
        <button type="button" aria-label="Zoom out" onClick={() => { setFit(false); setZoom((value) => Math.max(0.25, value - 0.25)); }}>−</button>
        <output aria-label="Image zoom">{Math.round(zoom * 100)}%</output>
        <button type="button" aria-label="Zoom in" onClick={() => { setFit(false); setZoom((value) => Math.min(8, value + 0.25)); }}>+</button>
      </div>
      <div
        ref={viewportRef}
        className="files-image-viewport"
        onPointerDown={pointerDown}
        onPointerMove={pointerMove}
        onPointerUp={() => { dragRef.current = null; }}
      >
        {url ? (
          <img
            alt={descriptor.display_name}
            className={fit ? "files-image-preview is-fit" : "files-image-preview"}
            draggable={false}
            src={url}
            style={fit ? undefined : { transform: `scale(${zoom})` }}
          />
        ) : <div className="files-resource-state" role="status">Loading image…</div>}
      </div>
    </section>
  );
}
