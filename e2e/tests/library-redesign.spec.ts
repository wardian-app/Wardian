import { test, expect, type Page } from "@playwright/test";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";
import { seedLibraryFixtures } from "../fixtures/mockAgent";

/**
 * Library redesign browser E2E tests.
 *
 * These cover browser-layer rendering and interaction only: section rail
 * switching, list rendering from a seeded fixture, search/star, and
 * detail-pane open/edit flows. Like every other browser E2E spec in this
 * suite (see e2e/tests/graph-topology.spec.ts), there is no real Tauri
 * backend behind the Vite dev server the webServer block boots — so the
 * fixture below is installed by mocking `window.__TAURI_INTERNALS__.invoke`
 * directly, rather than by pointing the app at a real seeded WARDIAN_HOME.
 *
 * Real filesystem claims — a deploy creating an actual junction/reparse
 * point, undeploy leaving the source untouched, rename relinking a
 * deployment — cannot be proven at this layer and are covered by
 * e2e-native/tests/library-deployment-native.test.mjs instead.
 */

interface LibraryEntryFixture {
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

interface LibraryFolderFixture {
  path: string;
  name: string;
  children: (LibraryFolderFixture | LibraryEntryFixture)[];
}

function buildLibraryIndexFixture() {
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

function buildContentFixture(): Record<string, string> {
  return {
    "skills/dev/planner": "---\ndescription: Plans work\n---\n# Planner\nBody",
    "skills/ops/reviewer": "---\ndescription: Reviews code\n---\n# Reviewer\nBody",
    "prompts/greeting.md": "# Greeting\nSay hello to the team",
    "workflows/triage.md": "---\ndescription: Triage workflow\n---\n# Triage",
    "classes/Architect": "# Role: Architect\nDesigns systems",
  };
}

async function installLibraryIpcMock(
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

async function openLibraryView(page: Page) {
  await page.setViewportSize({ width: 1600, height: 1000 });
  await page.goto("/", { waitUntil: "domcontentloaded" });
  await page.locator('[data-testid="app-shell"]').waitFor({ timeout: 15_000 });
  await page.getByRole("button", { name: "Library", exact: true }).click();
  await expect(page.locator('[data-testid="library-view"]')).toBeVisible({ timeout: 10_000 });
}

test.describe("Library Redesign", () => {
  test.describe.configure({ mode: "serial" });

  let page: Page;

  test.beforeAll(async ({ browser }) => {
    page = await browser.newPage();
    await installLibraryIpcMock(page, buildLibraryIndexFixture(), buildContentFixture());
    await openLibraryView(page);
  });

  test.afterAll(async () => {
    await page.close();
  });

  test("renders all five rail sections with entry counts", async () => {
    const rail = page.locator('[data-testid="library-section-rail"]');
    await expect(rail).toBeVisible();

    await expect(page.locator('[data-testid="library-section-skills"]')).toContainText("2");
    await expect(page.locator('[data-testid="library-section-prompts"]')).toContainText("1");
    await expect(page.locator('[data-testid="library-section-classes"]')).toContainText("1");
    await expect(page.locator('[data-testid="library-section-workflows"]')).toContainText("1");
    // MCPs is stubbed with zero entries: no count badge renders at all.
    await expect(page.locator('[data-testid="library-section-mcps"]')).toBeVisible();
    await expect(page.locator('[data-testid="library-section-mcps"] span').nth(1)).toHaveCount(0);
  });

  test("navigates a nested skill folder and opens the entry", async () => {
    await page.locator('[data-testid="library-section-skills"]').click();
    await expect(page.locator('[data-testid="library-list"]')).toBeVisible();

    // Folders start collapsed: the nested skill row isn't rendered yet.
    await expect(page.locator('[data-testid="library-row-skills/dev/planner"]')).toHaveCount(0);

    await page.locator('[data-testid="library-folder-dev"]').click();
    const row = page.locator('[data-testid="library-row-skills/dev/planner"]');
    await expect(row).toBeVisible();

    await row.click();
    await expect(page.locator('[data-testid="skill-detail"]')).toBeVisible();
    await expect(page.locator('[data-testid="detail-header"]')).toContainText("planner");
    await expect(page.locator('[data-testid="markdown-editor-textarea"]')).toHaveValue(/Planner/);
  });

  test("search flattens results and shows a path subtitle", async () => {
    await page.locator('[data-testid="library-section-skills"]').click();
    await page.locator('[data-testid="library-search"]').fill("planner");

    const subtitle = page.locator('[data-testid="library-row-subtitle-skills/dev/planner"]');
    await expect(subtitle).toBeVisible();
    await expect(subtitle).toContainText("dev");

    await page.locator('[data-testid="library-search"]').fill("");
  });

  test("toggles the star on a skill row", async () => {
    await page.locator('[data-testid="library-section-skills"]').click();
    const starButton = page.locator('[data-testid="library-star-skills/dev/planner"]');
    await expect(starButton).toHaveAttribute("aria-pressed", "false");

    await starButton.click();
    await expect(starButton).toHaveAttribute("aria-pressed", "true");
  });

  test("edits and saves a prompt via Ctrl+S", async () => {
    await page.locator('[data-testid="library-section-prompts"]').click();
    await page.locator('[data-testid="library-row-prompts/greeting.md"]').click();

    const editor = page.locator('[data-testid="markdown-editor"]');
    await expect(editor).toBeVisible();
    await expect(editor).toContainText("Saved");

    const textarea = page.locator('[data-testid="markdown-editor-textarea"]');
    await textarea.click();
    await textarea.type("\nExtra line.");
    await expect(editor).toContainText("Unsaved changes");
    await expect(page.locator('[data-testid="markdown-editor-dirty-dot"]')).toBeVisible();

    await textarea.press("Control+s");
    await expect(editor).toContainText("Saved");
    await expect(page.locator('[data-testid="markdown-editor-dirty-dot"]')).toHaveCount(0);
  });

  test("opens a class and shows the AGENTS.md editor", async () => {
    await page.locator('[data-testid="library-section-classes"]').click();
    await page.locator('[data-testid="library-row-classes/Architect"]').click();

    await expect(page.locator('[data-testid="class-detail"]')).toBeVisible();
    await expect(page.locator('[data-testid="markdown-editor-textarea"]')).toHaveValue(/Role: Architect/);
  });

  test("shows the MCP stub copy", async () => {
    await page.locator('[data-testid="library-section-mcps"]').click();
    await expect(page.locator('[data-testid="library-mcp-stub"]')).toContainText(
      "MCP servers are coming to the library",
    );
    await expect(page.locator('[data-testid="mcp-stub-detail"]')).toContainText(
      "MCP servers are coming to the library",
    );
  });

  test("deploying a skill creates a real junction/reparse point", () => {
    test.skip(
      true,
      "junction behavior — see library-deployment-native.test.mjs", // @native-only
    );
  });
});

test.describe("seedLibraryFixtures fixture", () => {
  test("writes the nested skills, prompt, workflow, and class the native tests read back", () => {
    // Runs in the Playwright Node context: the browser layer above never
    // reads this directory (there is no real Tauri backend behind the Vite
    // dev server), so the fixture is verified by its on-disk output here —
    // the same split used for `seedTopology` in graph-topology.spec.ts. The
    // native E2E layer is what actually exercises this helper against a
    // live app.
    const home = fs.mkdtempSync(path.join(os.tmpdir(), "wardian-library-fixture-"));
    try {
      seedLibraryFixtures(home);

      const planner = fs.readFileSync(
        path.join(home, "library", "skills", "dev", "planner", "SKILL.md"),
        "utf8",
      );
      expect(planner).toContain("description: Plans work");

      const reviewer = fs.readFileSync(
        path.join(home, "library", "skills", "ops", "reviewer", "SKILL.md"),
        "utf8",
      );
      expect(reviewer).toContain("description: Reviews code");

      const greeting = fs.readFileSync(path.join(home, "library", "prompts", "greeting.md"), "utf8");
      expect(greeting).toContain("Say hello to the team");

      const triage = fs.readFileSync(path.join(home, "library", "workflows", "triage.md"), "utf8");
      expect(triage).toContain("description: Triage workflow");

      const agentsMd = fs.readFileSync(path.join(home, "classes", "Architect", "AGENTS.md"), "utf8");
      expect(agentsMd).toContain("Role: Architect");
    } finally {
      fs.rmSync(home, { recursive: true, force: true });
    }
  });
});
