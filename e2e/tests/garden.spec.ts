import { test, expect, type Page } from "@playwright/test";
import { openSurface, surfacePanel } from "../fixtures/workbench";
import { makeWorkbenchDocument } from "../fixtures/workbenchIpcMock";

async function installGardenTestIpcMock(page: Page) {
  const workbenchDocument = makeWorkbenchDocument();
  // Mock agents so that the Garden view has units to drag.
  // This mirrors the pattern in agent-lifecycle.spec.ts.
  await page.addInitScript((workbenchDocument) => {
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

    // No seeded geometry. Positions are derived from the metric now, and the v1
    // envelope this used to write is discarded by the store's migration anyway —
    // it left the drag below landing on empty canvas. The drag test reads the
    // unit's real position out of the persisted scene instead.

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
        if (command === "get_workbench_boot_config")
          return { safe_mode: false };
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
            {
              name: "TestClass",
              description: "Garden test class",
              is_default: true,
            },
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
        if (command === "load_onboarding_hints") {
          return { dismissed_hint_ids: ["spawn-agent-first-run:v1"] };
        }
        if (command === "dismiss_onboarding_hint") {
          return { dismissed_hint_ids: ["spawn-agent-first-run:v1"] };
        }
        if (command === "list_automations" || command === "schedule_list") return [];
        if (command === "automation_list_blueprints") return { blueprints: [], truncated: false, next_offset: null };
        if (command === "automation_list_runs") return { runs: [], truncated: false, next_offset: null };
        if (command === "list_scheduled_runs") return [];
        if (command === "load_automation_library")
          return { folders: [], rootAutomationIds: [] };
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
  }, workbenchDocument);
}

test.describe("Garden View", () => {
  test.describe.configure({ mode: "serial" });

  let page: Page;
  let gardenStorageValue: string | null = null;

  test.beforeAll(async ({ browser }) => {
    page = await browser.newPage();
    await installGardenTestIpcMock(page);
    await page.goto("/", { waitUntil: "domcontentloaded" });
    await page
      .locator('[data-testid="app-shell"]')
      .waitFor({ timeout: 15_000 });
  });

  test.afterAll(async () => {
    await page.close();
  });

  test("renders a canvas when Garden tab is clicked", async () => {
    await openSurface(page, "garden");
    const garden = surfacePanel(page, "garden");
    const canvas = garden.locator(".garden-canvas canvas");
    await expect(canvas).toBeVisible();
    await expect(
      garden.getByRole("region", { name: "Garden status legend" }),
    ).toContainText("Action Required");
    await expect(garden.getByTestId("garden-selection-summary")).toContainText(
      "Select to inspect",
    );

    if (process.env.WARDIAN_GARDEN_SCREENSHOT) {
      await garden.screenshot({
        path: process.env.WARDIAN_GARDEN_SCREENSHOT,
        animations: "disabled",
      });
    }
  });

  test("dragging a unit persists its position to localStorage", async () => {
    await openSurface(page, "garden");
    // Authored placement is a Workstream action under semantic composition.
    await surfacePanel(page, "garden").locator('[data-garden-object="district:workspace:c:/projects/garden-test"]').press("Enter");

    // Wait for the canvas to be visible
    const canvas = surfacePanel(page, "garden").locator(
      ".garden-canvas canvas",
    );
    await expect(canvas).toBeVisible({ timeout: 10_000 });

    // Wait for the district camera to settle, then use the accessible object's
    // screen location to target the underlying canvas inhabitant.
    const container = surfacePanel(page, "garden").locator(".garden-canvas");
    let transform: string | null = null;
    await expect
      .poll(
        async () => {
          const next = await container.getAttribute("data-garden-fit");
          const stable = next !== null && next === transform;
          transform = next;
          return stable;
        },
        { timeout: 10_000 },
      )
      .toBe(true);

    const box = await canvas.boundingBox();
    if (!box) throw new Error("no canvas bounding box");

    const hit = await surfacePanel(page, "garden").locator('[data-garden-object="agent:garden-test-agent-01"]').boundingBox();
    if (!hit) throw new Error("no agent hit target");
    const startX = hit.x + hit.width / 2;
    const startY = hit.y + hit.height / 2;
    const endX = startX + 40;
    const endY = startY + 30;

    // Released explicitly: a drag is committed on `dragend` now, not on every
    // intermediate move. Committing per move re-pinned the unit and re-ran the
    // whole layout on each mouse event, so the map re-solved and slid under the
    // cursor while the user was still dragging.
    await page.mouse.move(startX, startY);
    await expect(
      surfacePanel(page, "garden").locator(".konvajs-content"),
    ).toHaveCSS("cursor", "pointer");
    await page.mouse.down();
    await page.mouse.move(endX, endY, { steps: 8 });
    await page.mouse.up();

    // Wait a moment for the store to persist
    await page.waitForTimeout(500);

    // A drag is an authored placement, so it must land in `pins` — the layer
    // that outranks the metric. `positions` alone proves nothing: the layout
    // writes warm-start seeds there on every pass whether or not a drag landed.
    const scene = await page.evaluate(
      () => JSON.parse(localStorage.getItem("wardian-garden") ?? "{}")?.state?.scene,
    );
    expect(Object.keys(scene?.pins ?? {})).toContain("agent:garden-test-agent-01");

    // Store the value for the next test.
    gardenStorageValue = await page.evaluate(() =>
      localStorage.getItem("wardian-garden"),
    );
  });

  test("dragged position persists across page reload", async () => {
    // Get the previously stored value
    const storedBefore = gardenStorageValue;
    expect(storedBefore).toBeTruthy();

    // Reload the page
    await page.reload({ waitUntil: "domcontentloaded" });
    await page
      .locator('[data-testid="app-shell"]')
      .waitFor({ timeout: 15_000 });

    await openSurface(page, "garden");

    // Read localStorage again
    const storedAfter = await page.evaluate(() =>
      localStorage.getItem("wardian-garden"),
    );

    // Verify the value is unchanged
    expect(storedAfter).toEqual(storedBefore);
  });

  test("right-click offers Reset layout and clears persisted positions", async () => {
    await openSurface(page, "garden");
    const garden = surfacePanel(page, "garden");
    const canvas = garden.locator(".garden-canvas canvas");
    await expect(canvas).toBeVisible();

    const box = await canvas.boundingBox();
    if (!box) throw new Error("no canvas bounding box");

    // Open near the canvas' left edge so the fixed-width portal cannot extend
    // beneath the persistent roster and lose pointer hit-testing.
    await page.mouse.move(box.x + 120, box.y + 60);
    await page.mouse.down({ button: "right" });
    await page.mouse.up({ button: "right" });
    // The context menu is rendered in a document-level portal so it can escape
    // the clipped canvas/workbench panel.
    await expect(
      page.locator('[data-testid="garden-context-menu"]'),
    ).toBeVisible();

    await page.locator('[data-testid="garden-reset-layout"]').click();
    await page.waitForTimeout(200);

    // Reset clears the authored layer. `positions` is not the thing to assert
    // on: those are warm-start seeds, and the layout legitimately rewrites them
    // on the very next pass.
    const parsed = await page.evaluate(() =>
      JSON.parse(localStorage.getItem("wardian-garden") ?? "{}"),
    );
    expect(parsed.state.scene.pins).toEqual({});
  });
});
