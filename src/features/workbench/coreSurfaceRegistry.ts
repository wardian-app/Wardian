import type {
  OpenSurfaceRequest,
  SurfaceDefinition,
  SurfaceRenderPolicy,
  SurfaceOpenPolicy,
  SurfaceRuntimePolicy,
  WorkbenchSurfaceV1,
} from "../../types";
import { createSurfaceRegistry, type WorkbenchSurfaceRegistry } from "./surfaceRegistry";

export type CoreSurfaceGroup = "Core views" | "Sessions" | "Reserved";

export type CoreSurfaceContribution = {
  surface_type: string;
  title: string;
  description: string;
  group: CoreSurfaceGroup;
  reserved?: boolean;
  requires_resource?: boolean;
};

export const CORE_SURFACE_CONTRIBUTIONS: readonly CoreSurfaceContribution[] = Object.freeze([
  { surface_type: "agents-overview", title: "Agents Overview", description: "Monitor active agents.", group: "Core views" },
  { surface_type: "dashboard", title: "Dashboard", description: "Review habitat telemetry.", group: "Core views" },
  { surface_type: "queue", title: "Queue", description: "Review signals and action-needed work.", group: "Core views" },
  { surface_type: "graph", title: "Graph", description: "Explore agent relationships.", group: "Core views" },
  { surface_type: "garden", title: "Garden", description: "Explore the living habitat.", group: "Core views" },
  { surface_type: "library", title: "Library", description: "Browse reusable assets.", group: "Core views" },
  { surface_type: "workflows", title: "Workflows", description: "Build and monitor workflows.", group: "Core views" },
  { surface_type: "agent-session", title: "Agent Session", description: "Open a specific agent session.", group: "Sessions", requires_resource: true },
  { surface_type: "file-editor", title: "File Editor", description: "Reserved for a future editor contribution.", group: "Reserved", reserved: true },
  { surface_type: "browser", title: "Browser", description: "Reserved for a future browser contribution.", group: "Reserved", reserved: true },
]);

type DefinitionOptions = {
  type: string;
  title: string;
  render_policy: SurfaceRenderPolicy;
  open_policy: SurfaceOpenPolicy;
  runtime_policy?: SurfaceRuntimePolicy;
  default_state?: () => unknown;
  resource_key?: (request: OpenSurfaceRequest) => string | undefined;
};

function surfaceDefinition(options: DefinitionOptions): SurfaceDefinition {
  return {
    type: options.type,
    title: (surface: WorkbenchSurfaceV1) => options.type === "agent-session" && surface.resource_key
      ? `${options.title}: ${surface.resource_key}`
      : options.title,
    icon: options.type,
    render_policy: options.render_policy,
    open_policy: options.open_policy,
    runtime_policy: options.runtime_policy ?? "view_only",
    close_policy: "close_view",
    state_schema_version: 1,
    max_state_bytes: 64 * 1024,
    ...(options.resource_key ? { resource_key: options.resource_key } : {}),
    default_state: options.default_state ?? (() => ({})),
    serialize_state: (state) => state,
    restore_state: (value, version) => version === 1
      ? { ok: true, state: value }
      : { ok: false, error: `unsupported ${options.type} state version ${version}` },
    commands: [],
  };
}

const CORE_SURFACE_DEFINITIONS: readonly SurfaceDefinition[] = [
  surfaceDefinition({
    type: "agents-overview",
    title: "Agents Overview",
    render_policy: "keep_alive",
    open_policy: "singleton",
    default_state: () => ({ focused_agent_id: null, presentation_mode: "auto" }),
  }),
  surfaceDefinition({ type: "dashboard", title: "Dashboard", render_policy: "recreate_from_state", open_policy: "singleton" }),
  surfaceDefinition({ type: "queue", title: "Queue", render_policy: "recreate_from_state", open_policy: "singleton" }),
  surfaceDefinition({ type: "graph", title: "Graph", render_policy: "suspend_when_hidden", open_policy: "singleton" }),
  surfaceDefinition({ type: "garden", title: "Garden", render_policy: "suspend_when_hidden", open_policy: "singleton" }),
  surfaceDefinition({ type: "library", title: "Library", render_policy: "keep_alive", open_policy: "singleton" }),
  surfaceDefinition({ type: "workflows", title: "Workflows", render_policy: "keep_alive", open_policy: "singleton" }),
  surfaceDefinition({
    type: "agent-session",
    title: "Agent Session",
    render_policy: "suspend_when_hidden",
    open_policy: "focus_resource",
    runtime_policy: "runtime_backed",
    resource_key: (request) => {
      const resourceKey = request.resource_key?.trim();
      if (!resourceKey) throw new Error("Agent Session requires a resource_key");
      return resourceKey;
    },
  }),
];

export function createCoreWorkbenchSurfaceRegistry(): WorkbenchSurfaceRegistry {
  return createSurfaceRegistry(CORE_SURFACE_DEFINITIONS);
}
