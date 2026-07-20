import type {
  SurfaceCommandDefinition,
  SurfaceBadge,
  SurfaceDefinition,
  SurfaceRestoreResult,
  WorkbenchSurfaceV1,
} from "../../../types";
import { useQueueStore } from "../../../store/useQueueStore";
import type { GraphRelationshipReason } from "../../graph/graphProjection";

export const DEFAULT_HEAVY_SURFACE_HIDDEN_GRACE_MS = 30_000;
export const MIN_HEAVY_SURFACE_HIDDEN_GRACE_MS = 1;
export const MAX_HEAVY_SURFACE_HIDDEN_GRACE_MS = 300_000;

/**
 * Resolves the build-time heavy-renderer grace override without weakening the
 * production default. Invalid, fractional, zero, negative, and excessive
 * values fail closed to 30 seconds.
 */
export function resolveHeavySurfaceHiddenGraceMs(
  value: unknown,
  benchmark_enabled = false,
): number {
  if (!benchmark_enabled || typeof value !== "string" || !/^[1-9]\d*$/.test(value)) {
    return DEFAULT_HEAVY_SURFACE_HIDDEN_GRACE_MS;
  }
  const parsed = Number(value);
  return Number.isSafeInteger(parsed)
    && parsed >= MIN_HEAVY_SURFACE_HIDDEN_GRACE_MS
    && parsed <= MAX_HEAVY_SURFACE_HIDDEN_GRACE_MS
    ? parsed
    : DEFAULT_HEAVY_SURFACE_HIDDEN_GRACE_MS;
}

export const HEAVY_SURFACE_HIDDEN_GRACE_MS = resolveHeavySurfaceHiddenGraceMs(
  import.meta.env.VITE_WARDIAN_HEAVY_SURFACE_GRACE_MS,
  import.meta.env.VITE_WARDIAN_WORKBENCH_PERF === "1",
);
export const CORE_VIEW_SURFACE_STATE_SCHEMA_VERSION = 1;
export const CORE_VIEW_SURFACE_MAX_STATE_BYTES = 4 * 1024;

export type CoreViewSurfaceType = "dashboard" | "queue" | "graph" | "garden";
export type EmptyCoreViewSurfaceState = Readonly<Record<string, never>>;
export type GraphSurfaceState = Readonly<{
  enabled_reasons: readonly GraphRelationshipReason[];
  inspected_agent_id: string | null;
  inspector_open: boolean;
  selected_edge_id: string | null;
  picker_search: string;
}>;
export type GardenSurfaceState = Readonly<{ selected_unit_key: string | null }>;
export type CoreViewSurfaceState = EmptyCoreViewSurfaceState | GraphSurfaceState | GardenSurfaceState;
export type SurfaceVisibility = "visible" | "hidden";

const EMPTY_STATE: EmptyCoreViewSurfaceState = Object.freeze({});
const GRAPH_REASONS = new Set<GraphRelationshipReason>([
  "same_team",
  "shared_workspace",
  "same_worktree",
]);
export const DEFAULT_GRAPH_SURFACE_STATE: GraphSurfaceState = Object.freeze({
  enabled_reasons: Object.freeze([]),
  inspected_agent_id: null,
  inspector_open: true,
  selected_edge_id: null,
  picker_search: "",
});
export const DEFAULT_GARDEN_SURFACE_STATE: GardenSurfaceState = Object.freeze({
  selected_unit_key: null,
});

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function restoreEmptyState(
  value: unknown,
  version: number,
  surfaceType: "dashboard" | "queue",
): SurfaceRestoreResult<EmptyCoreViewSurfaceState> {
  if (version !== CORE_VIEW_SURFACE_STATE_SCHEMA_VERSION) {
    return { ok: false, error: `unsupported ${surfaceType} state version ${version}` };
  }
  if (!isRecord(value) || Object.keys(value).length > 0) {
    return { ok: false, error: `${surfaceType} state must be an empty object` };
  }
  return { ok: true, state: EMPTY_STATE };
}

function restoreGraphState(value: unknown, version: number): SurfaceRestoreResult<GraphSurfaceState> {
  if (version !== CORE_VIEW_SURFACE_STATE_SCHEMA_VERSION) {
    return { ok: false, error: `unsupported graph state version ${version}` };
  }
  if (!isRecord(value)) return { ok: false, error: "graph state must be an object" };
  const enabledReasons = value.enabled_reasons;
  if (
    !Array.isArray(enabledReasons)
    || enabledReasons.some((reason) => typeof reason !== "string" || !GRAPH_REASONS.has(reason as GraphRelationshipReason))
    || (value.inspected_agent_id !== null && typeof value.inspected_agent_id !== "string")
    || typeof value.inspector_open !== "boolean"
    || (value.selected_edge_id !== null && typeof value.selected_edge_id !== "string")
    || typeof value.picker_search !== "string"
  ) return { ok: false, error: "graph state is malformed" };
  return {
    ok: true,
    state: {
      enabled_reasons: enabledReasons as GraphRelationshipReason[],
      inspected_agent_id: value.inspected_agent_id as string | null,
      inspector_open: value.inspector_open,
      selected_edge_id: value.selected_edge_id as string | null,
      picker_search: value.picker_search,
    },
  };
}

function restoreGardenState(value: unknown, version: number): SurfaceRestoreResult<GardenSurfaceState> {
  if (version !== CORE_VIEW_SURFACE_STATE_SCHEMA_VERSION) {
    return { ok: false, error: `unsupported garden state version ${version}` };
  }
  if (
    !isRecord(value)
    || (value.selected_unit_key !== null && typeof value.selected_unit_key !== "string")
  ) return { ok: false, error: "garden state is malformed" };
  return { ok: true, state: { selected_unit_key: value.selected_unit_key as string | null } };
}

function openCommand(surfaceType: CoreViewSurfaceType, title: string): SurfaceCommandDefinition {
  return {
    command_id: `workbench.open.${surfaceType}`,
    title: `Open ${title}`,
    accessibility_label: `Open ${title} surface`,
  };
}

function defineCoreViewSurface(
  type: CoreViewSurfaceType,
  title: string,
  renderPolicy: SurfaceDefinition["render_policy"],
  stateContract: Pick<SurfaceDefinition, "default_state" | "serialize_state" | "restore_state">,
): SurfaceDefinition {
  return {
    type,
    title: () => title,
    icon: type,
    render_policy: renderPolicy,
    open_policy: "singleton",
    runtime_policy: "view_only",
    close_policy: "close_view",
    state_schema_version: CORE_VIEW_SURFACE_STATE_SCHEMA_VERSION,
    max_state_bytes: CORE_VIEW_SURFACE_MAX_STATE_BYTES,
    ...stateContract,
    commands: [openCommand(type, title)],
    badges: () => [],
  };
}

export const DASHBOARD_SURFACE_DEFINITION = defineCoreViewSurface(
  "dashboard", "Dashboard", "recreate_from_state", {
    default_state: () => EMPTY_STATE,
    serialize_state: () => EMPTY_STATE,
    restore_state: (value, version) => restoreEmptyState(value, version, "dashboard"),
  },
);

function queueUnreadBadges(): SurfaceBadge[] {
  const unreadCount = useQueueStore.getState().items.filter((item) => !item.read).length;
  if (unreadCount === 0) return [];
  return [{
    badge_id: "unread",
    label: `${unreadCount} unread queue item${unreadCount === 1 ? "" : "s"}`,
    value: unreadCount > 9 ? "9+" : String(unreadCount),
  }];
}

export const QUEUE_SURFACE_DEFINITION: SurfaceDefinition = {
  ...defineCoreViewSurface(
    "queue", "Queue", "recreate_from_state", {
      default_state: () => EMPTY_STATE,
      serialize_state: () => EMPTY_STATE,
      restore_state: (value, version) => restoreEmptyState(value, version, "queue"),
    },
  ),
  presentation_subscribe: (listener: () => void) => useQueueStore.subscribe(listener),
  badges: () => queueUnreadBadges(),
};
export const GRAPH_SURFACE_DEFINITION = defineCoreViewSurface(
  "graph", "Graph", "suspend_when_hidden", {
    default_state: () => DEFAULT_GRAPH_SURFACE_STATE,
    serialize_state: (state) => state,
    restore_state: restoreGraphState,
  },
);
export const GARDEN_SURFACE_DEFINITION = defineCoreViewSurface(
  "garden", "Garden", "suspend_when_hidden", {
    default_state: () => DEFAULT_GARDEN_SURFACE_STATE,
    serialize_state: (state) => state,
    restore_state: restoreGardenState,
  },
);

export const CORE_VIEW_SURFACE_DEFINITIONS: readonly SurfaceDefinition[] = Object.freeze([
  DASHBOARD_SURFACE_DEFINITION,
  QUEUE_SURFACE_DEFINITION,
  GRAPH_SURFACE_DEFINITION,
  GARDEN_SURFACE_DEFINITION,
]);

export function normalizeCoreViewSurfaceState(
  surface: Pick<WorkbenchSurfaceV1, "surface_type" | "state" | "state_schema_version">,
): CoreViewSurfaceState {
  if (surface.surface_type === "graph") return normalizeGraphSurfaceState(surface);
  if (surface.surface_type === "garden") return normalizeGardenSurfaceState(surface);
  const definition = CORE_VIEW_SURFACE_DEFINITIONS.find(
    (candidate) => candidate.type === surface.surface_type,
  );
  if (!definition) return EMPTY_STATE;
  const restored = definition.restore_state(surface.state, surface.state_schema_version);
  return restored.ok ? restored.state as CoreViewSurfaceState : EMPTY_STATE;
}

export function normalizeGraphSurfaceState(
  surface: Pick<WorkbenchSurfaceV1, "state" | "state_schema_version">,
): GraphSurfaceState {
  const restored = GRAPH_SURFACE_DEFINITION.restore_state(surface.state, surface.state_schema_version);
  return restored.ok ? restored.state as GraphSurfaceState : DEFAULT_GRAPH_SURFACE_STATE;
}

export function normalizeGardenSurfaceState(
  surface: Pick<WorkbenchSurfaceV1, "state" | "state_schema_version">,
): GardenSurfaceState {
  const restored = GARDEN_SURFACE_DEFINITION.restore_state(surface.state, surface.state_schema_version);
  return restored.ok ? restored.state as GardenSurfaceState : DEFAULT_GARDEN_SURFACE_STATE;
}
