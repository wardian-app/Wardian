import type {
  WorkbenchGroupV1,
  WorkbenchSurfaceV1,
} from "../../types";
import type { WorkbenchStoreState } from "./useWorkbenchStore";

type WorkbenchDocumentState = Pick<WorkbenchStoreState, "document">;

export function selectActiveWorkbenchGroup(
  state: WorkbenchDocumentState,
): WorkbenchGroupV1 | undefined {
  return state.document.groups[state.document.active_group_id];
}

export function selectActiveWorkbenchSurface(
  state: WorkbenchDocumentState,
): WorkbenchSurfaceV1 | undefined {
  const group = selectActiveWorkbenchGroup(state);
  return group?.active_surface_id
    ? state.document.surfaces[group.active_surface_id]
    : undefined;
}

export function selectWorkbenchGroup(groupId: string) {
  return (state: WorkbenchDocumentState): WorkbenchGroupV1 | undefined =>
    state.document.groups[groupId];
}

export function selectWorkbenchSurface(surfaceId: string) {
  return (state: WorkbenchDocumentState): WorkbenchSurfaceV1 | undefined =>
    state.document.surfaces[surfaceId];
}

export function selectWorkbenchHasPendingSave(
  state: Pick<WorkbenchStoreState, "save_pending">,
): boolean {
  return state.save_pending;
}
