import type {
  AgentsOverviewSurfaceState,
  OpenSurfaceRequest,
  SurfaceDefinition,
  SurfaceClosePolicy,
  SurfaceRenderPolicy,
  SurfaceOpenPolicy,
  SurfaceRuntimePolicy,
  WorkbenchSurfaceV1,
} from "../../types";
import { useBuilderStore } from "../../store/useBuilderStore";
import { useLibraryStore } from "../../store/useLibraryStore";
import { createSurfaceRegistry, type WorkbenchSurfaceRegistry } from "./surfaceRegistry";
import {
  CORE_VIEW_SURFACE_DEFINITIONS,
  CORE_VIEW_SURFACE_MAX_STATE_BYTES,
} from "./surfaces/coreSurfaceMetadata";
import {
  createLibrarySurfaceCloseGuard,
  createWorkflowsSurfaceCloseGuard,
  type DirtySurfacePrompt,
} from "./surfaces/dirtySurfaceGuards";

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
  { surface_type: "agents-overview", title: "Agents", description: "Monitor active agents.", group: "Core views" },
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
  close_policy?: SurfaceClosePolicy;
  max_state_bytes?: number;
  default_state?: () => unknown;
  resource_key?: (request: OpenSurfaceRequest) => string | undefined;
  can_close?: SurfaceDefinition["can_close"];
  commands?: SurfaceDefinition["commands"];
  badges?: SurfaceDefinition["badges"];
  serialize_state?: SurfaceDefinition["serialize_state"];
  restore_state?: SurfaceDefinition["restore_state"];
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
    close_policy: options.close_policy ?? "close_view",
    state_schema_version: 1,
    max_state_bytes: options.max_state_bytes ?? 64 * 1024,
    ...(options.resource_key ? { resource_key: options.resource_key } : {}),
    default_state: options.default_state ?? (() => ({})),
    serialize_state: options.serialize_state ?? ((state) => state),
    restore_state: options.restore_state ?? ((value, version) => version === 1
      ? { ok: true, state: value }
      : { ok: false, error: `unsupported ${options.type} state version ${version}` }),
    ...(options.can_close ? { can_close: options.can_close } : {}),
    commands: options.commands ?? [],
    ...(options.badges ? { badges: options.badges } : {}),
  };
}

function dirtySurfaceCommand(type: "library" | "workflows", title: string) {
  return {
    command_id: `workbench.open.${type}`,
    title: `Open ${title}`,
    accessibility_label: `Open ${title} surface`,
  } as const;
}

export type CoreWorkbenchSurfaceRegistryOptions = {
  dirty_surface_prompt?: DirtySurfacePrompt;
};

const failClosedDirtyPrompt: DirtySurfacePrompt = () => "cancel";

const DEFAULT_AGENTS_OVERVIEW_STATE: AgentsOverviewSurfaceState = {
  mode: "auto",
  last_multi_agent_mode: "auto",
  focused_agent_id: null,
  search_query: "",
  status_filter: [],
};

function restoreAgentsOverviewState(value: unknown, version: number) {
  if (version !== 1) {
    return { ok: false as const, error: `unsupported agents-overview state version ${version}` };
  }
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    return { ok: false as const, error: "agents-overview state must be an object" };
  }
  const state = value as Record<string, unknown>;
  const lastMultiAgentMode = state.last_multi_agent_mode;
  if (
    !["auto", "grid", "single"].includes(state.mode as string)
    || (lastMultiAgentMode !== undefined && !["auto", "grid"].includes(lastMultiAgentMode as string))
    || (state.focused_agent_id !== null && typeof state.focused_agent_id !== "string")
    || typeof state.search_query !== "string"
    || !Array.isArray(state.status_filter)
    || state.status_filter.some((status) => typeof status !== "string")
  ) return { ok: false as const, error: "agents-overview state is malformed" };
  return {
    ok: true as const,
    state: {
      mode: state.mode as AgentsOverviewSurfaceState["mode"],
      last_multi_agent_mode: (lastMultiAgentMode === "auto" || lastMultiAgentMode === "grid")
        ? lastMultiAgentMode
        : state.mode === "grid"
          ? "grid"
          : "auto",
      focused_agent_id: state.focused_agent_id as string | null,
      search_query: state.search_query,
      status_filter: state.status_filter as string[],
    },
  };
}

function restoreEmptySurfaceState(type: string, value: unknown, version: number) {
  if (version !== 1) return { ok: false as const, error: `unsupported ${type} state version ${version}` };
  if (typeof value !== "object" || value === null || Array.isArray(value) || Object.keys(value).length > 0) {
    return { ok: false as const, error: `${type} state must be an empty object` };
  }
  return { ok: true as const, state: {} };
}

function coreSurfaceDefinitions(
  dirtySurfacePrompt: DirtySurfacePrompt,
): readonly SurfaceDefinition[] {
  return [
    surfaceDefinition({
      type: "new-tab",
      title: "New Tab",
      render_policy: "recreate_from_state",
      open_policy: "allow_multiple",
      default_state: () => ({}),
      restore_state: (value, version) => restoreEmptySurfaceState("new-tab", value, version),
    }),
    surfaceDefinition({
      type: "agents-overview",
      title: "Agents",
      render_policy: "keep_alive",
      open_policy: "singleton",
      default_state: (): AgentsOverviewSurfaceState => ({ ...DEFAULT_AGENTS_OVERVIEW_STATE }),
      restore_state: restoreAgentsOverviewState,
    }),
    ...CORE_VIEW_SURFACE_DEFINITIONS,
    surfaceDefinition({
      type: "library",
      title: "Library",
      render_policy: "keep_alive",
      open_policy: "singleton",
      close_policy: "confirm_if_dirty",
      max_state_bytes: CORE_VIEW_SURFACE_MAX_STATE_BYTES,
      can_close: createLibrarySurfaceCloseGuard(dirtySurfacePrompt),
      commands: [dirtySurfaceCommand("library", "Library")],
      badges: (surface) => useLibraryStore.getState().isEditorSurfaceDirty(surface.surface_id)
        ? [{ badge_id: "dirty", label: "Unsaved changes" }]
        : [],
      serialize_state: () => ({}),
      restore_state: (value, version) => restoreEmptySurfaceState("library", value, version),
    }),
    surfaceDefinition({
      type: "workflows",
      title: "Workflows",
      render_policy: "keep_alive",
      open_policy: "singleton",
      close_policy: "confirm_if_dirty",
      max_state_bytes: CORE_VIEW_SURFACE_MAX_STATE_BYTES,
      can_close: createWorkflowsSurfaceCloseGuard(dirtySurfacePrompt),
      commands: [dirtySurfaceCommand("workflows", "Workflows")],
      badges: () => useBuilderStore.getState().dirty
        ? [{ badge_id: "dirty", label: "Unsaved changes" }]
        : [],
      serialize_state: () => ({}),
      restore_state: (value, version) => restoreEmptySurfaceState("workflows", value, version),
    }),
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
}

export function createCoreWorkbenchSurfaceRegistry(
  options: CoreWorkbenchSurfaceRegistryOptions = {},
): WorkbenchSurfaceRegistry {
  return createSurfaceRegistry(coreSurfaceDefinitions(
    options.dirty_surface_prompt ?? failClosedDirtyPrompt,
  ));
}
