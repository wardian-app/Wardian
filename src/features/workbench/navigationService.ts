import type {
  CloseDecision,
  OpenSurfaceRequest,
  WorkbenchNodeV1,
  WorkbenchSurfaceV1,
} from "../../types";
import type { WorkbenchCommand } from "./workbenchModel";
import type { WorkbenchSurfaceRegistry } from "./surfaceRegistry";
import type { WorkbenchStore } from "./useWorkbenchStore";

export type WorkbenchIdKind = "surface" | "group" | "node";

export type WorkbenchNavigationOptions = {
  registry: WorkbenchSurfaceRegistry;
  store: WorkbenchStore;
  create_id?: (kind: WorkbenchIdKind) => string;
  /** Rejects only a split whose destination has been measured below its pane floor. */
  can_split_group?: (
    group_id: string,
    direction: "horizontal" | "vertical",
  ) => boolean;
  reset_document?: (expected_transaction_version: number) => boolean | Promise<boolean>;
};

export interface WorkbenchNavigationService {
  open(request: OpenSurfaceRequest): string;
  /** Converts an inline New Tab in place, or discards it before focusing a matching singleton. */
  open_from_placeholder(surface_id: string, request: OpenSurfaceRequest): string;
  /** Atomically consumes an inline New Tab before reopening the latest closed surface. */
  reopen_closed_from_placeholder(surface_id: string): void;
  open_to_side(
    request: OpenSurfaceRequest,
    direction?: "horizontal" | "vertical",
  ): string | null;
  focus(surface_id: string): void;
  rebind_resource(surface_id: string, request: OpenSurfaceRequest): Promise<CloseDecision>;
  reset_surface(surface_id: string): Promise<CloseDecision>;
  close(surface_id: string): Promise<CloseDecision>;
  close_group(group_id: string): Promise<CloseDecision>;
  reset_workbench(): Promise<CloseDecision>;
}

function defaultCreateId(kind: WorkbenchIdKind): string {
  return `${kind}-${globalThis.crypto.randomUUID()}`;
}

function groupIdsDepthFirst(node: WorkbenchNodeV1): string[] {
  return node.kind === "group"
    ? [node.group_id]
    : [...groupIdsDepthFirst(node.first), ...groupIdsDepthFirst(node.second)];
}

function commandFailure(errors: readonly { path: string; message: string }[]): Error {
  return new Error(
    `workbench command rejected: ${errors
      .map((error) => `${error.path} ${error.message}`)
      .join(", ")}`,
  );
}

export function createWorkbenchNavigationService(
  options: WorkbenchNavigationOptions,
): WorkbenchNavigationService {
  const { registry, store } = options;
  const createId = options.create_id ?? defaultCreateId;

  const apply = (commands: readonly WorkbenchCommand[]): void => {
    const result = store.getState().apply_commands(commands);
    if (!result.accepted) throw commandFailure(result.errors);
  };

  const createSurface = (
    request: OpenSurfaceRequest,
    surfaceId: string,
  ): WorkbenchSurfaceV1 => {
    const definition = registry.require(request.surface_type);
    const state = request.state === undefined
      ? registry.default_state(request.surface_type)
      : request.state;
    const serialized = registry.serialize_state(request.surface_type, state);
    const resourceKey = registry.resource_key(request);
    return {
      surface_id: surfaceId,
      surface_type: definition.type,
      ...(resourceKey === undefined ? {} : { resource_key: resourceKey }),
      ...serialized,
    };
  };

  const guardSurfaces = async (
    snapshot: ReturnType<WorkbenchStore["getState"]>["document"],
    expectedTransactionVersion: number,
    surfaceIds: readonly string[],
  ): Promise<CloseDecision> => {
    for (const surfaceId of surfaceIds) {
      if (store.getState().transaction_version !== expectedTransactionVersion) return "cancel";
      const surface = snapshot.surfaces[surfaceId];
      if (!surface) return "cancel";
      if (await registry.can_close(surface) === "cancel") return "cancel";
      if (store.getState().transaction_version !== expectedTransactionVersion) return "cancel";
    }
    return "allow";
  };

  const closeResult = (
    result: ReturnType<ReturnType<WorkbenchStore["getState"]>["compare_and_apply_commands"]>,
  ): CloseDecision => {
    if (result.accepted) return "allow";
    if (result.stale) return "cancel";
    throw commandFailure(result.errors);
  };

  return {
    open: (request) => {
      const definition = registry.require(request.surface_type);
      const state = store.getState();
      const document = state.document;
      const candidates = state.surface_mru
        .map((surfaceId) => document.surfaces[surfaceId])
        .filter((surface): surface is WorkbenchSurfaceV1 => surface !== undefined);
      for (const surface of Object.values(document.surfaces)) {
        if (!candidates.some((candidate) => candidate.surface_id === surface.surface_id)) {
          candidates.push(surface);
        }
      }
      const existingId = registry.resolve_existing(
        definition.open_policy === "singleton"
          ? { ...request, duplicate: false }
          : request,
        candidates,
      );
      if (existingId) {
        apply([{ type: "focus_surface", surface_id: existingId }]);
        return existingId;
      }

      const surfaceId = createId("surface");
      const surface = createSurface(request, surfaceId);
      apply([{
        type: "open_surface",
        surface,
        ...(request.group_id === undefined ? {} : { group_id: request.group_id }),
      }]);
      return surfaceId;
    },

    open_from_placeholder: (surfaceId, request) => {
      const definition = registry.require(request.surface_type);
      const document = store.getState().document;
      const placeholder = document.surfaces[surfaceId];
      if (!placeholder) throw new Error(`surface ${surfaceId} does not exist`);
      if (placeholder.surface_type !== "new-tab") {
        throw new Error(`surface ${surfaceId} is not a New Tab placeholder`);
      }
      const candidates = store.getState().surface_mru
        .map((candidateId) => document.surfaces[candidateId])
        .filter((surface): surface is WorkbenchSurfaceV1 => (
          surface !== undefined && surface.surface_id !== surfaceId
        ));
      for (const surface of Object.values(document.surfaces)) {
        if (
          surface.surface_id !== surfaceId
          && !candidates.some((candidate) => candidate.surface_id === surface.surface_id)
        ) candidates.push(surface);
      }
      const existingId = registry.resolve_existing(
        definition.open_policy === "singleton"
          ? { ...request, duplicate: false }
          : request,
        candidates,
      );
      if (existingId) {
        apply([
          { type: "discard_surface", surface_id: surfaceId },
          { type: "focus_surface", surface_id: existingId },
        ]);
        return existingId;
      }

      apply([
        {
          type: "replace_surface",
          surface: createSurface(request, surfaceId),
        },
        { type: "focus_surface", surface_id: surfaceId },
      ]);
      return surfaceId;
    },

    reopen_closed_from_placeholder: (surfaceId) => {
      apply([{ type: "reopen_closed_in_placeholder", surface_id: surfaceId }]);
    },

    open_to_side: (request, direction = "horizontal") => {
      const definition = registry.require(request.surface_type);
      const document = store.getState().document;
      if (definition.open_policy === "singleton") {
        const existingId = registry.resolve_existing(
          { ...request, duplicate: false },
          Object.values(document.surfaces),
        );
        if (existingId) {
          apply([{ type: "focus_surface", surface_id: existingId }]);
          return existingId;
        }
      }
      const sourceGroupId = request.group_id ?? document.active_group_id;
      if (!(sourceGroupId in document.groups)) {
        throw new Error(`group ${sourceGroupId} does not exist`);
      }
      if (options.can_split_group && !options.can_split_group(sourceGroupId, direction)) {
        return null;
      }
      const groupId = createId("group");
      const nodeId = createId("node");
      const surfaceId = createId("surface");
      const surface = createSurface({ ...request, duplicate: true }, surfaceId);
      apply([
        {
          type: "split_group",
          group_id: sourceGroupId,
          new_group_id: groupId,
          node_id: nodeId,
          direction,
          placement: "after",
        },
        { type: "open_surface", surface, group_id: groupId },
      ]);
      return surfaceId;
    },

    focus: (surfaceId) => apply([{ type: "focus_surface", surface_id: surfaceId }]),

    rebind_resource: async (surfaceId, request) => {
      registry.require(request.surface_type);
      const snapshotState = store.getState();
      const snapshot = snapshotState.document;
      if (!(surfaceId in snapshot.surfaces)) {
        throw new Error(`surface ${surfaceId} does not exist`);
      }
      const replacement = createSurface(request, surfaceId);
      if (
        await guardSurfaces(snapshot, snapshotState.transaction_version, [surfaceId]) === "cancel"
      ) return "cancel";
      return closeResult(store.getState().compare_and_apply_commands(
        snapshotState.transaction_version,
        [{ type: "replace_surface", surface: replacement }],
      ));
    },

    reset_surface: async (surfaceId) => {
      const snapshotState = store.getState();
      const snapshot = snapshotState.document;
      const surface = snapshot.surfaces[surfaceId];
      if (!surface) throw new Error(`surface ${surfaceId} does not exist`);
      if (
        await guardSurfaces(snapshot, snapshotState.transaction_version, [surfaceId]) === "cancel"
      ) return "cancel";

      const definition = registry.get(surface.surface_type);
      const resetState = definition
        ? registry.serialize_state(surface.surface_type, registry.default_state(surface.surface_type))
        : { state_schema_version: surface.state_schema_version, state: {} };
      return closeResult(store.getState().compare_and_apply_commands(
        snapshotState.transaction_version,
        [{
          type: "update_surface_state",
          surface_id: surfaceId,
          ...resetState,
        }],
      ));
    },

    close: async (surfaceId) => {
      const snapshotState = store.getState();
      const snapshot = snapshotState.document;
      if (!(surfaceId in snapshot.surfaces)) {
        throw new Error(`surface ${surfaceId} does not exist`);
      }
      if (
        await guardSurfaces(snapshot, snapshotState.transaction_version, [surfaceId]) === "cancel"
      ) return "cancel";
      return closeResult(store.getState().compare_and_apply_commands(
        snapshotState.transaction_version,
        [{ type: "close_surface", surface_id: surfaceId }],
      ));
    },

    close_group: async (groupId) => {
      const snapshotState = store.getState();
      const snapshot = snapshotState.document;
      const group = snapshot.groups[groupId];
      if (!group) throw new Error(`group ${groupId} does not exist`);
      if (
        await guardSurfaces(snapshot, snapshotState.transaction_version, group.surface_ids)
          === "cancel"
      ) return "cancel";
      return closeResult(store.getState().compare_and_apply_commands(
        snapshotState.transaction_version,
        [{ type: "close_group", group_id: groupId }],
      ));
    },

    reset_workbench: async () => {
      const snapshotState = store.getState();
      const snapshot = snapshotState.document;
      const surfaceIds = groupIdsDepthFirst(snapshot.root).flatMap(
        (groupId) => snapshot.groups[groupId]?.surface_ids ?? [],
      );
      if (
        await guardSurfaces(snapshot, snapshotState.transaction_version, surfaceIds) === "cancel"
      ) return "cancel";
      if (options.reset_document) {
        if (store.getState().transaction_version !== snapshotState.transaction_version) {
          return "cancel";
        }
        return await options.reset_document(snapshotState.transaction_version) ? "allow" : "cancel";
      }
      const result = store.getState().compare_and_reset_document(
        snapshotState.transaction_version,
      );
      if (!result.accepted && result.stale) return "cancel";
      if (!result.accepted) throw commandFailure(result.errors);
      return "allow";
    },
  };
}
