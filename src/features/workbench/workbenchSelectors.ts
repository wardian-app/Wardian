import type {
  WorkbenchDocumentV1,
  WorkbenchGroupV1,
  WorkbenchNodeV1,
  WorkbenchSurfaceV1,
} from "../../types";
import type { WorkbenchStoreState } from "./useWorkbenchStore";

type WorkbenchDocumentState = Pick<WorkbenchStoreState, "document">;

const groupsInTreeOrderCache = new WeakMap<
  WorkbenchDocumentV1,
  readonly WorkbenchGroupV1[]
>();
const surfacesInTreeOrderCache = new WeakMap<
  WorkbenchDocumentV1,
  readonly WorkbenchSurfaceV1[]
>();

function groupIdsInTreeOrder(node: WorkbenchNodeV1): string[] {
  return node.kind === "group"
    ? [node.group_id]
    : [...groupIdsInTreeOrder(node.first), ...groupIdsInTreeOrder(node.second)];
}

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

export function selectWorkbenchGroupsInTreeOrder(
  state: WorkbenchDocumentState,
): readonly WorkbenchGroupV1[] {
  const cached = groupsInTreeOrderCache.get(state.document);
  if (cached) return cached;
  const groups = Object.freeze(
    groupIdsInTreeOrder(state.document.root).map(
      (groupId) => state.document.groups[groupId],
    ),
  );
  groupsInTreeOrderCache.set(state.document, groups);
  return groups;
}

export function selectWorkbenchSurfacesInTreeOrder(
  state: WorkbenchDocumentState,
): readonly WorkbenchSurfaceV1[] {
  const cached = surfacesInTreeOrderCache.get(state.document);
  if (cached) return cached;
  const surfaces = Object.freeze(
    selectWorkbenchGroupsInTreeOrder(state).flatMap((group) =>
      group.surface_ids.map((surfaceId) => state.document.surfaces[surfaceId])),
  );
  surfacesInTreeOrderCache.set(state.document, surfaces);
  return surfaces;
}

export function selectWorkbenchGroup(groupId: string) {
  return (state: WorkbenchDocumentState): WorkbenchGroupV1 | undefined =>
    state.document.groups[groupId];
}

export function selectWorkbenchSurface(surfaceId: string) {
  return (state: WorkbenchDocumentState): WorkbenchSurfaceV1 | undefined =>
    state.document.surfaces[surfaceId];
}

export function selectWorkbenchGroupForSurface(surfaceId: string) {
  return (state: WorkbenchDocumentState): WorkbenchGroupV1 | undefined =>
    selectWorkbenchGroupsInTreeOrder(state).find(
      (group) => group.surface_ids.includes(surfaceId),
    );
}

export function selectWorkbenchGroupShowsHome(groupId: string) {
  return (state: WorkbenchDocumentState): boolean =>
    state.document.groups[groupId]?.surface_ids.length === 0;
}

export function selectWorkbenchZoomedGroupId(
  state: Pick<WorkbenchStoreState, "zoomed_group_id">,
): string | null {
  return state.zoomed_group_id;
}

export function selectWorkbenchLauncherOpen(
  state: Pick<WorkbenchStoreState, "launcher_open">,
): boolean {
  return state.launcher_open;
}

export function selectWorkbenchTransactionVersion(
  state: Pick<WorkbenchStoreState, "transaction_version">,
): number {
  return state.transaction_version;
}

export function selectWorkbenchDurableDocument(
  state: Pick<WorkbenchStoreState, "durable_document">,
): WorkbenchDocumentV1 {
  return state.durable_document;
}

export function selectWorkbenchDurableRevision(
  state: Pick<WorkbenchStoreState, "durable_revision">,
): number {
  return state.durable_revision;
}

export function selectWorkbenchDurableToken(
  state: Pick<WorkbenchStoreState, "durable_token">,
): string | null {
  return state.durable_token;
}

export function selectWorkbenchPendingRequestId(
  state: Pick<WorkbenchStoreState, "pending_request_id">,
): string | null {
  return state.pending_request_id;
}

export function selectWorkbenchPendingRevision(
  state: Pick<WorkbenchStoreState, "pending_revision">,
): number | null {
  return state.pending_revision;
}

export function selectWorkbenchHasPendingSave(
  state: Pick<WorkbenchStoreState, "save_pending">,
): boolean {
  return state.save_pending;
}

export function selectWorkbenchConflict(
  state: Pick<WorkbenchStoreState, "conflict">,
): string | null {
  return state.conflict;
}

export function selectWorkbenchLoading(
  state: Pick<WorkbenchStoreState, "loading">,
): boolean {
  return state.loading;
}

export function selectWorkbenchReadOnly(
  state: Pick<WorkbenchStoreState, "read_only">,
): boolean {
  return state.read_only;
}

export function selectWorkbenchDirty(
  state: Pick<WorkbenchStoreState, "is_dirty">,
): boolean {
  return state.is_dirty;
}

export function selectWorkbenchSaveError(
  state: Pick<WorkbenchStoreState, "save_error">,
): string | null {
  return state.save_error;
}
