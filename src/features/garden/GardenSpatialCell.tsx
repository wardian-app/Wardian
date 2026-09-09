import { useEffect, useLayoutEffect, useRef, useState, type CSSProperties, type ReactNode } from "react";
import type { GardenCamera, GardenEntityRef } from "./garden.types";
import { CELL_WIDTH, projectBounds, recordPlaneBounds, revealBetween, type GardenWorldBounds } from "./gardenSpatialZoom";
import { agentMonogram } from "./agentMonogram";

interface Props {
  target: GardenEntityRef;
  bounds: GardenWorldBounds;
  camera: GardenCamera;
  label: string;
  status?: string;
  focused?: boolean;
  /** A selected child must not intercept the second click before zoom has begun. */
  revealFromScale?: number;
  viewport: { width: number; height: number };
  receding?: boolean;
  children: ReactNode;
  onSelect: () => void;
  onEnter: () => void;
}

/** A cell is part of the world: the same camera transforms its shell and contents. */
export function GardenSpatialCell({ target, bounds, camera, label, status, focused = false, revealFromScale, viewport, receding = false, children, onSelect, onEnter }: Props) {
  const plane = target.kind === "agent" ? bounds : recordPlaneBounds(bounds, revealBetween(bounds.width * camera.scale, 180, 540));
  const screen = projectBounds(plane, camera);
  const shell = revealBetween(screen.width, 70, 150);
  const activation = revealFromScale ? revealBetween(camera.scale / revealFromScale, 1, 1.25) : 1;
  const regions = revealBetween(screen.width, 120, 300);
  const nearDetail = revealBetween(screen.width, 420, 720);
  const membrane = revealBetween(screen.width, 180, 540);
  const context = target.kind === "agent" || receding ? 1 - revealBetween(screen.width, 1400, 2400) : 1;
  const detail = nearDetail * context;
  const readable = screen.width >= 540 && context > .1 && activation > .5;
  const navigable = screen.width >= 70 && context > .1;
  const [readerMounted, setReaderMounted] = useState(screen.width >= 360 && activation > .5);
  useEffect(() => {
    setReaderMounted((previous) => screen.width >= (previous ? 280 : 360) && activation > .5);
  }, [screen.width, activation]);
  const root = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => { if (focused && navigable) root.current?.focus({ preventScroll: true }); }, [focused, navigable]);
  // Containers use a fixed layout: their children own world-space anchors.
  // Leaf readers lay out at projected pixels so
  // large camera magnifications do not enlarge a composited text/shape bitmap.
  const isAgent = target.kind === "agent";
  const isContainer = isAgent || target.kind === "workspace" || target.kind === "automation";
  const layoutWidth = isContainer ? CELL_WIDTH : screen.width;
  const style = {
    left: screen.x, top: screen.y, width: layoutWidth, height: layoutWidth * plane.height / plane.width,
    transform: isContainer ? `scale(${screen.width / CELL_WIDTH})` : undefined, opacity: shell * activation * context,
    // Coarse agent previews are visual only; Konva owns placement dragging.
    pointerEvents: context > .1 && activation > .5 && (target.kind !== "agent" || screen.width >= 280) ? "auto" : "none",
    "--garden-status": status, "--garden-regions": regions, "--garden-detail": detail, "--garden-context": context,
    "--garden-object-detail": revealBetween(screen.width, 400, 800) * context,
    // Memory keeps the seed's asymmetric boundary as it grows into a reading plane.
    "--garden-shell-radius": target.kind === "agent" ? "50%" : target.kind === "memory"
      ? `${(1 - membrane) * screen.width * .65 + membrane * 12}px ${(1 - membrane) * screen.width * .35 + membrane * 12}px ${(1 - membrane) * screen.width * .6 + membrane * 12}px ${(1 - membrane) * screen.width * .4 + membrane * 12}px` : "12px",
  } as CSSProperties;
  return <div ref={root} tabIndex={-1} className={`garden-spatial-cell garden-composition garden-spatial-${target.kind}`}
    data-garden-cell={`${target.kind}:${target.id}`} data-garden-detail={detail.toFixed(3)}
    data-garden-world={JSON.stringify(bounds)} style={style} aria-label={`${label} composition`}
    onClick={(event) => { if (event.target === event.currentTarget) { event.stopPropagation(); onSelect(); } }}
    onDoubleClick={(event) => { if (event.target === event.currentTarget) { event.stopPropagation(); onEnter(); } }}>
    {target.kind === "agent" && <div className="garden-spatial-nucleus-preview" aria-hidden="true" style={{
      opacity: 1 - revealBetween(screen.width, 300, 600), top: `${50 - 8 * revealBetween(screen.width, 180, 420)}%`,
      fontSize: Math.min(22, screen.width * .24) * CELL_WIDTH / screen.width,
    }}>{agentMonogram(label)}</div>}
    <div className="garden-spatial-caption" style={{ opacity: (1 - revealBetween(screen.width, 300, 420)) * context * revealBetween(screen.width, 180, 300),
      fontSize: 14 * layoutWidth / screen.width }} aria-hidden="true">{label}</div>
    <div className="garden-spatial-contents" inert={!readable} aria-hidden={!readable}
      tabIndex={target.kind !== "agent" && readable ? 0 : undefined}
      role={target.kind !== "agent" ? "region" : undefined}
      aria-label={target.kind !== "agent" ? `${label} reading area` : undefined}
      style={isAgent ? { opacity: context, visibility: context === 0 ? "hidden" : undefined } : isContainer ? {
        width: CELL_WIDTH - 80,
      } : {
        width: Math.max(0, Math.min(screen.width - 48, viewport.width - 64, 820)),
        left: "50%", right: "auto", transform: "translateX(-50%)",
        top: 24, bottom: 24,
        maxHeight: Math.max(100, viewport.height - 170),
      }}>
      {target.kind === "agent" || readerMounted ? children : null}
    </div>
  </div>;
}
