export const MAX_XTERM_RENDERERS = 24;
export const MAX_WEBGL_RENDERERS = 12;

export type TerminalRendererKind = "xterm" | "webgl";

type EvictionHandler = () => void;

/**
 * Process-local renderer budget shared by every desktop terminal presentation.
 *
 * Maps retain insertion order, which gives us a small deterministic LRU. A
 * caller must `touch` a visible or interacted-with presentation to keep it
 * warm. Xterm and WebGL leases are independent: losing WebGL keeps the DOM
 * renderer alive, while losing xterm requires a broker snapshot before the
 * presentation can render again.
 */
export class TerminalRendererBudget {
  readonly #xtermLimit: number;
  readonly #webglLimit: number;
  readonly #xterms = new Map<string, EvictionHandler>();
  readonly #webgl = new Map<string, EvictionHandler>();

  constructor(options?: { xtermLimit?: number; webglLimit?: number }) {
    this.#xtermLimit = options?.xtermLimit ?? MAX_XTERM_RENDERERS;
    this.#webglLimit = options?.webglLimit ?? MAX_WEBGL_RENDERERS;
    if (this.#xtermLimit < 1 || this.#webglLimit < 1) {
      throw new Error("Terminal renderer limits must be positive");
    }
  }

  acquire(kind: TerminalRendererKind, presentationId: string, onEvict: EvictionHandler) {
    const pool = kind === "xterm" ? this.#xterms : this.#webgl;
    const limit = kind === "xterm" ? this.#xtermLimit : this.#webglLimit;
    const existing = pool.get(presentationId);
    if (existing) {
      pool.delete(presentationId);
      pool.set(presentationId, onEvict);
      return { granted: true, evictedPresentationId: null } as const;
    }

    let evictedPresentationId: string | null = null;
    if (pool.size >= limit) {
      const oldest = pool.entries().next().value as [string, EvictionHandler] | undefined;
      if (oldest) {
        evictedPresentationId = oldest[0];
        pool.delete(oldest[0]);
        oldest[1]();
      }
    }
    pool.set(presentationId, onEvict);
    return { granted: true, evictedPresentationId } as const;
  }

  touch(kind: TerminalRendererKind, presentationId: string) {
    const pool = kind === "xterm" ? this.#xterms : this.#webgl;
    const handler = pool.get(presentationId);
    if (!handler) {
      return false;
    }
    pool.delete(presentationId);
    pool.set(presentationId, handler);
    return true;
  }

  release(kind: TerminalRendererKind, presentationId: string) {
    const pool = kind === "xterm" ? this.#xterms : this.#webgl;
    return pool.delete(presentationId);
  }

  releasePresentation(presentationId: string) {
    const releasedXterm = this.#xterms.delete(presentationId);
    const releasedWebgl = this.#webgl.delete(presentationId);
    return releasedXterm || releasedWebgl;
  }

  has(kind: TerminalRendererKind, presentationId: string) {
    return (kind === "xterm" ? this.#xterms : this.#webgl).has(presentationId);
  }

  size(kind: TerminalRendererKind) {
    return (kind === "xterm" ? this.#xterms : this.#webgl).size;
  }

  clear() {
    this.#xterms.clear();
    this.#webgl.clear();
  }
}

export type TerminalMirrorFit = {
  scale: number;
  content_width: number;
  content_height: number;
  offset_x: number;
  offset_y: number;
  pan_x: boolean;
  pan_y: boolean;
  letterboxed: boolean;
};

/**
 * Fits a canonical terminal grid into a mirror without changing PTY geometry.
 * Normal scale wins, then scale is reduced to the readability floor, and any
 * remaining overflow is left pannable. Extra room is centered/letterboxed.
 */
export function calculateTerminalMirrorFit(options: {
  cols: number;
  rows: number;
  cellWidth: number;
  cellHeight: number;
  viewportWidth: number;
  viewportHeight: number;
  minimumScale?: number;
}): TerminalMirrorFit {
  const cols = Math.max(1, options.cols);
  const rows = Math.max(1, options.rows);
  const cellWidth = Math.max(1, options.cellWidth);
  const cellHeight = Math.max(1, options.cellHeight);
  const viewportWidth = Math.max(0, options.viewportWidth);
  const viewportHeight = Math.max(0, options.viewportHeight);
  const minimumScale = Math.min(1, Math.max(0.5, options.minimumScale ?? 0.75));
  const naturalWidth = cols * cellWidth;
  const naturalHeight = rows * cellHeight;
  const fitScale = Math.min(
    viewportWidth / naturalWidth || 0,
    viewportHeight / naturalHeight || 0,
    1,
  );
  const scale = fitScale >= 1 ? 1 : Math.max(minimumScale, fitScale);
  const contentWidth = naturalWidth * scale;
  const contentHeight = naturalHeight * scale;
  const panX = contentWidth > viewportWidth;
  const panY = contentHeight > viewportHeight;

  return {
    scale,
    content_width: contentWidth,
    content_height: contentHeight,
    offset_x: panX ? 0 : (viewportWidth - contentWidth) / 2,
    offset_y: panY ? 0 : (viewportHeight - contentHeight) / 2,
    pan_x: panX,
    pan_y: panY,
    letterboxed: !panX && !panY && (contentWidth < viewportWidth || contentHeight < viewportHeight),
  };
}

export const terminalRendererBudget = new TerminalRendererBudget();
