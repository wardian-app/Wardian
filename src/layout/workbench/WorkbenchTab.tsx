import { useLayoutEffect, useRef, useState } from "react";
import { X } from "lucide-react";

import type { DeepReadonly } from "../../features/workbench/useWorkbenchStore";
import type { WorkbenchSurfaceV1 } from "../../types";
import {
  WorkbenchContextMenu,
  type WorkbenchMenuItem,
  type WorkbenchPaneTarget,
} from "./WorkbenchGroupHeader";

export type WorkbenchTabPointerDragIdentity = {
  surface_id: string;
  source_group_id: string;
  pointer_id: number;
};

export type WorkbenchTabProps = {
  surface: DeepReadonly<WorkbenchSurfaceV1>;
  title: string;
  group_id: string;
  pane_targets?: readonly WorkbenchPaneTarget[];
  on_close?: () => void;
  on_split?: (direction: "horizontal" | "vertical") => void;
  on_move?: (targetGroupId: string) => void;
  on_pointer_drag_start?: (identity: WorkbenchTabPointerDragIdentity) => void;
  on_pointer_drag_end?: (identity: WorkbenchTabPointerDragIdentity) => void;
};

/** Decorates Dockview's owned ARIA tab without introducing a nested tab role. */
export function WorkbenchTab({
  surface,
  title,
  group_id,
  pane_targets = [],
  on_close,
  on_split,
  on_move,
  on_pointer_drag_start,
  on_pointer_drag_end,
}: WorkbenchTabProps) {
  const descriptorRef = useRef<HTMLSpanElement>(null);
  const [menuPosition, setMenuPosition] = useState<{ x: number; y: number } | null>(null);

  useLayoutEffect(() => {
    const tab = descriptorRef.current?.closest<HTMLElement>('[role="tab"]');
    if (!tab) return;
    tab.dataset.surfaceId = surface.surface_id;
    tab.dataset.surfaceType = surface.surface_type;
    if (surface.resource_key === undefined) delete tab.dataset.resourceKey;
    else tab.dataset.resourceKey = surface.resource_key;
    const handleCloseKey = (event: KeyboardEvent): void => {
      if (
        document.activeElement !== tab
        || (event.key !== "Delete" && event.key !== "Backspace")
      ) return;
      event.preventDefault();
      event.stopImmediatePropagation();
      on_close?.();
    };
    const handleContextMenu = (event: globalThis.MouseEvent): void => {
      event.preventDefault();
      event.stopPropagation();
      setMenuPosition({ x: event.clientX, y: event.clientY });
    };
    let activePointerDrag: WorkbenchTabPointerDragIdentity | null = null;
    const clearPointerDrag = (event?: PointerEvent): void => {
      const identity = activePointerDrag;
      if (!identity || (event && event.pointerId !== identity.pointer_id)) return;
      activePointerDrag = null;
      window.removeEventListener("pointerup", clearPointerDrag, { capture: true });
      window.removeEventListener("pointercancel", clearPointerDrag, { capture: true });
      on_pointer_drag_end?.(identity);
    };
    const handlePointerDown = (event: PointerEvent): void => {
      const target = event.target instanceof Element ? event.target : null;
      if (event.button !== 0 || target?.closest("[data-tab-close]")) return;
      clearPointerDrag();
      const identity = {
        surface_id: surface.surface_id,
        source_group_id: group_id,
        pointer_id: event.pointerId,
      };
      activePointerDrag = identity;
      on_pointer_drag_start?.(identity);
      window.addEventListener("pointerup", clearPointerDrag, { capture: true });
      window.addEventListener("pointercancel", clearPointerDrag, { capture: true });
    };
    tab.addEventListener("keydown", handleCloseKey, { capture: true });
    tab.addEventListener("contextmenu", handleContextMenu);
    tab.addEventListener("pointerdown", handlePointerDown);
    return () => {
      clearPointerDrag();
      tab.removeEventListener("keydown", handleCloseKey, { capture: true });
      tab.removeEventListener("contextmenu", handleContextMenu);
      tab.removeEventListener("pointerdown", handlePointerDown);
    };
  }, [
    group_id,
    on_close,
    on_pointer_drag_end,
    on_pointer_drag_start,
    surface.resource_key,
    surface.surface_id,
    surface.surface_type,
  ]);
  const menuItems: WorkbenchMenuItem[] = [
    { label: "Close tab", on_select: () => on_close?.() },
    { label: "Split tab right", on_select: () => on_split?.("horizontal") },
    { label: "Split tab down", on_select: () => on_split?.("vertical") },
    ...pane_targets.map((target): WorkbenchMenuItem => ({
      label: `Move to ${target.position} pane`,
      on_select: () => on_move?.(target.group_id),
    })),
  ];

  return (
    <span
      ref={descriptorRef}
      className="wardian-workbench-tab"
      data-tab-group-id={group_id}
    >
      <span className="wardian-workbench-tab-label">{title}</span>
      <span
        className="wardian-workbench-tab-close"
        aria-hidden="true"
        data-tab-close
        title={`Close ${title}`}
        onPointerDown={(event) => event.stopPropagation()}
        onClick={(event) => {
          event.preventDefault();
          event.stopPropagation();
          on_close?.();
        }}
      >
        <X aria-hidden="true" size={14} strokeWidth={1.75} />
      </span>
      {menuPosition && (
        <WorkbenchContextMenu
          aria_label={`${title} tab actions`}
          items={menuItems}
          position={menuPosition}
          return_focus={() => (
            descriptorRef.current?.closest<HTMLElement>('[role="tab"]')
            ?? [...document.querySelectorAll<HTMLElement>('[role="tab"][data-surface-id]')]
              .find((tab) => tab.dataset.surfaceId === surface.surface_id)
          )}
          on_close={() => setMenuPosition(null)}
        />
      )}
    </span>
  );
}
