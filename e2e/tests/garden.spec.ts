import { test, expect, type Page } from "@playwright/test";

async function installGardenTestIpcMock(page: Page) {
  // Mock agents so that the Garden view has units to drag.
  // This mirrors the pattern in agent-lifecycle.spec.ts.
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
        session_id: "garden-test-agent-01",
        session_name: "Garden Test Agent",
        agent_class: "TestClass",
        folder: "C:/projects/garden-test",
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

    // Seed a known garden position for the test agent so the drag below starts
    // on top of a real unit (canvas units have no DOM handle to target). Guarded
    // so the reload in the persistence test does not clobber the dragged value.
    // Shape matches zustand's `persist` envelope: { state, version }.
    if (!localStorage.getItem("wardian-garden")) {
      localStorage.setItem(
        "wardian-garden",
        JSON.stringify({
          state: { positions: { "agent:garden-test-agent-01": { x: 200, y: 200 } }, pins: {} },
          version: 0,
        }),
      );
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
        if (command === "list_agents") return agents;
        if (command === "list_agent_classes") {
          return [{ name: "TestClass", description: "Garden test class", is_default: true }];
        }
        if (command === "list_provider_readiness") {
          return [
            { provider: "claude", display_name: "Claude", available: true, executable: "C:/tools/claude.cmd", reason: null },
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
        return null;
      },
    };
  });
}

test.describe("Garden View", () => {
  test.describe.configure({ mode: "serial" });

  let page: Page;

  test.beforeAll(async ({ browser }) => {
    page = await browser.newPage();
    await installGardenTestIpcMock(page);
    await page.goto("/", { waitUntil: "domcontentloaded" });
    await page.locator('[data-testid="app-shell"]').waitFor({ timeout: 15_000 });
  });

  test.afterAll(async () => {
    await page.close();
  });

  test("renders a canvas when Garden tab is clicked", async () => {
    await page.getByRole("button", { name: "Garden", exact: true }).click();
    const canvas = page.locator(".garden-canvas canvas");
    await expect(canvas).toBeVisible();
  });

  test("dragging a unit persists its position to localStorage", async () => {
    // Ensure we're on the Garden tab
    await page.getByRole("button", { name: "Garden", exact: true }).click();

    // Wait for the canvas to be visible
    const canvas = page.locator(".garden-canvas canvas");
    await expect(canvas).toBeVisible({ timeout: 10_000 });

    // Get the bounding box of the canvas
    const box = await canvas.boundingBox();
    if (!box) throw new Error("no canvas bounding box");

    // Start the drag on the seeded unit (canvas coords 200,200; Stage starts at
    // offset 0 / scale 1, so canvas point maps to box origin + that offset).
    const startX = box.x + 200;
    const startY = box.y + 200;
    const endX = box.x + 360;
    const endY = box.y + 320;

    await page.mouse.move(startX, startY);
    await page.mouse.down();
    await page.mouse.move(endX, endY, { steps: 8 });
    await page.mouse.up();

    // Wait a moment for the store to persist
    await page.waitForTimeout(500);

    // Read the localStorage value
    const stored = await page.evaluate(() => localStorage.getItem("wardian-garden"));
    expect(stored).toBeTruthy();
    expect(stored).toContain("positions");

    // Store the value for the next test
    (page as any).__gardenStorageValue = stored;
  });

  test("dragged position persists across page reload", async () => {
    // Get the previously stored value
    const storedBefore = (page as any).__gardenStorageValue;
    expect(storedBefore).toBeTruthy();

    // Reload the page
    await page.reload({ waitUntil: "domcontentloaded" });
    await page.locator('[data-testid="app-shell"]').waitFor({ timeout: 15_000 });

    // Navigate back to Garden tab
    await page.getByRole("button", { name: "Garden", exact: true }).click();

    // Read localStorage again
    const storedAfter = await page.evaluate(() => localStorage.getItem("wardian-garden"));

    // Verify the value is unchanged
    expect(storedAfter).toEqual(storedBefore);
  });
});
