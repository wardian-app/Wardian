/**
 * Agent Lifecycle E2E tests.
 *
 * Tests are split into two groups:
 *   1. UI-only (browser E2E): spawn form interaction, validation, empty-state guards.
 *   2. @native-only: tests that require an actually-running mock agent.
 *      These are skipped here and belong in the native E2E harness.
 *      Run via: npm run test:e2e:native
 *
 * Why @native-only? The mock provider is not exposed in the spawn form dropdown,
 * and status indicator transitions require real PTY/IPC events from the backend.
 * See e2e/fixtures/mockAgent.ts for the unlock path.
 */

import { test, expect, type Page } from "@playwright/test";
import * as path from "path";
import { openSurface, surfacePanel } from "../fixtures/workbench";

async function installCustomCloneIpcMock(page: Page) {
  await page.addInitScript(() => {
    type Agent = {
      session_id: string;
      session_name: string;
      agent_class: string;
      folder: string;
      provider: string;
      is_off: boolean;
    };

    const agents: Agent[] = [
      {
        session_id: "mock-session-e2e-001",
        session_name: "E2E Mock Agent",
        agent_class: "TestClass",
        folder: "C:/projects/e2e",
        provider: "claude",
        is_off: false,
      },
    ];
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
        if (command === "list_agent_classes") {
          return [{ name: "TestClass", description: "E2E test class", is_default: true }];
        }
        if (command === "list_provider_readiness") {
          return [
            { provider: "claude", display_name: "Claude", available: true, executable: "C:/tools/claude.cmd", reason: null },
            { provider: "codex", display_name: "Codex", available: true, executable: "C:/tools/codex.cmd", reason: null },
            { provider: "gemini", display_name: "Gemini", available: true, executable: "C:/tools/gemini.cmd", reason: null },
            { provider: "antigravity", display_name: "Antigravity", available: true, executable: "C:/tools/agy.exe", reason: null },
            { provider: "opencode", display_name: "OpenCode", available: true, executable: "C:/tools/opencode.cmd", reason: null },
          ];
        }
        if (command === "load_watchlists") return [];
        if (command === "load_watchlist_prefs") return null;
        if (command === "load_agent_interactions") return {};
        if (command === "load_queue_items") return [];
        if (command === "load_onboarding_hints") {
          return { dismissed_hint_ids: ["spawn-agent-first-run:v1"] };
        }
        if (command === "dismiss_onboarding_hint") {
          return { dismissed_hint_ids: ["spawn-agent-first-run:v1"] };
        }
        if (command === "list_workflows") return [];
        if (command === "list_scheduled_runs") return [];
        if (command === "load_workflow_library") return { folders: [], rootWorkflowIds: [] };
        if (command === "get_library_tree") {
          return { type: "Folder", path: "", name: "Root", children: [] };
        }
        if (command === "list_deployed_skills") return [];
        if (command === "plugin:event|listen") return callbackId++;
        if (command === "plugin:event|unlisten") return null;
        if (command === "sync_provider_theme_settings") return null;
        if (command === "get_agent_clone_preview") {
          return {
            source_session_id: "mock-session-e2e-001",
            source_session_name: "E2E Mock Agent",
            suggested_session_name: "E2E Mock Agent-copy",
            provider: "claude",
            agent_class: "TestClass",
            folder: "C:/projects/e2e",
            files: {
              name: "mock-session-e2e-001",
              path: "",
              kind: "directory",
              children: [
                { name: "AGENTS.md", path: "AGENTS.md", kind: "file", children: [] },
                { name: "notes.md", path: "notes.md", kind: "file", children: [] },
              ],
            },
            default_selected_files: ["AGENTS.md", "notes.md"],
            skills: [],
            default_selected_skills: [],
          };
        }
        if (command === "clone_agent") {
          const request = args?.req as { session_name?: string } | undefined;
          agents.push({
            ...agents[0],
            session_id: "mock-session-e2e-clone",
            session_name: request?.session_name ?? "E2E Mock Agent-copy",
          });
          return agents[agents.length - 1];
        }
        return null;
      },
    };
  });
}

test.describe("Agent Spawn Form", () => {
  test.describe.configure({ mode: "serial" });

  let page: Page;

  test.beforeAll(async ({ browser }) => {
    page = await browser.newPage();
    await page.goto("/", { waitUntil: "domcontentloaded" });
    await page.locator('[data-testid="app-shell"]').waitFor({ timeout: 15_000 });
    await page.locator('[data-testid="sidebar-tab-agent-config"]').click();
    await page.locator('[data-testid="spawn-agent-name"]').waitFor();
  });

  test.afterAll(async () => {
    await page.close();
  });

  test("spawn form renders all required fields", async () => {
    await expect(page.locator('[data-testid="spawn-agent-name"]')).toBeVisible();
    await expect(page.locator('[data-testid="spawn-workspace-path"]')).toBeVisible();
    await expect(page.locator('[data-testid="spawn-provider"]')).toBeVisible();
    await expect(page.locator('[data-testid="spawn-submit"]')).toBeVisible();
  });

  test("submit button is present when name is filled", async () => {
    await page.locator('[data-testid="spawn-agent-name"]').fill("test-agent");
    await expect(page.locator('[data-testid="spawn-submit"]')).toBeVisible();
  });

  test("provider dropdown has expected options", async () => {
    const select = page.locator('[data-testid="spawn-provider"]');
    await expect(select).toBeVisible();
    const options = await select.locator("option").allTextContents();
    expect(options).toContain("Claude");
    expect(options).toContain("Antigravity");
    expect(options.length).toBeGreaterThanOrEqual(3);
  });

  test("workspace path field accepts input", async () => {
    const input = page.locator('[data-testid="spawn-workspace-path"]');
    await input.fill("C:/projects/test");
    await expect(input).toHaveValue("C:/projects/test");
  });

  test("grid is empty before any agent is spawned", async () => {
    await openSurface(page, "agents-overview");
    const overview = surfacePanel(page, "agents-overview");
    await overview.getByRole("button", { name: "Grid", exact: true }).click();
    const cards = overview.locator('[data-testid="agent-card"]');
    await expect(cards).toHaveCount(0);
  });

  test("watchlist is empty before any agent is spawned", async () => {
    const watchlist = page.locator('[data-testid="agent-watchlist"]');
    await expect(watchlist).toBeVisible();
    // No agent rows expected in empty state.
    const cards = page.locator('[data-testid="agent-card"]');
    await expect(cards).toHaveCount(0);
  });
});

test.describe("Custom Agent Clone", () => {
  test("anchors an agent context menu at the pointer in the document overlay", async ({ page }) => {
    await installCustomCloneIpcMock(page);
    await page.goto("/", { waitUntil: "domcontentloaded" });
    await page.locator('[data-testid="app-shell"]').waitFor({ timeout: 15_000 });

    const sourceRow = page.locator('[data-testid="agent-watchlist"] .watchlist-row', { hasText: "E2E Mock Agent" });
    await expect(sourceRow).toBeVisible();
    const rowBox = await sourceRow.boundingBox();
    if (!rowBox) throw new Error("Agent row has no bounding box");
    const pointer = { x: rowBox.x + 24, y: rowBox.y + rowBox.height / 2 };

    await page.mouse.click(pointer.x, pointer.y, { button: "right" });

    const menu = page.locator('[data-testid="agent-context-menu"]');
    await expect(menu).toBeVisible();
    const menuBox = await menu.boundingBox();
    if (!menuBox) throw new Error("Agent context menu has no bounding box");
    expect(menuBox.x).toBeGreaterThanOrEqual(pointer.x - 1);
    expect(menuBox.y).toBeGreaterThanOrEqual(pointer.y - 1);
    expect(await menu.evaluate((element) => element.parentElement === document.body)).toBe(true);

    await page.screenshot({
      path: path.join("e2e", "screenshots", "context-menu-positioning", "2026-07-20", "agent-menu-at-cursor.png"),
      animations: "disabled",
    });
  });

  test("opens the modal, changes file selection, and creates a clone row", async ({ page }) => {
    await installCustomCloneIpcMock(page);
    await page.goto("/", { waitUntil: "domcontentloaded" });
    await page.locator('[data-testid="app-shell"]').waitFor({ timeout: 15_000 });

    const watchlist = page.locator('[data-testid="agent-watchlist"]');
    const sourceRow = watchlist.locator(".watchlist-row", { hasText: "E2E Mock Agent" });
    await expect(sourceRow).toBeVisible();

    await sourceRow.click({ button: "right" });
    const menu = page.locator('[data-testid="agent-context-menu"]');
    await menu.getByRole("button", { name: "Clone" }).hover();
    await page.getByRole("button", { name: "Custom Clone" }).click();

    const modal = page.locator('[data-testid="custom-clone-modal"]');
    await expect(modal).toBeVisible();
    await expect(modal.getByLabel("Clone Name")).toHaveValue("E2E Mock Agent-copy");

    await modal.locator('[data-testid="custom-clone-file-notes-md"]').uncheck();
    await modal.locator('[data-testid="custom-clone-submit"]').click();

    await expect(watchlist.locator(".watchlist-row", { hasText: "E2E Mock Agent-copy" })).toBeVisible();
  });
});

// @native-only: the tests below require a running mock agent.
// The mock provider is not selectable via the spawn form dropdown.
// Unlock path: add "mock" to SpawnAgentPanel provider options, then
// use seededHome() fixture from e2e/fixtures/mockAgent.ts.
test.describe("Agent Status Transitions (@native-only)", () => {
  test.skip("spawning agent appears in watchlist with Processing status", async () => {
    // Spawn mock agent (basic scenario).
    // Assert agent card appears in watchlist.
    // Assert status indicator has cyan (Processing) color.
  });

  test.skip("agent transitions to Idle after turn completes", async () => {
    // Spawn mock agent (basic scenario).
    // Wait for turn_completed event.
    // Assert status indicator has emerald (Idle) color.
  });

  test.skip("action_needed scenario shows Amber status indicator", async () => {
    // Spawn mock agent (action_needed scenario).
    // Assert status indicator has amber color.
  });

  test.skip("killing agent removes it from watchlist and grid", async () => {
    // Spawn mock agent.
    // Trigger kill via UI.
    // Assert agent card count drops to 0.
  });
});
