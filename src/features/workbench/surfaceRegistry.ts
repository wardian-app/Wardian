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
  default_state(type: string): SurfaceState;
  resource_key(request: OpenSurfaceRequest): string | undefined;
  serialize_state<TState extends SurfaceState>(
    type: string,
    state: TState,
  ): SerializedSurfaceState;
  resolve_surface(surface: WorkbenchSurfaceV1): ResolvedSurface;
  resolve_existing(
    request: OpenSurfaceRequest,
    candidates: readonly WorkbenchSurfaceV1[],
  ): string | undefined;
  presentation(surface: WorkbenchSurfaceV1): SurfacePresentationMetadata;
  sync_presentations(surfaces: readonly WorkbenchSurfaceV1[]): void;
  subscribe_presentation(listener: () => void): () => void;
  presentation_version(): number;
  can_close(surface: WorkbenchSurfaceV1): Promise<CloseDecision>;
}

function deepFreeze<T>(value: T, seen = new WeakSet<object>()): T {
  if (typeof value !== "object" || value === null || seen.has(value)) return value;
  seen.add(value);
  for (const key of Reflect.ownKeys(value)) {
    const descriptor = Object.getOwnPropertyDescriptor(value, key);
    if (descriptor && "value" in descriptor) deepFreeze(descriptor.value, seen);
  }
  return Object.freeze(value);
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function invalidRestore(error: string): SurfaceRestoreResult {
  return deepFreeze({ ok: false, error });
}

function copyDefinition<TState extends SurfaceState>(
  definition: SurfaceDefinition<TState>,
): SurfaceDefinition {
  const commands = definition.commands.map((command) => deepFreeze({ ...command }));
  return deepFreeze({
    ...definition,
    ...(definition.transient_state
      ? { transient_state: { ...definition.transient_state } }
      : {}),
    commands,
  }) as SurfaceDefinition;
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
  if (typeof definition.icon !== "string") throw new Error("icon must be a string");
  if (
    definition.presentation_icon !== undefined
    && typeof definition.presentation_icon !== "function"
  ) throw new Error("presentation_icon must be a function");
  if (
    definition.presentation_subscribe !== undefined
    && typeof definition.presentation_subscribe !== "function"
  ) throw new Error("presentation_subscribe must be a function");
  if (
    definition.presentation_sync !== undefined
    && typeof definition.presentation_sync !== "function"
  ) throw new Error("presentation_sync must be a function");
  if (typeof definition.title !== "function") throw new Error("title must be a function");
  if (!Array.isArray(definition.commands) || definition.commands.some(
    (command) => typeof command.command_id !== "string" || typeof command.title !== "string",
  )) throw new Error("commands must contain string command_id and title fields");
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
  if (!["keep_alive", "suspend_when_hidden", "recreate_from_state"].includes(
    definition.render_policy,
  )) throw new Error("render_policy is invalid");
  if (!["singleton", "focus_resource", "allow_multiple"].includes(
    definition.open_policy,
  )) throw new Error("open_policy is invalid");
  if (!["view_only", "runtime_backed"].includes(definition.runtime_policy)) {
    throw new Error("runtime_policy is invalid");
  }
  if (!["close_view", "confirm_if_dirty"].includes(definition.close_policy)) {
    throw new Error("close_policy is invalid");
  }
  if (
    definition.transient_state !== undefined
    && (
      typeof definition.transient_state.is_transient !== "function"
      || typeof definition.transient_state.pin !== "function"
    )
  ) throw new Error("transient_state must define is_transient and pin functions");
}

function canonicalRequest(
  request: OpenSurfaceRequest,
  maxStateBytes = MAX_WORKBENCH_SURFACE_STATE_BYTES,
): OpenSurfaceRequest {
  return deepFreeze({
    surface_type: request.surface_type,
    ...(request.resource_key === undefined ? {} : { resource_key: request.resource_key }),
    ...(request.state === undefined
      ? {}
      : { state: canonicalizeState(request.state, maxStateBytes) }),
    ...(request.group_id === undefined ? {} : { group_id: request.group_id }),
    ...(request.duplicate === undefined ? {} : { duplicate: request.duplicate }),
  });
}

function canonicalSurface(
  surface: WorkbenchSurfaceV1,
  maxStateBytes: number,
): WorkbenchSurfaceV1 {
  return deepFreeze({
    surface_id: surface.surface_id,
    surface_type: surface.surface_type,
    ...(surface.resource_key === undefined ? {} : { resource_key: surface.resource_key }),
    ...(surface.presentation_provenance === undefined
      ? {}
      : { presentation_provenance: { ...surface.presentation_provenance } }),
    state_schema_version: surface.state_schema_version,
    state: canonicalizeState(surface.state, maxStateBytes),
  });
}

function missingDefinition(surface: WorkbenchSurfaceV1): SurfaceDefinition {
  const snapshot = canonicalSurface(surface, MAX_WORKBENCH_SURFACE_STATE_BYTES);
  return deepFreeze({
    type: MISSING_SURFACE_TYPE,
    title: () => `Missing surface: ${snapshot.surface_type}`,
    icon: "missing-surface",
    render_policy: "recreate_from_state",
    open_policy: "allow_multiple",
    runtime_policy: "view_only",
    close_policy: "close_view",
    state_schema_version: snapshot.state_schema_version,
    max_state_bytes: MAX_WORKBENCH_SURFACE_STATE_BYTES,
    default_state: () => snapshot.state,
    serialize_state: (state) => deepFreeze(canonicalizeState(
      state,
      MAX_WORKBENCH_SURFACE_STATE_BYTES,
    )),
    restore_state: (value) => {
      try {
        return deepFreeze({
          ok: true as const,
          state: deepFreeze(canonicalizeState(value, MAX_WORKBENCH_SURFACE_STATE_BYTES)),
        });
      } catch (error) {
        return invalidRestore(error instanceof Error ? error.message : String(error));
      }
    },
    commands: [
      { command_id: "workbench.close_surface", title: "Close Surface" },
      { command_id: "workbench.reset_surface", title: "Reset Surface" },
    ],
    badges: () => [{ badge_id: "missing", label: `Missing ${snapshot.surface_type}` }],
  });
}

class SurfaceRegistry implements WorkbenchSurfaceRegistry {
  private readonly rawDefinitionsByType = new Map<string, SurfaceDefinition>();
  private readonly publicDefinitionsByType = new Map<string, SurfaceDefinition>();
  private readonly registrationOrder: SurfaceDefinition[] = [];
  private readonly presentationListeners = new Set<() => void>();
  private readonly presentationSources = new Set<(listener: () => void) => () => void>();
  private readonly presentationSourceUnsubscribers = new Map<
    (listener: () => void) => () => void,
    () => void
  >();
  private presentationVersion = 0;

  private readonly invalidatePresentation = () => {
    this.presentationVersion += 1;
    for (const listener of this.presentationListeners) listener();
  };

  register<TState extends SurfaceState>(definition: SurfaceDefinition<TState>): void {
    validateDefinition(definition as SurfaceDefinition);
    if (this.rawDefinitionsByType.has(definition.type)) {
      throw new Error(`surface type ${definition.type} is already registered`);
    }
    const rawDefinition = copyDefinition(definition);
    this.rawDefinitionsByType.set(rawDefinition.type, rawDefinition);
    const publicDefinition = this.safeDefinition(rawDefinition);
    this.publicDefinitionsByType.set(rawDefinition.type, publicDefinition);
    this.registrationOrder.push(publicDefinition);
    if (rawDefinition.presentation_subscribe) {
      this.presentationSources.add(rawDefinition.presentation_subscribe);
      if (this.presentationListeners.size > 0) {
        this.attachPresentationSource(rawDefinition.presentation_subscribe);
      }
    }
  }

  list(): readonly SurfaceDefinition[] {
    return Object.freeze([...this.registrationOrder]);
  }

  get(type: string): SurfaceDefinition | undefined {
    return this.publicDefinitionsByType.get(type);
  }

  require(type: string): SurfaceDefinition {
    const definition = this.get(type);
    if (!definition) throw new Error(`surface type ${type} is not registered`);
    return definition;
  }

  default_state(type: string): SurfaceState {
    const definition = this.requireRaw(type);
    const state = deepFreeze(canonicalizeState(
      definition.default_state(),
      MAX_WORKBENCH_SURFACE_STATE_BYTES,
    ));
    canonicalizeState(definition.serialize_state(state), definition.max_state_bytes);
    return state;
  }

  resource_key(request: OpenSurfaceRequest): string | undefined {
    const definition = this.requireRaw(request.surface_type);
    const snapshot = canonicalRequest(request, definition.max_state_bytes);
    const resourceKey = definition.resource_key?.(snapshot) ?? snapshot.resource_key;
    if (resourceKey !== undefined && typeof resourceKey !== "string") {
      throw new Error("resource_key callback must return a string or undefined");
    }
    return resourceKey;
  }

  serialize_state<TState extends SurfaceState>(
    type: string,
    state: TState,
  ): SerializedSurfaceState {
    const definition = this.requireRaw(type);
    const callbackState = deepFreeze(canonicalizeState(
      state,
      MAX_WORKBENCH_SURFACE_STATE_BYTES,
    ));
    const serialized = definition.serialize_state(callbackState);
    return deepFreeze({
      state_schema_version: definition.state_schema_version,
      state: canonicalizeState(serialized, definition.max_state_bytes),
    });
  }

  resolve_surface(surface: WorkbenchSurfaceV1): ResolvedSurface {
    const rawDefinition = this.rawDefinitionsByType.get(surface.surface_type);
    if (!rawDefinition) {
      try {
        const snapshot = canonicalSurface(surface, MAX_WORKBENCH_SURFACE_STATE_BYTES);
        return {
          definition: missingDefinition(snapshot),
          missing_surface_type: surface.surface_type,
          restore_result: deepFreeze({ ok: true, state: snapshot.state }),
        };
      } catch (error) {
        const sanitized = { ...surface, state: null };
        return {
          definition: missingDefinition(sanitized),
          missing_surface_type: surface.surface_type,
          restore_result: invalidRestore(
            `restore failed: ${error instanceof Error ? error.message : String(error)}`,
          ),
        };
      }
    }
    try {
      const snapshot = canonicalSurface(surface, rawDefinition.max_state_bytes);
      return {
        definition: this.require(surface.surface_type),
        restore_result: this.restoreKnownSurface(rawDefinition, snapshot),
      };
    } catch (error) {
      return {
        definition: this.require(surface.surface_type),
        restore_result: invalidRestore(
          `restore failed: ${error instanceof Error ? error.message : String(error)}`,
        ),
      };
    }
  }

  resolve_existing(
    request: OpenSurfaceRequest,
    candidates: readonly WorkbenchSurfaceV1[],
  ): string | undefined {
    const definition = this.requireRaw(request.surface_type);
    const requestSnapshot = canonicalRequest(request, definition.max_state_bytes);
    if (requestSnapshot.duplicate === true) return undefined;
    const typedCandidates = Object.freeze(candidates
      .filter((candidate) => candidate.surface_type === definition.type)
      .map((candidate) => canonicalSurface(candidate, definition.max_state_bytes))
      .filter((candidate) => this.restoreKnownSurface(definition, candidate).ok));
    if (typedCandidates.length === 0 || definition.open_policy === "allow_multiple") {
      return undefined;
    }

    if (definition.resolve_existing) {
      const resolvedId = definition.resolve_existing(requestSnapshot, typedCandidates);
      if (resolvedId !== undefined && typeof resolvedId !== "string") {
        throw new Error("resolve_existing must return a surface ID or undefined");
      }
      if (resolvedId && typedCandidates.some((candidate) => candidate.surface_id === resolvedId)) {
        return resolvedId;
      }
    }
    if (definition.open_policy === "singleton") return typedCandidates[0]?.surface_id;

    const resourceKey = definition.resource_key?.(requestSnapshot) ?? requestSnapshot.resource_key;
    if (resourceKey === undefined) return undefined;
    if (typeof resourceKey !== "string") {
      throw new Error("resource_key callback must return a string or undefined");
    }
    return typedCandidates.find((candidate) => candidate.resource_key === resourceKey)?.surface_id;
  }

  presentation(surface: WorkbenchSurfaceV1): SurfacePresentationMetadata {
    const rawDefinition = this.rawDefinitionsByType.get(surface.surface_type);
    if (!rawDefinition) {
      const snapshot = canonicalSurface(surface, MAX_WORKBENCH_SURFACE_STATE_BYTES);
      const definition = missingDefinition(snapshot);
      return deepFreeze({
        title: definition.title(snapshot),
        icon: definition.icon,
        commands: definition.commands.map((command) => ({ ...command })),
        badges: definition.badges?.(snapshot) ?? [],
      });
    }
    const snapshot = canonicalSurface(surface, rawDefinition.max_state_bytes);
    const restored = this.restoreKnownSurface(rawDefinition, snapshot);
    if (!restored.ok) {
      return deepFreeze({
        title: rawDefinition.type,
        icon: rawDefinition.icon,
        commands: rawDefinition.commands.map((command) => ({ ...command })),
        badges: [{ badge_id: "recovery", label: "Recovery needed" }],
      });
    }
    const presentationSurface = deepFreeze({
      ...snapshot,
      state_schema_version: rawDefinition.state_schema_version,
      state: restored.state,
    });
    const title = rawDefinition.title(presentationSurface);
    if (typeof title !== "string") throw new Error("title callback must return a string");
    const icon = rawDefinition.presentation_icon?.(presentationSurface) ?? rawDefinition.icon;
    if (typeof icon !== "string") {
      throw new Error("presentation_icon callback must return a string");
    }
    const badges = canonicalizeState(
      rawDefinition.badges?.(presentationSurface) ?? [],
      MAX_WORKBENCH_SURFACE_STATE_BYTES,
    );
    if (
      !Array.isArray(badges)
      || badges.some((badge) => !isRecord(badge)
        || typeof badge.badge_id !== "string"
        || typeof badge.label !== "string")
    ) throw new Error("badges callback must return badge_id/label string records");
    return deepFreeze({
      title,
      icon,
      commands: rawDefinition.commands.map((command) => ({ ...command })),
      badges,
    }) as SurfacePresentationMetadata;
  }

  sync_presentations(surfaces: readonly WorkbenchSurfaceV1[]): void {
    for (const definition of this.rawDefinitionsByType.values()) {
      if (!definition.presentation_sync) continue;
      const snapshots = surfaces
        .filter((surface) => surface.surface_type === definition.type)
        .map((surface) => canonicalSurface(surface, definition.max_state_bytes))
        .filter((surface) => this.restoreKnownSurface(definition, surface).ok);
      definition.presentation_sync(deepFreeze(snapshots));
    }
  }

  subscribe_presentation(listener: () => void): () => void {
    this.presentationListeners.add(listener);
    if (this.presentationListeners.size === 1) {
      for (const source of this.presentationSources) this.attachPresentationSource(source);
    }
    let subscribed = true;
    return () => {
      if (!subscribed) return;
      subscribed = false;
      this.presentationListeners.delete(listener);
      if (this.presentationListeners.size === 0) {
        for (const unsubscribe of this.presentationSourceUnsubscribers.values()) unsubscribe();
        this.presentationSourceUnsubscribers.clear();
      }
    };
  }

  presentation_version(): number {
    return this.presentationVersion;
  }

  async can_close(surface: WorkbenchSurfaceV1): Promise<CloseDecision> {
    const definition = this.rawDefinitionsByType.get(surface.surface_type);
    if (!definition) return "allow";
    if (definition.close_policy === "close_view") return "allow";
    if (!definition.can_close) return "cancel";
    try {
      const snapshot = canonicalSurface(surface, definition.max_state_bytes);
      return await definition.can_close(snapshot) === "allow" ? "allow" : "cancel";
    } catch {
      return "cancel";
    }
  }

  private requireRaw(type: string): SurfaceDefinition {
    const definition = this.rawDefinitionsByType.get(type);
    if (!definition) throw new Error(`surface type ${type} is not registered`);
    return definition;
  }

  private attachPresentationSource(source: (listener: () => void) => () => void): void {
    if (this.presentationSourceUnsubscribers.has(source)) return;
    const unsubscribe = source(this.invalidatePresentation);
    this.presentationSourceUnsubscribers.set(source, unsubscribe);
  }

  private restoreKnown(
    definition: SurfaceDefinition,
    persistedState: unknown,
    version: number,
  ): SurfaceRestoreResult {
    try {
      const callbackState = deepFreeze(canonicalizeState(
        persistedState,
        definition.max_state_bytes,
      ));
      const restoreResult: unknown = definition.restore_state(callbackState, version);
      if (!isRecord(restoreResult) || typeof restoreResult.ok !== "boolean") {
        return invalidRestore("restore_state returned an invalid result");
      }
      if (restoreResult.ok === true) {
        if (!Object.prototype.hasOwnProperty.call(restoreResult, "state")) {
          return invalidRestore("restore_state success omitted state");
        }
        const state = deepFreeze(canonicalizeState(
          restoreResult.state,
          MAX_WORKBENCH_SURFACE_STATE_BYTES,
        ));
        this.serialize_state(definition.type, state);
        return deepFreeze({ ok: true, state });
      }
      return typeof restoreResult.error === "string"
        ? invalidRestore(restoreResult.error)
        : invalidRestore("restore_state failure omitted a string error");
    } catch (error) {
      return invalidRestore(
        `restore failed: ${error instanceof Error ? error.message : String(error)}`,
      );
    }
  }

  private restoreKnownSurface(
    definition: SurfaceDefinition,
    surface: WorkbenchSurfaceV1,
  ): SurfaceRestoreResult {
    const restored = this.restoreKnown(
      definition,
      surface.state,
      surface.state_schema_version,
    );
    if (!restored.ok || !definition.resource_key) return restored;

    try {
      const request = canonicalRequest({
        surface_type: definition.type,
        ...(surface.resource_key === undefined ? {} : { resource_key: surface.resource_key }),
        state: restored.state,
      }, definition.max_state_bytes);
      const resourceKey = definition.resource_key(request) ?? request.resource_key;
      if (resourceKey !== undefined && typeof resourceKey !== "string") {
        return invalidRestore("resource_key callback returned an invalid result");
      }
      if (resourceKey !== surface.resource_key) {
        return invalidRestore("restored surface resource identity does not match resource_key");
      }
      return restored;
    } catch (error) {
      return invalidRestore(
        `surface resource validation failed: ${error instanceof Error ? error.message : String(error)}`,
      );
    }
  }

  private validatedTransientState(
    definition: SurfaceDefinition,
    state: SurfaceState,
  ): SurfaceState {
    const serialized = this.serialize_state(definition.type, state);
    const restored = this.restoreKnown(
      definition,
      serialized.state,
      serialized.state_schema_version,
    );
    if (!restored.ok) throw new Error(restored.error);
    return restored.state;
  }

  private safeDefinition(definition: SurfaceDefinition): SurfaceDefinition {
    const type = definition.type;
    const { presentation_sync: _presentationSync, ...publicDefinition } = definition;
    return deepFreeze({
      ...publicDefinition,
      title: (surface: WorkbenchSurfaceV1) => this.presentation(surface).title,
      presentation_icon: definition.presentation_icon
        ? (surface: WorkbenchSurfaceV1) => this.presentation(surface).icon
        : undefined,
      default_state: () => this.default_state(type),
      serialize_state: (state: SurfaceState) => this.serialize_state(type, state).state,
      restore_state: (value: unknown, version: number) =>
        this.restoreKnown(definition, value, version),
      transient_state: definition.transient_state
        ? {
            is_transient: (state: SurfaceState) => {
              const callbackState = this.validatedTransientState(definition, state);
              const result = definition.transient_state!.is_transient(callbackState);
              if (typeof result !== "boolean") {
                throw new Error("is_transient callback must return a boolean");
              }
              return result;
            },
            pin: (state: SurfaceState) => {
              const callbackState = this.validatedTransientState(definition, state);
              const pinned = definition.transient_state!.pin(callbackState);
              return this.validatedTransientState(definition, pinned);
            },
          }
        : undefined,
      resource_key: definition.resource_key
        ? (request: OpenSurfaceRequest) => this.resource_key({ ...request, surface_type: type })
        : undefined,
      resolve_existing: definition.resolve_existing
        ? (request: OpenSurfaceRequest, candidates: readonly WorkbenchSurfaceV1[]) =>
            this.resolve_existing({ ...request, surface_type: type }, candidates)
        : undefined,
      can_close: (surface: WorkbenchSurfaceV1) => this.can_close(surface),
      badges: definition.badges
        ? (surface: WorkbenchSurfaceV1) => this.presentation(surface).badges
        : undefined,
      commands: definition.commands.map((command) => ({ ...command })),
    }) as SurfaceDefinition;
  }
}

export function createSurfaceRegistry<TState extends SurfaceState = SurfaceState>(
  definitions: readonly SurfaceDefinition<TState>[] = [],
): WorkbenchSurfaceRegistry {
  const registry = new SurfaceRegistry();
  for (const definition of definitions) registry.register(definition);
  return registry;
}
