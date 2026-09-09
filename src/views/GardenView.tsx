import React, { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { AgentConfig, AgentTelemetry } from "../types";
import type { AgentInteractions, AgentTeam, Watchlist } from "../layout/watchlist/types";
import { buildAgentGraph, type GraphRelationshipReason } from "../features/graph/graphProjection";
import {
  buildAgentUnits,
  buildAutomationUnits,
  computeGardenLayout,
  gardenLayoutSignature,
} from "../features/garden/gardenProjection";
import { GardenCanvas } from "../features/garden/GardenCanvas";
import { unitKey, type GardenPosition, type GardenEntityRef } from "../features/garden/garden.types";
import { enterGardenObject, gardenRecordKind, type GardenNavigationFrame, type GardenCamera, type GardenTimeLens } from "../features/garden/gardenNavigation";
import { GardenAgentInterior } from "../features/garden/GardenAgentInterior";
import { createGardenContentsCache } from "../features/garden/useGardenAgentContents";
import { GardenWorkspaceInterior } from "../features/garden/GardenWorkspaceInterior";
import { GardenRecord } from "../features/garden/GardenRecord";
import { GardenAutomationInterior } from "../features/garden/GardenAutomationInterior";
import { GardenSpatialCell } from "../features/garden/GardenSpatialCell";
import { agentCellBounds, cameraForBounds, projectBounds, recordPlaneBounds, type GardenWorldBounds } from "../features/garden/gardenSpatialZoom";
import { useGardenCameraMotion } from "../features/garden/useGardenCameraMotion";
import { wheelZoomFactor } from "../utils/wheelZoom";
import { zoomAt } from "../features/garden/gardenViewport";
import { normalizeEntityPath } from "../features/garden/entityRef";
import { gardenAgentStatusColor } from "../features/garden/gardenStatus";
import "../features/garden/gardenComposition.css";
import {
  GARDEN_AGENT_STATUS_LEGEND,
  GARDEN_AREA_NOTE,
  GARDEN_CENTRALITY_NOTE,
  GARDEN_CHANGE_LEGEND,
  gardenAgentStatusLabel,
  gardenChangeBaselineLabel,
  gardenGroundLabel,
  gardenSkillReachLabel,
  gardenAutomationStatusLabel,
} from "../features/garden/gardenStatus";
import { agentsCarrying, crownPositions, CROWN_CAP } from "../features/garden/skillGlyphs";
import { useGardenAutomations } from "../features/garden/useGardenAutomations";
import { useGardenSkills } from "../features/garden/useGardenSkills";
import { useGardenReach } from "../features/garden/useGardenReach";
import { useGardenTerrain } from "../features/garden/useGardenTerrain";
import { useTerrainChanges } from "../features/garden/useTerrainChanges";
import { useTerrainOpen } from "../features/garden/useTerrainOpen";
import { basename as terrainCellName } from "../features/garden/terrain";
import type { TerrainViewport } from "../features/garden/terrainFrontier";
import { useGardenStore } from "../store/useGardenStore";
import { useLibraryStore } from "../store/useLibraryStore";
import { useAutomationsView } from "../store/useAutomationsView";
import { useAppShellWorkbenchNavigation } from "../layout/AppShell";
import type { GardenSurfaceState } from "../features/workbench/surfaces/coreSurfaceMetadata";

const ALL_REASONS: Set<GraphRelationshipReason> = new Set([
  "same_team",
  "shared_workspace",
  "same_worktree",
]);

/** Shared empty set, so "nothing highlighted" keeps a stable identity. */
const EMPTY_HIGHLIGHT: ReadonlySet<string> = new Set<string>();

/**
 * Hold a dropped unit inside the district it belongs to.
 *
 * A drag says where within its neighbourhood a unit should sit. It cannot say
 * which neighbourhood the unit is in — that comes from canonical facts about the
 * agent, not from where a cursor was released — so a drop into a neighbouring
 * district is a placement the map cannot honour without lying about membership.
 *
 * Letting it through was worse than an approximation. The unit stayed a member
 * of its own district at an enormous offset, so the district grew to that size,
 * every ring grew with it, and the unit rode its own origin outward: dropped on
 * a neighbour, it landed 600 units away, and the map inflated 2.3x in one drag.
 *
 * Clamping to the boundary keeps the gesture honest — you can place a unit
 * anywhere in its own territory, and the edge is where the territory ends.
 */
function clampToDistrict(
  point: GardenPosition,
  where: { districtOrigin: GardenPosition; districtRadius: number },
): GardenPosition {
  const dx = point.x - where.districtOrigin.x;
  const dy = point.y - where.districtOrigin.y;
  const distance = Math.hypot(dx, dy);
  if (distance <= where.districtRadius || distance === 0) return point;
  const scale = where.districtRadius / distance;
  return {
    x: where.districtOrigin.x + dx * scale,
    y: where.districtOrigin.y + dy * scale,
  };
}

export interface GardenViewProps {
  visibility?: "visible" | "hidden";
  rendererActive?: boolean;
  initialSurfaceState?: GardenSurfaceState;
  onSurfaceStateChange?: (state: GardenSurfaceState) => void;
  filteredAgents: AgentConfig[];
  telemetry: Record<string, AgentTelemetry>;
  teams: AgentTeam[];
  activeList: Watchlist | null;
  interactions: AgentInteractions;
  selectedAgentIds: Set<string>;
  offAgentIds: Set<string>;
  onSelectionChange: (ids: Set<string>) => void;
  onOpenAgent?: (agentId: string) => void;
  /** @deprecated Legacy flag-off adapter. Workbench surfaces use onOpenAgent. */
  onOpenAgentInGrid?: (agentId: string) => void;
}

export const GardenView: React.FC<GardenViewProps> = ({
  filteredAgents,
  telemetry,
  teams,
  activeList,
  interactions,
  selectedAgentIds,
  offAgentIds,
  onSelectionChange,
  onOpenAgent,
  onOpenAgentInGrid,
  visibility = "visible",
  rendererActive = true,
  initialSurfaceState,
  onSurfaceStateChange,
}) => {
  const scene = useGardenStore((s) => s.scene);
  const navigation = useAppShellWorkbenchNavigation();
  const pinUnit = useGardenStore((s) => s.pin);
  const visitUnit = useGardenStore((s) => s.visit);
  const adoptScene = useGardenStore((s) => s.adoptScene);
  const resetLayout = useGardenStore((s) => s.reset);
  const skillInputs = useGardenSkills(visibility === "visible");
  // Deep-links into the Library the same way the agent config panel's "Manage
  // skills" affordance does, so the Garden does not invent a second navigation
  // path to the same surface.
  const openLibraryAt = useLibraryStore((s) => s.openLibraryAt);
  const savedTrail = initialSurfaceState?.trail ?? [];
  const missingAnchor = savedTrail.findIndex((frame) => !frame.bounds && frame.ref.kind !== "agent" && frame.ref.kind !== "district");
  const restoredTrail = missingAnchor < 0 ? savedTrail : savedTrail.slice(0, missingAnchor);
  const legacyCameraRestore = useRef(restoredTrail.some((frame) => frame.ref.kind === "agent" && !frame.bounds));
  const [recoveryNotice, setRecoveryNotice] = useState(missingAnchor < 0 ? null :
    `Returned to ${restoredTrail[restoredTrail.length - 1]?.label ?? "Habitat"}; the saved record's location is unavailable.`);

  // Canvas highlight is keyed by unitKey so agent and automation ids can't collide,
  // and it stays local so selecting an automation never leaks into the app's
  // agent-only selection set. Agent clicks still propagate up (for Grid routing).
  const [selectedKey, setSelectedKey] = useState<string | null>(
    missingAnchor < 0 ? initialSurfaceState?.selected_unit_key ?? null : restoredTrail.length ? unitKey(restoredTrail[restoredTrail.length - 1].ref) : null,
  );
  const [trail, setTrail] = useState<GardenNavigationFrame[]>(restoredTrail);
  const [camera, setCamera] = useState<GardenCamera | undefined>(initialSurfaceState?.camera);
  const [contentsCache] = useState(createGardenContentsCache);
  const [draggingAgentId, setDraggingAgentId] = useState<string | null>(null);
  const viewRef = useRef<HTMLDivElement>(null);
  const motion = useGardenCameraMotion(camera, setCamera);
  const panStart = useRef<{ pointer: number; x: number; y: number; camera: GardenCamera } | null>(null);
  const panned = useRef(false);
  const [candidate, setCandidate] = useState<GardenNavigationFrame | null>(null);
  const [timeLens, setTimeLens] = useState<GardenTimeLens>(initialSurfaceState?.time_lens ?? "recent");
  const currentFrame = trail[trail.length - 1];
  const {
    automations: automationInputs,
    retainedAutomations = [],
    truncated: automationsTruncated,
    nextOffset: definitionsNextOffset,
    runsNextOffset,
    sourceEvidence: automationSources,
    loading: automationLoading,
    loadMore: loadMoreAutomations,
    error: automationError,
    refresh: refreshAutomations,
  } = useGardenAutomations(visibility === "visible", {
    retainedProjectionIds: trail.filter((frame) => frame.ref.kind === "automation").map((frame) => frame.ref.id),
  });
  // Focused evidence outlives its transient map trail without repopulating the map.
  const compositionAutomations = [...new Map([...automationInputs, ...retainedAutomations].map((item) => [item.id, item])).values()];
  const onSurfaceStateChangeRef = useRef(onSurfaceStateChange);
  onSurfaceStateChangeRef.current = onSurfaceStateChange;
  const surfaceSnapshot = useRef({ selected_unit_key: selectedKey, trail, camera, time_lens: timeLens });
  surfaceSnapshot.current = { selected_unit_key: selectedKey, trail, camera, time_lens: timeLens };
  // Selection and navigation persist immediately; a moving camera settles once
  // the gesture ends instead of causing Workbench/window IPC on every frame.
  useEffect(() => {
    onSurfaceStateChangeRef.current?.(surfaceSnapshot.current);
  }, [selectedKey, trail, timeLens]);
  useEffect(() => {
    const timer = window.setTimeout(() => onSurfaceStateChangeRef.current?.(surfaceSnapshot.current), 250);
    return () => window.clearTimeout(timer);
  }, [camera]);
  useEffect(() => () => { onSurfaceStateChangeRef.current?.(surfaceSnapshot.current); }, []);

  const projection = useMemo(
    () =>
      buildAgentGraph({
        agents: filteredAgents,
        telemetry,
        teams,
        activeList,
        interactions,
        selectedAgentIds,
        enabledReasons: ALL_REASONS,
        offAgentIds,
      }),
    [filteredAgents, telemetry, teams, activeList, interactions, selectedAgentIds, offAgentIds],
  );

  // Which districts coordinate others, so the lattice can seat them nearer the
  // middle. Roots come from the roster rather than from `layout.districts`,
  // because this feeds the layout and reading them back out of it would close a
  // loop. Fetched once per root set — see `useGardenReach` on why geometry does
  // not subscribe to writes the way the paint does.
  const reach = useGardenReach(visibility === "visible", filteredAgents);

  // Layout output — district cells and settled positions — is carried forward
  // through a ref, never through the reactive dependency chain.
  //
  // Routing it back through state makes the layout an input to itself: each pass
  // produces slightly different positions, which triggers another pass. A
  // convergence epsilon is not a sufficient brake, because the pipeline is not
  // guaranteed to converge monotonically — overlap removal ranks units by their
  // incoming positions, so two units sitting at nearly the same coordinate can
  // swap separation order between passes and oscillate indefinitely. When that
  // happens the write-back becomes an unbounded render loop (React error #185)
  // rather than a slightly jittery map.
  //
  // Excluding positions from the dependencies also states the intent correctly:
  // settled positions are a *result*, and only genuine inputs — the roster, the
  // teams, the blueprints, and the user's own placements — should provoke a
  // relayout.
  const carriedSceneRef = useRef(scene);

  // Geometry is keyed on a digest of the inputs that may legitimately move
  // something, not on the projection's identity. The projection is rebuilt on
  // every telemetry tick; running the pipeline that often would advance
  // convergence a few pixels each time and the map would drift continuously for
  // no reason a user could see. `projectionRef` supplies the current value
  // without making identity a trigger.
  const { pins, exclusions } = scene;
  const projectionRef = useRef(projection);
  projectionRef.current = projection;
  // Carried the same way and for the same reason: `signature` already covers
  // every reach change that may move something, so the array's identity must
  // not be a second trigger.
  const reachRef = useRef(reach);
  reachRef.current = reach;
  const signature = useMemo(
    () => gardenLayoutSignature(projection, teams, [], skillInputs, reach),
    [projection, teams, skillInputs, reach],
  );

  // A reset discards the scene rather than advancing it, and the carried copy
  // would otherwise put everything straight back: it holds the settled positions
  // and district cells the reset just cleared, and the next pass warm-starts
  // from them. The counter is what tells the two cases apart.
  const generation = useGardenStore((s) => s.generation);
  const generationRef = useRef(generation);

  const layout = useMemo(() => {
    if (generationRef.current !== generation) {
      generationRef.current = generation;
      carriedSceneRef.current = scene;
    }
    const result = computeGardenLayout({
      projection: projectionRef.current,
      teams,
      automations: [],
      skills: skillInputs,
      reach: reachRef.current,
      scene: { ...carriedSceneRef.current, pins, exclusions },
    });
    carriedSceneRef.current = result.scene;
    return result;
    // `signature` stands in for projection/teams/automations: it captures exactly
    // the geometry-relevant content and omits status, colour, and selection.
    //
    // `scene` is read only on a generation change, and is deliberately not a
    // dependency: it is republished by every adopted layout, and depending on it
    // would make the layout an input to itself.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [signature, pins, exclusions, generation]);
  const { placement } = layout;
  // How many districts the arrangement is actually making a claim about. Zero
  // means every district keeps to itself and the centrality legend would be
  // explaining a distinction nothing on screen is drawing.
  const coordinatingDistricts = useMemo(
    () => [...layout.reachTiers.values()].filter((tier) => tier > 0).length,
    [layout.reachTiers],
  );

  // Display fields are attached per render from the live projection, so status
  // and colour stay current without touching geometry.
  const agentUnits = useMemo(
    () => buildAgentUnits(projection, layout.positions, layout.crowns),
    [projection, layout.positions, layout.crowns],
  );
  const automationUnits = useMemo(
    () => buildAutomationUnits(automationInputs, layout.positions),
    [automationInputs, layout.positions],
  );

  // Persist district cells and settled positions so a later session can
  // warm-start from them; without that the map re-derives from scratch on every
  // reload and visibly rearranges itself. `adoptScene` ignores a scene that has
  // not moved materially, so this cannot churn storage.
  const adoptSceneRef = useRef(adoptScene);
  adoptSceneRef.current = adoptScene;
  useEffect(() => {
    adoptSceneRef.current(layout.scene);
  }, [layout]);

  // Fall back to an externally-selected single agent (e.g. chosen in Grid) when
  // there is no local Garden selection yet.
  const externalAgentKey =
    selectedAgentIds.size === 1 ? unitKey({ kind: "agent", id: [...selectedAgentIds][0] }) : null;
  const activeSelectionKey = selectedKey ?? (trail.length === 0 ? externalAgentKey : null);

  // A selected skill has no unit of its own, so it is summarized from its
  // carriers instead — and those carriers light up on the map. This is the
  // reverse index that replaces "go to where the skill lives": the honest
  // answer to "where is this used?" is a set, not a point.
  const selectedSkillRef = activeSelectionKey?.startsWith("skill:")
    ? activeSelectionKey.slice("skill:".length)
    : null;
  const selectedSkillLabel = selectedSkillRef
    ? (skillInputs.find((skill) => skill.entryRef === selectedSkillRef)?.label ?? selectedSkillRef)
    : null;
  const selectedPath = activeSelectionKey?.startsWith("path:") || activeSelectionKey?.startsWith("workspace:")
    ? activeSelectionKey.slice(activeSelectionKey.indexOf(":") + 1)
    : null;
  const selectedAgentId = activeSelectionKey?.startsWith("agent:")
    ? activeSelectionKey.slice("agent:".length)
    : null;

  // Terrain is ingested against the visible world rectangle, so the canvas
  // reports its viewport up. It arrives already coalesced to one value per
  // animation frame; the expansion pass inside the hook debounces on top of
  // that, so a pan produces listings once it settles rather than while it moves.
  const [viewport, setViewport] = useState<TerrainViewport | null>(null);
  const terrainEnabled = visibility === "visible" && rendererActive;
  const terrain = useGardenTerrain({
    enabled: terrainEnabled,
    rootOnly: true,
    districts: layout.districts,
    viewport,
  });
  // Change review is fetched per visible workspace root, not per agent:
  // attribution already spans every conversation in a workspace, so one call
  // answers for all of them.
  const changeRoots = useMemo(() => [...new Set([...terrain.visibleRoots, ...[...layout.districts.values()].flatMap((district) => district.roots)])], [terrain.visibleRoots, layout.districts]);
  const changes = useTerrainChanges({ enabled: terrainEnabled, roots: changeRoots });
  const activityCells = useMemo(() => terrain.cells.filter((cell) => cell.depth === 0 || changes.paint.has(cell.path)), [terrain.cells, changes.paint]);

  // Both reverse indexes, in one place. A skill and a piece of ground answer
  // "who?" the same way — with a set of agents — because neither is a thing you
  // can navigate to. Selecting an agent runs it the other direction and lights
  // up the disk it has written to, across every district.
  const highlightedAgentIds = useMemo(() => {
    if (selectedSkillRef) return agentsCarrying(layout.crowns, selectedSkillRef);
    if (selectedPath) return new Set(changes.paint.get(selectedPath)?.agentIds ?? []);
    if (activeSelectionKey?.startsWith("automation:")) return new Set(automationInputs.find((item) => item.id === activeSelectionKey.slice(11))?.agentIds ?? []);
    return EMPTY_HIGHLIGHT;
  }, [selectedSkillRef, selectedPath, layout.crowns, changes.paint, activeSelectionKey, automationInputs]);

  const highlightedPaths = useMemo(() => {
    if (!selectedAgentId) return EMPTY_HIGHLIGHT;
    const written = new Set<string>();
    for (const [path, paint] of changes.paint) {
      if (paint.agentIds.includes(selectedAgentId)) written.add(path);
    }
    return written;
  }, [selectedAgentId, changes.paint]);

  const openPath = useTerrainOpen({ entries: changes.entries, baseline: changes.baseline });
  // Stable identities: `TerrainLayer` is memoized, and a fresh empty set or a
  // fresh closure per render would re-render two thousand ground cells on every
  // telemetry tick.
  const handleSelectPath = useCallback(
    (path: string) => setSelectedKey(unitKey({ kind: terrain.cells.some((cell) => cell.path === path && cell.isDir) ? "workspace" : "path", id: path })),
    [terrain.cells],
  );

  const selectedUnit = [...agentUnits, ...automationUnits].find(
    (unit) => unitKey(unit.ref) === activeSelectionKey,
  );
  const selectedPaint = selectedPath ? changes.paint.get(selectedPath) : undefined;
  const summaryLabel =
    selectedSkillLabel ?? (selectedPath ? terrainCellName(selectedPath) : null) ?? selectedUnit?.label ?? null;
  const summaryStatus = selectedSkillRef
    ? gardenSkillReachLabel(highlightedAgentIds.size)
    : selectedPath
      ? gardenGroundLabel(selectedPaint)
      : selectedUnit
        ? "status" in selectedUnit
          ? gardenAgentStatusLabel(selectedUnit.status)
          : gardenAutomationStatusLabel(selectedUnit.runStatus)
        : null;

  const districtByAgentId = useMemo(() => new Map([...placement].filter(([key]) => key.startsWith("agent:")).map(([key, value]) => [key.slice(6), value.districtId])), [placement]);
  const districtLabels = useMemo(() => new Map([...layout.districts.keys()].map((id) => [id, teams.find((team) => id === `team:${team.id}`)?.name ?? (id === "commons" ? "Commons" : terrainCellName(id.replace(/^[^:]+:/, "")))])), [layout.districts, teams]);
  const labelFor = (ref: GardenEntityRef): string => {
    if (ref.kind === "agent" || ref.kind === "identity") return filteredAgents.find((agent) => agent.session_id === ref.id)?.session_name ?? "Agent";
    if (ref.kind === "district") return districtLabels.get(ref.id) ?? "Workstream";
    if (ref.kind === "automation") return compositionAutomations.find((item) => item.id === ref.id)?.label ?? "Routine";
    if (ref.kind === "skill") return skillInputs.find((item) => item.entryRef === ref.id)?.label ?? ref.id;
    if (ref.kind === "memory") return "Memory";
    if (ref.kind === "stage") {
      try {
        const reference: unknown = JSON.parse(ref.id);
        if (Array.isArray(reference) && typeof reference[1] === "string") return reference[1];
      } catch { /* Invalid persisted references retain breadcrumb recovery. */ }
      return "Stage";
    }
    return terrainCellName(ref.id);
  };
  const viewSize = () => ({ width: viewRef.current?.clientWidth ?? 1, height: viewRef.current?.clientHeight ?? 1 });
  useEffect(() => {
    if (!legacyCameraRestore.current || !rendererActive) return;
    const frame = trail[trail.length - 1];
    if (frame?.ref.kind !== "agent") { legacyCameraRestore.current = false; return; }
    const unit = agentUnits.find((item) => item.ref.id === frame.ref.id);
    if (!unit && filteredAgents.some((agent) => agent.session_id === frame.ref.id)) return;
    legacyCameraRestore.current = false;
    if (unit) {
      const bounds = agentCellBounds(unit.position);
      setTrail([...trail.slice(0, -1), { ...frame, bounds }]);
      setCamera(cameraForBounds(bounds, { width: viewRef.current?.clientWidth ?? 1, height: viewRef.current?.clientHeight ?? 1 }, 720));
    } else {
      const ancestors = trail.filter((item) => item.ref.kind === "district");
      setTrail(ancestors);
      setSelectedKey(ancestors.length ? unitKey(ancestors[ancestors.length - 1].ref) : null);
    }
  }, [agentUnits, filteredAgents, rendererActive, trail]);
  const boundsFor = (ref: GardenEntityRef): GardenWorldBounds | undefined => {
    // Relationship ports travel to another inhabitant; they do not nest that agent inside a label.
    if (ref.kind === "agent") {
      const unit = agentUnits.find((item) => item.ref.id === ref.id);
      if (unit) return agentCellBounds(unit.position);
    }
    // Prefer the actual occurrence under focus: a shared skill can occur in several agents.
    const matches = [...(viewRef.current?.querySelectorAll<HTMLElement>("[data-garden-ref]") ?? [])]
      .filter((element) => element.dataset.gardenRef === unitKey(ref));
    const element = matches.find((item) => item === document.activeElement || item.contains(document.activeElement)) ?? matches[0];
    const root = viewRef.current?.getBoundingClientRect();
    if (element && root && camera) {
      // A memory unfolds from its visible seed, independent of caption or hit-target padding.
      const source = ref.kind === "memory" ? element.querySelector(".garden-object-mark-memory") ?? element : element;
      const rect = source.getBoundingClientRect();
      if (rect.width > 0 && rect.height > 0) return { x: (rect.left - root.left - camera.position.x) / camera.scale,
        y: (rect.top - root.top - camera.position.y) / camera.scale, width: rect.width / camera.scale, height: rect.height / camera.scale };
    }
    if (ref.kind === "identity") {
      const unit = agentUnits.find((item) => item.ref.id === ref.id);
      if (unit) return agentCellBounds(unit.position);
    }
    if (ref.kind === "district") {
      const district = layout.districts.get(ref.id);
      if (district) return { x: district.origin.x - district.radius, y: district.origin.y - district.radius, width: district.radius * 2, height: district.radius * 2 };
    }
    if (ref.kind === "workspace" || ref.kind === "path") {
      const cell = activityCells.find((item) => item.path === ref.id);
      if (cell) return cell.rect;
    }
    if (ref.kind === "automation") {
      const routine = compositionAutomations.find((item) => item.id === ref.id);
      const carrier = agentUnits.find((item) => routine?.agentIds?.includes(item.ref.id));
      if (carrier) return { x: carrier.position.x + 24, y: carrier.position.y - 8, width: 32, height: 24 };
      const district = [...layout.districts.values()].find((item) => item.roots.some((root) => routine?.workspacePaths?.includes(root)));
      if (district) return { x: district.origin.x - 16, y: district.origin.y - 12, width: 32, height: 24 };
    }
    if (ref.kind === "skill") {
      const carrier = agentUnits.find((item) => item.crown.some((glyph) => glyph.entryRef === ref.id));
      if (carrier) {
        const index = carrier.crown.findIndex((glyph) => glyph.entryRef === ref.id);
        const positions = crownPositions(Math.min(carrier.crown.length, CROWN_CAP.near) + (carrier.crown.length > CROWN_CAP.near ? 1 : 0));
        const point = positions[Math.min(index, positions.length - 1)];
        if (point) return { x: carrier.position.x + point.x - 6.5, y: carrier.position.y + point.y - 6.5, width: 13, height: 13 };
      }
    }
    return [...trail].reverse().find((frame) => unitKey(frame.ref) === unitKey(ref))?.bounds;
  };
  const selectObject = (ref: GardenEntityRef) => {
    setSelectedKey(unitKey(ref));
    const bounds = boundsFor(ref);
    if (bounds && ref.kind !== "district" && unitKey(ref) !== (currentFrame && unitKey(currentFrame.ref))) {
      setCandidate({ ref, label: labelFor(ref), camera, bounds });
    }
    if (ref.kind === "agent") { visitUnit(unitKey(ref)); onSelectionChange(new Set([ref.id])); }
  };
  const openCanonicalAgent = (id: string) => (onOpenAgent ?? onOpenAgentInGrid)?.(id);
  const enterObject = (ref: GardenEntityRef) => {
    setRecoveryNotice(null);
    if (currentFrame?.ref.kind === ref.kind && currentFrame.ref.id === ref.id && gardenRecordKind(ref.kind)) {
      if (ref.kind === "identity") openCanonicalAgent(ref.id);
      if (ref.kind === "skill") openLibraryAt("skills", ref.id);
      if (ref.kind === "path") openPath(ref.id);
      return;
    }
    let frames = trail;
    const district = ref.kind === "agent" ? districtByAgentId.get(ref.id) : undefined;
    if (frames.length === 0 && district) frames = [{ ref: { kind: "district", id: district }, label: districtLabels.get(district) ?? "Workstream", camera }];
    const bounds = candidate && unitKey(candidate.ref) === unitKey(ref) ? candidate.bounds : boundsFor(ref);
    setTrail(enterGardenObject(frames, { ref, label: labelFor(ref), camera, bounds }));
    setCandidate(null);
    setSelectedKey(unitKey(ref));
    if (bounds) motion.move(cameraForBounds(ref.kind === "agent" || ref.kind === "district" ? bounds : recordPlaneBounds(bounds), viewSize(), ref.kind === "district" ? 0 : 720), viewSize());
  };
  const returnTo = (length: number) => {
    const departed = trail[length];
    const finish = () => setTrail(trail.slice(0, length));
    if (departed?.camera) motion.move(departed.camera, viewSize(), finish);
    else if (length && trail[length - 1].bounds) motion.move(cameraForBounds(trail[length - 1].bounds!, viewSize()), viewSize(), finish);
    else finish();
    // Keep the departing world projections during the camera move, then retire them below.
    setCandidate(null);
    setSelectedKey(length ? unitKey(trail[length - 1].ref) : null);
    if (!length) onSelectionChange(new Set());
  };
  const selectionRef = activeSelectionKey ? { kind: activeSelectionKey.slice(0, activeSelectionKey.indexOf(":")), id: activeSelectionKey.slice(activeSelectionKey.indexOf(":") + 1) } as GardenEntityRef : null;
  const interiorOpen = !!currentFrame && currentFrame.ref.kind !== "district";

  const wheelAction = useRef<(event: WheelEvent) => void>(() => undefined);
  wheelAction.current = (event) => {
    if (!camera || !viewRef.current || (event.target instanceof Element && event.target.closest(".garden-navigation, .garden-selection"))) return;
    // Alt-wheel is an explicit content-scroll gesture; ordinary wheel always traverses scales.
    if (event.altKey) return;
    event.preventDefault(); event.stopPropagation(); motion.cancel();
    const rect = viewRef.current.getBoundingClientRect();
    const next = zoomAt({ x: event.clientX - rect.left, y: event.clientY - rect.top }, camera,
      wheelZoomFactor(event.deltaY, event.deltaMode), { min: Math.min(.04, camera.scale), max: 100000 });
    setCamera(next);
    if (event.deltaY < 0 && !interiorOpen && !candidate) {
      const unit = agentUnits.find((item) => {
        const screen = projectBounds(agentCellBounds(item.position), next);
        return screen.width >= 90 && event.clientX - rect.left >= screen.x && event.clientX - rect.left <= screen.x + screen.width
          && event.clientY - rect.top >= screen.y && event.clientY - rect.top <= screen.y + screen.height;
      });
      if (unit) setCandidate({ ref: unit.ref, label: unit.label, bounds: agentCellBounds(unit.position), camera });
    }
    if (event.deltaY < 0 && candidate?.bounds && candidate.bounds.width * next.scale >= 540) {
      const screen = projectBounds(candidate.bounds, next);
      const x = event.clientX - rect.left, y = event.clientY - rect.top;
      if (x >= screen.x && x <= screen.x + screen.width && y >= screen.y && y <= screen.y + screen.height) {
        setTrail(enterGardenObject(trail, candidate)); setCandidate(null);
      }
    }
    if (event.deltaY > 0 && currentFrame?.bounds && currentFrame.ref.kind !== "district") {
      const parent = trail[trail.length - 2];
      const parentBounds = parent?.bounds ?? (parent ? boundsFor(parent.ref) : undefined);
      // Small seeds must return after their parent has reappeared, not to an inert magnified shell.
      const exitWidth = parentBounds ? Math.min(140, currentFrame.bounds.width / parentBounds.width * 2000) : 140;
      if (currentFrame.bounds.width * next.scale < exitWidth) setTrail(trail.slice(0, -1));
    }
  };
  useEffect(() => {
    const element = viewRef.current;
    const onWheel = (event: WheelEvent) => wheelAction.current(event);
    element?.addEventListener("wheel", onWheel, { passive: false, capture: true });
    return () => element?.removeEventListener("wheel", onWheel, { capture: true });
  }, []);

  const spatialFrames = [...trail.filter((frame) => frame.ref.kind !== "district" && frame.ref.kind !== "agent"),
    ...(candidate && candidate.ref.kind !== "agent" && !trail.some((frame) => unitKey(frame.ref) === unitKey(candidate.ref)) ? [candidate] : [])];

  const renderContents = (frame: GardenNavigationFrame) => {
    const ref = frame.ref;
    if (ref.kind === "workspace") return <GardenWorkspaceInterior path={normalizeEntityPath(ref.id) ?? ref.id}
      entries={changes.entries} paint={changes.paint} lens={timeLens} baseline={changes.baseline} onLensChange={setTimeLens} selectedKey={activeSelectionKey} onSelect={selectObject} onEnter={enterObject} />;
    if (ref.kind === "automation" || ref.kind === "stage") return <GardenAutomationInterior
      automation={compositionAutomations.find((item) => item.id === (ref.kind === "automation" ? ref.id : [...trail].reverse().find((parent) => parent.ref.kind === "automation")?.ref.id))}
      agents={filteredAgents} selectedKey={activeSelectionKey} onSelect={selectObject} onEnter={enterObject}
      stageId={ref.kind === "stage" ? ref.id : undefined} onOpenDefinition={openPath}
      onInspectRun={(blueprintId, runId) => { useAutomationsView.getState().observeRun(blueprintId, runId); navigation?.open({ surface_type: "automations" }); }}
      onManageSchedule={() => { useAutomationsView.getState().setMode("monitor"); navigation?.open({ surface_type: "automations" }); }} />;
    return <GardenRecord target={ref} agent={filteredAgents.find((item) => item.session_id === ref.id)}
      glyph={[...layout.crowns.values()].flat().find((glyph) => glyph.entryRef === ref.id)} change={changes.entries.get(ref.id)}
      onOpenAgent={openCanonicalAgent} onOpenSkill={(id) => openLibraryAt("skills", id)} onOpenPath={openPath} />;
  };

  return (
    <div ref={viewRef} className="garden-view relative flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden"
      onPointerDownCapture={(event) => {
        if (event.button !== 0 || !camera || !(event.target instanceof Element)
          || !event.target.matches(".garden-spatial-cell, .garden-agent-interior, .garden-agent-interior-region, .garden-spatial-contents")) return;
        motion.cancel(); panned.current = false;
        panStart.current = { pointer: event.pointerId, x: event.clientX, y: event.clientY, camera };
        event.currentTarget.setPointerCapture(event.pointerId);
      }}
      onPointerMove={(event) => {
        const start = panStart.current;
        if (!start || start.pointer !== event.pointerId) return;
        const dx = event.clientX - start.x, dy = event.clientY - start.y;
        if (Math.hypot(dx, dy) > 4) panned.current = true;
        setCamera({ scale: start.camera.scale, position: { x: start.camera.position.x + dx, y: start.camera.position.y + dy } });
      }}
      onPointerUp={(event) => { if (panStart.current?.pointer === event.pointerId) { panStart.current = null; event.currentTarget.releasePointerCapture(event.pointerId); } }}
      onPointerCancel={() => { panStart.current = null; }}
      onClickCapture={(event) => { if (panned.current) { panned.current = false; event.stopPropagation(); } }}
      onKeyDown={(event) => {
        if (event.key === "Escape") { event.stopPropagation(); returnTo(Math.max(0, trail.length - 1)); return; }
        if (event.target !== event.currentTarget && !(event.target instanceof Element && event.target.matches(".garden-spatial-cell"))) return;
        if (!camera) return;
        const size = viewSize();
        if (["+", "=", "-", "_"].includes(event.key)) {
          event.preventDefault(); motion.cancel();
          setCamera(zoomAt({ x: size.width / 2, y: size.height / 2 }, camera, ["-", "_"].includes(event.key) ? .8 : 1.25, { min: .04, max: 100000 }));
        } else if (event.key === "0" || event.key === "f") { event.preventDefault(); returnTo(0); }
        else if (event.key.startsWith("Arrow")) {
          event.preventDefault(); motion.cancel();
          setCamera({ scale: camera.scale, position: { x: camera.position.x + (event.key === "ArrowLeft" ? 80 : event.key === "ArrowRight" ? -80 : 0),
            y: camera.position.y + (event.key === "ArrowUp" ? 80 : event.key === "ArrowDown" ? -80 : 0) } });
        }
      }}>
      <div className="garden-navigation">
        <nav aria-label="Garden breadcrumb"><button onClick={() => returnTo(0)}>Habitat</button>{trail.map((frame, index) => <React.Fragment key={`${index}:${unitKey(frame.ref)}`}><span aria-hidden="true">›</span><button aria-current={index === trail.length - 1 ? "location" : undefined} onClick={() => returnTo(index + 1)}>{frame.label}</button></React.Fragment>)}</nav>
        {(automationsTruncated || terrain.truncatedRoots.size > 0) && <details className="garden-coverage">
          <summary>Map coverage</summary>
          <div>
            {automationsTruncated && <>
              {definitionsNextOffset != null && <p>More automation definitions are available.</p>}
              {runsNextOffset != null && <><p>{automationSources?.runs.runs.length ?? "Some"} run records loaded. Some active or recent runs may be missing.</p>
                <p>Only active runs and runs updated within 24 hours appear on the map. Checking more records may leave the map unchanged.</p></>}
              <button type="button" disabled={automationLoading} onClick={() => void loadMoreAutomations()}>
                {automationLoading ? "Checking coverage…" : definitionsNextOffset != null ? runsNextOffset != null ? "Expand map coverage" : "Load more definitions" : "Check more runs"}
              </button>
            </>}
            {terrain.truncatedRoots.size > 0 && <><p>Some folders have additional contents.</p><button type="button"
              onClick={() => void Promise.all([...terrain.truncatedRoots].map((path) => terrain.loadMoreRoot(path)))}>Load more folder contents</button></>}
          </div>
        </details>}
      </div>
      <section
        aria-label="Garden status legend"
        className="sr-only"
      >
        <span className="font-bold text-primary">Status</span>
        {GARDEN_AGENT_STATUS_LEGEND.map((item) => (
          <span key={item.label} className="inline-flex items-center gap-1 text-muted-neutral">
            <span className={`h-1.5 w-1.5 rounded-full ${item.indicatorClass}`} aria-hidden="true" />
            {item.label}
          </span>
        ))}
        {changes.paint.size > 0 && (
          <>
            <span className="h-3 w-px bg-wardian-border" aria-hidden="true" />
            {/* The baseline is named rather than assumed: the ground uses a
                workspace-level baseline while the Changes pane may be showing an
                agent-scoped one, and the two must not differ silently. */}
            <span className="font-bold text-primary" title={gardenChangeBaselineLabel(changes.baseline)}>
              Ground
            </span>
            {GARDEN_CHANGE_LEGEND.map((item) => (
              <span key={item.kind} className="inline-flex items-center gap-1 text-muted-neutral">
                <span
                  className="h-1.5 w-1.5 rounded-sm"
                  style={{ backgroundColor: item.colorVar }}
                  aria-hidden="true"
                />
                {item.label}
              </span>
            ))}
            <span className="text-muted-neutral">{gardenChangeBaselineLabel(changes.baseline)}</span>
            {/* Area is a share of the parent folder, not a file size, and it
                reads as size unless it is said out loud — a loose file at a
                repository root is a peer of `src/` and is drawn as one. */}
            <span className="text-muted-neutral" title={GARDEN_AREA_NOTE}>
              Area = share of folder
            </span>
          </>
        )}
        {coordinatingDistricts > 0 && (
          <>
            <span className="h-3 w-px bg-wardian-border" aria-hidden="true" />
            {/* An arrangement that encodes a claim nobody can read is no better
                than one that encodes nothing, so the map says what its middle
                means — and only when some district is actually claiming it. */}
            <span className="text-muted-neutral" title={GARDEN_CENTRALITY_NOTE}>
              Centre = coordinates others
            </span>
          </>
        )}
      </section>
      {recoveryNotice && <p role="status" className="garden-recovery-notice">{recoveryNotice}</p>}
      <div
        id="garden-selection-summary"
        data-testid="garden-selection-summary"
        aria-live="polite"
        className="garden-selection"
      >
        {selectionRef ? (
          <><span className="font-semibold text-primary">{summaryLabel ?? labelFor(selectionRef)}</span><span className="text-muted">{summaryStatus ?? selectionRef.kind}</span>{(!currentFrame || unitKey(currentFrame.ref) !== unitKey(selectionRef)) && <button onClick={() => enterObject(selectionRef)}>{gardenRecordKind(selectionRef.kind) ? "Open record" : "Enter"}</button>}{selectedAgentId && <button onClick={() => openCanonicalAgent(selectedAgentId)}>Open agent session</button>}</>
        ) : (
          <span className="text-muted">Select to inspect · double-click or Enter to explore</span>
        )}
      </div>
      {automationError && <div className="absolute top-16 right-3 z-30 max-w-sm rounded-md border border-wardian-border bg-[var(--color-wardian-bg)] p-2 text-xs" role="status">Automation data is incomplete or stale. <button onClick={() => void refreshAutomations()}>Retry</button><details><summary>Details</summary>{automationError}</details></div>}
      {rendererActive ? <GardenCanvas
        continuousZoom
        agentUnits={agentUnits}
        automationUnits={automationUnits}
        automationProjections={automationInputs}
        districtLabels={districtLabels}
        districtByAgentId={districtByAgentId}
        focusedDistrictId={[...trail].reverse().find((frame) => frame.ref.kind === "district")?.ref.id}
        camera={camera}
        onCameraChange={setCamera}
        onDraggingAgentChange={setDraggingAgentId}
        onEnter={enterObject}
        onOpenParent={() => returnTo(Math.max(0, trail.length - 1))}
        onClearSelection={() => { setSelectedKey(null); onSelectionChange(new Set()); }}
        selectedKey={activeSelectionKey}
        highlightedAgentIds={highlightedAgentIds}
        terrainCells={activityCells}
        terrainDistricts={layout.districts}
        terrainPaint={changes.paint}
        highlightedPaths={highlightedPaths}
        onSelectPath={handleSelectPath}
        onOpenPath={(path) => enterObject({ kind: terrain.cells.some((cell) => cell.path === path && cell.isDir) ? "workspace" : "path", id: path })}
        onViewportChange={setViewport}
        onSelect={selectObject}
        onOpenAgent={(agentId) => (onOpenAgent ?? onOpenAgentInGrid)?.(agentId)}
        onOpenSkill={(glyph) => openLibraryAt("skills", glyph.entryRef)}
        onMoveUnit={(key, x, y) => {
          // A drag is a pin, stored relative to the unit's district so the
          // placement survives the district being relocated on the grid.
          const where = placement.get(key);
          if (!where) return;
          pinUnit(key, where.districtId, clampToDistrict({ x, y }, where), where.districtOrigin);
        }}
        onResetLayout={() => {
          resetLayout();
          setSelectedKey(null);
        }}
      /> : (
        // A visible surface reaches this branch too: the renderer is mounted
        // from an effect so the surface frame paints first, and the Konva chunk
        // is fetched after that. Saying "paused while hidden" there would be
        // wrong, so the two states read differently.
        <div className="flex flex-1 items-center justify-center text-sm text-muted">
          {visibility === "hidden"
            ? "Garden renderer paused while hidden"
            : "Preparing the garden…"}
        </div>
      )}
      {rendererActive && camera && <div className="garden-spatial-world">
        {agentUnits.map((unit) => {
          if (unit.ref.id === draggingAgentId) return null;
          const bounds = agentCellBounds(unit.position);
          const screen = projectBounds(bounds, camera);
          const size = viewSize();
          if (screen.width < 70 || screen.x + screen.width < 0 || screen.y + screen.height < 0 || screen.x > size.width || screen.y > size.height) return null;
          const agent = filteredAgents.find((item) => item.session_id === unit.ref.id);
          if (!agent) return null;
          return <GardenSpatialCell key={unitKey(unit.ref)} target={unit.ref} bounds={bounds} camera={camera}
            viewport={viewSize()}
            focused={currentFrame?.ref.kind === "agent" && currentFrame.ref.id === unit.ref.id}
            label={unit.label} status={gardenAgentStatusColor(unit.status)} onSelect={() => selectObject(unit.ref)} onEnter={() => enterObject(unit.ref)}>
            <GardenAgentInterior agent={agent} status={unit.status} crown={unit.crown} agents={filteredAgents} teams={teams}
              projectedWidth={screen.width} contentsCache={contentsCache}
              automations={automationInputs} selectedKey={activeSelectionKey} onSelect={selectObject} onEnter={enterObject} onOpenAgent={openCanonicalAgent} />
          </GardenSpatialCell>;
        })}
        {spatialFrames.map((frame, index) => {
          const bounds = frame.bounds ?? boundsFor(frame.ref);
          if (!bounds) return null;
          return <GardenSpatialCell key={`${index}:${unitKey(frame.ref)}`} target={frame.ref} bounds={bounds} camera={camera}
            viewport={viewSize()}
            receding={index < spatialFrames.length - 1}
            focused={currentFrame === frame}
            revealFromScale={frame === candidate ? frame.camera?.scale : undefined}
            label={frame.label} onSelect={() => selectObject(frame.ref)} onEnter={() => enterObject(frame.ref)}>
            {renderContents(frame)}
          </GardenSpatialCell>;
        })}
      </div>}
    </div>
  );
};
