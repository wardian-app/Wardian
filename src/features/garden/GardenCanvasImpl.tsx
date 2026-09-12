import React, { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { Group, Layer, Stage } from "react-konva";
import { revealBetween } from "./gardenSpatialZoom";
import type Konva from "konva";
import { AgentUnit, AGENT_UNIT_NAME } from "./AgentUnit";
import { canvasWorldViewport, pointInCanvasViewport, rectInCanvasViewport, routeInCanvasViewport } from "./canvasVisibility";
import { crownPositions, CROWN_CAP } from "./skillGlyphs";
import { DistrictLayer } from "./DistrictLayer";
import { AutomationRoutesLayer } from "./AutomationRoutesLayer";
import { agentDistrict, agentLabelWidths, districtBand, districtPopulations, situatedRoutes, type DistrictBand } from "./canvasHierarchy";
import type { GardenAutomationInput } from "./gardenProjection";
import type { GardenCamera } from "./garden.types";
import { GardenContextMenu } from "./GardenContextMenu";
import { gardenDetailForScale, type GardenSkillGlyph } from "./skillGlyphs";
import { useGardenPulse } from "./useGardenPulse";
import { MAX_SCALE, MIN_SCALE, fitTransform, zoomAt } from "./gardenViewport";
import type { GardenAgentUnit, GardenEntityRef, GardenAutomationUnit } from "./garden.types";
import { unitKey } from "./garden.types";
import { isActiveAgentStatus } from "./gardenStatus";
import { useGardenTheme } from "./useGardenTheme";
import { TerrainLayer } from "./TerrainLayer";
import { AttributionLayer } from "./AttributionLayer";
import type { TerrainCell, TerrainDistrict } from "./terrain";
import type { TerrainPaint } from "./terrainPaint";
import type { TerrainViewport } from "./terrainFrontier";
import { wheelZoomFactor } from "../../utils/wheelZoom";
import "./garden-canvas.css";

export interface GardenCanvasProps {
  /** Semantic opens stay in Garden; canonical sessions use onOpenAgent. */
  onEnter?: (ref: GardenEntityRef) => void;
  onClearSelection?: () => void;
  onOpenParent?: () => void;
  focusedDistrictId?: string | null;
  onFocusDistrict?: (id: string) => void;
  camera?: GardenCamera | null;
  onCameraChange?: (camera: GardenCamera) => void;
  districtLabels?: ReadonlyMap<string, string>;
  districtByAgentId?: ReadonlyMap<string, string>;
  automationProjections?: readonly GardenAutomationInput[];
  /** The DOM spatial container owns focus and navigation while open. */
  compositionActive?: boolean;
  /** World-anchored interiors share this camera; semantic entry must not replace it. */
  continuousZoom?: boolean;
  agentUnits: GardenAgentUnit[];
  automationUnits: GardenAutomationUnit[];
  selectedKey: string | null;
  /** Agents carrying the selected skill; empty unless a skill is selected. */
  highlightedAgentIds?: ReadonlySet<string>;
  /** Ground cells drawn beneath the units. Empty until terrain is ingested. */
  terrainCells?: readonly TerrainCell[];
  terrainDistricts?: ReadonlyMap<string, TerrainDistrict>;
  /** Change tint per terrain path. Absent until a change set has loaded. */
  terrainPaint?: ReadonlyMap<string, TerrainPaint>;
  /** Ground written to by the current selection, highlighted across districts. */
  highlightedPaths?: ReadonlySet<string>;
  onSelectPath?: (path: string) => void;
  onOpenPath?: (path: string) => void;
  /**
   * Reports the visible world rectangle and zoom.
   *
   * Terrain ingestion is driven by what is on screen and how large it is, so
   * the viewport has to leave the canvas. It is coalesced to one report per
   * animation frame: a wheel gesture fires dozens of events, and each one
   * reaching React would re-render the map for a value only the debounced
   * expansion pass reads.
   */
  onViewportChange?: (viewport: TerrainViewport) => void;
  onSelect: (ref: GardenEntityRef) => void;
  onOpenAgent: (id: string) => void;
  onOpenSkill?: (glyph: GardenSkillGlyph) => void;
  onMoveUnit: (key: string, x: number, y: number) => void;
  /** Hide the static DOM counterpart while Konva moves an agent locally. */
  onDraggingAgentChange?: (id: string | null) => void;
  onResetLayout: () => void;
}

interface GardenMenuState {
  x: number;
  y: number;
  agentId: string | null;
}

/** Coarser than the wheel: a keypress is a deliberate step, not a nudge. */
const KEY_ZOOM_STEP = 1.25;
/** Screen pixels moved per arrow press, so panning feels the same at any zoom. */
const PAN_STEP = 80;

export const GardenCanvas: React.FC<GardenCanvasProps> = ({
  agentUnits,
  selectedKey,
  highlightedAgentIds,
  terrainCells,
  terrainDistricts,
  terrainPaint,
  highlightedPaths,
  onSelectPath,
  onOpenPath,
  onViewportChange,
  onSelect,
  onOpenAgent,
  onOpenSkill,
  onMoveUnit,
  onDraggingAgentChange,
  onResetLayout,
  onEnter,
  onClearSelection,
  onOpenParent,
  focusedDistrictId,
  onFocusDistrict,
  camera,
  onCameraChange,
  districtLabels,
  districtByAgentId,
  automationProjections = [],
  compositionActive = false,
  continuousZoom = false,
}) => {
  const containerRef = useRef<HTMLDivElement>(null);
  const stageRef = useRef<Konva.Stage>(null);
  const layerRef = useRef<Konva.Layer>(null);
  const [size, setSize] = useState({ width: 0, height: 0 });
  const [scale, setScale] = useState(camera?.scale ?? 1);
  const [draggingAgentId, setDraggingAgentId] = useState<string | null>(null);
  const transformRef = useRef<GardenCamera>(camera ?? { scale: 1, position: { x: 0, y: 0 } });
  const [cameraPosition, setCameraPosition] = useState(transformRef.current.position);
  const onCameraChangeRef = useRef(onCameraChange);
  onCameraChangeRef.current = onCameraChange;
  const districts = useMemo(() => terrainDistricts ?? new Map<string, TerrainDistrict>(), [terrainDistricts]);
  const bandsRef = useRef(new Map<string, DistrictBand>());
  const bands = useMemo(() => {
    const next = new Map([...districts].map(([id, district]) => [id, districtBand(district.radius, scale, bandsRef.current.get(id))]));
    bandsRef.current = next;
    return next;
  }, [districts, scale]);
  const isWorkstream = (unit: GardenAgentUnit) => {
    const id = agentDistrict(unit, districts, districtByAgentId);
    return id ? bands.get(id) === "workstream" : scale >= 0.7;
  };
  const routes = useMemo(() => situatedRoutes(automationProjections, agentUnits, districts), [automationProjections, agentUnits, districts]);
  const paintViewport = canvasWorldViewport({ scale, position: cameraPosition }, size);
  const visibleRoutes = routes.filter((route) => (continuousZoom || route.points.some((point) => [...districts].some(([id, district]) => bands.get(id) === "workstream" && Math.hypot(point.x - district.origin.x, point.y - district.origin.y) <= district.radius)) || (districts.size === 0 && scale >= 0.7))
    && (routeInCanvasViewport(route.points.length === 1 ? [route.points[0], route.anchor] : route.points, paintViewport)
      || pointInCanvasViewport(route.anchor, paintViewport, 200 / scale)
      || route.presentation.markers.some((marker) => pointInCanvasViewport(marker.position, paintViewport, 200 / scale))));
  const visibleTerrainCells = (terrainCells ?? []).filter((cell) => (continuousZoom || bands.get(cell.districtId) === "workstream") &&
    (cell.isDir || (cell.rect.width * scale >= 100 && cell.rect.height * scale >= 40)));
  const paintedTerrainCells = visibleTerrainCells.filter((cell) => rectInCanvasViewport(cell.rect, paintViewport));
  const paintedAgents = agentUnits.filter((unit) => {
    // Konva owns a drag's live position until drop; never cull it using its old authored position.
    if (unit.ref.id === draggingAgentId) return true;
    if (continuousZoom && scale >= 12 && !highlightedAgentIds?.has(unit.ref.id)) return false;
    const crownRadius = Math.max(32, ...crownPositions(Math.min(unit.crown.length, CROWN_CAP.near) + 1).map((point) => Math.hypot(point.x, point.y) + 8));
    return pointInCanvasViewport(unit.position, paintViewport, crownRadius);
  });
  const [rovingKey, setRovingKey] = useState<string | null>(null);
  const [keyboardKey, setKeyboardKey] = useState<string | null>(null);
  const diveLatchRef = useRef<string | null>(null);
  const [menu, setMenu] = useState<GardenMenuState | null>(null);
  // The transform the last automatic fit applied, published on the container as
  // `data-garden-fit`. Canvas units have no DOM handles, so this is the only way
  // a test (or a bug report) can turn a world position into a screen point. It
  // is not updated once the user takes over the viewport.
  const [fit, setFit] = useState<string | null>(null);
  const fitRef = useRef<string | null>(null);
  const minScaleRef = useRef(MIN_SCALE);
  const theme = useGardenTheme();

  // Publish the visible world rectangle, coalesced to one report per frame.
  //
  // A wheel gesture fires dozens of events and a drag fires one per pointer
  // move; letting each reach React would re-render the map for a value only the
  // debounced expansion pass reads. The world rect is the inverse of the stage
  // transform: world = (screen - position) / scale.
  const onViewportChangeRef = useRef(onViewportChange);
  onViewportChangeRef.current = onViewportChange;
  const sizeRef = useRef(size);
  sizeRef.current = size;
  const viewportFrameRef = useRef<number | null>(null);
  const reportViewport = useCallback(() => {
    if (!onViewportChangeRef.current) return;
    if (viewportFrameRef.current !== null) return;
    viewportFrameRef.current = requestAnimationFrame(() => {
      viewportFrameRef.current = null;
      const { scale: current, position } = transformRef.current;
      const { width, height } = sizeRef.current;
      if (!Number.isFinite(current) || current <= 0 || width <= 0 || height <= 0) return;
      onViewportChangeRef.current?.({
        scale: current,
        world: {
          x: -position.x / current,
          y: -position.y / current,
          width: width / current,
          height: height / current,
        },
      });
    });
  }, []);
  useEffect(
    () => () => {
      if (viewportFrameRef.current !== null) cancelAnimationFrame(viewportFrameRef.current);
    },
    [],
  );

  // Konva owns the live viewport transform. React still receives the scale so
  // detail labels and the readout can react to it, but the Stage transform is
  // not passed back as props: doing both lets reconciliation briefly restore a
  // stale scale while a wheel event is moving the stage position.
  const applyTransform = useCallback((transform: GardenCamera, publish = true) => {
    if (!Number.isFinite(transform.scale) || transform.scale <= 0 || !Number.isFinite(transform.position.x) || !Number.isFinite(transform.position.y)) return;
    const stage = stageRef.current;
    transformRef.current = transform;
    if (stage) {
      stage.scale({ x: transform.scale, y: transform.scale });
      stage.position(transform.position);
      stage.batchDraw();
    }
    setScale(transform.scale);
    setCameraPosition(transform.position);
    if (publish) onCameraChangeRef.current?.(transform);
    reportViewport();
  }, [reportViewport]);

  useLayoutEffect(() => {
    if (!camera) return;
    userAdjustedRef.current = true;
    minScaleRef.current = Math.min(MIN_SCALE, camera.scale);
    applyTransform(camera, false);
  }, [camera, applyTransform]);

  const enterObject = useCallback((target: GardenEntityRef) => {
    if (compositionActive) return;
    const ref: GardenEntityRef = target.kind === "path" && terrainCells?.some((cell) => cell.path === target.id && cell.isDir)
      ? { kind: "workspace", id: target.id } : target;
    if (ref.kind === "district" && !continuousZoom) {
      const district = districts.get(ref.id);
      if (district && size.width > 0 && size.height > 0) {
        const nextScale = Math.min(MAX_SCALE, Math.max(0.01, Math.min(size.width, size.height) * 0.8 / (district.radius * 2)));
        userAdjustedRef.current = true;
        minScaleRef.current = Math.min(MIN_SCALE, nextScale);
        applyTransform({ scale: nextScale, position: { x: size.width / 2 - district.origin.x * nextScale, y: size.height / 2 - district.origin.y * nextScale } });
      }
      onFocusDistrict?.(ref.id);
    }
    if (onEnter) onEnter(ref);
    else if (ref.kind === "path") onOpenPath?.(ref.id);
  }, [districts, size, applyTransform, onFocusDistrict, onEnter, onOpenPath, terrainCells, compositionActive, continuousZoom]);

  // Progressive disclosure. Detail is a pure function of zoom and touches only
  // what is painted — the layout reserved the crown's footprint regardless, so
  // crossing a threshold cannot move a unit.
  const detail = useMemo(() => gardenDetailForScale(scale), [scale]);
  const selectedSkillRef = selectedKey?.startsWith("skill:")
    ? selectedKey.slice("skill:".length)
    : null;
  const selectedTerrainPath = selectedKey?.startsWith("path:")
    ? selectedKey.slice("path:".length)
    : null;
  const selectedAgentIdForThreads = selectedKey?.startsWith("agent:")
    ? selectedKey.slice("agent:".length)
    : null;

  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;
    const update = () => setSize({ width: el.clientWidth, height: el.clientHeight });
    update();
    const observer = new ResizeObserver(update);
    observer.observe(el);
    return () => observer.disconnect();
  }, []);

  // A resize changes the visible world rectangle without touching the transform,
  // so terrain would keep expanding against the old viewport without this.
  useEffect(() => {
    reportViewport();
  }, [size, reportViewport]);

  // Open the menu on right-click via a native listener on the container (the
  // contextmenu event bubbles up from Konva's canvas, but binding directly to
  // the DOM node is more reliable than React delegation through the canvas).
  // Resolve which agent, if any, sits under the cursor via Konva hit-testing.
  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;
    const onContextMenu = (e: MouseEvent) => {
      if (compositionActive) return;
      e.preventDefault();
      let agentId: string | null = null;
      try {
        const stage = stageRef.current;
        const pointer = stage?.getPointerPosition();
        if (stage && pointer) {
          const hit = stage.getIntersection(pointer);
          const group = hit?.findAncestor(`.${AGENT_UNIT_NAME}`, true);
          if (group) agentId = group.id() || null;
        }
      } catch {
        agentId = null;
      }
      setMenu({ x: e.clientX, y: e.clientY, agentId });
    };
    el.addEventListener("contextmenu", onContextMenu);
    return () => el.removeEventListener("contextmenu", onContextMenu);
  }, [compositionActive]);

  // Zoom about a fixed screen point, keeping the world point under it still.
  //
  // Scaling without this moves everything except the stage's own origin, so the
  // map slides across the viewport as it grows — which reads as panning, not
  // zooming, and is why the wheel felt like it was scrolling the canvas.
  const zoomAround = useCallback((screenPoint: { x: number; y: number }, factor: number) => {
    if (compositionActive) return;
    if (!stageRef.current) return;
    userAdjustedRef.current = true;
    const next = zoomAt(screenPoint, transformRef.current, factor, {
      min: minScaleRef.current,
      max: continuousZoom ? 100000 : MAX_SCALE,
    });
    if (next.scale === transformRef.current.scale) return;
    applyTransform(next);
    if (!selectedKey || compositionActive || continuousZoom) return;
    const separator = selectedKey.indexOf(":");
    const kind = selectedKey.slice(0, separator);
    const id = selectedKey.slice(separator + 1);
    let extent = 0;
    let threshold = 280;
    let ref: GardenEntityRef | undefined;
    if (kind === "agent") {
      const unit = agentUnits.find((unit) => unit.ref.id === id);
      if (unit) { extent = 140 * next.scale; ref = unit.ref; }
    } else if (kind === "district") {
      const district = districts.get(id);
      if (district) { extent = district.radius * 2 * next.scale; threshold = 340; ref = { kind, id }; }
    } else if (kind === "workspace" || kind === "path") {
      const cell = terrainCells?.find((cell) => cell.path === id);
      if (cell) { extent = Math.min(cell.rect.width, cell.rect.height) * next.scale; threshold = 400; ref = { kind, id }; }
    } else if (kind === "automation") {
      const route = routes.find((route) => route.input.id === id);
      if (route) {
        extent = Math.max(140, Math.max(...route.points.map((p) => p.x)) - Math.min(...route.points.map((p) => p.x)), Math.max(...route.points.map((p) => p.y)) - Math.min(...route.points.map((p) => p.y))) * next.scale;
        ref = { kind, id };
      }
    }
    if (diveLatchRef.current !== selectedKey || extent < threshold - 40) diveLatchRef.current = null;
    if (factor > 1 && ref && extent >= threshold && diveLatchRef.current !== selectedKey) {
      diveLatchRef.current = selectedKey;
      enterObject(ref);
    }
  }, [applyTransform, selectedKey, compositionActive, agentUnits, districts, terrainCells, routes, enterObject, continuousZoom]);

  /** Zoom about the middle of the viewport, for the keyboard and the buttons. */
  const zoomByStep = useCallback(
    (factor: number) => zoomAround({ x: size.width / 2, y: size.height / 2 }, factor),
    [zoomAround, size.width, size.height],
  );

  const panBy = useCallback((dx: number, dy: number) => {
    const stage = stageRef.current;
    if (!stage) return;
    userAdjustedRef.current = true;
    applyTransform({
      scale: transformRef.current.scale,
      position: { x: stage.x() - dx, y: stage.y() - dy },
    });
  }, [applyTransform]);

  // Fit the map into view until the user takes control of the viewport.
  //
  // The layout places units around each district's own origin, so coordinates
  // are freely negative and a district can sit hundreds of pixels off the
  // Stage's top-left. Opening onto a viewport that happens to show two of five
  // districts is not a map.
  //
  // Fitting only once is not enough: the container is measured by a
  // ResizeObserver, and its first measurement can precede the surrounding
  // layout settling. A single fit against that early size leaves the content
  // permanently off-centre. So it re-fits on every size and content change, and
  // stops for good the moment the user pans, zooms, or moves a unit — after
  // which the viewport is theirs and must never be yanked back.
  const userAdjustedRef = useRef(Boolean(camera));
  const applyFit = useCallback(
    (force: boolean) => {
      const stage = stageRef.current;
      if (!stage) return;
      const transform = fitTransform(
        [...agentUnits.map((unit) => unit.position), ...[...districts.values()].flatMap(({ origin, radius }) => [
          { x: origin.x - radius, y: origin.y - radius }, { x: origin.x + radius, y: origin.y + radius },
        ])],
        size,
      );
      if (!transform) return;

      // This runs on every telemetry tick, because the unit arrays are rebuilt
      // to keep status live. Writing an unchanged transform back would redraw
      // the layer each time for nothing.
      const applied = `${transform.position.x},${transform.position.y},${transform.scale}`;
      if (!force && applied === fitRef.current) return;
      fitRef.current = applied;
      // The wheel must not snap the user back the moment they touch it.
      minScaleRef.current = Math.min(MIN_SCALE, transform.scale);
      applyTransform(transform);
      setFit(applied);
    },
    [agentUnits, districts, size, applyTransform],
  );

  useEffect(() => {
    if (userAdjustedRef.current) return;
    applyFit(false);
  }, [applyFit]);

  /**
   * Fit on demand. Unlike the automatic fit this does not hand the viewport
   * back: the user asked for this framing, so it is theirs until they move
   * again, and a later telemetry tick must not re-frame around them.
   */
  const fitNow = useCallback(() => {
    applyFit(true);
    userAdjustedRef.current = true;
  }, [applyFit]);

  /**
   * Reset the arrangement, and hand the viewport back.
   *
   * Resetting only the layout is not a reset from where the user sits: if they
   * had zoomed into a corner, the map rearranges somewhere off screen and
   * nothing appears to happen. Releasing `userAdjusted` lets the automatic fit
   * re-frame the new arrangement, which is the visible half of the action.
   */
  const handleResetLayout = useCallback(() => {
    onResetLayout();
    userAdjustedRef.current = false;
    fitRef.current = null;
  }, [onResetLayout]);

  // One animation for every breathing halo. See `useGardenPulse` for why this
  // is not per unit.
  //
  // Keyed on *which* units are breathing rather than on the unit arrays: those
  // are rebuilt on every telemetry tick, and re-running the effect that often
  // would rescan the scene graph for no reason. Only a status crossing the
  // active boundary, or a unit appearing or leaving, changes what to animate.
  const pulsingKey = useMemo(
    () =>
      [
        ...paintedAgents.filter((unit) => isActiveAgentStatus(unit.status) && (!continuousZoom || scale * 32 < 150 || unit.ref.id === draggingAgentId)).map((u) => u.ref.id),
      ].join(","),
    [paintedAgents, continuousZoom, scale, draggingAgentId],
  );
  useGardenPulse(layerRef, pulsingKey);

  // Stable identities so `AgentUnit` can skip re-rendering agents that did not
  // change. A closure created per unit per render would defeat that.
  const handleSelectAgent = useCallback(
    (id: string) => onSelect({ kind: "agent", id }),
    [onSelect],
  );
  const handleSelectSkill = useCallback(
    (glyph: GardenSkillGlyph) => onSelect({ kind: "skill", id: glyph.entryRef }),
    [onSelect],
  );
  const handleOpenSkill = useCallback(
    (glyph: GardenSkillGlyph) => onEnter ? enterObject({ kind: "skill", id: glyph.entryRef }) : onOpenSkill?.(glyph),
    [onEnter, enterObject, onOpenSkill],
  );
  const handleEnterAgent = useCallback((id: string) => enterObject({ kind: "agent", id }), [enterObject]);
  const handleDragAgentStart = useCallback((id: string) => {
    setDraggingAgentId(id);
    onDraggingAgentChange?.(id);
  }, [onDraggingAgentChange]);
  const handleDragAgent = useCallback(
    (id: string, x: number, y: number) => {
      setDraggingAgentId(null);
      onDraggingAgentChange?.(null);
      if (compositionActive) return;
      userAdjustedRef.current = true;
      onMoveUnit(unitKey({ kind: "agent", id }), x, y);
    },
    [onMoveUnit, compositionActive, onDraggingAgentChange],
  );

  // The wheel always zooms, and never scrolls. A canvas that pans on wheel and
  // zooms on modifier-wheel forces the user to discover which they are doing by
  // trying it; one gesture with one meaning does not.
  const handleWheel = (e: Konva.KonvaEventObject<WheelEvent>) => {
    if (compositionActive) return;
    e.evt.preventDefault();
    const stage = stageRef.current;
    const pointer = stage?.getPointerPosition() ?? { x: size.width / 2, y: size.height / 2 };
    zoomAround(pointer, wheelZoomFactor(e.evt.deltaY, e.evt.deltaMode));
  };

  const handleKeyDown = (e: React.KeyboardEvent<HTMLDivElement>) => {
    if (compositionActive) return;
    if (e.target !== e.currentTarget) return;
    // A pan step in screen pixels, so it feels the same at every zoom level.
    const step = e.shiftKey ? PAN_STEP * 4 : PAN_STEP;
    const actions: Record<string, () => void> = {
      Escape: () => onOpenParent?.(),
      "+": () => zoomByStep(KEY_ZOOM_STEP),
      "=": () => zoomByStep(KEY_ZOOM_STEP),
      "-": () => zoomByStep(1 / KEY_ZOOM_STEP),
      _: () => zoomByStep(1 / KEY_ZOOM_STEP),
      "0": fitNow,
      f: fitNow,
      ArrowLeft: () => panBy(-step, 0),
      ArrowRight: () => panBy(step, 0),
      ArrowUp: () => panBy(0, -step),
      ArrowDown: () => panBy(0, step),
    };
    const action = actions[e.key];
    if (!action) return;
    e.preventDefault();
    action();
  };

  // At Habitat, overlapping screen targets resolve to the district population.
  const populations = districtPopulations(agentUnits, districts, bands, scale, districtByAgentId);
  const clusteredAgents = new Set([...populations.values()].filter((population) => population.clustered).flatMap((population) => population.agentIds));
  const labelWidths = agentLabelWidths(continuousZoom ? agentUnits : agentUnits.filter(isWorkstream), scale);
  const objects: { ref: GardenEntityRef; label: string; position: { x: number; y: number } }[] = [
    ...[...districts].map(([id, district]) => ({ ref: { kind: "district" as const, id }, label: `${districtLabels?.get(id) ?? id}, district, ${populations.get(id)?.summary ?? "0 agents"}`, position: { x: district.origin.x, y: populations.get(id)?.clustered ? district.origin.y : district.origin.y - district.radius } })),
    ...agentUnits.filter((unit) => !clusteredAgents.has(unit.ref.id)).map((unit) => ({ ref: unit.ref, label: `${unit.label}, agent, ${unit.status}`, position: unit.position })),
    ...visibleRoutes.map((route) => ({ ref: { kind: "automation" as const, id: route.input.id }, label: `${route.presentation.summary}, automation${route.presentation.markers.length ? `, ${route.presentation.markers.map((marker) => marker.label).join(", ")}` : ""}`, position: route.anchor })),
    ...visibleTerrainCells.map((cell) => ({ ref: { kind: cell.isDir ? "workspace" as const : "path" as const, id: cell.path }, label: `${cell.name}, ${cell.isDir ? "workspace" : "file"}`, position: { x: cell.rect.x + cell.rect.width / 2, y: cell.rect.y + cell.rect.height / 2 } })),
    ...agentUnits.filter(isWorkstream).flatMap((unit) => unit.crown.slice(0, 3).map((glyph) => ({ ref: { kind: "skill" as const, id: glyph.entryRef }, label: `${glyph.label}, skill`, position: { x: unit.position.x, y: unit.position.y - 30 } }))),
  ].filter((object, index, all) => all.findIndex((item) => unitKey(item.ref) === unitKey(object.ref)) === index)
    .filter(({ position }) => size.width === 0 || (position.x * scale + cameraPosition.x >= 0 && position.x * scale + cameraPosition.x <= size.width && position.y * scale + cameraPosition.y >= 0 && position.y * scale + cameraPosition.y <= size.height));
  const activeRovingKey = objects.some((object) => unitKey(object.ref) === rovingKey) ? rovingKey : objects[0] && unitKey(objects[0].ref);
  const selectedObject = objects.find((object) => unitKey(object.ref) === selectedKey);
  const selectObject = (target: GardenEntityRef) => {
    if (compositionActive) return;
    const ref: GardenEntityRef = target.kind === "path" && terrainCells?.some((cell) => cell.path === target.id && cell.isDir)
      ? { kind: "workspace", id: target.id } : target;
    if (ref.kind === "path" && onSelectPath) onSelectPath(ref.id);
    else onSelect(ref);
  };

  return (
    <div
      ref={containerRef}
      className="relative flex-1 min-h-0 min-w-0 overflow-hidden garden-canvas"
      style={compositionActive ? { pointerEvents: "none" } : undefined}
      data-garden-fit={fit ?? undefined}
      data-focused-district={focusedDistrictId ?? undefined}
      // Focusable so the canvas can own its navigation keys. Without a tabIndex
      // the map is reachable only by mouse, which is the one input the report
      // said was confusing.
      tabIndex={compositionActive ? -1 : 0}
      onKeyDown={handleKeyDown}
      role="region"
      aria-label={`Garden canvas showing ${agentUnits.length} agents and ${automationProjections.length} automations. Select a unit to read its status. Scroll to zoom, drag to pan, or use plus and minus to zoom, arrow keys to pan, and zero to fit.`}
    >
      <Stage
        ref={stageRef}
        // Retained Dockview surfaces can measure zero during mount, resize or
        // hide. Konva's perfect-draw buffer must remain drawable even then;
        // keep the real measurement above for fit and viewport reporting.
        width={Math.max(1, size.width)}
        height={Math.max(1, size.height)}
        draggable={!compositionActive}
        listening={!compositionActive}
        onWheel={handleWheel}
        onClick={(event) => { if (!compositionActive && event.target === event.currentTarget) onClearSelection?.(); }}
        onTap={(event) => { if (!compositionActive && event.target === event.currentTarget) onClearSelection?.(); }}
        onDblClick={(event) => {
          if (compositionActive || event.target !== event.currentTarget) return;
          zoomAround(stageRef.current?.getPointerPosition() ?? { x: size.width / 2, y: size.height / 2 }, KEY_ZOOM_STEP);
        }}
        onDragMove={(event) => {
          if (compositionActive || event.target !== event.currentTarget) return;
          const stage = stageRef.current;
          if (stage) applyTransform({ scale: transformRef.current.scale, position: { x: stage.x(), y: stage.y() } });
        }}
        onDragEnd={(event) => {
          // Only a pan of the Stage itself; a unit drag reports the unit as
          // target and is handled by onMoveUnit.
          if (compositionActive || event.target !== event.currentTarget) return;
          userAdjustedRef.current = true;
          const stage = stageRef.current;
          if (stage) {
            applyTransform({
              scale: transformRef.current.scale,
              position: { x: stage.x(), y: stage.y() },
            });
          }
        }}
      >
        <Layer ref={layerRef}>
          <Group opacity={continuousZoom ? 1 - revealBetween(scale, 4, 18) * .96 : 1}>
          <DistrictLayer viewport={paintViewport} continuousZoom={continuousZoom} districts={districts} labels={districtLabels} populations={populations} bands={bands} scale={scale} selectedKey={keyboardKey ?? selectedKey} theme={theme}
            onSelect={(id) => onSelect({ kind: "district", id })} onOpen={(id) => enterObject({ kind: "district", id })} />
          {terrainCells && terrainCells.length > 0 && terrainDistricts && (
            <Group opacity={continuousZoom ? revealBetween(scale, .35, .8) : 1}>
              <TerrainLayer
                cells={paintedTerrainCells}
                districts={terrainDistricts}
                scale={scale}
                theme={theme}
                paint={terrainPaint}
                selectedPath={selectedTerrainPath}
                highlightedPaths={highlightedPaths}
                onSelectPath={(id) => selectObject({ kind: "path", id })}
                onOpenPath={(id) => enterObject({ kind: "path", id })}
              />
              {terrainPaint && (
                <AttributionLayer
                  cells={terrainCells}
                  paint={terrainPaint}
                  agentUnits={agentUnits}
                  selectedAgentId={selectedAgentIdForThreads}
                  selectedPath={selectedTerrainPath}
                  theme={theme}
                />
              )}
            </Group>
          )}
          <AutomationRoutesLayer viewport={paintViewport} continuousZoom={continuousZoom} mode="routes" routes={visibleRoutes} theme={theme} scale={scale} selectedKey={keyboardKey ?? selectedKey} onSelect={onSelect} onOpen={enterObject} />
          </Group>
          {paintedAgents.filter((unit) => continuousZoom || !clusteredAgents.has(unit.ref.id)).map((unit) => (
            <AgentUnit
              key={unitKey(unit.ref)}
              unit={unit}
              dragging={draggingAgentId === unit.ref.id}
              onDragStart={handleDragAgentStart}
              selected={selectedKey === unitKey(unit.ref) || keyboardKey === unitKey(unit.ref)}
              highlighted={highlightedAgentIds?.has(unit.ref.id) ?? false}
              detail={detail}
              signal={!isWorkstream(unit)}
              scale={scale}
              continuousZoom={continuousZoom}
              populationOpacity={continuousZoom && clusteredAgents.has(unit.ref.id) ? revealBetween((districts.get(agentDistrict(unit, districts, districtByAgentId) ?? "")?.radius ?? 120) * 2 * scale, 80, 240) : 1}
              labelWidthPx={labelWidths.get(unit.ref.id)}
              draggable={!compositionActive && isWorkstream(unit)}
              theme={theme}
              selectedSkillRef={selectedSkillRef}
              onSelect={handleSelectAgent}
              onOpen={handleEnterAgent}
              onSelectSkill={handleSelectSkill}
              onOpenSkill={handleOpenSkill}
              onDragEnd={handleDragAgent}
            />
          ))}
          <Group opacity={continuousZoom ? 1 - revealBetween(scale, 4, 18) * .96 : 1}><AutomationRoutesLayer viewport={paintViewport} continuousZoom={continuousZoom} mode="markers" routes={visibleRoutes} theme={theme} scale={scale} selectedKey={keyboardKey ?? selectedKey} onSelect={onSelect} onOpen={enterObject} /></Group>
        </Layer>
      </Stage>
      {!compositionActive && <div className="garden-object-controls" aria-label="Garden objects">
        {objects.map((object, index) => <button key={unitKey(object.ref)} type="button"
          className="garden-object-control" data-garden-object={unitKey(object.ref)}
          style={{ left: object.position.x * scale + cameraPosition.x, top: object.position.y * scale + cameraPosition.y }}
          tabIndex={activeRovingKey === unitKey(object.ref) ? 0 : -1}
          aria-label={`${object.label}. Space selects; Enter opens.`}
          aria-pressed={selectedKey === unitKey(object.ref)}
          onFocus={() => { setRovingKey(unitKey(object.ref)); setKeyboardKey(unitKey(object.ref)); }}
          onBlur={() => setKeyboardKey(null)}
          onClick={() => selectObject(object.ref)}
          onKeyDown={(event) => {
            event.stopPropagation();
            if (event.key === " " || event.key === "Enter" || event.key === "Escape") {
              event.preventDefault();
              if (event.key === " ") selectObject(object.ref);
              else if (event.key === "Enter") enterObject(object.ref);
              else onOpenParent?.();
            } else if (["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", "Home", "End"].includes(event.key)) {
              event.preventDefault();
              const next = event.key === "Home" ? 0 : event.key === "End" ? objects.length - 1 : (index + (event.key === "ArrowLeft" || event.key === "ArrowUp" ? -1 : 1) + objects.length) % objects.length;
              const controls = event.currentTarget.parentElement?.querySelectorAll<HTMLButtonElement>("button");
              controls?.[next]?.focus();
            }
          }}>{object.label}</button>)}
      </div>}
      {!compositionActive && selectedObject && <button type="button" className="garden-enter-control" onClick={() => enterObject(selectedObject.ref)}>Enter {selectedObject.label.split(",")[0]}</button>}
      <div
        hidden={compositionActive}
        style={compositionActive ? { display: "none" } : undefined}
        data-testid="garden-viewport-controls"
        className="absolute bottom-3 right-3 z-40 flex items-center gap-1 rounded-md border border-wardian-border bg-[var(--color-wardian-bg)]/90 px-1 py-1 text-[11px] shadow-sm backdrop-blur"
      >
        <button
          type="button"
          aria-label="Zoom out"
          title="Zoom out (-)"
          className="rounded px-1.5 py-0.5 text-muted hover:text-primary"
          onClick={() => zoomByStep(1 / KEY_ZOOM_STEP)}
        >
          &minus;
        </button>
        {/* Doubles as the readout that tells you how far out you are, which is
            the thing a blank-looking canvas never explains by itself. */}
        <span
          data-testid="garden-zoom-level"
          className="min-w-[3.5rem] text-center tabular-nums text-muted"
        >
          {Math.round(scale * 100)}%
        </span>
        <button
          type="button"
          aria-label="Zoom in"
          title="Zoom in (+)"
          className="rounded px-1.5 py-0.5 text-muted hover:text-primary"
          onClick={() => zoomByStep(KEY_ZOOM_STEP)}
        >
          +
        </button>
        <span className="mx-0.5 h-3 w-px bg-wardian-border" aria-hidden="true" />
        <button
          type="button"
          data-testid="garden-fit-view"
          title="Fit everything in view (0)"
          className="rounded px-1.5 py-0.5 text-muted hover:text-primary"
          onClick={fitNow}
        >
          Fit
        </button>
      </div>
      <div className="pointer-events-none absolute bottom-3 left-1/2 z-10 -translate-x-1/2 rounded-md border border-wardian-border bg-[var(--color-wardian-bg)]/80 px-2 py-1 text-[10px] text-muted-neutral shadow-sm backdrop-blur">
        {continuousZoom ? "Wheel to zoom · Alt + wheel to read · drag to pan · Escape to return" : compositionActive ? "Scroll to read · Escape or breadcrumbs to return" : "Scroll to zoom · drag to pan · arrows to move · 0 to fit"}
      </div>
      {menu && !compositionActive && (
        <GardenContextMenu
          x={menu.x}
          y={menu.y}
          agentId={menu.agentId}
          onOpenAgent={onOpenAgent}
          onResetLayout={handleResetLayout}
          onClose={() => setMenu(null)}
        />
      )}
    </div>
  );
};
