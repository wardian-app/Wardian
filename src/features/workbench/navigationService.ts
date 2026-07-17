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
  /** Opens one replaceable preview in the target group without disturbing other groups. */
  open_transient(request: OpenSurfaceRequest): string;
  /** Converts a replaceable preview into a permanent surface in place. */
  pin_transient(surface_id: string): void;
  /** Converts an inline New Tab in place, or discards it before focusing a matching singleton. */
  open_from_placeholder(surface_id: string, request: OpenSurfaceRequest): string;
  /** Atomically consumes an inline New Tab before reopening the latest closed surface. */
  reopen_closed_from_placeholder(surface_id: string): void;
  open_to_side(
    request: OpenSurfaceRequest,
    direction?: "horizontal" | "vertical",
  ): string | null;
  focus(surface_id: string): void;
  /** Converges a provisional resource key after the backend returns canonical identity. */
  canonicalize_resource(surface_id: string, request: OpenSurfaceRequest): Promise<CloseDecision>;
  rebind_resource(surface_id: string, request: OpenSurfaceRequest): Promise<CloseDecision>;
  reset_surface(surface_id: string): Promise<CloseDecision>;
  close(surface_id: string): Promise<CloseDecision>;
  close_group(group_id: string): Promise<CloseDecision>;
  reset_workbench(): Promise<CloseDecision>;
}

type SurfaceGuardResult = "allow" | "cancel" | "stale";
type ExplicitDuplicateProvenance = {
  partner_surface_id: string | null;
  provisional_resource_key: string | undefined;
};
const MAX_CANONICALIZE_ATTEMPTS = 8;

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
  const explicitDuplicateProvenance = new Map<string, ExplicitDuplicateProvenance>();

  const discardDuplicateProvenance = (surfaceId: string): void => {
    explicitDuplicateProvenance.delete(surfaceId);
    for (const [duplicateId, provenance] of explicitDuplicateProvenance) {
      if (provenance.partner_surface_id === surfaceId) {
        explicitDuplicateProvenance.delete(duplicateId);
      }
    }
  };

  const pruneSettledDuplicateProvenance = (
    document: ReturnType<WorkbenchStore["getState"]>["document"],
    surfaceId: string,
  ): void => {
    for (const [duplicateId, provenance] of explicitDuplicateProvenance) {
      if (duplicateId !== surfaceId && provenance.partner_surface_id !== surfaceId) continue;
      const duplicate = document.surfaces[duplicateId];
      const partner = provenance.partner_surface_id
        ? document.surfaces[provenance.partner_surface_id]
        : undefined;
      const duplicateIsProvisional = duplicate?.resource_key
        === provenance.provisional_resource_key;
      const partnerIsProvisional = partner?.resource_key
        === provenance.provisional_resource_key;
      if (!duplicate || !partner || duplicateIsProvisional === partnerIsProvisional) {
        explicitDuplicateProvenance.delete(duplicateId);
      }
    }
  };

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

  const transientStatus = (surface: WorkbenchSurfaceV1): boolean | undefined => {
    const resolved = registry.resolve_surface(surface);
    if (!resolved.restore_result.ok || !resolved.definition.transient_state) return undefined;
    try {
      return resolved.definition.transient_state.is_transient(
        resolved.restore_result.state,
      ) === true;
    } catch {
      return undefined;
    }
  };

  const orderedSurfaces = (
    state: ReturnType<WorkbenchStore["getState"]>,
  ): WorkbenchSurfaceV1[] => {
    const candidates = state.surface_mru
      .map((surfaceId) => state.document.surfaces[surfaceId])
      .filter((surface): surface is WorkbenchSurfaceV1 => surface !== undefined);
    for (const surface of Object.values(state.document.surfaces)) {
      if (!candidates.some((candidate) => candidate.surface_id === surface.surface_id)) {
        candidates.push(surface);
      }
    }
    return candidates;
  };

  const guardSurfaces = async (
    snapshot: ReturnType<WorkbenchStore["getState"]>["document"],
    expectedTransactionVersion: number,
    surfaceIds: readonly string[],
  ): Promise<SurfaceGuardResult> => {
    for (const surfaceId of surfaceIds) {
      if (store.getState().transaction_version !== expectedTransactionVersion) return "stale";
      const surface = snapshot.surfaces[surfaceId];
      if (!surface) return "stale";
      if (await registry.can_close(surface) === "cancel") return "cancel";
      if (store.getState().transaction_version !== expectedTransactionVersion) return "stale";
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
      const candidates = orderedSurfaces(state);
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

    open_transient: (request) => {
      const definition = registry.require(request.surface_type);
      if (!definition.transient_state) {
        throw new Error(`surface type ${request.surface_type} does not support transient previews`);
      }
      const state = store.getState();
      const document = state.document;
      const targetGroupId = request.group_id ?? document.active_group_id;
      const targetGroup = document.groups[targetGroupId];
      if (!targetGroup) throw new Error(`group ${targetGroupId} does not exist`);

      const permanentCandidates = orderedSurfaces(state).filter((surface) => (
        surface.surface_type === definition.type && transientStatus(surface) === false
      ));
      const existingPermanent = registry.resolve_existing(
        { ...request, duplicate: false },
        permanentCandidates,
      );
      if (existingPermanent) {
        apply([{ type: "focus_surface", surface_id: existingPermanent }]);
        return existingPermanent;
      }

      const targetSurfaceIds = [
        ...(targetGroup.active_surface_id ? [targetGroup.active_surface_id] : []),
        ...[...targetGroup.surface_ids].reverse().filter(
          (surfaceId) => surfaceId !== targetGroup.active_surface_id,
        ),
      ];
      const replaceId = targetSurfaceIds.find((surfaceId) => {
        const surface = document.surfaces[surfaceId];
        return surface?.surface_type === definition.type && transientStatus(surface) === true;
      });
      if (replaceId) {
        const replacement = createSurface(request, replaceId);
        if (transientStatus(replacement) !== true) {
          throw new Error("open_transient requires transient surface state");
        }
        apply([
          { type: "replace_surface", surface: replacement },
          { type: "focus_surface", surface_id: replaceId },
        ]);
        return replaceId;
      }

      const surfaceId = createId("surface");
      const surface = createSurface(request, surfaceId);
      if (transientStatus(surface) !== true) {
        throw new Error("open_transient requires transient surface state");
      }
      apply([{ type: "open_surface", surface, group_id: targetGroupId }]);
      return surfaceId;
    },

    pin_transient: (surfaceId) => {
      const surface = store.getState().document.surfaces[surfaceId];
      if (!surface) throw new Error(`surface ${surfaceId} does not exist`);
      const resolved = registry.resolve_surface(surface);
      if (!resolved.restore_result.ok) {
        throw new Error(`surface ${surfaceId} has invalid state: ${resolved.restore_result.error}`);
      }
      const transientState = resolved.definition.transient_state;
      if (!transientState) {
        throw new Error(`surface type ${surface.surface_type} does not support transient previews`);
      }
      if (!transientState.is_transient(resolved.restore_result.state)) return;
      const serialized = registry.serialize_state(
        surface.surface_type,
        transientState.pin(resolved.restore_result.state),
      );
      apply([{
        type: "update_surface_state",
        surface_id: surfaceId,
        ...serialized,
      }]);
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
      const duplicatePartnerId = registry.resolve_existing(
        { ...request, duplicate: false },
        Object.values(document.surfaces),
      );
      if (definition.open_policy === "singleton") {
        if (duplicatePartnerId) {
          apply([{ type: "focus_surface", surface_id: duplicatePartnerId }]);
          return duplicatePartnerId;
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
      explicitDuplicateProvenance.set(surfaceId, {
        partner_surface_id: duplicatePartnerId ?? null,
        provisional_resource_key: surface.resource_key,
      });
      return surfaceId;
    },

    focus: (surfaceId) => apply([{ type: "focus_surface", surface_id: surfaceId }]),

    canonicalize_resource: async (surfaceId, request) => {
      registry.require(request.surface_type);
      for (let attempt = 0; attempt < MAX_CANONICALIZE_ATTEMPTS; attempt += 1) {
        const snapshotState = store.getState();
        const snapshot = snapshotState.document;
        const source = snapshot.surfaces[surfaceId];
        const candidates = orderedSurfaces(snapshotState).filter(
          (candidate) => candidate.surface_id !== surfaceId,
        );
        const existingId = registry.resolve_existing(
          { ...request, duplicate: false },
          candidates,
        );

        if (!source) {
          discardDuplicateProvenance(surfaceId);
          if (!existingId) return "allow";
          const focusResult = store.getState().compare_and_apply_commands(
            snapshotState.transaction_version,
            [{ type: "focus_surface", surface_id: existingId }],
          );
          if (focusResult.accepted) return "allow";
          if (focusResult.stale) continue;
          throw commandFailure(focusResult.errors);
        }

        const replacement = createSurface(request, surfaceId);
        if (replacement.resource_key === source.resource_key) {
          pruneSettledDuplicateProvenance(snapshot, surfaceId);
          return "allow";
        }

        const sourceProvenance = explicitDuplicateProvenance.get(surfaceId);
        const existingProvenance = existingId
          ? explicitDuplicateProvenance.get(existingId)
          : undefined;
        const completesExplicitDuplicate = existingProvenance !== undefined
          && existingProvenance.partner_surface_id === surfaceId
          && existingProvenance.provisional_resource_key === source.resource_key;
        const preserveExplicitDuplicate = sourceProvenance !== undefined
          || completesExplicitDuplicate;
        let commands: WorkbenchCommand[];
        let guardedSurfaceIds = [surfaceId];

        if (existingId && !preserveExplicitDuplicate) {
          const existing = snapshot.surfaces[existingId];
          if (!existing) continue;
          if (transientStatus(source) === false && transientStatus(existing) === true) {
            guardedSurfaceIds = [surfaceId, existingId];
            commands = [
              {
                type: "discard_surface",
                surface_id: surfaceId,
                provisional_identity: true,
              },
              { type: "replace_surface", surface: createSurface(request, existingId) },
              { type: "focus_surface", surface_id: existingId },
            ];
          } else {
            commands = [
              {
                type: "discard_surface",
                surface_id: surfaceId,
                provisional_identity: true,
              },
              { type: "focus_surface", surface_id: existingId },
            ];
          }
        } else {
          commands = [{ type: "replace_surface", surface: replacement }];
        }

        const guard = await guardSurfaces(
          snapshot,
          snapshotState.transaction_version,
          guardedSurfaceIds,
        );
        if (guard === "cancel") return "cancel";
        if (guard === "stale") continue;
        const result = store.getState().compare_and_apply_commands(
          snapshotState.transaction_version,
          commands,
        );
        if (result.accepted) {
          const currentDocument = store.getState().document;
          pruneSettledDuplicateProvenance(currentDocument, surfaceId);
          return "allow";
        }
        if (result.stale) continue;
        throw commandFailure(result.errors);
      }
      return "cancel";
    },

    rebind_resource: async (surfaceId, request) => {
      registry.require(request.surface_type);
      const snapshotState = store.getState();
      const snapshot = snapshotState.document;
      if (!(surfaceId in snapshot.surfaces)) {
        throw new Error(`surface ${surfaceId} does not exist`);
      }
      const replacement = createSurface(request, surfaceId);
      if (
        await guardSurfaces(snapshot, snapshotState.transaction_version, [surfaceId]) !== "allow"
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
        await guardSurfaces(snapshot, snapshotState.transaction_version, [surfaceId]) !== "allow"
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
        await guardSurfaces(snapshot, snapshotState.transaction_version, [surfaceId]) !== "allow"
      ) return "cancel";
      const decision = closeResult(store.getState().compare_and_apply_commands(
        snapshotState.transaction_version,
        [{ type: "close_surface", surface_id: surfaceId }],
      ));
      if (decision === "allow") discardDuplicateProvenance(surfaceId);
      return decision;
    },

    close_group: async (groupId) => {
      const snapshotState = store.getState();
      const snapshot = snapshotState.document;
      const group = snapshot.groups[groupId];
      if (!group) throw new Error(`group ${groupId} does not exist`);
      if (
        await guardSurfaces(snapshot, snapshotState.transaction_version, group.surface_ids)
          !== "allow"
      ) return "cancel";
      const decision = closeResult(store.getState().compare_and_apply_commands(
        snapshotState.transaction_version,
        [{ type: "close_group", group_id: groupId }],
      ));
      if (decision === "allow") {
        for (const surfaceId of group.surface_ids) {
          discardDuplicateProvenance(surfaceId);
        }
      }
      return decision;
    },

    reset_workbench: async () => {
      const snapshotState = store.getState();
      const snapshot = snapshotState.document;
      const surfaceIds = groupIdsDepthFirst(snapshot.root).flatMap(
        (groupId) => snapshot.groups[groupId]?.surface_ids ?? [],
      );
      if (
        await guardSurfaces(snapshot, snapshotState.transaction_version, surfaceIds) !== "allow"
      ) return "cancel";
      if (options.reset_document) {
        if (store.getState().transaction_version !== snapshotState.transaction_version) {
          return "cancel";
        }
        const decision = await options.reset_document(snapshotState.transaction_version)
          ? "allow"
          : "cancel";
        if (decision === "allow") explicitDuplicateProvenance.clear();
        return decision;
      }
      const result = store.getState().compare_and_reset_document(
        snapshotState.transaction_version,
      );
      if (!result.accepted && result.stale) return "cancel";
      if (!result.accepted) throw commandFailure(result.errors);
      explicitDuplicateProvenance.clear();
      return "allow";
    },
  };
}
