import { test, expect, type Page } from "@playwright/test";
import { openSurface, surfacePanel } from "../fixtures/workbench";
import { makeWorkbenchDocument } from "../fixtures/workbenchIpcMock";

/**
 * Screenshot evidence for the skill crown.
 *
 * Kept separate from `garden.spec.ts` because it needs a roster and a library
 * rich enough for a crown to say something — several agents across two classes,
 * a skill deployed directly, one inherited from a class, one global, and one
 * that fell back to a copy. The shared garden spec deliberately runs with an
 * empty library so its assertions stay about geometry.
 */
async function installCrownIpcMock(page: Page) {
  const workbenchDocument = makeWorkbenchDocument();
  await page.addInitScript((workbenchDocument) => {
    const agents = [
      ["hw-01", "Kicad Reviewer", "Architect", "D:/Trading/trident"],
      ["hw-02", "Board Layout", "Architect", "D:/Trading/trident"],
      ["web-01", "Docs Writer", "Coder", "D:/Development/Wardian"],
      ["web-02", "API Builder", "Coder", "D:/Development/Wardian"],
    ].map(([session_id, session_name, agent_class, folder]) => ({
      session_id,
      session_name,
      agent_class,
      folder,
      provider: "claude",
      is_off: false,
    }));

    const skillEntry = (name: string, path: string) => ({
      kind: "skill",
      entry_ref: `skills/${path}`,
      path,
      name,
      description: "",
      tags: [],
      is_starred: false,
      deployment_count: 1,
    });

    const libraryIndex = {
      sections: {
        skills: {
          stubbed: false,
          tree: {
            path: "",
            name: "Root",
            children: [
              skillEntry("KiCad Review", "kicad-review"),
              skillEntry("Trident LEAPS Automation", "trident-leaps-automation"),
              skillEntry("Trident LEAPS Refresh", "trident-leaps-refresh"),
              skillEntry("Spec Writer", "spec-writer"),
              skillEntry("Repo Conventions", "repo-conventions"),
            ],
          },
        },
      },
      deployments: {
        "skills/kicad-review": [{ target_type: "agent", target_id: "hw-01", linked: true }],
        "skills/trident-leaps-automation": [
          { target_type: "agent", target_id: "hw-01", linked: true },
          { target_type: "agent", target_id: "hw-02", linked: false },
        ],
        "skills/trident-leaps-refresh": [
          { target_type: "agent", target_id: "hw-01", linked: true },
        ],
        // Inherited by every Architect, so it is ringed rather than solid.
        "skills/spec-writer": [{ target_type: "class", target_id: "Architect", linked: true }],
        // On everyone, so IDF is 0 and it sinks to the end of every crown.
        "skills/repo-conventions": [{ target_type: "user", target_id: "global", linked: true }],
      },
      orphans: [],
    };

    let callbackId = 1;
    const tauriWindow = window as Window & {
      __TAURI_INTERNALS__?: Record<string, unknown>;
      __TAURI_EVENT_PLUGIN_INTERNALS__?: Record<string, unknown>;
    };
    tauriWindow.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => undefined };

    tauriWindow.__TAURI_INTERNALS__ = {
      metadata: {
        currentWindow: { label: "main" },
        currentWebview: { label: "main" },
      },
      plugins: {},
      transformCallback: (callback: unknown) => {
        const id = callbackId++;
        (window as unknown as Record<string, unknown>)[`_${id}`] = callback;
        return id;
      },
      invoke: async (command: string, args?: Record<string, unknown>) => {
        if (command === "list_agents") return agents;
        if (command === "get_library_index") return libraryIndex;
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
          // Echo the *proposed* revision; the adapter rejects a save whose
          // durable revision does not match what it sent.
          const proposed = args?.document as { revision?: number } | undefined;
          const revision = proposed?.revision ?? workbenchDocument.revision;
          return {
            outcome: "saved",
            durable_revision: revision,
            durable_token: `mock-token-${revision}`,
            request_id: args?.request_id,
          };
        }
        if (command === "list_agent_classes") {
          return [
            { name: "Architect", description: "", is_default: false },
            { name: "Coder", description: "", is_default: true },
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
        if (command === "automation_list_blueprints") {
          return { blueprints: [
            { id: "trident-alerts", path: "/w/library/automations/trident/trident-alerts.md" },
            { id: "trident-scan", path: "/w/library/automations/trident/trident-scan.md" },
            { id: "autoreview", path: "/w/library/automations/autoreview.md" },
          ], truncated: false, next_offset: null };
        }
        if (command === "automation_parse") {
          const path = String(args?.path ?? "");
          // The Trident blueprints name the directory they operate on, which is
          // the same workspace two agents live in; the loose one names nothing.
          const trident = path.includes("/trident/");
          return {
            blueprint: {
              schema: 2,
              id: path.split("/").pop()?.replace(".md", ""),
              name: trident ? "Trident " + (path.includes("scan") ? "Scan" : "Alerts") : "Autoreview",
              nodes: trident
                ? [
                    { id: "t", type: "manual_trigger" },
                    {
                      id: "c",
                      type: "shell",
                      // Forward slashes on purpose: a backslash does not survive
                      // serialization into addInitScript, and the normalizer
                      // accepts either form. Backslash handling is covered by
                      // the unit tests in automationContext.test.ts.
                      fields: { command: "python alerts.py", cwd: "D:/Trading/trident" },
                    },
                  ]
                : [{ id: "t", type: "manual_trigger" }],
              edges: [],
            },
          };
        }
        if (command === "automation_list_runs") return { runs: [], truncated: false, next_offset: null };
        if (command === "list_scheduled_runs") return [];
        if (command === "load_automation_library") return { folders: [], rootAutomationIds: [] };
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

test.describe("Garden skill crown", () => {
  test("draws a crown per agent and reports a selected skill's reach", async ({ browser }) => {
    const page = await browser.newPage();
    await installCrownIpcMock(page);
    await page.goto("/", { waitUntil: "domcontentloaded" });
    await page.locator('[data-testid="app-shell"]').waitFor({ timeout: 15_000 });

    await openSurface(page, "garden");
    const garden = surfacePanel(page, "garden");
    await expect(garden.locator(".garden-canvas canvas")).toBeVisible({ timeout: 10_000 });
    // Let the layout settle before capturing.
    await page.waitForTimeout(1_000);

    if (process.env.WARDIAN_GARDEN_SCREENSHOT) {
      await garden.screenshot({
        path: process.env.WARDIAN_GARDEN_SCREENSHOT,
        animations: "disabled",
      });
    }

    // Workspace territories come from agents. Undeployed blueprints do not
    // acquire independent Garden objects merely by naming a workspace.
    const districts = await page.evaluate(
      () =>
        JSON.parse(localStorage.getItem("wardian-garden") ?? "{}")?.state?.scene?.districts
          ?.cells ?? {},
    );
    expect(Object.keys(districts)).toContain("workspace:d:/trading/trident");

    await expect(garden.getByTestId("garden-selection-summary")).toContainText(
      "Select to inspect",
    );
    await expect(garden.locator('[data-garden-object^="automation:"]')).toHaveCount(0);
    await garden.locator('[data-garden-object="agent:hw-01"]').press("Enter");
    const capabilities = garden.getByRole("region", { name: "Skills", exact: true });
    await expect(capabilities).toContainText("KiCad Review");
    await expect(capabilities).toContainText("Class-inherited");
    await page.close();
  });
});
