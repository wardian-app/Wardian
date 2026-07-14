import { useCallback, useLayoutEffect, useRef } from "react";

import type { ClosedSurfaceV1 } from "../../types";
import { HomeSurface } from "./HomeSurface";
import type { WorkbenchSurfaceRegistry } from "./surfaceRegistry";

export type SurfaceHomeDialogProps = {
  open: boolean;
  group_id: string;
  registry: WorkbenchSurfaceRegistry;
  recently_closed?: readonly ClosedSurfaceV1[];
  on_select_surface: (surfaceType: string, groupId: string) => void;
  on_browse_all: () => void;
  on_reopen_closed?: () => void;
  on_close: () => void;
};

/** Presents the existing Home surface chooser as a modal new-tab launcher. */
export function SurfaceHomeDialog({
  open,
  group_id,
  registry,
  recently_closed = [],
  on_select_surface,
  on_browse_all,
  on_reopen_closed,
  on_close,
}: SurfaceHomeDialogProps) {
  const dialogRef = useRef<HTMLDivElement>(null);
  const returnFocusRef = useRef<HTMLElement | null>(null);
  const wasOpenRef = useRef(false);

  useLayoutEffect(() => {
    if (open && !wasOpenRef.current) {
      returnFocusRef.current = document.activeElement instanceof HTMLElement
        ? document.activeElement
        : null;
      window.requestAnimationFrame(() => {
        dialogRef.current?.querySelector<HTMLElement>(
          'button:not([disabled]), [href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])',
        )?.focus();
      });
    }
    wasOpenRef.current = open;
  }, [open]);

  const requestClose = useCallback(() => {
    on_close();
    returnFocusRef.current?.focus();
  }, [on_close]);

  if (!open) return null;

  return (
    <div
      className="wardian-surface-home-backdrop"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) requestClose();
      }}
    >
      <div
        ref={dialogRef}
        role="dialog"
        aria-modal="true"
        aria-label="Choose a surface"
        className="wardian-surface-home-dialog"
        onKeyDown={(event) => {
          if (event.key !== "Escape") return;
          event.preventDefault();
          requestClose();
        }}
      >
        <HomeSurface
          group_id={group_id}
          registry={registry}
          recently_closed={recently_closed}
          on_open_surface={on_browse_all}
          on_select_surface={(surfaceType, targetGroupId) => {
            on_select_surface(surfaceType, targetGroupId);
            requestClose();
          }}
          on_reopen_closed={() => {
            on_reopen_closed?.();
            requestClose();
          }}
        />
      </div>
    </div>
  );
}
