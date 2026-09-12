import type {
  SurfaceCommandDefinition,
  SurfaceBadge,
  SurfaceDefinition,
  SurfaceRestoreResult,
  WorkbenchSurfaceV1,
} from "../../../types";
import { useQueueStore } from "../../../store/useQueueStore";
import type { GardenCamera, GardenNavigationFrame, GardenTimeLens } from "../../garden/gardenNavigation";
import type { GraphRelationshipReason } from "../../graph/graphProjection";
import {
  DEFAULT_DASHBOARD_PREFS,
  mergeDashboardPrefs,
  type DashboardPrefs,
} from "../../telemetry/dashboardColumns";

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

export type CoreViewSurfaceType = "dashboard" | "analytics" | "inbox" | "graph" | "garden";
export type EmptyCoreViewSurfaceState = Readonly<Record<string, never>>;
export type GraphSurfaceState = Readonly<{
  enabled_reasons: readonly GraphRelationshipReason[];
  inspected_agent_id: string | null;
  inspector_open: boolean;
  selected_edge_id: string | null;
  picker_search: string;
  show_all_labels: boolean;
}>;
export type GardenSurfaceState = Readonly<{
  selected_unit_key: string | null;
  trail?: GardenNavigationFrame[];
  camera?: GardenCamera;
  time_lens?: GardenTimeLens;
}>;
/**
 * What the Analytics grid is currently asking.
 *
 * Persisted so the surface survives a reload and so the Dashboard can open it
 * already scoped to one agent, rather than dropping the reader on a default
 * view they have to re-navigate.
 */
export type AnalyticsSurfaceState = Readonly<{
  horizon: string;
  dimension: string;
  measure: string;
  /** Session id to focus, when opened by drilling through from a row. */
  focus_key: string | null;
}>;
/**
 * How one Dashboard instance is configured.
 *
 * Per instance, so two Dashboards can watch different things — one on token
 * volume, one on turn rate — which is the point of the workbench holding
 * several. A new
 * instance is seeded from the globally saved preferences rather than a cold
 * default, so this is what the operator last used, not what shipped.
 */
export type DashboardSurfaceState = Readonly<{
  prefs: DashboardPrefs;
}>;
export type CoreViewSurfaceState =
  | EmptyCoreViewSurfaceState
  | GraphSurfaceState
  | GardenSurfaceState
  | AnalyticsSurfaceState
  | DashboardSurfaceState;
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
  show_all_labels: true,
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
  surfaceType: "dashboard" | "inbox",
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
    || (value.show_all_labels !== undefined && typeof value.show_all_labels !== "boolean")
  ) return { ok: false, error: "graph state is malformed" };
  return {
    ok: true,
    state: {
      enabled_reasons: enabledReasons as GraphRelationshipReason[],
      inspected_agent_id: value.inspected_agent_id as string | null,
      inspector_open: value.inspector_open,
      selected_edge_id: value.selected_edge_id as string | null,
      picker_search: value.picker_search,
      // Older graph surfaces did not persist label visibility; retain their
      // existing all-label behavior when restoring them.
      show_all_labels: value.show_all_labels !== false,
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
  const cameraValid = (camera: unknown): camera is GardenCamera => {
    if (!isRecord(camera) || typeof camera.scale !== "number" || !Number.isFinite(camera.scale) || camera.scale <= 0 || !isRecord(camera.position)) return false;
    return typeof camera.position.x === "number" && Number.isFinite(camera.position.x)
      && typeof camera.position.y === "number" && Number.isFinite(camera.position.y);
  };
  const kinds = new Set(["district", "workspace", "agent", "automation", "identity", "skill", "memory", "path", "stage"]);
  const boundsValid = (bounds: unknown) => isRecord(bounds)
    && [bounds.x, bounds.y, bounds.width, bounds.height].every((coordinate) => typeof coordinate === "number" && Number.isFinite(coordinate))
    && Number(bounds.width) > 0 && Number(bounds.height) > 0;
  const trail = Array.isArray(value.trail) ? value.trail.filter((frame): frame is GardenNavigationFrame =>
    isRecord(frame) && isRecord(frame.ref) && kinds.has(String(frame.ref.kind))
    && typeof frame.ref.id === "string" && typeof frame.label === "string"
    && (frame.camera === undefined || cameraValid(frame.camera))
    && (frame.bounds === undefined || boundsValid(frame.bounds)),
  ) : undefined;
  return { ok: true, state: {
    selected_unit_key: value.selected_unit_key as string | null,
    ...(trail ? { trail } : {}),
    ...(cameraValid(value.camera) ? { camera: value.camera } : {}),
    ...(["now", "recent", "branch"].includes(String(value.time_lens)) ? { time_lens: value.time_lens as GardenTimeLens } : {}),
  } };
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

export const DEFAULT_DASHBOARD_SURFACE_STATE: DashboardSurfaceState = Object.freeze({
  prefs: DEFAULT_DASHBOARD_PREFS,
});

function restoreDashboardState(
  value: unknown,
  version: number,
): SurfaceRestoreResult<DashboardSurfaceState> {
  if (version !== CORE_VIEW_SURFACE_STATE_SCHEMA_VERSION) {
    return { ok: false, error: `unsupported dashboard state version ${version}` };
  }
  if (!isRecord(value)) return { ok: false, error: "dashboard state must be an object" };
  return { ok: true, state: coerceDashboardState(value) };
}

/**
 * Best-effort shaping, used where a restore failure should not blank the table.
 *
 * Column preferences go through the same default-anchored merge the watchlist
 * uses, so a column added after this state was written still appears, and a
 * corrupt blob degrades to the defaults rather than rendering undefined.
 */
function coerceDashboardState(raw: unknown): DashboardSurfaceState {
  const record = (typeof raw === "object" && raw !== null ? raw : {}) as Record<string, unknown>;
  return Object.freeze({ prefs: mergeDashboardPrefs(record.prefs) });
}

/** One Dashboard's configuration, from a persisted surface. */
export function normalizeDashboardSurfaceState(
  surface: Pick<WorkbenchSurfaceV1, "state">,
): DashboardSurfaceState {
  return coerceDashboardState(surface.state);
}

/**
 * Whether this surface carries a configuration of its own.
 *
 * Restore is deliberately tolerant, so it answers with the defaults for a
 * surface that has never been configured — which is indistinguishable from one
 * genuinely configured to the defaults. This asks the narrower question the
 * seeding rule needs: did anyone actually write preferences here? Without it a
 * fresh Dashboard would render shipped defaults instead of what was last used.
 */
export function surfaceConfiguredDashboard(
  surface: Pick<WorkbenchSurfaceV1, "state">,
): boolean {
  return isRecord(surface.state) && isRecord(surface.state.prefs);
}

export const DASHBOARD_SURFACE_DEFINITION = defineCoreViewSurface(
  "dashboard", "Dashboard", "recreate_from_state", {
    // Carries its column and window configuration, so two Dashboards can watch
    // different things and each survives a reload as the operator left it.
    default_state: () => DEFAULT_DASHBOARD_SURFACE_STATE,
    serialize_state: (state) => state,
    restore_state: restoreDashboardState,
  },
);

function queueUnreadBadges(): SurfaceBadge[] {
  const unreadCount = useQueueStore.getState().items.filter((item) => !item.read).length;
  if (unreadCount === 0) return [];
  return [{
    badge_id: "unread",
    label: `${unreadCount} unread Inbox item${unreadCount === 1 ? "" : "s"}`,
    value: unreadCount > 9 ? "9+" : String(unreadCount),
  }];
}

export const INBOX_SURFACE_DEFINITION: SurfaceDefinition = {
  ...defineCoreViewSurface(
    "inbox", "Inbox", "recreate_from_state", {
      default_state: () => EMPTY_STATE,
      serialize_state: () => EMPTY_STATE,
      restore_state: (value, version) => restoreEmptyState(value, version, "inbox"),
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
export const DEFAULT_ANALYTICS_SURFACE_STATE: AnalyticsSurfaceState = Object.freeze({
  horizon: "week",
  dimension: "agent",
  measure: "active_ms",
  focus_key: null,
});

function restoreAnalyticsState(
  value: unknown,
  version: number,
): SurfaceRestoreResult<AnalyticsSurfaceState> {
  if (version !== CORE_VIEW_SURFACE_STATE_SCHEMA_VERSION) {
    return { ok: false, error: `unsupported analytics state version ${version}` };
  }
  if (!isRecord(value)) return { ok: false, error: "analytics state must be an object" };
  return { ok: true, state: coerceAnalyticsState(value) };
}

/** Best-effort shaping, used where a restore failure should not blank the grid. */
function coerceAnalyticsState(raw: unknown): AnalyticsSurfaceState {
  // Every field is validated by the command layer before it reaches SQL, so a
  // restored value that is stale or nonsense degrades to an error message
  // rather than a bad query. Shape is still checked here so a corrupt blob
  // cannot make the surface render undefined.
  const record = (typeof raw === "object" && raw !== null ? raw : {}) as Record<string, unknown>;
  const text = (value: unknown, fallback: string) =>
    typeof value === "string" && value.length > 0 ? value : fallback;
  return Object.freeze({
    horizon: text(record.horizon, DEFAULT_ANALYTICS_SURFACE_STATE.horizon),
    dimension: text(record.dimension, DEFAULT_ANALYTICS_SURFACE_STATE.dimension),
    measure: text(record.measure, DEFAULT_ANALYTICS_SURFACE_STATE.measure),
    focus_key: typeof record.focus_key === "string" ? record.focus_key : null,
  });
}

/** The Analytics grid's opening question, from a persisted surface. */
export function normalizeAnalyticsSurfaceState(
  surface: Pick<WorkbenchSurfaceV1, "state">,
): AnalyticsSurfaceState {
  return coerceAnalyticsState(surface.state);
}

export const ANALYTICS_SURFACE_DEFINITION = defineCoreViewSurface(
  "analytics", "Analytics", "suspend_when_hidden", {
    default_state: () => DEFAULT_ANALYTICS_SURFACE_STATE,
    serialize_state: (state) => state,
    restore_state: restoreAnalyticsState,
  },
);
export const CORE_VIEW_SURFACE_DEFINITIONS: readonly SurfaceDefinition[] = Object.freeze([
  DASHBOARD_SURFACE_DEFINITION,
  ANALYTICS_SURFACE_DEFINITION,
  INBOX_SURFACE_DEFINITION,
  GRAPH_SURFACE_DEFINITION,
  GARDEN_SURFACE_DEFINITION,
]);

export function normalizeCoreViewSurfaceState(
  surface: Pick<WorkbenchSurfaceV1, "surface_type" | "state" | "state_schema_version">,
): CoreViewSurfaceState {
  if (surface.surface_type === "graph") return normalizeGraphSurfaceState(surface);
  if (surface.surface_type === "garden") return normalizeGardenSurfaceState(surface);
  if (surface.surface_type === "analytics") return coerceAnalyticsState(surface.state);
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
