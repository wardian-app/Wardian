import { test, expect, type Page } from "@playwright/test";
import { existsSync, mkdirSync } from "node:fs";
import path from "node:path";
import { openSurface, surfacePanel } from "../fixtures/workbench";
import { makeWorkbenchDocument } from "../fixtures/workbenchIpcMock";

/**
 * PR evidence for the Garden file terrain, its change paint, and district
 * centrality.
 *
 * Deliberately not an assertion suite — `src/features/garden/*.test.ts` owns the
 * geometry and this cannot see a canvas anyway. It exists to render the state
 * the PR changes, on a roster wide enough for the arrangement to say something:
 * one coordinating workspace that writes into two others, so the lattice has a
 * reason to seat it inward.
 */

const SCREENSHOT_DIR = "e2e/screenshots/garden";

function screenshotTimestamp(): string {
  return new Date().toISOString().replace(/[:.]/g, "-");
}

const ROOTS = {
  academic: "C:/work/academic",
  papers: "C:/work/papers",
  bench: "C:/work/bench",
} as const;

async function installMock(page: Page) {
  const workbenchDocument = makeWorkbenchDocument();
  await page.addInitScript(
    ({ workbenchDocument, roots }) => {
      const agents = [
        {
          session_id: "manager",
          session_name: "Academic-Manager",
          agent_class: "Architect",
          folder: roots.academic,
          provider: "claude",
          is_off: false,
        },
        {
          session_id: "writer",
          session_name: "Paper-Writer",
          agent_class: "Coder",
          folder: roots.papers,
          provider: "claude",
          is_off: false,
        },
        {
          session_id: "reviewer",
          session_name: "Paper-Reviewer",
          agent_class: "Coder",
          folder: roots.papers,
          provider: "claude",
          is_off: false,
        },
        {
          session_id: "bencher",
          session_name: "Bench-Runner",
          agent_class: "Coder",
          folder: roots.bench,
          provider: "claude",
          is_off: false,
        },
      ];

      // One listing per root, deep enough that the treemap subdivides and the
      // folder weighting is visible against loose files.
      const treeFor = (target: string) => {
        const normalized = String(target).replace(/\\/g, "/").replace(/\/$/, "");
        const leaf = (name: string, isDir: boolean) => ({
          name,
          path: `${normalized}/${name}`,
          is_dir: isDir,
          extension: isDir ? null : name.split(".").pop() ?? null,
        });
        if (normalized.endsWith("/src")) {
          return [
            leaf("components", true),
            leaf("features", true),
            leaf("index.ts", false),
            leaf("model.ts", false),
            leaf("view.ts", false),
            leaf("utils.ts", false),
            leaf("types.ts", false),
          ];
        }
        if (normalized.endsWith("/docs")) {
          return [
            leaf("guide.md", false),
            leaf("api.md", false),
            leaf("design.md", false),
          ];
        }
        if (normalized.endsWith("/components") || normalized.endsWith("/features")) {
          return [
            leaf("panel.tsx", false),
            leaf("list.tsx", false),
            leaf("row.tsx", false),
          ];
        }
        return [
          leaf("src", true),
          leaf("docs", true),
          leaf("tests", true),
          leaf("README.md", false),
          leaf("package.json", false),
          leaf("LICENSE", false),
        ];
      };

      const changedFor = (cwd: string) => {
        const normalized = String(cwd).replace(/\\/g, "/").replace(/\/$/, "");
        const file = (
          relative: string,
          kind: string,
          insertions: number,
          agentIds: string[],
        ) => ({
          path: relative,
          change_kind: kind,
          old_path: null,
          insertions,
          deletions: 2,
          evidence: "attributed",
          agent_ids: agentIds,
          turn_indices: [4],
          binary: false,
          truncated: false,
          reviewed: false,
        });
        // Only a few paths change, so the paint reads as a claim about *where*
        // work happened rather than tinting the whole plot one colour.
        if (normalized === roots.papers) {
          return [
            file("src/model.ts", "modified", 64, ["writer", "manager"]),
            file("src/components/panel.tsx", "added", 30, ["manager"]),
            file("docs/api.md", "modified", 8, ["reviewer"]),
          ];
        }
        if (normalized === roots.bench) {
          return [file("src/utils.ts", "modified", 12, ["bencher", "manager"])];
        }
        return [file("src/features/row.tsx", "modified", 40, ["manager"])];
      };

      const callbacks = new Map<number, unknown>();
      let callbackId = 1;
      const tauriWindow = window as Window & {
        __TAURI_INTERNALS__?: Record<string, unknown>;
        __TAURI_EVENT_PLUGIN_INTERNALS__?: Record<string, unknown>;
      };
      tauriWindow.__TAURI_EVENT_PLUGIN_INTERNALS__ = {
        unregisterListener: () => undefined,
      };

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
          if (command === "list_agents") return agents;
          if (command === "get_directory_tree") return treeFor(String(args?.path ?? ""));
          if (command === "load_change_review_prefs")
            return { schema: 1, baseline: "branch_point" };
          if (command === "load_change_review") {
            const request = (args?.request ?? {}) as { cwd?: string };
            const cwd = String(request.cwd ?? "");
            return {
              summary: {
                schema: 1,
                baseline: "branch_point",
                baseline_ref: "abc1234",
                from_turn_index: null,
                to_turn_index: 9,
                files: changedFor(cwd),
                computed_at: new Date().toISOString(),
                truncated: false,
                baseline_diverged: false,
              },
              git_available: true,
              head_ref: "abc1234",
              workspace_root: cwd.replace(/\\/g, "/").replace(/\/$/, ""),
              skipped_turn_records: 0,
            };
          }
          // The coordinator writes into both of the other workspaces, which is
          // what earns it a seat nearer the middle.
          if (command === "load_agent_reach") {
            return {
              schema: 1,
              agents: [
                {
                  agent_id: "manager",
                  roots: [roots.academic, roots.papers, roots.bench],
                },
                { agent_id: "writer", roots: [roots.papers] },
                { agent_id: "bencher", roots: [roots.bench] },
              ],
              skipped_turn_records: 0,
            };
          }
          if (command === "get_workbench_boot_config") return { safe_mode: false };
          if (command === "load_workbench_state") {
            return {
              source: "default",
              document: workbenchDocument,
              notice: null,
              durable_revision: workbenchDocument.revision,
              durable_token: `mock-token-${workbenchDocument.revision}`,
            };
          }
          if (command === "save_workbench_state") {
            const document = args?.document as { revision?: number } | undefined;
            const revision = document?.revision ?? workbenchDocument.revision;
            return {
              outcome: "saved",
              durable_revision: revision,
              durable_token: `mock-token-${revision}`,
              request_id: args?.request_id,
            };
          }
          if (command === "list_agent_classes") {
            return [
              { name: "Architect", description: "Plans", is_default: false },
              { name: "Coder", description: "Builds", is_default: true },
            ];
          }
          if (command === "list_provider_readiness") {
            return [
              {
                provider: "claude",
                display_name: "Claude",
                available: true,
                executable: "C:/tools/claude.cmd",
                reason: null,
              },
            ];
          }
          if (command === "load_watchlists") return [];
          if (command === "load_watchlist_prefs") return null;
          if (command === "load_agent_interactions") return {};
          if (command === "load_queue_items") return [];
          if (command === "load_onboarding_hints")
            return { dismissed_hint_ids: ["spawn-agent-first-run:v1"] };
          if (command === "dismiss_onboarding_hint")
            return { dismissed_hint_ids: ["spawn-agent-first-run:v1"] };
          if (command === "list_automations" || command === "schedule_list") return [];
          if (command === "memory_list") return [];
          if (command === "list_conversations") return { schema: 1, conversations: [] };
          if (command === "automation_list_blueprints") return { blueprints: [], truncated: false, next_offset: null };
          if (command === "automation_list_runs") return { runs: [], truncated: false, next_offset: null };
          if (command === "list_scheduled_runs") return [];
          if (command === "load_automation_library")
            return { folders: [], rootAutomationIds: [] };
          if (command === "get_library_tree")
            return { type: "Folder", path: "", name: "Root", children: [] };
          if (command === "list_deployed_skills") return [];
          if (command === "plugin:event|listen") return callbackId++;
          if (command === "plugin:event|unlisten") return null;
          if (command === "sync_provider_theme_settings") return null;
          return null;
        },
      };
    },
    { workbenchDocument, roots: ROOTS },
  );
}

test("captures district ground, change paint, and the centrality legend", async ({
  page,
}) => {
  await installMock(page);
  await page.goto("/", { waitUntil: "domcontentloaded" });
  await page.locator('[data-testid="app-shell"]').waitFor({ timeout: 15_000 });

  await openSurface(page, "garden");
  const garden = surfacePanel(page, "garden");
  await expect(garden.locator(".garden-canvas canvas")).toBeVisible({ timeout: 15_000 });

  const legend = garden.getByRole("region", { name: "Garden status legend" });
  // The paint legend only appears once change review has landed for a root, so
  // waiting on it is also the wait for terrain to have been ingested.
  await expect(legend).toContainText("Ground", { timeout: 15_000 });
  // The claim this PR adds: distance from the centre is about coordination.
  await expect(legend).toContainText("Centre = coordinates others");

  const stamp = screenshotTimestamp();
  const capture = async (name: string) => {
    const target = path.resolve(SCREENSHOT_DIR, stamp, name);
    mkdirSync(path.dirname(target), { recursive: true });
    await garden.screenshot({ path: target, animations: "disabled" });
    expect(existsSync(target)).toBe(true);
    console.log(`Garden PR screenshot evidence: ${target}`);
  };

  await capture("garden-districts-ground-and-centrality.png");

  // Workspace composition reveals activity ancestry rather than recursively
  // drawing unchanged filesystem contents as the camera magnifies.
  await garden.locator('[data-garden-object="agent:writer"]').press("Enter");
  await garden.getByRole("region", { name: "Workspace, Teams, Agents" }).getByRole("button", { name: /work\/papers/ }).press("Enter");
  await expect(garden.getByRole("region", { name: "Workspace activity" })).toBeVisible();
  await expect(garden.getByRole("checkbox", { name: "Show full tree" })).not.toBeChecked();
  await capture("garden-workspace-activity.png");
});
