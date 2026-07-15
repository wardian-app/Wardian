import {
  createContext,
  useCallback,
  useContext,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
  type MutableRefObject,
  type PointerEvent as ReactPointerEvent,
  type ReactNode,
} from "react";
import {
  DockviewReact,
  type DockviewApi,
  type DropOverlayModelParams,
  type DockviewReadyEvent,
  type DockviewWillDropEvent,
  type DockviewWillShowOverlayLocationEvent,
  type DroptargetOverlayModel,
  type GroupDragEvent,
  type IDockviewHeaderActionsProps,
  type IDockviewPanelHeaderProps,
  type IDockviewPanelProps,
  type IWatermarkPanelProps,
} from "dockview-react";
import "dockview-react/dist/styles/dockview.css";

import {
  groupsAreWorkbenchAdjacent,
  type WorkbenchCommand,
} from "../../features/workbench/workbenchModel";
import type {
  DeepReadonly,
  ReadonlyWorkbenchDocumentV1,
} from "../../features/workbench/useWorkbenchStore";
import type { WorkbenchNodeV1, WorkbenchSurfaceV1 } from "../../types";
import {
  WorkbenchGroupHeader,
  WorkbenchNewSurfaceAction,
  type WorkbenchPaneTarget,
} from "./WorkbenchGroupHeader";
import {
  WorkbenchTab,
  type WorkbenchTabPointerDragIdentity,
} from "./WorkbenchTab";

export type WorkbenchSurfaceRenderLifecycle = {
  visible: boolean;
};

export type WorkbenchSurfaceRenderer = (
  surface: DeepReadonly<WorkbenchSurfaceV1>,
  lifecycle?: WorkbenchSurfaceRenderLifecycle,
) => ReactNode;

export type WorkbenchPanelRendererPolicy = (
  surface: DeepReadonly<WorkbenchSurfaceV1>,
) => "always" | "onlyWhenVisible";

export type WorkbenchSurfaceTitle = (
  surface: DeepReadonly<WorkbenchSurfaceV1>,
) => string;

export type WorkbenchSurfaceIcon = (
  surface: DeepReadonly<WorkbenchSurfaceV1>,
) => string;

export type DockviewLayoutAdapterProps = {
  document: ReadonlyWorkbenchDocumentV1;
  safe_mode?: boolean;
  zoomed_group_id?: string | null;
  render_surface?: WorkbenchSurfaceRenderer;
  surface_title?: WorkbenchSurfaceTitle;
  surface_icon?: WorkbenchSurfaceIcon;
  renderer_policy?: WorkbenchPanelRendererPolicy;
  on_command?: (command: WorkbenchCommand) => boolean;
  on_open_surface?: (groupId: string) => void;
  on_toggle_zoom?: (groupId: string) => void;
  on_split_group?: (groupId: string, direction: "horizontal" | "vertical") => void;
  on_close_group?: (groupId: string) => void;
  on_close_surface?: (surfaceId: string) => void;
  on_join_group?: (sourceGroupId: string, targetGroupId: string) => void;
  on_surface_drop?: (
    surfaceId: string,
    targetGroupId: string,
    position: "top" | "bottom" | "left" | "right" | "center",
  ) => void;
  render_home?: (groupId: string) => ReactNode;
};

type WorkbenchPanelParams = Record<string, unknown> & {
  surface: DeepReadonly<WorkbenchSurfaceV1>;
  title: string;
  render_surface?: WorkbenchSurfaceRenderer;
};

type AdapterRuntime = Pick<
  DockviewLayoutAdapterProps,
  | "document"
  | "on_open_surface"
  | "on_toggle_zoom"
  | "on_split_group"
  | "on_close_group"
  | "on_join_group"
  | "render_home"
> & {
  render_surface?: WorkbenchSurfaceRenderer;
  surface_title?: WorkbenchSurfaceTitle;
  surface_icon?: WorkbenchSurfaceIcon;
  zoomed_group_id: string | null;
  on_close_surface: (surfaceId: string) => void;
  on_pointer_drag_start: (identity: WorkbenchPointerDragIdentity) => void;
  on_pointer_drag_end: (identity: WorkbenchPointerDragIdentity) => void;
  on_move_surface: (surfaceId: string, targetGroupId: string) => void;
  on_split_surface: (
    surfaceId: string,
    groupId: string,
    direction: "horizontal" | "vertical",
  ) => void;
};

const AdapterRuntimeContext = createContext<AdapterRuntime | null>(null);
const WARDIAN_DOCKVIEW_THEME = {
  name: "wardian",
  className: "dockview-theme-wardian",
  dndOverlayMounting: "relative" as const,
  dndPanelOverlay: "group" as const,
  tabGroupIndicator: "none" as const,
};
const KEEP_ALIVE_SURFACE_TYPES = new Set([
  "agents-overview",
  "library",
  "workflows",
]);
const SPLIT_RATIO_FEEDBACK_EPSILON = 0.005;
/** Matches Dockview's group floor while making Wardian's split contract explicit. */
export const WORKBENCH_PANE_MINIMUM_WIDTH = 100;
/** Matches Dockview's group floor while making Wardian's split contract explicit. */
export const WORKBENCH_PANE_MINIMUM_HEIGHT = 100;
const WORKBENCH_PANE_CONSTRAINTS = Object.freeze({
  minimumWidth: WORKBENCH_PANE_MINIMUM_WIDTH,
  minimumHeight: WORKBENCH_PANE_MINIMUM_HEIGHT,
});
const WORKBENCH_DOCKVIEW_DROP_OVERLAY: DroptargetOverlayModel = {
  size: { type: "percentage", value: 50 },
  activationSize: { type: "percentage", value: 20 },
  smallWidthBoundary: WORKBENCH_PANE_MINIMUM_WIDTH * 2,
  smallHeightBoundary: WORKBENCH_PANE_MINIMUM_HEIGHT * 2,
};
const WORKBENCH_DOCKVIEW_CENTER_ONLY_OVERLAY: DroptargetOverlayModel = {
  ...WORKBENCH_DOCKVIEW_DROP_OVERLAY,
  activationSize: { type: "percentage", value: 0 },
};

export type WorkbenchPointerDragIdentity = WorkbenchTabPointerDragIdentity;

export type WorkbenchRectangle = {
  left: number;
  top: number;
  width: number;
  height: number;
};

export type WorkbenchDropPosition = "top" | "bottom" | "left" | "right" | "center";
export type WorkbenchPaneSplitAdmission = "allowed" | "blocked" | "unmeasured";

/**
 * Distinguishes a measured split decision from transiently unavailable layout.
 * Center moves never create panes and are always allowed.
 */
export function workbenchPaneSplitAdmission(
  bounds: WorkbenchRectangle | undefined,
  position: WorkbenchDropPosition,
): WorkbenchPaneSplitAdmission {
  if (position === "center") return "allowed";
  if (!bounds) return "unmeasured";
  if (position === "left" || position === "right") {
    if (!Number.isFinite(bounds.width)) return "unmeasured";
    return bounds.width < WORKBENCH_PANE_MINIMUM_WIDTH * 2 ? "blocked" : "allowed";
  }
  if (!Number.isFinite(bounds.height)) return "unmeasured";
  return bounds.height < WORKBENCH_PANE_MINIMUM_HEIGHT * 2 ? "blocked" : "allowed";
}

/**
 * Returns false only for known-too-small panes. Unmeasured geometry stays
 * enabled so Dockview readiness cannot suppress otherwise valid operations.
 */
export function canSplitWorkbenchPane(
  bounds: WorkbenchRectangle | undefined,
  position: WorkbenchDropPosition,
): boolean {
  return workbenchPaneSplitAdmission(bounds, position) !== "blocked";
}

/** Prevents Dockview from promising an edge preview its destination cannot hold. */
export function handleWorkbenchDockviewOverlayAdmission(
  event: DockviewWillShowOverlayLocationEvent,
): void {
  if (event.position === "center") return;
  if (!canSplitWorkbenchPane(event.group?.api.boundingBox, event.position)) {
    event.preventDefault();
  }
}

/** Keeps top-edge native window dragging from also starting a Dockview group drag. */
export function handleWorkbenchDockviewGroupDrag(
  event: GroupDragEvent,
  root: DeepReadonly<WorkbenchNodeV1>,
  zoomedGroupId: string | null,
): void {
  if (workbenchGroupOwnsWindowChrome(root, event.group.id, zoomedGroupId)) {
    event.nativeEvent.preventDefault();
  }
}

export type DockviewGroupPlacement = {
  group_id: string;
  reference_group_id?: string;
  direction?: "right" | "below";
};

export type NormalizedWorkbenchGeometry = {
  groups: Record<string, WorkbenchRectangle>;
  splits: Record<string, WorkbenchRectangle & {
    direction: "horizontal" | "vertical";
    ratio: number;
    boundary: number;
  }>;
};

function useAdapterRuntime(): AdapterRuntime {
  const runtime = useContext(AdapterRuntimeContext);
  if (!runtime) throw new Error("workbench adapter runtime is unavailable");
  return runtime;
}

function groupIdsInTreeOrder(node: ReadonlyWorkbenchDocumentV1["root"]): string[] {
  return node.kind === "group"
    ? [node.group_id]
    : [...groupIdsInTreeOrder(node.first), ...groupIdsInTreeOrder(node.second)];
}

/** Names the immediate spatially valid pane before and after a group without leaking group IDs. */
export function workbenchPaneTargets(
  root: DeepReadonly<WorkbenchNodeV1>,
  groupId: string,
): WorkbenchPaneTarget[] {
  const ordered = groupIdsInTreeOrder(root);
  const index = ordered.indexOf(groupId);
  if (index < 0) return [];
  return ([
    { group_id: ordered[index - 1], position: "previous" as const },
    { group_id: ordered[index + 1], position: "next" as const },
  ]).filter((target): target is WorkbenchPaneTarget => (
    target.group_id !== undefined
    && groupsAreWorkbenchAdjacent(root, groupId, target.group_id)
  ));
}

/** Reports whether a group header contributes to the window's top chrome. */
export function workbenchGroupTouchesTopEdge(
  node: DeepReadonly<WorkbenchNodeV1>,
  groupId: string,
  touchesTop = true,
): boolean {
  if (node.kind === "group") return touchesTop && node.group_id === groupId;
  if (node.direction === "horizontal") {
    return workbenchGroupTouchesTopEdge(node.first, groupId, touchesTop)
      || workbenchGroupTouchesTopEdge(node.second, groupId, touchesTop);
  }
  return workbenchGroupTouchesTopEdge(node.first, groupId, touchesTop)
    || workbenchGroupTouchesTopEdge(node.second, groupId, false);
}

export function workbenchGroupTouchesRightEdge(
  node: DeepReadonly<WorkbenchNodeV1>,
  groupId: string,
  touchesRight = true,
): boolean {
  if (node.kind === "group") return touchesRight && node.group_id === groupId;
  if (node.direction === "vertical") {
    return workbenchGroupTouchesRightEdge(node.first, groupId, touchesRight)
      || workbenchGroupTouchesRightEdge(node.second, groupId, touchesRight);
  }
  return workbenchGroupTouchesRightEdge(node.first, groupId, false)
    || workbenchGroupTouchesRightEdge(node.second, groupId, touchesRight);
}

export function workbenchGroupTouchesLeftEdge(
  node: DeepReadonly<WorkbenchNodeV1>,
  groupId: string,
  touchesLeft = true,
): boolean {
  if (node.kind === "group") return touchesLeft && node.group_id === groupId;
  if (node.direction === "vertical") {
    return workbenchGroupTouchesLeftEdge(node.first, groupId, touchesLeft)
      || workbenchGroupTouchesLeftEdge(node.second, groupId, touchesLeft);
  }
  return workbenchGroupTouchesLeftEdge(node.first, groupId, touchesLeft)
    || workbenchGroupTouchesLeftEdge(node.second, groupId, false);
}

export function workbenchGroupOwnsWindowChrome(
  node: DeepReadonly<WorkbenchNodeV1>,
  groupId: string,
  zoomedGroupId: string | null,
): boolean {
  return zoomedGroupId === groupId || workbenchGroupTouchesTopEdge(node, groupId);
}

function workbenchGroupNeedsWindowControlClearance(
  node: DeepReadonly<WorkbenchNodeV1>,
  groupId: string,
  zoomedGroupId: string | null,
): boolean {
  return zoomedGroupId === groupId || (
    workbenchGroupTouchesTopEdge(node, groupId)
    && workbenchGroupTouchesRightEdge(node, groupId)
  );
}

function firstGroupId(node: DeepReadonly<WorkbenchNodeV1>): string {
  return node.kind === "group" ? node.group_id : firstGroupId(node.first);
}

function stableNumber(value: number): number {
  return Number(value.toFixed(12));
}

/** Builds a parent-first placement plan so each split exists before either side is subdivided. */
export function planDockviewGroupPlacements(
  root: DeepReadonly<WorkbenchNodeV1>,
): DockviewGroupPlacement[] {
  const placements: DockviewGroupPlacement[] = [{ group_id: firstGroupId(root) }];
  const visit = (node: DeepReadonly<WorkbenchNodeV1>): void => {
    if (node.kind === "group") return;
    placements.push({
      group_id: firstGroupId(node.second),
      reference_group_id: firstGroupId(node.first),
      direction: node.direction === "horizontal" ? "right" : "below",
    });
    visit(node.first);
    visit(node.second);
  };
  visit(root);
  return placements;
}

/** Computes canonical leaf rectangles and split boundaries in normalized coordinates. */
export function normalizedWorkbenchGeometry(
  root: DeepReadonly<WorkbenchNodeV1>,
): NormalizedWorkbenchGeometry {
  const groups: Record<string, WorkbenchRectangle> = {};
  const splits: NormalizedWorkbenchGeometry["splits"] = {};
  const visit = (
    node: DeepReadonly<WorkbenchNodeV1>,
    rectangle: WorkbenchRectangle,
  ): void => {
    if (node.kind === "group") {
      groups[node.group_id] = rectangle;
      return;
    }
    const boundary = node.direction === "horizontal"
      ? stableNumber(rectangle.left + (rectangle.width * node.ratio))
      : stableNumber(rectangle.top + (rectangle.height * node.ratio));
    splits[node.node_id] = { ...rectangle, direction: node.direction, ratio: node.ratio, boundary };
    if (node.direction === "horizontal") {
      const firstWidth = stableNumber(rectangle.width * node.ratio);
      visit(node.first, { ...rectangle, width: firstWidth });
      visit(node.second, {
        ...rectangle,
        left: boundary,
        width: stableNumber(rectangle.width - firstWidth),
      });
      return;
    }
    const firstHeight = stableNumber(rectangle.height * node.ratio);
    visit(node.first, { ...rectangle, height: firstHeight });
    visit(node.second, {
      ...rectangle,
      top: boundary,
      height: stableNumber(rectangle.height - firstHeight),
    });
  };
  visit(root, { left: 0, top: 0, width: 1, height: 1 });
  return { groups, splits };
}

function unionRectangles(
  first: WorkbenchRectangle | undefined,
  second: WorkbenchRectangle | undefined,
): WorkbenchRectangle | undefined {
  if (!first) return second;
  if (!second) return first;
  const left = Math.min(first.left, second.left);
  const top = Math.min(first.top, second.top);
  const right = Math.max(first.left + first.width, second.left + second.width);
  const bottom = Math.max(first.top + first.height, second.top + second.height);
  return { left, top, width: right - left, height: bottom - top };
}

/** Derives canonical ratios from live public group bounding boxes. */
export function deriveWorkbenchSplitRatios(
  root: DeepReadonly<WorkbenchNodeV1>,
  groupRectangles: Readonly<Record<string, WorkbenchRectangle | undefined>>,
): Record<string, number> {
  const ratios: Record<string, number> = {};
  const visit = (node: DeepReadonly<WorkbenchNodeV1>): WorkbenchRectangle | undefined => {
    if (node.kind === "group") return groupRectangles[node.group_id];
    const first = visit(node.first);
    const second = visit(node.second);
    const combined = unionRectangles(first, second);
    if (!first || !second || !combined) return combined;
    const total = node.direction === "horizontal" ? combined.width : combined.height;
    if (!Number.isFinite(total) || total <= 0) return combined;
    const firstEnd = node.direction === "horizontal"
      ? first.left + first.width
      : first.top + first.height;
    const secondStart = node.direction === "horizontal" ? second.left : second.top;
    const start = node.direction === "horizontal" ? combined.left : combined.top;
    const ratio = ((firstEnd + secondStart) / 2 - start) / total;
    if (Number.isFinite(ratio)) {
      ratios[node.node_id] = stableNumber(Math.max(0.1, Math.min(0.9, ratio)));
    }
    return combined;
  };
  visit(root);
  return ratios;
}

/** Converts live geometry into the minimal canonical ratio command set. */
export function workbenchSplitRatioCommands(
  root: DeepReadonly<WorkbenchNodeV1>,
  groupRectangles: Readonly<Record<string, WorkbenchRectangle | undefined>>,
  epsilon = SPLIT_RATIO_FEEDBACK_EPSILON,
): Array<Extract<WorkbenchCommand, { type: "set_split_ratio" }>> {
  const ratios = deriveWorkbenchSplitRatios(root, groupRectangles);
  return splitNodesInTreeOrder(root).flatMap((split) => {
    const ratio = ratios[split.node_id];
    return ratio === undefined || Math.abs(ratio - split.ratio) <= epsilon
      ? []
      : [{ type: "set_split_ratio" as const, node_id: split.node_id, ratio }];
  });
}

/** Reports whether an edge drop would split a sole tab back into its own group. */
export function isSoleTabSelfDrop(
  document: ReadonlyWorkbenchDocumentV1,
  surfaceId: string,
  targetGroupId: string,
): boolean {
  const target = document.groups[targetGroupId];
  return target?.surface_ids.length === 1 && target.surface_ids[0] === surfaceId;
}

/** Uses accurate half-pane previews, except when a sole tab targets its own content. */
export function workbenchDockviewDropOverlayModel(
  { location }: DropOverlayModelParams,
  document?: ReadonlyWorkbenchDocumentV1,
  dragIdentity?: WorkbenchPointerDragIdentity | null,
  targetGroupId?: string,
): DroptargetOverlayModel | undefined {
  if (location !== "content" && location !== "header_space" && location !== "tab") {
    return undefined;
  }
  return document
    && dragIdentity
    && targetGroupId === dragIdentity.source_group_id
    && isSoleTabSelfDrop(document, dragIdentity.surface_id, targetGroupId)
    ? WORKBENCH_DOCKVIEW_CENTER_ONLY_OVERLAY
    : WORKBENCH_DOCKVIEW_DROP_OVERLAY;
}

/** Reports whether a transient Dockview destination still exists canonically. */
export function isCanonicalWorkbenchGroupDestination(
  document: ReadonlyWorkbenchDocumentV1,
  groupId: string,
): boolean {
  return document.groups[groupId] !== undefined;
}

/** Leaves valid center/tab drops to Dockview and routes valid edge drops to Wardian. */
export function routeWorkbenchDockviewDrop(
  event: DockviewWillDropEvent,
  document: ReadonlyWorkbenchDocumentV1,
  onSurfaceDrop?: DockviewLayoutAdapterProps["on_surface_drop"],
): void {
  const transfer = event.getData();
  const surfaceId = transfer?.panelId ?? event.panel?.id;
  if (!surfaceId) {
    event.preventDefault();
    return;
  }
  const targetGroupId = event.group?.id;
  if (
    event.position === "center"
    && targetGroupId
    && isCanonicalWorkbenchGroupDestination(document, targetGroupId)
  ) return;
  event.preventDefault();
  if (!targetGroupId || !isCanonicalWorkbenchGroupDestination(document, targetGroupId)) return;
  if (
    event.position !== "center"
    && isSoleTabSelfDrop(document, surfaceId, targetGroupId)
  ) return;
  if (!canSplitWorkbenchPane(event.group?.api.boundingBox, event.position)) return;
  onSurfaceDrop?.(surfaceId, targetGroupId, event.position);
}

/** Dispatches a canonical command and requests recovery only on explicit rejection. */
export function dispatchWorkbenchAdapterCommand(
  command: WorkbenchCommand,
  onCommand: DockviewLayoutAdapterProps["on_command"],
  onRejected: () => boolean,
): boolean {
  const accepted = onCommand?.(command);
  if (accepted !== true) {
    const recoveryScheduled = onRejected();
    if (accepted === false && recoveryScheduled) {
      console.error("Workbench canonical command rejected", { command });
    }
    return false;
  }
  return true;
}

/** Identifies user removals that diverge from the unchanged canonical document. */
export function shouldRecoverUnexpectedPanelRemoval(
  document: ReadonlyWorkbenchDocumentV1,
  surfaceId: string,
  adapterProjection: boolean,
): boolean {
  return !adapterProjection && surfaceId in document.surfaces;
}

function surfaceTitle(surface: DeepReadonly<WorkbenchSurfaceV1>): string {
  return surface.surface_type
    .split("-")
    .map((part) => part.length === 0 ? part : `${part[0].toUpperCase()}${part.slice(1)}`)
    .join(" ");
}

function splitNodesInTreeOrder(
  node: DeepReadonly<WorkbenchNodeV1>,
): Array<Extract<DeepReadonly<WorkbenchNodeV1>, { kind: "split" }>> {
  return node.kind === "group"
    ? []
    : [node, ...splitNodesInTreeOrder(node.first), ...splitNodesInTreeOrder(node.second)];
}

function adjustedRatio(
  direction: "horizontal" | "vertical",
  current: number,
  key: string,
): number | null {
  const delta = direction === "horizontal"
    ? key === "ArrowLeft" ? -0.05 : key === "ArrowRight" ? 0.05 : 0
    : key === "ArrowUp" ? -0.05 : key === "ArrowDown" ? 0.05 : 0;
  if (delta === 0) return null;
  return Math.max(0.1, Math.min(0.9, Number((current + delta).toFixed(2))));
}

function separatorStyle(
  split: NormalizedWorkbenchGeometry["splits"][string],
): CSSProperties {
  if (split.direction === "horizontal") {
    return {
      left: `${stableNumber(split.boundary * 100)}%`,
      top: `${stableNumber(split.top * 100)}%`,
      height: `${stableNumber(split.height * 100)}%`,
    };
  }
  return {
    left: `${stableNumber(split.left * 100)}%`,
    top: `${stableNumber(split.boundary * 100)}%`,
    width: `${stableNumber(split.width * 100)}%`,
  };
}

function defaultRendererPolicy(
  surface: DeepReadonly<WorkbenchSurfaceV1>,
): "always" | "onlyWhenVisible" {
  return KEEP_ALIVE_SURFACE_TYPES.has(surface.surface_type) ? "always" : "onlyWhenVisible";
}

function DockviewSurfacePanel({ params }: IDockviewPanelProps<WorkbenchPanelParams>) {
  const { surface } = params;
  const runtime = useAdapterRuntime();
  const group = Object.values(runtime.document.groups)
    .find((candidate) => candidate.surface_ids.includes(surface.surface_id));
  const visible = Boolean(
    group
    && group.active_surface_id === surface.surface_id
    && (!runtime.zoomed_group_id || runtime.zoomed_group_id === group.group_id),
  );
  return (
    <div
      id={`workbench-panel-${surface.surface_id}`}
      data-testid="surface-panel"
      data-surface-id={surface.surface_id}
      data-surface-type={surface.surface_type}
      {...(surface.resource_key === undefined
        ? {}
        : { "data-resource-key": surface.resource_key })}
      className="wardian-workbench-surface-panel"
    >
      {runtime.render_surface?.(surface, { visible })
        ?? params.render_surface?.(surface, { visible })
        ?? surface.surface_type}
    </div>
  );
}

function DockviewSurfaceTab({ params, api }: IDockviewPanelHeaderProps<WorkbenchPanelParams>) {
  const runtime = useAdapterRuntime();
  const groupId = api.group.id;
  return (
    <WorkbenchTab
      surface={params.surface}
      title={runtime.surface_title?.(params.surface) ?? params.title}
      icon={runtime.surface_icon?.(params.surface) ?? params.surface.surface_type}
      group_id={groupId}
      pane_targets={workbenchPaneTargets(runtime.document.root, groupId)}
      on_close={() => runtime.on_close_surface(params.surface.surface_id)}
      on_split={(direction) => runtime.on_split_surface(
        params.surface.surface_id,
        groupId,
        direction,
      )}
      on_move={(targetGroupId) => runtime.on_move_surface(
        params.surface.surface_id,
        targetGroupId,
      )}
      on_pointer_drag_start={runtime.on_pointer_drag_start}
      on_pointer_drag_end={runtime.on_pointer_drag_end}
    />
  );
}

function DockviewGroupActions({ group }: IDockviewHeaderActionsProps) {
  const runtime = useAdapterRuntime();
  return (
    <WorkbenchGroupHeader
      group_id={group.id}
      pane_targets={workbenchPaneTargets(runtime.document.root, group.id)}
      is_zoomed={runtime.zoomed_group_id === group.id}
      on_toggle_zoom={runtime.on_toggle_zoom}
      on_split_group={runtime.on_split_group}
      on_close_group={runtime.on_close_group}
      on_join_group={runtime.on_join_group}
    />
  );
}

function DockviewNewSurfaceAction({ group }: IDockviewHeaderActionsProps) {
  const runtime = useAdapterRuntime();
  return (
    <WorkbenchNewSurfaceAction
      group_id={group.id}
      window_drag_region={workbenchGroupOwnsWindowChrome(
        runtime.document.root,
        group.id,
        runtime.zoomed_group_id,
      )}
      window_left_clearance={runtime.zoomed_group_id === group.id || (
        workbenchGroupTouchesTopEdge(runtime.document.root, group.id)
        && workbenchGroupTouchesLeftEdge(runtime.document.root, group.id)
      )}
      window_controls_clearance={workbenchGroupNeedsWindowControlClearance(
        runtime.document.root,
        group.id,
        runtime.zoomed_group_id,
      )}
      on_open_surface={runtime.on_open_surface}
    />
  );
}

function DockviewEmptyGroup({ group }: IWatermarkPanelProps) {
  const runtime = useAdapterRuntime();
  const groupId = group?.id ?? runtime.document.active_group_id;
  if (runtime.render_home) return runtime.render_home(groupId);
  return (
    <div data-testid="workbench-empty-group" data-group-id={groupId}>
      <p>New Surface</p>
      <button type="button" onClick={() => runtime.on_open_surface?.(groupId)}>
        Open Surface
      </button>
    </div>
  );
}

const DOCKVIEW_COMPONENTS = { "wardian-surface": DockviewSurfacePanel };

function repairDockviewGroupHeader(group: ReturnType<DockviewApi["addGroup"]>) {
  if (group.header.hidden) {
    group.header.hidden = false;
  }
  if (group.api.getHeaderPosition() !== "top") {
    group.api.setHeaderPosition("top");
  }
}

function ensureGroups(
  api: DockviewApi,
  node: DeepReadonly<WorkbenchNodeV1>,
  repairExisting: boolean,
): Map<string, ReturnType<DockviewApi["addGroup"]>> {
  const groups = new Map(api.groups.map((group) => [group.id, group]));
  for (const placement of planDockviewGroupPlacements(node)) {
    const existing = groups.get(placement.group_id);
    const referenceGroup = placement.reference_group_id
      ? groups.get(placement.reference_group_id)
      : undefined;
    if (!existing) {
      const added = referenceGroup && placement.direction
        ? api.addGroup({
          id: placement.group_id,
          referenceGroup,
          direction: placement.direction,
          hideHeader: false,
          headerPosition: "top",
          constraints: WORKBENCH_PANE_CONSTRAINTS,
        })
        : api.addGroup({
          id: placement.group_id,
          direction: "right",
          hideHeader: false,
          headerPosition: "top",
          constraints: WORKBENCH_PANE_CONSTRAINTS,
        });
      if (added) groups.set(placement.group_id, added);
      continue;
    }
    // Header chrome is canonical Wardian state. Dockview owns its DOM, but
    // retained library-local state must not hide or relocate the tab strip.
    repairDockviewGroupHeader(existing);
    if (repairExisting && referenceGroup && placement.direction) {
      existing.api.moveTo({
        group: referenceGroup,
        position: placement.direction === "right" ? "right" : "bottom",
        skipSetActive: true,
      });
    }
  }
  return groups;
}

/** Projects canonical ratios using only public group sizing. */
export function projectWorkbenchGroupSizes(
  api: DockviewApi,
  root: DeepReadonly<WorkbenchNodeV1>,
  groups: Iterable<ReturnType<DockviewApi["addGroup"]>> = api.groups,
): void {
  if (!Number.isFinite(api.width) || !Number.isFinite(api.height) || api.width <= 0 || api.height <= 0) {
    return;
  }
  const geometry = normalizedWorkbenchGeometry(root).groups;
  for (const group of groups) {
    const rectangle = geometry[group.id];
    if (!rectangle) continue;
    group.api.setSize({
      width: stableNumber(rectangle.width * api.width),
      height: stableNumber(rectangle.height * api.height),
    });
  }
}

function topologySignature(node: DeepReadonly<WorkbenchNodeV1>): string {
  return node.kind === "group"
    ? `g:${node.group_id}`
    : `s:${node.direction}(${topologySignature(node.first)},${topologySignature(node.second)})`;
}

function liveGroupRectangles(api: DockviewApi): Record<string, WorkbenchRectangle | undefined> {
  return Object.fromEntries(api.groups.map((group) => [group.id, group.api.boundingBox]));
}

function holdProjectionGuard(guard: MutableRefObject<number>): () => void {
  guard.current += 1;
  return () => queueMicrotask(() => {
    guard.current = Math.max(0, guard.current - 1);
  });
}

function panelParams(
  surface: DeepReadonly<WorkbenchSurfaceV1>,
  renderSurface?: WorkbenchSurfaceRenderer,
  titleSurface?: WorkbenchSurfaceTitle,
): WorkbenchPanelParams {
  return {
    surface,
    title: titleSurface?.(surface) ?? surfaceTitle(surface),
    ...(renderSurface ? { render_surface: renderSurface } : {}),
  };
}

function reconcileDockview(
  api: DockviewApi,
  document: ReadonlyWorkbenchDocumentV1,
  renderSurface: WorkbenchSurfaceRenderer | undefined,
  titleSurface: WorkbenchSurfaceTitle | undefined,
  rendererPolicy: WorkbenchPanelRendererPolicy,
  expectedMoves: Map<string, { group_id: string; index: number; transaction: number }>,
  transaction: number,
  zoomedGroupId: string | null,
  repairTopology: boolean,
):
  | { status: "projected" }
  | { status: "deferred"; group_id: string; surface_ids: string[] } {
  const groups = ensureGroups(api, document.root, repairTopology);

  for (const groupId of groupIdsInTreeOrder(document.root)) {
    const modelGroup = document.groups[groupId];
    const dockviewGroup = groups.get(groupId);
    if (!dockviewGroup) {
      return {
        status: "deferred",
        group_id: groupId,
        surface_ids: [...modelGroup.surface_ids],
      };
    }
    for (const [index, surfaceId] of modelGroup.surface_ids.entries()) {
      const surface = document.surfaces[surfaceId];
      const params = panelParams(surface, renderSurface, titleSurface);
      const renderer = rendererPolicy(surface);
      let panel = api.getPanel(surfaceId);
      if (!panel) {
        panel = api.addPanel<WorkbenchPanelParams>({
          id: surface.surface_id,
          title: params.title,
          component: "wardian-surface",
          renderer,
          inactive: true,
          position: { referenceGroup: dockviewGroup },
          params,
        });
      } else {
        panel.api.updateParameters(params);
        panel.api.setTitle(params.title);
        if (panel.api.renderer !== renderer) panel.api.setRenderer(renderer);
      }
      const currentIndex = panel.group.panels.findIndex((candidate) => candidate.id === panel?.id);
      if (panel.group.id !== groupId || currentIndex !== index) {
        expectedMoves.set(surfaceId, { group_id: groupId, index, transaction });
        panel.api.moveTo({ group: dockviewGroup, index, skipSetActive: true });
        queueMicrotask(() => {
          if (expectedMoves.get(surfaceId)?.transaction === transaction) {
            expectedMoves.delete(surfaceId);
          }
        });
      }
    }
  }

  for (const panel of [...api.panels]) {
    if (!(panel.id in document.surfaces)) api.removePanel(panel);
  }
  for (const group of [...api.groups]) {
    if (!(group.id in document.groups)) api.removeGroup(group);
  }

  // Dockview removes a group's DOM node when its final panel is removed. The
  // canonical Wardian group still exists and must own its empty-surface
  // launcher, header actions, and routing context rather than falling through
  // to Dockview's container-level watermark.
  const refreshedGroups = ensureGroups(api, document.root, false);
  const canonicalGroupIds = groupIdsInTreeOrder(document.root);
  for (const groupId of canonicalGroupIds) {
    if (!refreshedGroups.has(groupId)) {
      return {
        status: "deferred",
        group_id: groupId,
        surface_ids: [...document.groups[groupId].surface_ids],
      };
    }
  }
  const canonicalGroups = canonicalGroupIds.map((groupId) => refreshedGroups.get(groupId)!);

  projectWorkbenchGroupSizes(api, document.root, canonicalGroups);

  for (const groupId of canonicalGroupIds) {
    const modelGroup = document.groups[groupId];
    if (modelGroup.active_surface_id) {
      api.getPanel(modelGroup.active_surface_id)?.api.setActive();
    }
  }
  const activeSurfaceId = document.groups[document.active_group_id]?.active_surface_id;
  if (activeSurfaceId) {
    api.getPanel(activeSurfaceId)?.api.setActive();
  } else {
    refreshedGroups.get(document.active_group_id)?.api.setActive();
  }

  if (zoomedGroupId) {
    const zoomed = refreshedGroups.get(zoomedGroupId);
    if (zoomed && !zoomed.api.isMaximized()) zoomed.api.maximize();
  } else if (api.hasMaximizedGroup()) {
    api.exitMaximizedGroup();
  }

  for (const group of canonicalGroups) {
    group.element.dataset.testid = "workbench-group";
    group.element.dataset.groupId = group.id;
    group.element.dataset.active = String(group.id === document.active_group_id);
    group.element.tabIndex = -1;
  }
  return { status: "projected" };
}

function SafeWorkbenchLayout({
  document,
  zoomed_group_id = null,
  render_surface,
  surface_title,
  surface_icon,
  on_command,
  on_open_surface,
  on_toggle_zoom,
  on_split_group,
  on_close_group,
  on_close_surface,
  on_join_group,
  on_surface_drop,
  render_home,
}: DockviewLayoutAdapterProps) {
  const group = document.groups[document.active_group_id];
  const activeSurface = group.active_surface_id
    ? document.surfaces[group.active_surface_id]
    : undefined;
  return (
    <section
      data-testid="workbench-group"
      data-group-id={group.group_id}
      data-active="true"
      tabIndex={-1}
    >
      <header
        className="wardian-workbench-group-header"
        data-left-chrome-clearance="true"
        data-window-controls-clearance="true"
      >
        <div role="tablist" aria-label={`Surfaces in ${group.group_id}`}>
          {group.surface_ids.map((surfaceId) => {
            const surface = document.surfaces[surfaceId];
            const active = surfaceId === group.active_surface_id;
            return (
              <div
                key={surface.surface_id}
                id={`workbench-tab-${surface.surface_id}`}
                role="tab"
                aria-label={surface_title?.(surface) ?? surfaceTitle(surface)}
                aria-selected={active}
                aria-controls={`workbench-panel-${surface.surface_id}`}
                tabIndex={active ? 0 : -1}
                data-surface-id={surface.surface_id}
                data-surface-type={surface.surface_type}
                {...(surface.resource_key === undefined
                  ? {}
                  : { "data-resource-key": surface.resource_key })}
                onClick={() => on_command?.({
                  type: "set_active_surface",
                  group_id: group.group_id,
                  surface_id: surface.surface_id,
                })}
              >
                <WorkbenchTab
                  surface={surface}
                  title={surface_title?.(surface) ?? surfaceTitle(surface)}
                  icon={surface_icon?.(surface) ?? surface.surface_type}
                  group_id={group.group_id}
                  pane_targets={workbenchPaneTargets(document.root, group.group_id)}
                  on_close={() => on_close_surface?.(surface.surface_id)}
                  on_split={(direction) => on_surface_drop?.(
                    surface.surface_id,
                    group.group_id,
                    direction === "horizontal" ? "right" : "bottom",
                  )}
                  on_move={(targetGroupId) => on_command?.({
                    type: "move_surface",
                    surface_id: surface.surface_id,
                    group_id: targetGroupId,
                    index: document.groups[targetGroupId]?.surface_ids.length ?? 0,
                  })}
                />
              </div>
            );
          })}
        </div>
        <WorkbenchNewSurfaceAction
          group_id={group.group_id}
          window_drag_region
          window_controls_clearance
          on_open_surface={on_open_surface}
        />
        <div
          className="wardian-workbench-safe-void"
          data-tauri-drag-region
        />
        <WorkbenchGroupHeader
          group_id={group.group_id}
          pane_targets={workbenchPaneTargets(document.root, group.group_id)}
          is_zoomed={zoomed_group_id === group.group_id}
          on_toggle_zoom={on_toggle_zoom}
          on_split_group={on_split_group}
          on_close_group={on_close_group}
          on_join_group={on_join_group}
        />
      </header>
      {activeSurface ? group.surface_ids.map((surfaceId) => {
        const surface = document.surfaces[surfaceId];
        const active = surfaceId === activeSurface.surface_id;
        return (
          <div
            key={surfaceId}
            id={`workbench-panel-${surfaceId}`}
            role="tabpanel"
            aria-labelledby={`workbench-tab-${surfaceId}`}
            hidden={!active}
            data-testid={active ? "surface-panel" : undefined}
            data-surface-id={surfaceId}
            data-surface-type={surface.surface_type}
            {...(surface.resource_key === undefined
              ? {}
              : { "data-resource-key": surface.resource_key })}
          >
            {active ? render_surface?.(surface, { visible: true }) ?? surface.surface_type : null}
          </div>
        );
      }) : (
        render_home?.(group.group_id)
          ?? <div data-testid="workbench-empty-group" data-group-id={group.group_id} />
      )}
    </section>
  );
}

/** Projects only Wardian's canonical model through Dockview's public APIs. */
export function DockviewLayoutAdapter(props: DockviewLayoutAdapterProps) {
  const {
    document,
    safe_mode = false,
    zoomed_group_id = null,
    render_surface,
    surface_title,
    surface_icon,
    renderer_policy = defaultRendererPolicy,
    on_command,
  } = props;
  const [api, setApi] = useState<DockviewApi | null>(null);
  const [reconcileNonce, setReconcileNonce] = useState(0);
  const [dropOverlayNonce, setDropOverlayNonce] = useState(0);
  const expectedMovesRef = useRef(new Map<
    string,
    { group_id: string; index: number; transaction: number }
  >());
  const transactionRef = useRef(0);
  const projectionGuardRef = useRef(0);
  const topologySignatureRef = useRef<string | null>(null);
  const ratioFeedbackScheduledRef = useRef(false);
  const reconcileScheduledRef = useRef(false);
  const deferredDocumentRef = useRef<ReadonlyWorkbenchDocumentV1 | null>(null);
  const lastApiSizeRef = useRef<{ width: number; height: number } | null>(null);
  const pendingCloseFocusRef = useRef<{ surface_id: string; group_id: string } | null>(null);
  const pointerDragIdentityRef = useRef<WorkbenchPointerDragIdentity | null>(null);
  const pointerTargetGroupIdRef = useRef<string | null>(null);
  const documentRef = useRef(document);
  const onCommandRef = useRef(on_command);
  const renderSurfaceRef = useRef(render_surface);
  const surfaceTitleRef = useRef(surface_title);
  documentRef.current = document;
  onCommandRef.current = on_command;
  renderSurfaceRef.current = render_surface;
  surfaceTitleRef.current = surface_title;

  const renderSurfaceProxy = useCallback<WorkbenchSurfaceRenderer>(
    (surface, lifecycle) => renderSurfaceRef.current?.(surface, lifecycle),
    [],
  );
  const surfaceTitleProxy = useCallback<WorkbenchSurfaceTitle>(
    (surface) => surfaceTitleRef.current?.(surface) ?? surfaceTitle(surface),
    [],
  );

  const requestCanonicalReconcile = useCallback((): boolean => {
    if (reconcileScheduledRef.current) return false;
    reconcileScheduledRef.current = true;
    queueMicrotask(() => {
      reconcileScheduledRef.current = false;
      setReconcileNonce((nonce) => nonce + 1);
    });
    return true;
  }, []);
  const emitCommand = useCallback((command: WorkbenchCommand): boolean => (
    dispatchWorkbenchAdapterCommand(command, onCommandRef.current, requestCanonicalReconcile)
  ), [requestCanonicalReconcile]);

  const normalizedGeometry = useMemo(
    () => normalizedWorkbenchGeometry(document.root),
    [document.root],
  );

  const beginPointerDrag = useCallback((identity: WorkbenchPointerDragIdentity): void => {
    pointerDragIdentityRef.current = identity;
    pointerTargetGroupIdRef.current = identity.source_group_id;
    setDropOverlayNonce((nonce) => nonce + 1);
  }, []);
  const endPointerDrag = useCallback((identity: WorkbenchPointerDragIdentity): void => {
    const activeIdentity = pointerDragIdentityRef.current;
    if (
      !activeIdentity
      || activeIdentity.surface_id !== identity.surface_id
      || activeIdentity.source_group_id !== identity.source_group_id
      || activeIdentity.pointer_id !== identity.pointer_id
    ) return;
    pointerDragIdentityRef.current = null;
    pointerTargetGroupIdRef.current = null;
    setDropOverlayNonce((nonce) => nonce + 1);
  }, []);
  const trackPointerTargetGroup = useCallback((event: ReactPointerEvent<HTMLDivElement>): void => {
    if (!pointerDragIdentityRef.current) return;
    const target = event.target instanceof Element ? event.target : null;
    const groupId = target?.closest<HTMLElement>("[data-group-id]")?.dataset.groupId ?? null;
    if (pointerTargetGroupIdRef.current === groupId) return;
    pointerTargetGroupIdRef.current = groupId;
    setDropOverlayNonce((nonce) => nonce + 1);
  }, []);
  const dropOverlayModel = useCallback((params: DropOverlayModelParams) => (
    workbenchDockviewDropOverlayModel(
      params,
      documentRef.current,
      pointerDragIdentityRef.current,
      params.group?.id ?? pointerTargetGroupIdRef.current ?? undefined,
    )
  ), [dropOverlayNonce]);

  const requestSurfaceClose = useCallback((surfaceId: string): void => {
    const currentDocument = documentRef.current;
    const group = Object.values(currentDocument.groups)
      .find((candidate) => candidate.surface_ids.includes(surfaceId));
    if (group?.active_surface_id === surfaceId) {
      pendingCloseFocusRef.current = { surface_id: surfaceId, group_id: group.group_id };
    }
    props.on_close_surface?.(surfaceId);
  }, [props.on_close_surface]);

  const runtime = useMemo<AdapterRuntime>(() => ({
    document,
    render_surface,
    surface_title,
    surface_icon,
    on_open_surface: props.on_open_surface,
    on_toggle_zoom: props.on_toggle_zoom,
    on_split_group: props.on_split_group,
    on_close_group: props.on_close_group,
    on_close_surface: requestSurfaceClose,
    on_pointer_drag_start: beginPointerDrag,
    on_pointer_drag_end: endPointerDrag,
    on_join_group: props.on_join_group,
    render_home: props.render_home,
    zoomed_group_id,
    on_move_surface: (surfaceId, targetGroupId) => {
      emitCommand({
        type: "move_surface",
        surface_id: surfaceId,
        group_id: targetGroupId,
        index: document.groups[targetGroupId]?.surface_ids.length ?? 0,
      });
    },
    on_split_surface: (surfaceId, groupId, direction) => {
      props.on_surface_drop?.(
        surfaceId,
        groupId,
        direction === "horizontal" ? "right" : "bottom",
      );
    },
  }), [
    document,
    beginPointerDrag,
    emitCommand,
    endPointerDrag,
    render_surface,
    props.on_close_group,
    props.on_join_group,
    props.on_open_surface,
    props.on_surface_drop,
    props.on_split_group,
    props.on_toggle_zoom,
    props.render_home,
    requestSurfaceClose,
    surface_icon,
    surface_title,
    zoomed_group_id,
  ]);

  const handleReady = useCallback((event: DockviewReadyEvent) => {
    setApi(event.api);
  }, []);

  useLayoutEffect(() => {
    if (!api || safe_mode) return;
    const transaction = transactionRef.current + 1;
    transactionRef.current = transaction;
    const nextTopologySignature = topologySignature(document.root);
    const repairTopology = topologySignatureRef.current !== null
      && topologySignatureRef.current !== nextTopologySignature;
    const releaseProjectionGuard = holdProjectionGuard(projectionGuardRef);
    try {
      const result = reconcileDockview(
        api,
        document,
        renderSurfaceProxy,
        surfaceTitleProxy,
        renderer_policy,
        expectedMovesRef.current,
        transaction,
        zoomed_group_id,
        repairTopology,
      );
      if (result.status === "deferred") {
        if (deferredDocumentRef.current !== document) {
          deferredDocumentRef.current = document;
          console.error("Dockview group projection deferred", {
            group_id: result.group_id,
            surface_ids: result.surface_ids,
          });
          requestCanonicalReconcile();
        }
        return;
      }
      deferredDocumentRef.current = null;
      topologySignatureRef.current = nextTopologySignature;
      lastApiSizeRef.current = { width: api.width, height: api.height };
    } finally {
      releaseProjectionGuard();
    }
  }, [
    api,
    document,
    reconcileNonce,
    renderSurfaceProxy,
    renderer_policy,
    requestCanonicalReconcile,
    safe_mode,
    surfaceTitleProxy,
    zoomed_group_id,
  ]);

  useLayoutEffect(() => {
    const pending = pendingCloseFocusRef.current;
    if (!pending) return;
    if (document.surfaces[pending.surface_id]) {
      if (document.groups[pending.group_id]?.active_surface_id !== pending.surface_id) {
        pendingCloseFocusRef.current = null;
      }
      return;
    }
    pendingCloseFocusRef.current = null;
    const group = document.groups[pending.group_id]
      ?? document.groups[document.active_group_id];
    const targetSurfaceId = group?.active_surface_id;
    const targetTab = targetSurfaceId
      ? [...globalThis.document.querySelectorAll<HTMLElement>('[role="tab"][data-surface-id]')]
          .find((tab) => tab.dataset.surfaceId === targetSurfaceId)
      : undefined;
    const targetGroup = [...globalThis.document.querySelectorAll<HTMLElement>('[data-group-id]')]
      .find((element) => element.dataset.groupId === group?.group_id);
    (targetTab ?? targetGroup)?.focus();
  }, [document]);

  useLayoutEffect(() => {
    if (!api || safe_mode) return;
    let activationScheduled = false;
    let activationRetryTimer: number | null = null;
    let pendingActivation: { group_id: string; surface_id: string | null } | null = null;
    const commitPendingActivation = (): void => {
      if (projectionGuardRef.current > 0) {
        activationRetryTimer = window.setTimeout(commitPendingActivation, 0);
        return;
      }
      activationRetryTimer = null;
      activationScheduled = false;
      const activation = pendingActivation;
      pendingActivation = null;
      if (!activation) return;
      if (!isCanonicalWorkbenchGroupDestination(
        documentRef.current,
        activation.group_id,
      )) return;
      emitCommand({ type: "set_active_surface", ...activation });
    };
    const scheduleActivation = (groupId: string, surfaceId: string | null): void => {
      const currentDocument = documentRef.current;
      if (!isCanonicalWorkbenchGroupDestination(currentDocument, groupId)) return;
      const currentGroup = currentDocument.groups[groupId];
      if (
        currentDocument.active_group_id === groupId
        && currentGroup?.active_surface_id === surfaceId
      ) return;
      pendingActivation = { group_id: groupId, surface_id: surfaceId };
      if (activationScheduled) return;
      activationScheduled = true;
      queueMicrotask(commitPendingActivation);
    };
    const moveDisposable = api.onDidMovePanel((event) => {
      const index = event.panel.group.panels.findIndex((panel) => panel.id === event.panel.id);
      const expected = expectedMovesRef.current.get(event.panel.id);
      if (expected?.group_id === event.panel.group.id && expected.index === index) {
        expectedMovesRef.current.delete(event.panel.id);
        return;
      }
      if (!isCanonicalWorkbenchGroupDestination(
        documentRef.current,
        event.panel.group.id,
      )) return;
      if (projectionGuardRef.current > 0) return;
      emitCommand({
        type: "move_surface",
        surface_id: event.panel.id,
        group_id: event.panel.group.id,
        index,
      });
    });
    const activeDisposable = api.onDidActivePanelChange((event) => {
      if (event.origin !== "user" || !event.panel) return;
      scheduleActivation(event.panel.group.id, event.panel.id);
    });
    const activeGroupDisposable = api.onDidActiveGroupChange((group) => {
      if (projectionGuardRef.current > 0 || !group) return;
      scheduleActivation(group.id, group.activePanel?.id ?? null);
    });
    const layoutDisposable = api.onDidLayoutChange(() => {
      for (const group of api.groups) {
        repairDockviewGroupHeader(group);
      }
      if (projectionGuardRef.current > 0) return;
      const previousSize = lastApiSizeRef.current;
      const currentSize = { width: api.width, height: api.height };
      lastApiSizeRef.current = currentSize;
      if (
        previousSize
        && (previousSize.width !== currentSize.width || previousSize.height !== currentSize.height)
      ) {
        const releaseProjectionGuard = holdProjectionGuard(projectionGuardRef);
        try {
          projectWorkbenchGroupSizes(api, documentRef.current.root);
        } finally {
          releaseProjectionGuard();
        }
        return;
      }
      if (ratioFeedbackScheduledRef.current) return;
      ratioFeedbackScheduledRef.current = true;
      queueMicrotask(() => {
        ratioFeedbackScheduledRef.current = false;
        if (projectionGuardRef.current > 0) return;
        const currentDocument = documentRef.current;
        const commands = workbenchSplitRatioCommands(
          currentDocument.root,
          liveGroupRectangles(api),
        );
        for (const command of commands) {
          emitCommand(command);
        }
      });
    });
    const removeDisposable = api.onDidRemovePanel((panel) => {
      if (!shouldRecoverUnexpectedPanelRemoval(
        documentRef.current,
        panel.id,
        projectionGuardRef.current > 0,
      )) return;
      requestCanonicalReconcile();
    });
    const overlayDisposable = api.onWillShowOverlay(
      handleWorkbenchDockviewOverlayAdmission,
    );
    const groupDragDisposable = api.onWillDragGroup((event) => {
      handleWorkbenchDockviewGroupDrag(
        event,
        documentRef.current.root,
        zoomed_group_id,
      );
    });
    return () => {
      if (activationRetryTimer !== null) window.clearTimeout(activationRetryTimer);
      moveDisposable.dispose();
      activeDisposable.dispose();
      activeGroupDisposable.dispose();
      layoutDisposable.dispose();
      removeDisposable.dispose();
      overlayDisposable.dispose();
      groupDragDisposable.dispose();
    };
  }, [api, emitCommand, requestCanonicalReconcile, safe_mode, zoomed_group_id]);

  const handleWillDrop = useCallback((event: DockviewWillDropEvent) => {
    routeWorkbenchDockviewDrop(event, documentRef.current, props.on_surface_drop);
  }, [props.on_surface_drop]);

  return (
    <AdapterRuntimeContext.Provider value={runtime}>
      <div
        className="wardian-workbench-layout"
        data-layout-source="wardian-model"
        data-safe-mode={String(safe_mode)}
        data-zoomed-group-id={zoomed_group_id ?? "none"}
        data-pointer-drag-surface-id={pointerDragIdentityRef.current?.surface_id ?? "none"}
        data-pointer-drag-pointer-id={pointerDragIdentityRef.current?.pointer_id ?? "none"}
        onPointerMoveCapture={trackPointerTargetGroup}
      >
        {safe_mode ? (
          <SafeWorkbenchLayout {...props} on_close_surface={requestSurfaceClose} />
        ) : (
          <DockviewReact
            components={DOCKVIEW_COMPONENTS}
            theme={WARDIAN_DOCKVIEW_THEME}
            defaultTabComponent={DockviewSurfaceTab}
            leftHeaderActionsComponent={DockviewNewSurfaceAction}
            rightHeaderActionsComponent={DockviewGroupActions}
            watermarkComponent={DockviewEmptyGroup}
            dndStrategy="pointer"
            dropOverlayModel={dropOverlayModel}
            keyboardNavigation
            onReady={handleReady}
            onWillDrop={handleWillDrop}
          />
        )}
        {!safe_mode && splitNodesInTreeOrder(document.root).map((split) => (
          <div
            key={split.node_id}
            className="wardian-workbench-separator-control"
            role="separator"
            tabIndex={0}
            aria-label={`Resize split ${split.node_id}`}
            aria-orientation={split.direction === "horizontal" ? "vertical" : "horizontal"}
            aria-valuemin={10}
            aria-valuemax={90}
            aria-valuenow={Math.round(split.ratio * 100)}
            data-split-node-id={split.node_id}
            data-split-direction={split.direction}
            style={separatorStyle(normalizedGeometry.splits[split.node_id])}
            onKeyDown={(event) => {
              const ratio = adjustedRatio(split.direction, split.ratio, event.key);
              if (ratio === null) return;
              event.preventDefault();
              emitCommand({ type: "set_split_ratio", node_id: split.node_id, ratio });
            }}
          />
        ))}
      </div>
    </AdapterRuntimeContext.Provider>
  );
}
