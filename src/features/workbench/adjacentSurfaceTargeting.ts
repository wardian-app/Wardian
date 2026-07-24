import type { WorkbenchNodeV1 } from "../../types";
import type { DeepReadonly, ReadonlyWorkbenchDocumentV1 } from "./useWorkbenchStore";

type PaneBounds = {
  left: number;
  top: number;
  right: number;
  bottom: number;
};

const BOUNDARY_EPSILON = 0.000_001;

function collectGroupBounds(
  node: DeepReadonly<WorkbenchNodeV1>,
  bounds: PaneBounds,
  groups: Map<string, PaneBounds>,
): void {
  if (node.kind === "group") {
    groups.set(node.group_id, bounds);
    return;
  }

  if (node.direction === "horizontal") {
    const split = bounds.left + ((bounds.right - bounds.left) * node.ratio);
    collectGroupBounds(node.first, { ...bounds, right: split }, groups);
    collectGroupBounds(node.second, { ...bounds, left: split }, groups);
    return;
  }

  const split = bounds.top + ((bounds.bottom - bounds.top) * node.ratio);
  collectGroupBounds(node.first, { ...bounds, bottom: split }, groups);
  collectGroupBounds(node.second, { ...bounds, top: split }, groups);
}

function overlapLength(firstStart: number, firstEnd: number, secondStart: number, secondEnd: number): number {
  return Math.max(0, Math.min(firstEnd, secondEnd) - Math.max(firstStart, secondStart));
}

/**
 * Returns the shared edge length for two workbench panes, or zero when they
 * do not share a visible boundary.
 */
function sharedBoundaryLength(first: PaneBounds, second: PaneBounds): number {
  let sharedLength = 0;
  const sharesVerticalBoundary = (
    Math.abs(first.right - second.left) < BOUNDARY_EPSILON
    || Math.abs(first.left - second.right) < BOUNDARY_EPSILON
  );
  if (sharesVerticalBoundary) {
    sharedLength = Math.max(
      sharedLength,
      overlapLength(first.top, first.bottom, second.top, second.bottom),
    );
  }

  const sharesHorizontalBoundary = (
    Math.abs(first.bottom - second.top) < BOUNDARY_EPSILON
    || Math.abs(first.top - second.bottom) < BOUNDARY_EPSILON
  );
  if (sharesHorizontalBoundary) {
    sharedLength = Math.max(
      sharedLength,
      overlapLength(first.left, first.right, second.left, second.right),
    );
  }

  return sharedLength > BOUNDARY_EPSILON ? sharedLength : 0;
}

/**
 * Finds the active surface of the requested type in a pane sharing an edge
 * with the invoking surface. A selected tab is the only visible surface in
 * its pane, so inactive tabs are deliberately not eligible targets.
 */
export function findAdjacentActiveSurface(
  document: ReadonlyWorkbenchDocumentV1,
  sourceSurfaceId: string,
  surfaceType: string,
): string | undefined {
  const sourceGroup = Object.values(document.groups).find((group) => (
    group.surface_ids.includes(sourceSurfaceId)
  ));
  if (!sourceGroup) return undefined;

  const groupBounds = new Map<string, PaneBounds>();
  collectGroupBounds(document.root, {
    left: 0,
    top: 0,
    right: 1,
    bottom: 1,
  }, groupBounds);
  const sourceBounds = groupBounds.get(sourceGroup.group_id);
  if (!sourceBounds) return undefined;

  let target: { surfaceId: string; sharedBoundaryLength: number } | undefined;
  for (const [groupId, bounds] of groupBounds) {
    if (groupId === sourceGroup.group_id) continue;
    const group = document.groups[groupId];
    const activeSurfaceId = group?.active_surface_id;
    const activeSurface = activeSurfaceId ? document.surfaces[activeSurfaceId] : undefined;
    if (!activeSurface || activeSurface.surface_type !== surfaceType) continue;

    const boundaryLength = sharedBoundaryLength(sourceBounds, bounds);
    if (
      boundaryLength === 0
      || (target && boundaryLength <= target.sharedBoundaryLength)
    ) continue;
    target = { surfaceId: activeSurface.surface_id, sharedBoundaryLength: boundaryLength };
  }

  return target?.surfaceId;
}
