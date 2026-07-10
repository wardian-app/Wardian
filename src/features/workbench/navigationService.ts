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
};

export interface WorkbenchNavigationService {
  open(request: OpenSurfaceRequest): string;
  open_to_side(
    request: OpenSurfaceRequest,
    direction?: "horizontal" | "vertical",
  ): string;
  focus(surface_id: string): void;
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
      ? definition.default_state()
      : request.state;
    const serialized = registry.serialize_state(request.surface_type, state);
    const resourceKey = definition.resource_key?.(request) ?? request.resource_key;
    return {
      surface_id: surfaceId,
      surface_type: definition.type,
      ...(resourceKey === undefined ? {} : { resource_key: resourceKey }),
      ...serialized,
    };
  };

  const guardSurfaces = async (
    snapshot: ReturnType<WorkbenchStore["getState"]>["document"],
    surfaceIds: readonly string[],
  ): Promise<CloseDecision> => {
    for (const surfaceId of surfaceIds) {
      if (store.getState().document !== snapshot) return "cancel";
      const surface = snapshot.surfaces[surfaceId];
      if (!surface) return "cancel";
      if (await registry.can_close(surface) === "cancel") return "cancel";
      if (store.getState().document !== snapshot) return "cancel";
    }
    return "allow";
  };

  return {
    open: (request) => {
      registry.require(request.surface_type);
      const document = store.getState().document;
      const existingId = registry.resolve_existing(
        request,
        Object.values(document.surfaces),
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

    open_to_side: (request, direction = "horizontal") => {
      registry.require(request.surface_type);
      const document = store.getState().document;
      const sourceGroupId = request.group_id ?? document.active_group_id;
      if (!(sourceGroupId in document.groups)) {
        throw new Error(`group ${sourceGroupId} does not exist`);
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

    close: async (surfaceId) => {
      const snapshot = store.getState().document;
      if (!(surfaceId in snapshot.surfaces)) {
        throw new Error(`surface ${surfaceId} does not exist`);
      }
      if (await guardSurfaces(snapshot, [surfaceId]) === "cancel") return "cancel";
      apply([{ type: "close_surface", surface_id: surfaceId }]);
      return "allow";
    },

    close_group: async (groupId) => {
      const snapshot = store.getState().document;
      const group = snapshot.groups[groupId];
      if (!group) throw new Error(`group ${groupId} does not exist`);
      if (await guardSurfaces(snapshot, group.surface_ids) === "cancel") return "cancel";
      apply([{ type: "close_group", group_id: groupId }]);
      return "allow";
    },

    reset_workbench: async () => {
      const snapshot = store.getState().document;
      const surfaceIds = groupIdsDepthFirst(snapshot.root).flatMap(
        (groupId) => snapshot.groups[groupId]?.surface_ids ?? [],
      );
      if (await guardSurfaces(snapshot, surfaceIds) === "cancel") return "cancel";
      const result = store.getState().reset_document();
      if (!result.accepted) throw commandFailure(result.errors);
      return "allow";
    },
  };
}
