import { useEffect, useId, useLayoutEffect, useRef, useState } from "react";

const VIEWPORT_MARGIN = 8;
const CONTEXT_MENU_OPEN_EVENT = "wardian:context-menu-open";

export function clampContextMenuPosition(
  requested: { x: number; y: number },
  menuSize: { width: number; height: number },
  viewport: { width: number; height: number },
): { x: number; y: number } {
  const maxX = Math.max(VIEWPORT_MARGIN, viewport.width - menuSize.width - VIEWPORT_MARGIN);
  const maxY = Math.max(VIEWPORT_MARGIN, viewport.height - menuSize.height - VIEWPORT_MARGIN);

  return {
    x: Math.max(VIEWPORT_MARGIN, Math.min(requested.x, maxX)),
    y: Math.max(VIEWPORT_MARGIN, Math.min(requested.y, maxY)),
  };
}

/** Keeps context menus cursor-anchored, in the viewport, and mutually exclusive. */
export function useContextMenuSurface<T extends HTMLElement>(
  x: number,
  y: number,
  onClose: () => void,
  isOpen = true,
) {
  const menuRef = useRef<T>(null);
  const surfaceId = useId();
  const onCloseRef = useRef(onClose);
  const [position, setPosition] = useState({ x, y });

  onCloseRef.current = onClose;

  useLayoutEffect(() => {
    if (!isOpen) return;
    const updatePosition = () => {
      const bounds = menuRef.current?.getBoundingClientRect();
      setPosition(clampContextMenuPosition(
        { x, y },
        { width: bounds?.width ?? 0, height: bounds?.height ?? 0 },
        { width: window.innerWidth, height: window.innerHeight },
      ));
    };

    updatePosition();
    const observer = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(updatePosition);
    if (menuRef.current && observer) observer.observe(menuRef.current);
    window.addEventListener("resize", updatePosition);
    return () => {
      observer?.disconnect();
      window.removeEventListener("resize", updatePosition);
    };
  }, [isOpen, x, y]);

  useEffect(() => {
    if (!isOpen) return;
    const closeWhenAnotherMenuOpens = (event: Event) => {
      if ((event as CustomEvent<string>).detail !== surfaceId) onCloseRef.current();
    };

    window.addEventListener(CONTEXT_MENU_OPEN_EVENT, closeWhenAnotherMenuOpens);
    window.dispatchEvent(new CustomEvent(CONTEXT_MENU_OPEN_EVENT, { detail: surfaceId }));
    return () => window.removeEventListener(CONTEXT_MENU_OPEN_EVENT, closeWhenAnotherMenuOpens);
  }, [isOpen, surfaceId]);

  return { menuRef, style: { left: position.x, top: position.y } };
}
