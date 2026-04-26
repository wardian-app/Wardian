import React, { useCallback, useEffect, useRef } from 'react';

interface Props {
  baseWidth: number;
  edge: 'left' | 'right';
  onResize: (newWidthPx: number) => void;
  onReset: () => void;
}

const RESIZING_CLASS = 'sidebar-resizing';

export const SidebarResizeHandle: React.FC<Props> = ({ baseWidth, edge, onResize, onReset }) => {
  const startXRef = useRef<number | null>(null);
  const baseRef = useRef<number>(baseWidth);
  const edgeRef = useRef(edge);
  const onResizeRef = useRef(onResize);

  useEffect(() => {
    edgeRef.current = edge;
    onResizeRef.current = onResize;
  }, [edge, onResize]);

  const onPointerMove = useCallback((e: PointerEvent) => {
    if (startXRef.current === null) return;
    const delta = e.clientX - startXRef.current;
    const next = edgeRef.current === 'right' ? baseRef.current + delta : baseRef.current - delta;
    onResizeRef.current(next);
  }, []);

  const endDrag = useCallback(() => {
    startXRef.current = null;
    window.removeEventListener('pointermove', onPointerMove);
    window.removeEventListener('pointerup', endDrag);
    document.documentElement.classList.remove(RESIZING_CLASS);
    document.body.style.userSelect = '';
    document.body.style.cursor = '';
  }, [onPointerMove]);

  // If the handle unmounts mid-drag (e.g. sidebar collapses), clean up listeners
  // and restore body styles so we don't leak handlers.
  useEffect(() => endDrag, [endDrag]);

  const onPointerDown = (e: React.PointerEvent) => {
    e.preventDefault();
    e.stopPropagation();
    startXRef.current = e.clientX;
    baseRef.current = baseWidth;
    document.documentElement.classList.add(RESIZING_CLASS);
    document.body.style.userSelect = 'none';
    document.body.style.cursor = 'col-resize';
    window.addEventListener('pointermove', onPointerMove);
    window.addEventListener('pointerup', endDrag);
  };

  return (
    <div
      data-testid="sidebar-resize-handle"
      onPointerDown={onPointerDown}
      onDoubleClick={onReset}
      className={`absolute top-0 bottom-0 w-1 hover:w-1.5 cursor-col-resize z-20 transition-[width] ${
        edge === 'right' ? 'right-0 hover:bg-[var(--color-wardian-accent)]/40' : 'left-0 hover:bg-[var(--color-wardian-accent)]/40'
      }`}
      title="Drag to resize · double-click to reset"
    />
  );
};
