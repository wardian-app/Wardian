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
  type ReactNode,
} from "react";
import {
  DockviewReact,
  type DockviewApi,
  type DockviewReadyEvent,
  type DockviewWillDropEvent,
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
import { WorkbenchGroupHeader } from "./WorkbenchGroupHeader";
import { WorkbenchTab } from "./WorkbenchTab";

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

export type DockviewLayoutAdapterProps = {
  document: ReadonlyWorkbenchDocumentV1;
  safe_mode?: boolean;
  zoomed_group_id?: string | null;
  render_surface?: WorkbenchSurfaceRenderer;
  surface_title?: WorkbenchSurfaceTitle;
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
  | "on_close_surface"
  | "on_join_group"
  | "render_home"
> & { zoomed_group_id: string | null };

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

export type WorkbenchRectangle = {
  left: number;
  top: number;
  width: number;
  height: number;
};

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

/** Leaves center/tab drops to Dockview and routes edge drops to Wardian. */
export function routeWorkbenchDockviewDrop(
  event: DockviewWillDropEvent,
  onSurfaceDrop?: DockviewLayoutAdapterProps["on_surface_drop"],
): void {
  const transfer = event.getData();
  const surfaceId = transfer?.panelId ?? event.panel?.id;
  if (!surfaceId) {
    event.preventDefault();
    return;
  }
  if (event.position === "center" && event.group) return;
  event.preventDefault();
  if (!event.group) return;
  onSurfaceDrop?.(surfaceId, event.group.id, event.position);
}

/** Dispatches a canonical command and requests recovery only on explicit rejection. */
export function dispatchWorkbenchAdapterCommand(
  command: WorkbenchCommand,
  onCommand: DockviewLayoutAdapterProps["on_command"],
  onRejected: () => void,
): boolean {
  const accepted = onCommand?.(command);
  if (accepted !== true) {
    onRejected();
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
      {params.render_surface?.(surface, { visible }) ?? surface.surface_type}
    </div>
  );
}

function DockviewSurfaceTab({ params }: IDockviewPanelHeaderProps<WorkbenchPanelParams>) {
  const runtime = useAdapterRuntime();
  return (
    <WorkbenchTab
      surface={params.surface}
      title={params.title}
      on_close={() => runtime.on_close_surface?.(params.surface.surface_id)}
    />
  );
}

function DockviewGroupActions({ group }: IDockviewHeaderActionsProps) {
  const runtime = useAdapterRuntime();
  return (
    <WorkbenchGroupHeader
      group_id={group.id}
      join_target_ids={groupIdsInTreeOrder(runtime.document.root).filter(
        (id) => id !== group.id
          && groupsAreWorkbenchAdjacent(runtime.document.root, group.id, id),
      )}
      on_open_surface={runtime.on_open_surface}
      on_toggle_zoom={runtime.on_toggle_zoom}
      on_split_group={runtime.on_split_group}
      on_close_group={runtime.on_close_group}
      on_join_group={runtime.on_join_group}
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

function ensureGroups(
  api: DockviewApi,
  node: DeepReadonly<WorkbenchNodeV1>,
  repairExisting: boolean,
): void {
  const findGroup = (groupId: string) => api.groups.find((group) => group.id === groupId);
  for (const placement of planDockviewGroupPlacements(node)) {
    const existing = findGroup(placement.group_id);
    const referenceGroup = placement.reference_group_id
      ? findGroup(placement.reference_group_id)
      : undefined;
    if (!existing) {
      if (referenceGroup && placement.direction) {
        api.addGroup({
          id: placement.group_id,
          referenceGroup,
          direction: placement.direction,
        });
      } else {
        api.addGroup({ id: placement.group_id, direction: "right" });
      }
      continue;
    }
    if (repairExisting && referenceGroup && placement.direction) {
      existing.api.moveTo({
        group: referenceGroup,
        position: placement.direction === "right" ? "right" : "bottom",
        skipSetActive: true,
      });
    }
  }
}

/** Projects canonical ratios using only public group sizing. */
export function projectWorkbenchGroupSizes(
  api: DockviewApi,
  root: DeepReadonly<WorkbenchNodeV1>,
): void {
  if (!Number.isFinite(api.width) || !Number.isFinite(api.height) || api.width <= 0 || api.height <= 0) {
    return;
  }
  const geometry = normalizedWorkbenchGeometry(root).groups;
  for (const group of api.groups) {
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
): void {
  ensureGroups(api, document.root, repairTopology);

  for (const groupId of groupIdsInTreeOrder(document.root)) {
    const modelGroup = document.groups[groupId];
    const dockviewGroup = api.groups.find((group) => group.id === groupId);
    if (!dockviewGroup) throw new Error(`Dockview group projection failed: ${groupId}`);
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

  projectWorkbenchGroupSizes(api, document.root);

  for (const groupId of groupIdsInTreeOrder(document.root)) {
    const modelGroup = document.groups[groupId];
    if (modelGroup.active_surface_id) {
      api.getPanel(modelGroup.active_surface_id)?.api.setActive();
    }
  }
  const activeSurfaceId = document.groups[document.active_group_id]?.active_surface_id;
  if (activeSurfaceId) {
    api.getPanel(activeSurfaceId)?.api.setActive();
  } else {
    api.groups.find((group) => group.id === document.active_group_id)?.api.setActive();
  }

  if (zoomedGroupId) {
    const zoomed = api.groups.find((group) => group.id === zoomedGroupId);
    if (zoomed && !zoomed.api.isMaximized()) zoomed.api.maximize();
  } else if (api.hasMaximizedGroup()) {
    api.exitMaximizedGroup();
  }

  for (const group of api.groups) {
    group.element.dataset.testid = "workbench-group";
    group.element.dataset.groupId = group.id;
    group.element.dataset.active = String(group.id === document.active_group_id);
    group.element.tabIndex = -1;
  }
}

function SafeWorkbenchLayout({
  document,
  render_surface,
  surface_title,
  on_command,
  on_open_surface,
  on_toggle_zoom,
  on_split_group,
  on_close_group,
  on_close_surface,
  on_join_group,
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
      <header className="wardian-workbench-group-header">
        <div role="tablist" aria-label={`Surfaces in ${group.group_id}`}>
          {group.surface_ids.map((surfaceId) => {
            const surface = document.surfaces[surfaceId];
            const active = surfaceId === group.active_surface_id;
            return (
              <button
                key={surface.surface_id}
                id={`workbench-tab-${surface.surface_id}`}
                type="button"
                role="tab"
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
                onKeyDownCapture={(event) => {
                  if (event.key !== "Delete" && event.key !== "Backspace") return;
                  event.preventDefault();
                  event.stopPropagation();
                  event.nativeEvent.stopImmediatePropagation();
                  on_close_surface?.(surface.surface_id);
                }}
              >
                {surface_title?.(surface) ?? surfaceTitle(surface)}
              </button>
            );
          })}
        </div>
        <WorkbenchGroupHeader
          group_id={group.group_id}
          join_target_ids={groupIdsInTreeOrder(document.root).filter(
            (id) => id !== group.group_id
              && groupsAreWorkbenchAdjacent(document.root, group.group_id, id),
          )}
          on_open_surface={on_open_surface}
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
    renderer_policy = defaultRendererPolicy,
    on_command,
  } = props;
  const [api, setApi] = useState<DockviewApi | null>(null);
  const [reconcileNonce, setReconcileNonce] = useState(0);
  const expectedMovesRef = useRef(new Map<
    string,
    { group_id: string; index: number; transaction: number }
  >());
  const transactionRef = useRef(0);
  const projectionGuardRef = useRef(0);
  const topologySignatureRef = useRef<string | null>(null);
  const ratioFeedbackScheduledRef = useRef(false);
  const reconcileScheduledRef = useRef(false);
  const lastApiSizeRef = useRef<{ width: number; height: number } | null>(null);
  const documentRef = useRef(document);
  const onCommandRef = useRef(on_command);
  documentRef.current = document;
  onCommandRef.current = on_command;

  const requestCanonicalReconcile = useCallback(() => {
    if (reconcileScheduledRef.current) return;
    reconcileScheduledRef.current = true;
    queueMicrotask(() => {
      reconcileScheduledRef.current = false;
      setReconcileNonce((nonce) => nonce + 1);
    });
  }, []);
  const emitCommand = useCallback((command: WorkbenchCommand): boolean => (
    dispatchWorkbenchAdapterCommand(command, onCommandRef.current, requestCanonicalReconcile)
  ), [requestCanonicalReconcile]);

  const normalizedGeometry = useMemo(
    () => normalizedWorkbenchGeometry(document.root),
    [document.root],
  );

  const runtime = useMemo<AdapterRuntime>(() => ({
    document,
    on_open_surface: props.on_open_surface,
    on_toggle_zoom: props.on_toggle_zoom,
    on_split_group: props.on_split_group,
    on_close_group: props.on_close_group,
    on_close_surface: props.on_close_surface,
    on_join_group: props.on_join_group,
    render_home: props.render_home,
    zoomed_group_id,
  }), [
    document,
    props.on_close_group,
    props.on_close_surface,
    props.on_join_group,
    props.on_open_surface,
    props.on_split_group,
    props.on_toggle_zoom,
    props.render_home,
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
      reconcileDockview(
        api,
        document,
        render_surface,
        surface_title,
        renderer_policy,
        expectedMovesRef.current,
        transaction,
        zoomed_group_id,
        repairTopology,
      );
      topologySignatureRef.current = nextTopologySignature;
      lastApiSizeRef.current = { width: api.width, height: api.height };
    } finally {
      releaseProjectionGuard();
    }
  }, [api, document, reconcileNonce, render_surface, renderer_policy, safe_mode, surface_title, zoomed_group_id]);

  useLayoutEffect(() => {
    if (!api || safe_mode) return;
    let activationScheduled = false;
    let pendingActivation: { group_id: string; surface_id: string | null } | null = null;
    const scheduleActivation = (groupId: string, surfaceId: string | null): void => {
      const currentDocument = documentRef.current;
      const currentGroup = currentDocument.groups[groupId];
      if (
        currentDocument.active_group_id === groupId
        && currentGroup?.active_surface_id === surfaceId
      ) return;
      pendingActivation = { group_id: groupId, surface_id: surfaceId };
      if (activationScheduled) return;
      activationScheduled = true;
      queueMicrotask(() => {
        activationScheduled = false;
        const activation = pendingActivation;
        pendingActivation = null;
        if (!activation || projectionGuardRef.current > 0) return;
        emitCommand({ type: "set_active_surface", ...activation });
      });
    };
    const moveDisposable = api.onDidMovePanel((event) => {
      const index = event.panel.group.panels.findIndex((panel) => panel.id === event.panel.id);
      const expected = expectedMovesRef.current.get(event.panel.id);
      if (expected?.group_id === event.panel.group.id && expected.index === index) {
        expectedMovesRef.current.delete(event.panel.id);
        return;
      }
      if (projectionGuardRef.current > 0) return;
      emitCommand({
        type: "move_surface",
        surface_id: event.panel.id,
        group_id: event.panel.group.id,
        index,
      });
    });
    const activeDisposable = api.onDidActivePanelChange((event) => {
      if (projectionGuardRef.current > 0 || event.origin !== "user" || !event.panel) return;
      scheduleActivation(event.panel.group.id, event.panel.id);
    });
    const activeGroupDisposable = api.onDidActiveGroupChange((group) => {
      if (projectionGuardRef.current > 0 || !group) return;
      scheduleActivation(group.id, group.activePanel?.id ?? null);
    });
    const layoutDisposable = api.onDidLayoutChange(() => {
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
    return () => {
      moveDisposable.dispose();
      activeDisposable.dispose();
      activeGroupDisposable.dispose();
      layoutDisposable.dispose();
      removeDisposable.dispose();
    };
  }, [api, emitCommand, requestCanonicalReconcile, safe_mode]);

  const handleWillDrop = useCallback((event: DockviewWillDropEvent) => {
    routeWorkbenchDockviewDrop(event, props.on_surface_drop);
  }, [props.on_surface_drop]);

  return (
    <AdapterRuntimeContext.Provider value={runtime}>
      <div
        className="wardian-workbench-layout"
        data-layout-source="wardian-model"
        data-safe-mode={String(safe_mode)}
        data-zoomed-group-id={zoomed_group_id ?? "none"}
      >
        {safe_mode ? (
          <SafeWorkbenchLayout {...props} />
        ) : (
          <DockviewReact
            components={DOCKVIEW_COMPONENTS}
            theme={WARDIAN_DOCKVIEW_THEME}
            defaultTabComponent={DockviewSurfaceTab}
            rightHeaderActionsComponent={DockviewGroupActions}
            watermarkComponent={DockviewEmptyGroup}
            dndStrategy="pointer"
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
