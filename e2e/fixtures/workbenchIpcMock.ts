import type { Page } from "@playwright/test";

import type {
  ClosedSurfaceV1,
  WorkbenchDocumentV1,
  WorkbenchGroupV1,
  WorkbenchNodeV1,
  WorkbenchShellV1,
  WorkbenchSurfaceV1,
} from "../../src/types";
import type {
  WorkbenchLoadResult,
  WorkbenchResetRequest,
  WorkbenchSaveRequest,
} from "../../src/features/workbench/workbenchPersistence";

export const DEFAULT_WORKBENCH_MOCK_SHELL: WorkbenchShellV1 = {
  left_sidebar_collapsed: false,
  left_sidebar_width: 240,
  right_sidebar_collapsed: false,
  right_sidebar_width: 240,
  bottom_terminal_open: false,
  bottom_terminal_height: 360,
};

export type WorkbenchAgentFixture = {
  session_id: string;
  session_name: string;
  agent_class: string;
  folder: string;
  provider: string;
  is_off: boolean;
  [key: string]: unknown;
};

export type WorkbenchIpcCall = {
  command: string;
  args?: Record<string, unknown>;
};

export type WorkbenchDocumentOptions = {
  revision?: number;
  saved_at?: string;
  root?: WorkbenchNodeV1;
  groups?: Record<string, WorkbenchGroupV1>;
  surfaces?: WorkbenchSurfaceV1[] | Record<string, WorkbenchSurfaceV1>;
  active_group_id?: string;
  recently_closed?: ClosedSurfaceV1[];
  shell?: Partial<WorkbenchShellV1>;
};

function defaultState(surfaceType: string): unknown {
  if (surfaceType === "agents-overview") {
    return {
      mode: "auto",
      focused_agent_id: null,
      search_query: "",
      status_filter: [],
    };
  }
  if (surfaceType === "graph") {
    return {
      enabled_reasons: [],
      inspected_agent_id: null,
      inspector_open: true,
      selected_edge_id: null,
      picker_search: "",
    };
  }
  if (surfaceType === "garden") {
    return { selected_unit_key: null };
  }
  return {};
}

export function makeWorkbenchSurface(
  surface_id: string,
  surface_type: string,
  overrides: Partial<Omit<WorkbenchSurfaceV1, "surface_id" | "surface_type">> = {},
): WorkbenchSurfaceV1 {
  return {
    surface_id,
    surface_type,
    state_schema_version: 1,
    state: defaultState(surface_type),
    ...overrides,
  };
}

export function makeWorkbenchDocument(
  options: WorkbenchDocumentOptions = {},
): WorkbenchDocumentV1 {
  const surfaces = Array.isArray(options.surfaces)
    ? Object.fromEntries(options.surfaces.map((surface) => [surface.surface_id, surface]))
    : { ...(options.surfaces ?? {}) };
  const defaultGroupId = options.active_group_id
    ?? Object.keys(options.groups ?? {})[0]
    ?? "group-1";
  const surfaceIds = Object.keys(surfaces);
  const groups = options.groups ?? {
    [defaultGroupId]: {
      group_id: defaultGroupId,
      surface_ids: surfaceIds,
      active_surface_id: surfaceIds[0] ?? null,
    },
  };

  return {
    schema_version: 1,
    revision: options.revision ?? 0,
    saved_at: options.saved_at ?? "2026-07-11T00:00:00.000Z",
    root: options.root ?? { kind: "group", group_id: defaultGroupId },
    groups,
    surfaces,
    active_group_id: defaultGroupId,
    recently_closed: options.recently_closed ?? [],
    shell: { ...DEFAULT_WORKBENCH_MOCK_SHELL, ...options.shell },
  };
}

export type WorkbenchIpcMockOptions = {
  load_result?: WorkbenchLoadResult;
  agents?: WorkbenchAgentFixture[];
  safe_mode?: boolean;
  reset_delay_ms?: number;
  reset_outcome?: "saved" | "revision_conflict" | "error";
  responses?: Record<string, unknown>;
};

export type WorkbenchIpcMockSnapshot = {
  load_result: WorkbenchLoadResult;
  agents: WorkbenchAgentFixture[];
  calls: WorkbenchIpcCall[];
};

type BrowserWorkbenchMockRuntime = {
  load_result: WorkbenchLoadResult;
  agents: WorkbenchAgentFixture[];
  calls: WorkbenchIpcCall[];
  set_load_result: (result: WorkbenchLoadResult) => void;
  set_agents: (agents: WorkbenchAgentFixture[], emit: boolean) => void;
  emit: (event: string, payload: unknown) => void;
};

export type WorkbenchIpcMockController = {
  calls: (command?: string) => Promise<WorkbenchIpcCall[]>;
  snapshot: () => Promise<WorkbenchIpcMockSnapshot>;
  setLoadResult: (result: WorkbenchLoadResult) => Promise<void>;
  setAgents: (
    agents: WorkbenchAgentFixture[],
    options?: { emit?: boolean },
  ) => Promise<void>;
};

/**
 * Installs a deterministic, stateful Tauri IPC bridge for workbench browser E2E.
 * Persistence commands implement the V1 CAS contract instead of returning
 * unconditional canned successes.
 */
export async function installWorkbenchIpcMock(
  page: Page,
  options: WorkbenchIpcMockOptions = {},
): Promise<WorkbenchIpcMockController> {
  const initialDocument = makeWorkbenchDocument();
  const loadResult: WorkbenchLoadResult = options.load_result ?? {
    source: "primary",
    document: initialDocument,
    notice: null,
    durable_revision: initialDocument.revision,
    durable_token: `mock-token-${initialDocument.revision}`,
  };

  await page.addInitScript(
    ({ loadResult, agents, safeMode, resetDelayMs, resetOutcome, responses }) => {
      type Callback = (value: unknown) => void;
      type Listener = { callback_id: number; event_id: number };
      type Runtime = BrowserWorkbenchMockRuntime;

      let callbackId = 1;
      let eventId = 1;
      const callbacks = new Map<number, Callback>();
      const listeners = new Map<string, Listener[]>();
      const clone = <T,>(value: T): T => structuredClone(value);
      const loadStorageKey = "wardian-e2e-workbench-load-result";
      const agentsStorageKey = "wardian-e2e-workbench-agents";
      const readStored = <T,>(key: string, fallback: T): T => {
        try {
          const stored = localStorage.getItem(key);
          return stored === null ? clone(fallback) : JSON.parse(stored) as T;
        } catch {
          return clone(fallback);
        }
      };
      const writeStored = (key: string, value: unknown): void => {
        localStorage.setItem(key, JSON.stringify(value));
      };
      const tauriWindow = window as Window & {
        __TAURI_INTERNALS__?: Record<string, unknown>;
        __TAURI_EVENT_PLUGIN_INTERNALS__?: Record<string, unknown>;
        __WARDIAN_WORKBENCH_IPC_MOCK__?: Runtime;
      };

      const runtime: Runtime = {
        load_result: readStored(loadStorageKey, loadResult),
        agents: readStored(agentsStorageKey, agents),
        calls: [],
        set_load_result: (result) => {
          runtime.load_result = clone(result);
          writeStored(loadStorageKey, runtime.load_result);
        },
        set_agents: (nextAgents, shouldEmit) => {
          runtime.agents = clone(nextAgents);
          writeStored(agentsStorageKey, runtime.agents);
          if (shouldEmit) runtime.emit("agents-updated", null);
        },
        emit: (event, payload) => {
          for (const listener of listeners.get(event) ?? []) {
            callbacks.get(listener.callback_id)?.({
              event,
              id: listener.event_id,
              payload: clone(payload),
            });
          }
        },
      };
      tauriWindow.__WARDIAN_WORKBENCH_IPC_MOCK__ = runtime;
      tauriWindow.__TAURI_EVENT_PLUGIN_INTERNALS__ = {
        unregisterListener: () => undefined,
      };

      const emptyLibraryIndex = {
        sections: {
          skills: { tree: { path: "", name: "Root", children: [] }, stubbed: false },
          prompts: { tree: { path: "", name: "Root", children: [] }, stubbed: false },
          workflows: { tree: { path: "", name: "Root", children: [] }, stubbed: false },
          classes: { tree: { path: "", name: "Root", children: [] }, stubbed: false },
          mcps: { tree: { path: "", name: "Root", children: [] }, stubbed: true },
        },
        deployments: {},
        orphans: [],
      };
      const defaults: Record<string, unknown> = {
        list_agent_classes: [],
        list_provider_readiness: [],
        load_watchlists: [],
        load_watchlist_prefs: null,
        load_agent_interactions: {},
        load_queue_items: [],
        load_queue_preferences: {},
        load_onboarding_hints: { dismissed_hint_ids: ["spawn-agent-first-run:v1"] },
        dismiss_onboarding_hint: { dismissed_hint_ids: ["spawn-agent-first-run:v1"] },
        list_workflows: [],
        list_scheduled_runs: [],
        load_workflow_library: { folders: [], rootWorkflowIds: [] },
        workflow_list_blueprints: [],
        workflow_list_runs: [],
        get_library_index: emptyLibraryIndex,
        get_library_tree: { type: "Folder", path: "", name: "Root", children: [] },
        list_deployed_skills: [],
        load_app_settings: null,
        load_shell_settings: null,
        list_available_shells: [],
        sync_provider_theme_settings: null,
        get_topology: { edges: [], ignored_pairs: [], fallback_groups: [] },
        get_pair_activity: [],
        library_watch: null,
        library_unwatch: null,
      };

      function conflictResult(requestId: string) {
        return {
          outcome: "revision_conflict",
          durable_revision: runtime.load_result.durable_revision,
          durable_token: runtime.load_result.durable_token,
          request_id: requestId,
        };
      }

      function futureResult(requestId: string) {
        return {
          outcome: "future_schema",
          durable_revision: null,
          durable_token: null,
          request_id: requestId,
        };
      }

      function hasExpectedIdentity(request: WorkbenchResetRequest): boolean {
        return request.expected_revision === runtime.load_result.durable_revision
          && request.expected_token === runtime.load_result.durable_token;
      }

      tauriWindow.__TAURI_INTERNALS__ = {
        metadata: {
          currentWindow: { label: "main" },
          currentWebview: { label: "main" },
        },
        transformCallback: (callback: Callback) => {
          const id = callbackId++;
          callbacks.set(id, callback);
          return id;
        },
        unregisterCallback: (id: number) => {
          callbacks.delete(id);
        },
        convertFileSrc: (filePath: string) => filePath,
        invoke: async (command: string, args?: Record<string, unknown>) => {
          runtime.calls.push({
            command,
            ...(args === undefined ? {} : { args: clone(args) }),
          });

          if (command === "get_workbench_boot_config") return { safe_mode: safeMode };
          if (command === "load_workbench_state") return clone(runtime.load_result);
          if (command === "list_agents") return clone(runtime.agents);

          if (command === "save_workbench_state") {
            const request = args as unknown as WorkbenchSaveRequest;
            if (runtime.load_result.source === "future_schema") {
              return futureResult(request.request_id);
            }
            if (!hasExpectedIdentity(request)) return conflictResult(request.request_id);
            const token = `mock-token-${request.document.revision}`;
            runtime.set_load_result({
              source: "primary",
              document: clone(request.document),
              notice: null,
              durable_revision: request.document.revision,
              durable_token: token,
            });
            return {
              outcome: "saved",
              durable_revision: request.document.revision,
              durable_token: token,
              request_id: request.request_id,
            };
          }

          if (command === "reset_workbench_state") {
            const request = args as unknown as WorkbenchResetRequest;
            if (runtime.load_result.source === "future_schema") {
              return futureResult(request.request_id);
            }
            if (!hasExpectedIdentity(request)) return conflictResult(request.request_id);
            if (resetDelayMs > 0) {
              await new Promise((resolve) => window.setTimeout(resolve, resetDelayMs));
            }
            if (resetOutcome === "revision_conflict") {
              return conflictResult(request.request_id);
            }
            if (resetOutcome === "error") throw new Error("mock reset failure");
            const revision = request.expected_revision + 1;
            const document: WorkbenchDocumentV1 = {
              schema_version: 1,
              revision,
              saved_at: "2026-07-11T00:00:00.000Z",
              root: { kind: "group", group_id: "group-1" },
              groups: {
                "group-1": {
                  group_id: "group-1",
                  surface_ids: [],
                  active_surface_id: null,
                },
              },
              surfaces: {},
              active_group_id: "group-1",
              recently_closed: [],
              shell: {
                left_sidebar_collapsed: false,
                left_sidebar_width: 240,
                right_sidebar_collapsed: false,
                right_sidebar_width: 240,
                bottom_terminal_open: false,
                bottom_terminal_height: 360,
              },
            };
            const token = `mock-token-${revision}`;
            runtime.set_load_result({
              source: "primary",
              document: clone(document),
              notice: null,
              durable_revision: revision,
              durable_token: token,
            });
            return {
              outcome: "saved",
              durable_revision: revision,
              durable_token: token,
              request_id: request.request_id,
              document,
            };
          }

          if (command === "plugin:event|listen") {
            const event = String(args?.event ?? "");
            const handler = Number(args?.handler);
            const nextEventId = eventId++;
            listeners.set(event, [
              ...(listeners.get(event) ?? []),
              { callback_id: handler, event_id: nextEventId },
            ]);
            return nextEventId;
          }
          if (command === "plugin:event|unlisten") return null;

          if (Object.prototype.hasOwnProperty.call(responses, command)) {
            return clone(responses[command]);
          }
          if (Object.prototype.hasOwnProperty.call(defaults, command)) {
            return clone(defaults[command]);
          }
          return null;
        },
      };
    },
    {
      loadResult,
      agents: options.agents ?? [],
      safeMode: options.safe_mode ?? false,
      resetDelayMs: options.reset_delay_ms ?? 0,
      resetOutcome: options.reset_outcome ?? "saved",
      responses: options.responses ?? {},
    },
  );

  return {
    calls: (command) => page.evaluate((requestedCommand) => {
      const runtime = (window as unknown as {
        __WARDIAN_WORKBENCH_IPC_MOCK__: BrowserWorkbenchMockRuntime;
      }).__WARDIAN_WORKBENCH_IPC_MOCK__;
      const calls = runtime.calls;
      return structuredClone(requestedCommand
        ? calls.filter((call) => call.command === requestedCommand)
        : calls);
    }, command),
    snapshot: () => page.evaluate(() => {
      const runtime = (window as unknown as {
        __WARDIAN_WORKBENCH_IPC_MOCK__: BrowserWorkbenchMockRuntime;
      }).__WARDIAN_WORKBENCH_IPC_MOCK__;
      return structuredClone({
        load_result: runtime.load_result,
        agents: runtime.agents,
        calls: runtime.calls,
      });
    }),
    setLoadResult: (result) => page.evaluate((nextResult) => {
      const runtime = (window as unknown as {
        __WARDIAN_WORKBENCH_IPC_MOCK__: BrowserWorkbenchMockRuntime;
      }).__WARDIAN_WORKBENCH_IPC_MOCK__;
      runtime.set_load_result(nextResult);
    }, result),
    setAgents: (agents, setOptions = {}) => page.evaluate(
      ({ nextAgents, emit }) => {
        const runtime = (window as unknown as {
          __WARDIAN_WORKBENCH_IPC_MOCK__: BrowserWorkbenchMockRuntime;
        }).__WARDIAN_WORKBENCH_IPC_MOCK__;
        runtime.set_agents(nextAgents, emit);
      },
      { nextAgents: agents, emit: setOptions.emit ?? true },
    ),
  };
}
