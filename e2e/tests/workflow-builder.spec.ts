import { expect, test, type Page } from "@playwright/test";
import { openSurface } from "../fixtures/workbench";

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

async function installWorkflowBuilderIpcMock(page: Page) {
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
      __workflowBuilderInvokes?: Array<{ command: string; args?: Record<string, unknown> }>;
      __workflowBuilderWrites?: Array<{ path: string; blueprint: Blueprint }>;
      __TAURI_INTERNALS__?: Record<string, unknown>;
      __TAURI_EVENT_PLUGIN_INTERNALS__?: Record<string, unknown>;
    };

    tauriWindow.__workflowBuilderInvokes = [];
    tauriWindow.__workflowBuilderWrites = [];

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
        tauriWindow.__workflowBuilderInvokes?.push({ command, args });

        if (command === "list_agents") return [];
        if (command === "list_agent_classes") return [];
        if (command === "list_provider_readiness") return [];
        if (command === "load_watchlists") return [];
        if (command === "load_watchlist_prefs") return null;
        if (command === "load_agent_interactions") return {};
        if (command === "load_queue_items") return [];
        if (command === "load_queue_preferences") return {};
        if (command === "load_onboarding_hints") return { dismissed_hint_ids: ["spawn-agent-first-run:v1"] };
        if (command === "dismiss_onboarding_hint") return { dismissed_hint_ids: ["spawn-agent-first-run:v1"] };
        if (command === "list_workflows") return [];
        if (command === "list_scheduled_runs") return [];
        if (command === "load_workflow_library") return { folders: [], rootWorkflowIds: [] };
        if (command === "get_library_tree") return { type: "Folder", path: "", name: "Root", children: [] };
        if (command === "list_deployed_skills") return [];
        if (command === "load_app_settings") return null;
        if (command === "load_shell_settings") {
          return {
            shell_id: "auto",
            custom_executable: null,
            custom_args: null,
            agent_session_persistence: "resume",
            default_provider: "codex",
          };
        }
        if (command === "list_available_shells") return [];
        if (command === "plugin:event|listen") return callbackId++;
        if (command === "plugin:event|unlisten") return null;
        if (command === "sync_provider_theme_settings") return null;

        if (command === "workflow_list_blueprints") return [];
        if (command === "workflow_list_runs") return [];
        if (command === "workflow_read_run") return { state: null, events: [], blueprint: null };
        if (command === "schedule_list") return [];
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
          tauriWindow.__workflowBuilderWrites?.push({
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

async function openWorkflowBuilder(page: Page) {
  await page.setViewportSize({ width: 1700, height: 980 });
  await page.goto("/", { waitUntil: "domcontentloaded" });
  await page.locator('[data-testid="app-shell"]').waitFor({ timeout: 15_000 });
  await openSurface(page, "workflows");
  await expect(page.getByTestId("workflows-view")).toBeVisible();
  await expect(page.getByTestId("workflows-edit-mode")).toBeVisible();
  await page.evaluate(async () => {
    const { useBuilderStore } = await import("/src/store/useBuilderStore.ts");
    useBuilderStore.setState({ path: "C:/tmp/workflow-builder-e2e.md" });
  });
}

async function addNode(page: Page, name: string) {
  await page.getByTestId("workflows-view").getByRole("button", { name: "Add node" }).click();
  await expect(page.getByTestId("node-library")).toBeVisible();
  await page.getByTestId("node-library").getByRole("button", { name }).click();
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

test("workflow builder authors, validates, and saves a workflow blueprint", async ({ page }) => {
  await installWorkflowBuilderIpcMock(page);
  await openWorkflowBuilder(page);

  await addNode(page, "Manual Trigger");
  await addNode(page, "Task");
  await connectBlueprintNodes(page);
  await page.evaluate(async () => {
    const { useBuilderStore } = await import("/src/store/useBuilderStore.ts");
    await useBuilderStore.getState().validate();
  });

  await expect(page.getByLabel(/Prompt/i)).toBeVisible();
  await expect(page.getByText("missing_required_field")).toBeVisible({ timeout: 5_000 });
  if (process.env.WARDIAN_WORKFLOW_BUILDER_SCREENSHOT_DIR) {
    await page.getByTestId("workflows-view").screenshot({
      path: `${process.env.WARDIAN_WORKFLOW_BUILDER_SCREENSHOT_DIR}/save-disabled-invalid-workflow.png`,
    });
  }
  await expect(page.getByRole("button", { name: "Save" })).toBeDisabled();

  await page.getByLabel(/Prompt/i).fill("Plan the work.");
  await page.evaluate(async () => {
    const { useBuilderStore } = await import("/src/store/useBuilderStore.ts");
    await useBuilderStore.getState().validate();
  });

  await expect(page.getByText("missing_required_field")).toHaveCount(0, { timeout: 5_000 });
  await expect(page.getByRole("button", { name: "Save" })).toBeEnabled();
  await page.getByRole("button", { name: "Save" }).click();

  await page.waitForFunction(() => window.__workflowBuilderWrites?.length === 1);
  const writes = await page.evaluate(() => window.__workflowBuilderWrites);
  expect(writes).toHaveLength(1);
  expect(writes?.[0].path).toBe("C:/tmp/workflow-builder-e2e.md");
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
