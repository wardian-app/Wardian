import { expect, test, type Page } from "@playwright/test";

type BlueprintNode = {
  id: string;
  type: string;
  name?: string;
  fields: Record<string, unknown>;
};

type Blueprint = {
  schema: number;
  id: string;
  name: string;
  nodes: BlueprintNode[];
  edges: unknown[];
};

async function installBuilderV2IpcMock(page: Page) {
  await page.addInitScript(() => {
    type BlueprintNode = {
      id: string;
      type: string;
      fields?: Record<string, unknown>;
    };
    type Blueprint = {
      nodes?: BlueprintNode[];
    };
    type Diagnostic = {
      severity: "error" | "warning";
      code: string;
      message: string;
      node?: string;
    };

    const callbacks = new Map<number, unknown>();
    let callbackId = 1;
    const tauriWindow = window as Window & {
      __builderV2Invokes?: Array<{ command: string; args?: Record<string, unknown> }>;
      __builderV2Writes?: Array<{ path: string; blueprint: Blueprint }>;
      __TAURI_INTERNALS__?: Record<string, unknown>;
      __TAURI_EVENT_PLUGIN_INTERNALS__?: Record<string, unknown>;
    };

    tauriWindow.__builderV2Invokes = [];
    tauriWindow.__builderV2Writes = [];

    const validateBlueprint = (blueprint?: Blueprint) => {
      const diagnostics: Diagnostic[] = [];
      for (const node of blueprint?.nodes ?? []) {
        if (node.type === "task" && String(node.fields?.prompt ?? "").trim() === "") {
          diagnostics.push({
            severity: "error",
            code: "missing_required_field",
            message: `node \`${node.id}\` missing \`prompt\``,
            node: node.id,
          });
        }
      }
      return diagnostics;
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
        tauriWindow.__builderV2Invokes?.push({ command, args });

        if (command === "list_agents") return [];
        if (command === "list_agent_classes") return [];
        if (command === "list_provider_readiness") return [];
        if (command === "load_watchlists") return [];
        if (command === "load_watchlist_prefs") return null;
        if (command === "load_agent_interactions") return {};
        if (command === "load_queue_items") return [];
        if (command === "load_onboarding_hints") return { dismissed_hint_ids: ["spawn-agent-first-run:v1"] };
        if (command === "dismiss_onboarding_hint") return { dismissed_hint_ids: ["spawn-agent-first-run:v1"] };
        if (command === "list_workflows") return [];
        if (command === "list_scheduled_runs") return [];
        if (command === "load_workflow_library") return { folders: [], rootWorkflowIds: [] };
        if (command === "get_library_tree") return { type: "Folder", path: "", name: "Root", children: [] };
        if (command === "list_deployed_skills") return [];
        if (command === "plugin:event|listen") return callbackId++;
        if (command === "plugin:event|unlisten") return null;
        if (command === "sync_provider_theme_settings") return null;

        if (command === "workflow_validate") {
          const diagnostics = validateBlueprint(args?.blueprint as Blueprint | undefined);
          return { ok: diagnostics.length === 0, diagnostics };
        }
        if (command === "workflow_write") {
          const blueprint = args?.blueprint as Blueprint | undefined;
          const diagnostics = validateBlueprint(blueprint);
          if (diagnostics.length > 0 || !blueprint) {
            return { written: false, diagnostics };
          }
          tauriWindow.__builderV2Writes?.push({
            path: String(args?.path),
            blueprint,
          });
          return { written: true, diagnostics: [] };
        }

        return null;
      },
    };
  });
}

async function openBuilderV2(page: Page) {
  await page.setViewportSize({ width: 1700, height: 980 });
  await page.goto("/", { waitUntil: "domcontentloaded" });
  await page.locator('[data-testid="app-shell"]').waitFor({ timeout: 15_000 });
  await page
    .locator(".titlebar-center")
    .getByRole("button", { name: "Blueprints" })
    .click();
  await expect(page.locator('[data-testid="builder-v2"]')).toBeVisible();
  await page.evaluate(async () => {
    const { useBuilderStore } = await import("/src/store/useBuilderStore.ts");
    useBuilderStore.setState({ path: "C:/tmp/builder-v2-e2e.md" });
  });
}

async function addNode(page: Page, name: string) {
  await page.locator('[data-testid="node-palette"]').getByRole("button", { name }).click();
  const node = page.locator(".react-flow__node").filter({ hasText: name }).last();
  await expect(node).toHaveCount(1);
  return node;
}

async function connectBlueprintNodes(page: Page) {
  await page.evaluate(async () => {
    const { useBuilderStore } = await import("/src/store/useBuilderStore.ts");
    const state = useBuilderStore.getState();
    const blueprint = state.blueprint;
    if (!blueprint) return;
    const trigger = blueprint.nodes.find((node) => node.type === "manual_trigger");
    const task = blueprint.nodes.find((node) => node.type === "task");
    if (!trigger || !task) return;
    state.setBlueprint({
      ...blueprint,
      edges: [{ from: trigger.id, to: task.id, from_port: "out", to_port: "in" }],
    });
  });
}

test("v2 builder authors, validates, and saves a workflow blueprint", async ({ page }) => {
  await installBuilderV2IpcMock(page);
  await openBuilderV2(page);

  await addNode(page, "Manual Trigger");
  await addNode(page, "Task");
  await connectBlueprintNodes(page);
  await page.evaluate(async () => {
    const { useBuilderStore } = await import("/src/store/useBuilderStore.ts");
    await useBuilderStore.getState().validate();
  });

  await expect(page.getByLabel(/Prompt/i)).toBeVisible();
  await expect(page.getByText("missing_required_field")).toBeVisible({ timeout: 5_000 });
  await expect(page.getByRole("button", { name: "Save" })).toBeDisabled();

  await page.getByLabel(/Prompt/i).fill("Plan the work.");
  await page.evaluate(async () => {
    const { useBuilderStore } = await import("/src/store/useBuilderStore.ts");
    await useBuilderStore.getState().validate();
  });

  await expect(page.getByText("missing_required_field")).toHaveCount(0, { timeout: 5_000 });
  await expect(page.getByRole("button", { name: "Save" })).toBeEnabled();
  await page.getByRole("button", { name: "Save" }).click();

  await page.waitForFunction(() => window.__builderV2Writes?.length === 1);
  const writes = await page.evaluate(() => window.__builderV2Writes);
  expect(writes).toHaveLength(1);
  expect(writes?.[0].path).toBe("C:/tmp/builder-v2-e2e.md");
  expect(writes?.[0].blueprint.nodes.some((node: BlueprintNode) => node.type === "manual_trigger")).toBe(true);
  expect(writes?.[0].blueprint.nodes.some((node: BlueprintNode) => node.type === "task")).toBe(true);
  expect(writes?.[0].blueprint.edges).toEqual([
    expect.objectContaining({ from_port: "out", to_port: "in" }),
  ]);
  expect(
    (writes?.[0].blueprint.nodes.find((node: BlueprintNode) => node.type === "task") as BlueprintNode | undefined)
      ?.fields.prompt,
  ).toBe("Plan the work.");
});
