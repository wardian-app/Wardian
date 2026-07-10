import { expect, test, type Page } from "@playwright/test";

type ProofModelSnapshot = {
  groups: Array<{ group_id: string; surface_ids: string[] }>;
};

async function installProofIpcMock(page: Page) {
  await page.addInitScript(() => {
    let callbackId = 1;
    const callbacks = new Map<number, unknown>();
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
      unregisterCallback: (id: number) => callbacks.delete(id),
      convertFileSrc: (filePath: string) => filePath,
      invoke: async (command: string) => {
        if (command === "list_agents") return [];
        if (command === "list_agent_classes") return [];
        if (command === "list_provider_readiness") return [];
        if (command === "load_watchlists") return [];
        if (command === "load_watchlist_prefs") return null;
        if (command === "load_agent_interactions") return {};
        if (command === "load_queue_items") return [];
        if (command === "load_queue_preferences") return {};
        if (command === "load_onboarding_hints") return { dismissed_hint_ids: [] };
        if (command === "dismiss_onboarding_hint") return { dismissed_hint_ids: [] };
        if (command === "list_workflows") return [];
        if (command === "list_scheduled_runs") return [];
        if (command === "load_workflow_library") return { folders: [], rootWorkflowIds: [] };
        if (command === "get_library_tree") return { type: "Folder", path: "", name: "Root", children: [] };
        if (command === "list_deployed_skills") return [];
        if (command === "load_app_settings") return null;
        if (command === "load_shell_settings") return null;
        if (command === "list_available_shells") return [];
        if (command === "get_topology") return { edges: [], ignored_pairs: [], fallback_groups: [] };
        if (command === "get_pair_activity") return [];
        if (command === "workflow_list_blueprints") return [];
        if (command === "workflow_list_runs") return [];
        if (command === "plugin:event|listen") return callbackId++;
        if (command === "plugin:event|unlisten") return null;
        return null;
      },
    };
  });
}

async function mountProof(page: Page, consoleErrors: string[], pageErrors: string[]) {
  await page.route("**/__workbench-proof-host.html", async (route) => {
    await route.fulfill({
      contentType: "text/html",
      body: `<!doctype html>
        <html>
          <head>
            <meta charset="UTF-8" />
            <meta name="viewport" content="width=device-width, initial-scale=1.0" />
            <link rel="stylesheet" href="/src/styles/App.css" />
          </head>
          <body>
            <div id="workbench-proof-root"></div>
            <script type="module">
              import RefreshRuntime from "/@react-refresh";
              RefreshRuntime.injectIntoGlobalHook(window);
              window.$RefreshReg$ = () => {};
              window.$RefreshSig$ = () => (type) => type;
              window.__vite_plugin_react_preamble_installed__ = true;
            </script>
            <script type="module">
              import { mountDockviewEvaluationHarness } from "/src/layout/workbench/proof/DockviewEvaluationHarness.tsx";
              mountDockviewEvaluationHarness(document.getElementById("workbench-proof-root"));
            </script>
          </body>
        </html>`,
    });
  });
  await page.goto("/__workbench-proof-host.html", { waitUntil: "domcontentloaded" });
  const proof = page.locator('[data-testid="workbench-proof"]');
  try {
    await expect(proof).toHaveAttribute("data-ready", "true", { timeout: 20_000 });
  } catch (error) {
    throw new Error([
      error instanceof Error ? error.message : String(error),
      `console errors: ${JSON.stringify(consoleErrors)}`,
      `page errors: ${JSON.stringify(pageErrors)}`,
    ].join("\n"));
  }
  return proof;
}

test("Dockview remains a Wardian-driven, replaceable workbench renderer", async ({ page }) => {
  const consoleErrors: string[] = [];
  const pageErrors: string[] = [];
  page.on("console", (message) => {
    if (message.type() === "error") consoleErrors.push(message.text());
  });
  page.on("pageerror", (error) => pageErrors.push(error.message));
  await installProofIpcMock(page);
  const proof = await mountProof(page, consoleErrors, pageErrors);

  await expect(proof.getByRole("tab")).toHaveCount(20);
  await expect(proof.locator('[data-testid^="proof-group-"]')).toHaveCount(4);
  await expect(proof).toHaveAttribute("data-layout-source", "wardian-model");

  const tabs = proof.getByRole("tab");
  await expect(tabs.filter({ hasText: "Terminal Owner" })).toHaveAttribute("aria-selected", "true");
  expect(await tabs.evaluateAll((elements) => elements.every((element) => element.getAttribute("role") === "tab"))).toBe(true);

  const terminalHosts = proof.locator('[data-testid^="proof-terminal-host-"]');
  await expect(terminalHosts).toHaveCount(4);
  await expect(proof.locator('[data-terminal-mode="owner"]')).toHaveCount(1);
  await expect(proof.locator('[data-terminal-mode="mirror"]')).toHaveCount(3);
  const terminalHostIds = await terminalHosts.evaluateAll((hosts) => hosts.map((host) => host.getAttribute("data-terminal-host-id")));
  expect(new Set(terminalHostIds).size).toBe(4);

  await expect(proof.locator('[data-testid="proof-surface-graph"]')).toHaveAttribute("data-visible", "false");
  await tabs.filter({ hasText: "Graph" }).click();
  await expect(proof.locator('[data-testid="proof-surface-graph"]')).toHaveAttribute("data-visible", "true");
  await expect(proof.locator('[data-testid="proof-graph-wrapper"]')).toBeAttached();
  await tabs.filter({ hasText: "Garden" }).click();
  await expect(proof.locator('[data-testid="proof-surface-garden"]')).toHaveAttribute("data-visible", "true");
  await expect(proof.locator('[data-testid="proof-garden-wrapper"]')).toBeAttached();

  const mountCountsBefore = await page.evaluate(() => {
    const runtime = window.__WARDIAN_WORKBENCH_PROOF__;
    if (!runtime) throw new Error("proof runtime is unavailable");
    return { ...runtime.metrics.surface_mounts };
  });

  await tabs.filter({ hasText: "Terminal Owner" }).click();
  await expect.poll(async () => page.evaluate(() => (
    window.__WARDIAN_WORKBENCH_PROOF__?.getModel().active_group_id
  ))).toBe("proof-group-1");
  await tabs.filter({ hasText: "Terminal Owner" }).focus();
  await page.keyboard.press("Control+]");
  await expect(tabs.filter({ hasText: "Graph" })).toHaveAttribute("aria-selected", "true");
  await page.keyboard.press("F6");
  const focusedGroupId = await page.evaluate(() => {
    const active = document.activeElement;
    return active?.querySelector<HTMLElement>("[data-group-id]")?.dataset.groupId
      ?? (active instanceof HTMLElement ? active.dataset.groupId : undefined);
  });
  expect(focusedGroupId).not.toBe("proof-group-1");

  await proof.locator('[data-testid="proof-surface-terminal-owner"]').evaluate((surface) => {
    window.__proofOwnerSurface = surface;
  });
  const ownerDragHandle = tabs.filter({ hasText: "Terminal Owner" });
  const mirrorTarget = tabs.filter({ hasText: "Terminal Mirror 1" });
  const sourceBox = await ownerDragHandle.boundingBox();
  const targetBox = await mirrorTarget.boundingBox();
  if (!sourceBox || !targetBox) throw new Error("Dockview drag handles are not measurable");
  await page.mouse.move(sourceBox.x + sourceBox.width / 2, sourceBox.y + sourceBox.height / 2);
  await page.mouse.down();
  await page.mouse.move(targetBox.x + targetBox.width / 2, targetBox.y + targetBox.height / 2, { steps: 16 });
  await page.mouse.up();
  await expect.poll(async () => page.evaluate(() => {
    const model = window.__WARDIAN_WORKBENCH_PROOF__?.getModel() as ProofModelSnapshot | undefined;
    return model?.groups.find((group) => group.group_id === "proof-group-2")?.surface_ids.includes("terminal-owner") ?? false;
  })).toBe(true);
  expect(await proof.locator('[data-testid="proof-surface-terminal-owner"]').evaluate(
    (surface) => window.__proofOwnerSurface === surface,
  )).toBe(true);

  await tabs.filter({ hasText: "Terminal Owner" }).focus();
  await page.keyboard.press("Alt+Shift+ArrowRight");
  await expect.poll(async () => page.evaluate(() => {
    const model = window.__WARDIAN_WORKBENCH_PROOF__?.getModel() as ProofModelSnapshot | undefined;
    return model?.groups.find((group) => group.group_id === "proof-group-2")?.surface_ids.includes("terminal-owner") ?? false;
  })).toBe(true);

  await tabs.filter({ hasText: "Graph" }).click();
  await expect.poll(async () => page.evaluate(() => (
    window.__WARDIAN_WORKBENCH_PROOF__?.getModel().active_group_id
  ))).toBe("proof-group-1");
  await tabs.filter({ hasText: "Graph" }).focus();
  const serializedBeforeZoom = await proof.locator('[data-testid="proof-model"]').textContent();
  await page.keyboard.press("Alt+Shift+z");
  await expect(proof).toHaveAttribute("data-zoomed-group-id", "proof-group-1");
  expect(await proof.locator('[data-testid="proof-model"]').textContent()).toBe(serializedBeforeZoom);
  await page.keyboard.press("Alt+Shift+z");
  await expect(proof).toHaveAttribute("data-zoomed-group-id", "none");

  await tabs.filter({ hasText: "Terminal Owner" }).focus();
  await page.keyboard.press("Alt+Shift+s");
  await expect(proof.locator('[data-testid^="proof-group-"]')).toHaveCount(5);
  await expect(proof.getByRole("tab")).toHaveCount(20);
  await tabs.filter({ hasText: "Terminal Owner" }).focus();
  await page.keyboard.press("Alt+Shift+c");
  await expect(proof.locator('[data-testid^="proof-group-"]')).toHaveCount(4);
  await expect(proof.getByRole("tab")).toHaveCount(20);

  const mountCountsAfter = await page.evaluate(() => {
    const runtime = window.__WARDIAN_WORKBENCH_PROOF__;
    if (!runtime) throw new Error("proof runtime is unavailable");
    return { ...runtime.metrics.surface_mounts };
  });
  expect(mountCountsAfter["terminal-owner"]).toBe(1);
  expect(mountCountsAfter.graph).toBe(1);
  expect(mountCountsAfter.garden).toBe(1);
  expect(mountCountsAfter).toEqual(mountCountsBefore);
  expect(pageErrors).toEqual([]);
  expect(consoleErrors).toEqual([]);
});

declare global {
  interface Window {
    __proofOwnerSurface?: Element;
    __WARDIAN_WORKBENCH_PROOF__?: {
      metrics: {
        surface_mounts: Record<string, number>;
      };
      getModel: () => ProofModelSnapshot;
    };
  }
}
