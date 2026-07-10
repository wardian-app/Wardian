import type {
  CloseDecision,
  OpenSurfaceRequest,
  SerializedSurfaceState,
  SurfaceDefinition,
  SurfacePresentationMetadata,
  SurfaceRestoreResult,
  SurfaceState,
  WorkbenchDocumentV1,
  WorkbenchSurfaceV1,
} from "../../types";
import {
  MAX_WORKBENCH_SURFACE_STATE_BYTES,
  createDefaultWorkbenchDocument,
  validateWorkbenchDocument,
} from "./workbenchModel";

const MISSING_SURFACE_TYPE = "missing_surface";

export type ResolvedSurface = {
  definition: SurfaceDefinition;
  restore_result: SurfaceRestoreResult;
  missing_surface_type?: string;
};

export interface WorkbenchSurfaceRegistry {
  register<TState extends SurfaceState>(definition: SurfaceDefinition<TState>): void;
  list(): readonly SurfaceDefinition[];
  get(type: string): SurfaceDefinition | undefined;
  require(type: string): SurfaceDefinition;
  serialize_state<TState extends SurfaceState>(
    type: string,
    state: TState,
  ): SerializedSurfaceState;
  resolve_surface(surface: WorkbenchSurfaceV1): ResolvedSurface;
  resolve_existing(
    request: OpenSurfaceRequest,
    candidates: WorkbenchSurfaceV1[],
  ): string | undefined;
  presentation(surface: WorkbenchSurfaceV1): SurfacePresentationMetadata;
  can_close(surface: WorkbenchSurfaceV1): Promise<CloseDecision>;
}

function stateValidationDocument(state: unknown): WorkbenchDocumentV1 {
  const document = createDefaultWorkbenchDocument();
  const surface: WorkbenchSurfaceV1 = {
    surface_id: "registry-state-validation",
    surface_type: "registry-state-validation",
    state_schema_version: 0,
    state,
  };
  return {
    ...document,
    groups: {
      "group-1": {
        group_id: "group-1",
        surface_ids: [surface.surface_id],
        active_surface_id: surface.surface_id,
      },
    },
    surfaces: { [surface.surface_id]: surface },
  };
}

function canonicalizeState(state: unknown, maxBytes: number): unknown {
  const validation = validateWorkbenchDocument(stateValidationDocument(state));
  if (!validation.valid) {
    const stateErrors = validation.errors
      .filter((error) => error.path.includes(".state"))
      .map((error) => error.message)
      .join(", ");
    throw new Error(`surface state is not canonical JSON${stateErrors ? `: ${stateErrors}` : ""}`);
  }

  const json = JSON.stringify(state);
  if (json === undefined) throw new Error("surface state is not serializable JSON");
  const byteLength = new TextEncoder().encode(json).byteLength;
  if (byteLength > maxBytes) {
    throw new Error(`serialized surface state exceeds the ${maxBytes} bytes limit`);
  }
  return JSON.parse(json) as unknown;
}

function validateDefinition(definition: SurfaceDefinition): void {
  if (definition.type.length === 0) throw new Error("surface type must not be empty");
  if (definition.type === MISSING_SURFACE_TYPE) {
    throw new Error(`${MISSING_SURFACE_TYPE} is reserved for inert placeholders`);
  }
  if (!Number.isSafeInteger(definition.state_schema_version) || definition.state_schema_version < 0) {
    throw new Error("state_schema_version must be a non-negative safe integer");
  }
  if (
    !Number.isSafeInteger(definition.max_state_bytes)
    || definition.max_state_bytes <= 0
    || definition.max_state_bytes > MAX_WORKBENCH_SURFACE_STATE_BYTES
  ) {
    throw new Error(
      `max_state_bytes must be within 1..${MAX_WORKBENCH_SURFACE_STATE_BYTES}`,
    );
  }
}

function missingDefinition(surface: WorkbenchSurfaceV1): SurfaceDefinition {
  return {
    type: MISSING_SURFACE_TYPE,
    title: () => `Missing surface: ${surface.surface_type}`,
    icon: "missing-surface",
    render_policy: "recreate_from_state",
    open_policy: "allow_multiple",
    runtime_policy: "view_only",
    close_policy: "close_view",
    state_schema_version: surface.state_schema_version,
    max_state_bytes: MAX_WORKBENCH_SURFACE_STATE_BYTES,
    default_state: () => surface.state,
    serialize_state: (state) => state,
    restore_state: (value) => ({ ok: true, state: value }),
    commands: [
      { command_id: "workbench.close_surface", title: "Close Surface" },
      { command_id: "workbench.reset_surface", title: "Reset Surface" },
    ],
    badges: () => [{ badge_id: "missing", label: `Missing ${surface.surface_type}` }],
  };
}

class SurfaceRegistry implements WorkbenchSurfaceRegistry {
  private readonly definitionsByType = new Map<string, SurfaceDefinition>();
  private readonly registrationOrder: SurfaceDefinition[] = [];

  register<TState extends SurfaceState>(definition: SurfaceDefinition<TState>): void {
    const storedDefinition = definition as SurfaceDefinition;
    validateDefinition(storedDefinition);
    if (this.definitionsByType.has(definition.type)) {
      throw new Error(`surface type ${definition.type} is already registered`);
    }
    this.definitionsByType.set(definition.type, storedDefinition);
    this.registrationOrder.push(storedDefinition);
  }

  list(): readonly SurfaceDefinition[] {
    return [...this.registrationOrder];
  }

  get(type: string): SurfaceDefinition | undefined {
    return this.definitionsByType.get(type);
  }

  require(type: string): SurfaceDefinition {
    const definition = this.get(type);
    if (!definition) throw new Error(`surface type ${type} is not registered`);
    return definition;
  }

  serialize_state<TState extends SurfaceState>(
    type: string,
    state: TState,
  ): SerializedSurfaceState {
    const definition = this.require(type);
    const serialized = definition.serialize_state(state);
    return {
      state_schema_version: definition.state_schema_version,
      state: canonicalizeState(serialized, definition.max_state_bytes),
    };
  }

  resolve_surface(surface: WorkbenchSurfaceV1): ResolvedSurface {
    const definition = this.get(surface.surface_type);
    if (!definition) {
      return {
        definition: missingDefinition(surface),
        missing_surface_type: surface.surface_type,
        restore_result: { ok: true, state: surface.state },
      };
    }

    try {
      const restoreResult = definition.restore_state(
        surface.state,
        surface.state_schema_version,
      );
      if (restoreResult.ok) {
        this.serialize_state(definition.type, restoreResult.state);
      }
      return { definition, restore_result: restoreResult };
    } catch (error) {
      return {
        definition,
        restore_result: {
          ok: false,
          error: error instanceof Error ? error.message : String(error),
        },
      };
    }
  }

  resolve_existing(
    request: OpenSurfaceRequest,
    candidates: WorkbenchSurfaceV1[],
  ): string | undefined {
    if (request.duplicate === true) return undefined;
    const definition = this.require(request.surface_type);
    const typedCandidates = candidates.filter(
      (candidate) => candidate.surface_type === definition.type,
    );
    if (typedCandidates.length === 0 || definition.open_policy === "allow_multiple") {
      return undefined;
    }

    if (definition.resolve_existing) {
      const resolvedId = definition.resolve_existing(request, typedCandidates);
      if (resolvedId && typedCandidates.some((candidate) => candidate.surface_id === resolvedId)) {
        return resolvedId;
      }
    }

    if (definition.open_policy === "singleton") {
      return typedCandidates[typedCandidates.length - 1]?.surface_id;
    }

    const resourceKey = definition.resource_key?.(request) ?? request.resource_key;
    if (resourceKey === undefined) return undefined;
    return [...typedCandidates]
      .reverse()
      .find((candidate) => candidate.resource_key === resourceKey)
      ?.surface_id;
  }

  presentation(surface: WorkbenchSurfaceV1): SurfacePresentationMetadata {
    const { definition } = this.resolve_surface(surface);
    return {
      title: definition.title(surface),
      icon: definition.icon,
      commands: [...definition.commands],
      badges: definition.badges?.(surface) ?? [],
    };
  }

  async can_close(surface: WorkbenchSurfaceV1): Promise<CloseDecision> {
    const definition = this.get(surface.surface_type);
    if (!definition?.can_close) return "allow";
    try {
      return await definition.can_close(surface);
    } catch {
      return "cancel";
    }
  }
}

export function createSurfaceRegistry<TState extends SurfaceState = SurfaceState>(
  definitions: readonly SurfaceDefinition<TState>[] = [],
): WorkbenchSurfaceRegistry {
  const registry = new SurfaceRegistry();
  for (const definition of definitions) registry.register(definition);
  return registry;
}
