import type { Page } from "@playwright/test";
import { createHash } from "node:crypto";

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

export type WorkbenchFileFixture = {
  path: string;
  content: string;
  mime_type?: string;
  renderer_kind?: "text" | "markdown" | "image" | "pdf";
  stream_url?: string;
  revision?: number;
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
  explorer_root?: string;
  files?: WorkbenchFileFixture[];
  /** Exact path returned by the next native Save As picker call; null models cancellation. */
  save_target_path?: string | null;
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
  update_file: (path: string, content: string) => Promise<void>;
};

export type WorkbenchIpcMockController = {
  calls: (command?: string) => Promise<WorkbenchIpcCall[]>;
  snapshot: () => Promise<WorkbenchIpcMockSnapshot>;
  setLoadResult: (result: WorkbenchLoadResult) => Promise<void>;
  setAgents: (
    agents: WorkbenchAgentFixture[],
    options?: { emit?: boolean },
  ) => Promise<void>;
  updateFile: (path: string, content: string) => Promise<void>;
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
  const browserFiles = (options.files ?? []).map((file) => ({
    ...file,
    content_hash: `sha256:${createHash("sha256").update(file.content).digest("hex")}`,
  }));

  await page.addInitScript(
    ({
      loadResult,
      agents,
      safeMode,
      resetDelayMs,
      resetOutcome,
      responses,
      explorerRoot,
      files,
      saveTargetPath,
    }) => {
      type Callback = (value: unknown) => void;
      type Listener = { callback_id: number; event_id: number };
      type Runtime = BrowserWorkbenchMockRuntime;

      let callbackId = 1;
      let eventId = 1;
      const callbacks = new Map<number, Callback>();
      const listeners = new Map<string, Listener[]>();
      let fileSubscriptionId = 1;
      let saveTargetGrantId = 1;
      let recoveryId = 1;
      const normalizePath = (path: string) => path.replace(/\\/g, "/").replace(/\/$/, "");
      const hashText = async (text: string) => {
        const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(text));
        return `sha256:${[...new Uint8Array(digest)]
          .map((byte) => byte.toString(16).padStart(2, "0"))
          .join("")}`;
      };
      const fileFixtures = new Map(files.map((file) => [normalizePath(file.path), {
        ...file,
        path: normalizePath(file.path),
        revision: file.revision ?? 1,
      }]));
      const fileSubscriptions = new Map<string, string>();
      const saveTargetGrants = new Map<string, string>();
      let nextSaveTargetPath = saveTargetPath === null ? null : normalizePath(saveTargetPath);
      const recoveries = new Map<string, {
        schema: 1;
        recovery_id: string;
        resource_key: string;
        base_content_hash: string;
        base_opaque_revision: string;
        recovery_revision: number;
        created_at_ms: number;
        updated_at_ms: number;
        display_name: string;
        extension: string | null;
        mime_type: string;
        base: string;
        buffer: string;
      }>();
      const resourceIdFor = (path: string) => `file:${normalizePath(path)}`;
      const descriptorFor = (file: typeof files[number]) => {
        const normalizedPath = normalizePath(file.path);
        const displayName = normalizedPath.split("/").pop() ?? normalizedPath;
        const extension = displayName.includes(".") ? displayName.split(".").pop() ?? null : null;
        const lines = file.content === "" ? 0 : file.content.split(/\r?\n/).length;
        const rendererKind = file.renderer_kind ?? (extension === "md" ? "markdown" : "text");
        const streamed = rendererKind === "image" || rendererKind === "pdf";
        return {
          schema: 1,
          canonical_path: normalizedPath,
          display_name: displayName,
          extension,
          mime_type: file.mime_type ?? (rendererKind === "markdown" ? "text/markdown" : "text/plain"),
          encoding: streamed ? null : "utf-8",
          renderer_kind: rendererKind,
          size_bytes: new TextEncoder().encode(file.content).byteLength,
          line_count: streamed ? null : lines,
          content_hash: file.content_hash,
          modified_at_ms: 1_752_624_000_000 + file.revision,
          capabilities: { preview: true, changes: false, draft: false, stream: streamed },
          unavailable_reason: null,
        };
      };
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
        update_file: async (path, content) => {
          const normalizedPath = normalizePath(path);
          const file = fileFixtures.get(normalizedPath);
          if (!file) throw new Error(`mock file not found: ${normalizedPath}`);
          file.content = content;
          file.content_hash = await hashText(content);
          file.revision += 1;
          if ([...fileSubscriptions.values()].includes(normalizedPath)) {
            runtime.emit("file-resource://revision", {
              schema: 1,
              resource_id: resourceIdFor(normalizedPath),
              revision: file.revision,
              descriptor: descriptorFor(file),
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

          if (command === "get_explorer_root") return explorerRoot;
          if (command === "get_directory_tree") {
            const requestedPath = normalizePath(String(args?.path ?? ""));
            const prefix = `${requestedPath}/`;
            const children = new Map<string, {
              name: string;
              path: string;
              is_dir: boolean;
              extension: string | null;
            }>();
            for (const file of fileFixtures.values()) {
              if (!file.path.startsWith(prefix)) continue;
              const relative = file.path.slice(prefix.length);
              const [name, ...remaining] = relative.split("/");
              if (!name) continue;
              const isDirectory = remaining.length > 0;
              children.set(name, {
                name,
                path: `${requestedPath}/${name}`,
                is_dir: isDirectory,
                extension: isDirectory || !name.includes(".") ? null : name.split(".").pop() ?? null,
              });
            }
            return [...children.values()].sort((left, right) => left.name.localeCompare(right.name));
          }
          if (command === "git_status") return { files: [] };
          if (command === "explorer_watch" || command === "explorer_unwatch") return null;

          if (command === "open_file_resource") {
            const request = args?.request as { path?: string } | undefined;
            const path = normalizePath(String(request?.path ?? ""));
            const file = fileFixtures.get(path);
            if (!file) throw new Error(`mock file not found: ${path}`);
            const subscription_id = `mock-subscription-${fileSubscriptionId++}`;
            fileSubscriptions.set(subscription_id, path);
            return {
              resource_id: resourceIdFor(path),
              subscription_id,
              revision: file.revision,
              descriptor: descriptorFor(file),
            };
          }
          if (command === "read_file_resource_text") {
            const request = args?.request as {
              resource_id?: string;
              subscription_id?: string;
              revision?: number;
            } | undefined;
            const subscriptionId = String(request?.subscription_id ?? "");
            const path = fileSubscriptions.get(subscriptionId);
            const file = path ? fileFixtures.get(path) : undefined;
            if (!path || !file || request?.resource_id !== resourceIdFor(path)) {
              throw new Error("mock file subscription is not active for this resource");
            }
            if (request.revision !== file.revision) throw new Error("stale_revision");
            return {
              schema: 1,
              resource_id: resourceIdFor(path),
              revision: file.revision,
              text: file.content,
            };
          }
          if (command === "save_file_resource_text") {
            const request = args?.request as {
              resource_id?: string;
              subscription_id?: string;
              expected_revision?: number;
              buffer_base_hash?: string;
              text?: string;
              recovery_cleanup?: {
                recovery_id?: string;
                expected_recovery_revision?: number;
              } | null;
            } | undefined;
            const subscriptionId = String(request?.subscription_id ?? "");
            const path = fileSubscriptions.get(subscriptionId);
            const file = path ? fileFixtures.get(path) : undefined;
            if (!path || !file || request?.resource_id !== resourceIdFor(path)) {
              throw new Error("mock file subscription is not active for this resource");
            }
            const currentHash = descriptorFor(file).content_hash;
            if (
              request.expected_revision !== file.revision
              || request.buffer_base_hash !== currentHash
            ) {
              return {
                status: "stale_conflict",
                revision: file.revision,
                content_hash: currentHash,
              };
            }
            if (typeof request.text !== "string") throw new Error("mock save text is required");
            if (request.text === file.content) {
              return {
                status: "unchanged",
                revision: file.revision,
                content_hash: currentHash,
              };
            }
            const cleanup = request.recovery_cleanup;
            if (cleanup) {
              const recovery = recoveries.get(String(cleanup.recovery_id ?? ""));
              if (
                !recovery
                || recovery.recovery_revision !== cleanup.expected_recovery_revision
              ) throw new Error("stale_recovery_revision");
              recoveries.delete(recovery.recovery_id);
            }
            file.content = request.text;
            file.content_hash = await hashText(request.text);
            file.revision += 1;
            return {
              status: "saved",
              revision: file.revision,
              content_hash: descriptorFor(file).content_hash,
            };
          }
          if (command === "pick_file_resource_save_target") {
            if (nextSaveTargetPath === null) return null;
            const grantId = `mock-save-target-${saveTargetGrantId++}`;
            const selectedPath = nextSaveTargetPath;
            nextSaveTargetPath = null;
            saveTargetGrants.set(grantId, selectedPath);
            return {
              schema: 1,
              save_target_grant_id: grantId,
              selected_path: selectedPath,
            };
          }
          if (command === "save_file_resource_as_text") {
            const request = args?.request as {
              save_target_grant_id?: string;
              text?: string;
            } | undefined;
            const grantId = String(request?.save_target_grant_id ?? "");
            const path = saveTargetGrants.get(grantId);
            if (!path) throw new Error("mock Save As grant is invalid or already consumed");
            if (typeof request?.text !== "string") throw new Error("mock Save As text is required");
            saveTargetGrants.delete(grantId);
            const existing = fileFixtures.get(path);
            if (existing) {
              existing.content = request.text;
              existing.content_hash = await hashText(request.text);
              existing.revision += 1;
            } else {
              fileFixtures.set(path, {
                path,
                content: request.text,
                content_hash: await hashText(request.text),
                revision: 1,
              });
            }
            const saved = fileFixtures.get(path);
            if (!saved) throw new Error("mock Save As target was not created");
            return {
              schema: 1,
              capability_id: `mock-user-file-${grantId}`,
              canonical_path: path,
              resource_id: resourceIdFor(path),
              content_hash: descriptorFor(saved).content_hash,
            };
          }
          if (command === "list_file_recoveries") {
            const request = args?.request as { resource_key?: string } | undefined;
            return [...recoveries.values()]
              .filter((recovery) => recovery.resource_key === request?.resource_key)
              .map(({ base: _base, buffer: _buffer, ...summary }) => summary);
          }
          if (command === "get_file_recovery") {
            const request = args?.request as {
              recovery_id?: string;
              resource_key?: string;
            } | undefined;
            const recovery = recoveries.get(String(request?.recovery_id ?? ""));
            if (!recovery || recovery.resource_key !== request?.resource_key) {
              throw new Error("mock file recovery not found");
            }
            return clone(recovery);
          }
          if (command === "checkpoint_file_recovery") {
            const request = args?.request as {
              recovery_id?: string | null;
              expected_recovery_revision?: number | null;
              resource_id?: string;
              subscription_id?: string;
              base_content_hash?: string;
              resource_key?: string;
              base?: string;
              buffer?: string;
            } | undefined;
            const subscriptionId = String(request?.subscription_id ?? "");
            const path = fileSubscriptions.get(subscriptionId);
            const file = path ? fileFixtures.get(path) : undefined;
            if (typeof request?.base !== "string" || typeof request.buffer !== "string") {
              throw new Error("mock recovery text is required");
            }
            if (await hashText(request.base) !== request.base_content_hash) {
              throw new Error("recovery base content does not match its declared hash");
            }
            if (
              !path
              || !file
              || request.resource_id !== resourceIdFor(path)
              || request.resource_key !== request.resource_id
            ) {
              throw new Error("mock recovery subscription is not active");
            }
            let existing: (typeof recoveries extends Map<string, infer T> ? T : never) | undefined;
            if (request.recovery_id === null) {
              if (request.expected_recovery_revision !== null) {
                throw new Error("stale_recovery_revision");
              }
            } else if (typeof request.recovery_id === "string" && request.recovery_id !== "") {
              existing = recoveries.get(request.recovery_id);
              if (
                !existing
                || existing.resource_key !== request.resource_key
                || existing.recovery_revision !== request.expected_recovery_revision
              ) throw new Error("stale_recovery_revision");
            } else {
              throw new Error("stale_recovery_revision");
            }
            const now = Date.now();
            const recovery_id = existing?.recovery_id ?? `mock-recovery-${recoveryId++}`;
            const recovery_revision = (existing?.recovery_revision ?? 0) + 1;
            const descriptor = descriptorFor(file);
            const base_opaque_revision = existing
              && existing.base_content_hash === request.base_content_hash
              ? existing.base_opaque_revision
              : `mock-base-${recovery_id}-${recovery_revision}`;
            const recovery = {
              schema: 1 as const,
              recovery_id,
              resource_key: request.resource_key,
              base_content_hash: request.base_content_hash,
              base_opaque_revision,
              recovery_revision,
              created_at_ms: existing?.created_at_ms ?? now,
              updated_at_ms: now,
              display_name: descriptor.display_name,
              extension: descriptor.extension,
              mime_type: descriptor.mime_type,
              base: request.base,
              buffer: request.buffer,
            };
            recoveries.set(recovery_id, recovery);
            const { base: _base, buffer: _buffer, display_name: _displayName,
              extension: _extension, mime_type: _mimeType, ...checkpoint } = recovery;
            return { ...checkpoint, file_authorization_error: null };
          }
          if (command === "discard_file_recovery") {
            const request = args?.request as {
              recovery_id?: string;
              expected_recovery_revision?: number;
              resource_key?: string;
            } | undefined;
            const recovery = recoveries.get(String(request?.recovery_id ?? ""));
            if (
              !recovery
              || recovery.resource_key !== request?.resource_key
              || recovery.recovery_revision !== request.expected_recovery_revision
            ) throw new Error("stale_recovery_revision");
            recoveries.delete(recovery.recovery_id);
            return null;
          }
          if (command === "issue_file_resource_ticket") {
            const request = args?.request as {
              resource_id?: string;
              subscription_id?: string;
              revision?: number;
              renderer_lease_id?: string;
            } | undefined;
            const subscriptionId = String(request?.subscription_id ?? "");
            const path = fileSubscriptions.get(subscriptionId);
            const file = path ? fileFixtures.get(path) : undefined;
            if (
              !path
              || !file
              || !file.stream_url
              || request?.resource_id !== resourceIdFor(path)
              || request.revision !== file.revision
            ) throw new Error("mock streamed file subscription is not active");
            return {
              schema: 1,
              ticket_id: `mock-ticket-${subscriptionId}`,
              url: file.stream_url,
              resource_id: resourceIdFor(path),
              revision: file.revision,
              renderer_lease_id: String(request.renderer_lease_id ?? ""),
              expires_at_ms: Date.now() + 60_000,
            };
          }
          if (command === "close_file_renderer_lease") return null;
          if (command === "close_file_resource") {
            const request = args?.request as { subscription_id?: string } | undefined;
            const subscriptionId = String(request?.subscription_id ?? "");
            if (!fileSubscriptions.delete(subscriptionId)) {
              throw new Error("mock file subscription is already closed");
            }
            return null;
          }

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
      explorerRoot: options.explorer_root ?? "/workspace",
      files: browserFiles,
      saveTargetPath: options.save_target_path ?? null,
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
    updateFile: (path, content) => page.evaluate(
      ({ targetPath, nextContent }) => {
        const runtime = (window as unknown as {
          __WARDIAN_WORKBENCH_IPC_MOCK__: BrowserWorkbenchMockRuntime;
        }).__WARDIAN_WORKBENCH_IPC_MOCK__;
      return runtime.update_file(targetPath, nextContent);
      },
      { targetPath: path, nextContent: content },
    ),
  };
}
