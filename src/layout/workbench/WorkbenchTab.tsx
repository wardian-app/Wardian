import { useLayoutEffect, useRef } from "react";

import type { DeepReadonly } from "../../features/workbench/useWorkbenchStore";
import type { WorkbenchSurfaceV1 } from "../../types";

export type WorkbenchTabProps = {
  surface: DeepReadonly<WorkbenchSurfaceV1>;
  title: string;
  on_close?: () => void;
};

/** Decorates Dockview's owned ARIA tab without introducing a nested tab role. */
export function WorkbenchTab({ surface, title, on_close }: WorkbenchTabProps) {
  const descriptorRef = useRef<HTMLSpanElement>(null);

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
    tab.addEventListener("keydown", handleCloseKey, { capture: true });
    return () => tab.removeEventListener("keydown", handleCloseKey, { capture: true });
  }, [on_close, surface.resource_key, surface.surface_id, surface.surface_type]);

  return (
    <span
      ref={descriptorRef}
      className="wardian-workbench-tab-label"
    >
      {title}
    </span>
  );
}
