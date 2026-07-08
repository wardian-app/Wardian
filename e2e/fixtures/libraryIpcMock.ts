import type { Page } from "@playwright/test";

/**
 * Shared library IPC mock for browser E2E specs.
 *
 * Installs a full `window.__TAURI_INTERNALS__.invoke` bridge covering every
 * command the app calls on initial mount (agents, classes, provider
 * readiness, watchlists, onboarding hints, workflows, shell/app settings)
 * plus the library-specific commands (`get_library_index`,
 * `read_library_item`, `save_library_item`, `update_library_metadata`, and
 * the rest of the library CRUD surface).
 *
 * Extracted from `e2e/tests/library-redesign.spec.ts` so other specs that
 * need a working Library view (e.g. `e2e/tests/features.spec.ts`) reuse the
 * exact same fixture shape instead of hand-rolling a divergent one. DTO
 * field names here must keep matching `crates/wardian-core/src/models/library.rs`.
 */

export interface LibraryEntryFixture {
  kind: "skill" | "prompt" | "workflow" | "class";
  path: string;
  entry_ref: string;
  name: string;
  description: string;
  tags: string[];
  is_starred: boolean;
  deployment_count: number;
  error: string | null;
}

export interface LibraryFolderFixture {
  path: string;
  name: string;
  children: (LibraryFolderFixture | LibraryEntryFixture)[];
}

export function buildLibraryIndexFixture() {
  const planner: LibraryEntryFixture = {
    kind: "skill",
    path: "dev/planner",
    entry_ref: "skills/dev/planner",
    name: "planner",
    description: "Plans work",
    tags: ["dev"],
    is_starred: false,
    deployment_count: 0,
    error: null,
  };
  const reviewer: LibraryEntryFixture = {
    kind: "skill",
    path: "ops/reviewer",
    entry_ref: "skills/ops/reviewer",
    name: "reviewer",
    description: "Reviews code",
    tags: [],
    is_starred: false,
    deployment_count: 0,
    error: null,
  };
  const greeting: LibraryEntryFixture = {
    kind: "prompt",
    path: "greeting.md",
    entry_ref: "prompts/greeting.md",
    name: "greeting",
    description: "Say hello to the team",
    tags: [],
    is_starred: false,
    deployment_count: 0,
    error: null,
  };
  const triage: LibraryEntryFixture = {
    kind: "workflow",
    path: "triage.md",
    entry_ref: "workflows/triage.md",
    name: "triage",
    description: "Triage workflow",
    tags: [],
    is_starred: false,
    deployment_count: 0,
    error: null,
  };
  const architect: LibraryEntryFixture = {
    kind: "class",
    path: "Architect",
    entry_ref: "classes/Architect",
    name: "Architect",
    description: "Role: Architect",
    tags: [],
    is_starred: false,
    deployment_count: 0,
    error: null,
  };

  const skillsTree: LibraryFolderFixture = {
    path: "",
    name: "Root",
    children: [
      { path: "dev", name: "dev", children: [planner] },
      { path: "ops", name: "ops", children: [reviewer] },
    ],
  };
  const promptsTree: LibraryFolderFixture = { path: "", name: "Root", children: [greeting] };
  const workflowsTree: LibraryFolderFixture = { path: "", name: "Root", children: [triage] };
  const classesTree: LibraryFolderFixture = { path: "", name: "Root", children: [architect] };
  const mcpsTree: LibraryFolderFixture = { path: "", name: "Root", children: [] };

  return {
    sections: {
      skills: { tree: skillsTree, stubbed: false },
      prompts: { tree: promptsTree, stubbed: false },
      workflows: { tree: workflowsTree, stubbed: false },
      classes: { tree: classesTree, stubbed: false },
      mcps: { tree: mcpsTree, stubbed: true },
    },
    deployments: {},
    orphans: [],
  };
}

export function buildLibraryContentFixture(): Record<string, string> {
  return {
    "skills/dev/planner": "---\ndescription: Plans work\n---\n# Planner\nBody",
    "skills/ops/reviewer": "---\ndescription: Reviews code\n---\n# Reviewer\nBody",
    "prompts/greeting.md": "# Greeting\nSay hello to the team",
    "workflows/triage.md": "---\ndescription: Triage workflow\n---\n# Triage",
    "classes/Architect": "# Role: Architect\nDesigns systems",
  };
}

export async function installLibraryIpcMock(
  page: Page,
  indexFixture: ReturnType<typeof buildLibraryIndexFixture>,
  contentFixture: Record<string, string>,
) {
  await page.addInitScript(
    ({ indexFixture, contentFixture }) => {
      let callbackId = 1;
      const callbacks = new Map<number, unknown>();
      const tauriWindow = window as Window & {
        __TAURI_INTERNALS__?: Record<string, unknown>;
        __TAURI_EVENT_PLUGIN_INTERNALS__?: Record<string, unknown>;
      };

      tauriWindow.__TAURI_EVENT_PLUGIN_INTERNALS__ = {
        unregisterListener: () => undefined,
      };

      // Mutable in-memory copies: save/metadata mutations below act on
      // these so a subsequent get_library_index reflects them, same as the
      // real backend would after a round trip to disk.
      const libraryIndex = JSON.parse(JSON.stringify(indexFixture));
      const content: Record<string, string> = { ...contentFixture };

      type Node = { path: string; name: string; children?: Node[]; entry_ref?: string };

      function findEntry(node: Node, entryRef: string): Node | null {
        if (node.entry_ref === entryRef) return node;
        for (const child of node.children ?? []) {
          const found = findEntry(child, entryRef);
          if (found) return found;
        }
        return null;
      }

      function updateMetadata(entryRef: string, metadata: { tags: string[]; is_starred: boolean }) {
        for (const section of Object.values(libraryIndex.sections) as { tree: Node }[]) {
          const entry = findEntry(section.tree, entryRef) as (Node & {
            tags?: string[];
            is_starred?: boolean;
          }) | null;
          if (entry) {
            entry.tags = metadata.tags;
            entry.is_starred = metadata.is_starred;
            return;
          }
        }
      }

      tauriWindow.__TAURI_INTERNALS__ = {
        metadata: {
          currentWindow: { label: "main" },
          currentWebview: { label: "main" },
        },
        transformCallback: (callback: unknown) => {
          const id = callbackId++;
          callbacks.set(id, callback);
          return id;
        },
        unregisterCallback: (id: number) => {
          callbacks.delete(id);
        },
        convertFileSrc: (filePath: string) => filePath,
        invoke: async (command: string, args?: Record<string, unknown>) => {
          if (command === "list_agents") return [];
          if (command === "list_agent_classes") {
            return [{ name: "Architect", description: "Designs systems", is_default: false }];
          }
          if (command === "list_provider_readiness") {
            return [
              { provider: "claude", display_name: "Claude", available: true, executable: "claude", reason: null },
            ];
          }
          if (command === "load_watchlists") return [];
          if (command === "load_watchlist_prefs") return null;
          if (command === "load_agent_interactions") return {};
          if (command === "load_queue_items") return [];
          if (command === "load_queue_preferences") return {};
          if (command === "load_onboarding_hints") {
            return { dismissed_hint_ids: ["spawn-agent-first-run:v1"] };
          }
          if (command === "dismiss_onboarding_hint") {
            return { dismissed_hint_ids: ["spawn-agent-first-run:v1"] };
          }
          if (command === "list_workflows") return [];
          if (command === "list_scheduled_runs") return [];
          if (command === "load_workflow_library") return { folders: [], rootWorkflowIds: [] };
          if (command === "list_deployed_skills") return [];
          if (command === "load_app_settings") return null;
          if (command === "load_shell_settings") {
            return {
              shell_id: "auto",
              custom_executable: null,
              custom_args: null,
              agent_session_persistence: "resume",
              default_provider: "claude",
            };
          }
          if (command === "list_available_shells") return [];
          if (command === "sync_provider_theme_settings") return null;
          if (command === "plugin:event|listen") return callbackId++;
          if (command === "plugin:event|unlisten") return null;

          if (command === "get_library_index") return libraryIndex;
          if (command === "read_library_item") {
            const key = `${args?.section}/${args?.path}`;
            return content[key] ?? "";
          }
          if (command === "save_library_item") {
            const key = `${args?.section}/${args?.path}`;
            content[key] = String(args?.content ?? "");
            return null;
          }
          if (command === "update_library_metadata") {
            updateMetadata(
              String(args?.entryRef),
              args?.metadata as { tags: string[]; is_starred: boolean },
            );
            return null;
          }
          if (command === "create_library_folder") return null;
          if (command === "rename_library_entry") return null;
          if (command === "delete_library_entry") return null;
          if (command === "set_skill_deployments") return null;
          if (command === "remove_orphan_deployment") return null;
          if (command === "open_library_folder") return null;
          if (command === "library_watch") return null;
          if (command === "library_unwatch") return null;

          return null;
        },
      };
    },
    { indexFixture, contentFixture },
  );
}
