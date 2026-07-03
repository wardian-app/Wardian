import { expect, test, type Page } from "@playwright/test";
import fs from "node:fs";

type BlueprintNode = {
  id: string;
  type: string;
  name?: string;
  fields: Record<string, unknown>;
  position?: { x: number; y: number };
};

const blueprint = {
  schema: 2,
  id: "wf",
  name: "WF",
  nodes: [
    { id: "trigger", type: "manual_trigger", name: "Trigger", fields: {}, position: { x: 0, y: 80 } },
    { id: "plan", type: "task", name: "Plan", fields: { agent: "role:planner", prompt: "Plan" }, position: { x: 320, y: 80 } },
    { id: "ship", type: "task", name: "Ship", fields: { agent: "role:builder", prompt: "Ship" }, position: { x: 640, y: 80 } },
  ],
  edges: [
    { from: "trigger", to: "plan", from_port: "out", to_port: "in" },
    { from: "plan", to: "ship", from_port: "out", to_port: "in" },
  ],
};

const loopBlueprint = {
  schema: 2,
  id: "loop-test",
  name: "Loop Test",
  nodes: [
    { id: "trigger-1", type: "manual_trigger", position: { x: -928, y: 497 } },
    { id: "loop-1", type: "loop", name: "Loop", fields: { max_iterations: 3 }, position: { x: -450, y: 508 } },
    {
      id: "command-1",
      type: "shell",
      name: "Shell Command",
      parent: "loop-1",
      fields: { command: "echo \"Hello!\"", cwd: "D:/Development" },
      position: { x: -42, y: 219 },
    },
    {
      id: "communication-1",
      type: "notify",
      name: "Notify",
      parent: "loop-1",
      fields: { message: "Test iteration" },
      position: { x: 288, y: 310 },
    },
    { id: "communication-2", type: "notify", name: "Notify Done", fields: { message: "Done!" }, position: { x: -30, y: 691 } },
  ],
  edges: [
    { from: "trigger-1", to: "loop-1", from_port: "out", to_port: "in" },
    { from: "loop-1", to: "command-1", from_port: "body", to_port: "in" },
    { from: "command-1", to: "communication-1", from_port: "out", to_port: "in" },
    { from: "loop-1", to: "communication-2", from_port: "done", to_port: "in" },
  ],
};

const parameterHeavyBlueprint = {
  schema: 2,
  id: "param-heavy",
  name: "Parameter Heavy Workflow",
  nodes: [
    {
      id: "trigger",
      type: "manual_trigger",
      name: "Trigger",
      fields: {
        input_schema: {
          type: "object",
          properties: Object.fromEntries(
            Array.from({ length: 28 }, (_, index) => [`parameter_${index + 1}`, { type: "string" }]),
          ),
        },
      },
      position: { x: 0, y: 80 },
    },
    { id: "work", type: "task", name: "Work", fields: { agent: "ephemeral", prompt: "Work." }, position: { x: 320, y: 80 } },
  ],
  edges: [
    { from: "trigger", to: "work", from_port: "out", to_port: "in" },
  ],
};

const events = [
  { seq: 0, ts: "t0", kind: "run_started", blueprint_id: "wf", schema: 2, trigger: {} },
  { seq: 1, ts: "t1", kind: "node_started", node: "plan" },
  { seq: 2, ts: "t2", kind: "node_completed", node: "plan", output: { ok: true } },
  { seq: 3, ts: "t3", kind: "node_started", node: "ship" },
  { seq: 4, ts: "t4", kind: "node_completed", node: "ship", output: { ok: true } },
  { seq: 5, ts: "t5", kind: "run_completed" },
];

async function installWorkflowsIpcMock(page: Page, blueprintFixture = blueprint) {
  await page.addInitScript(({ blueprintFixture, eventFixture }) => {
    let callbackId = 1;
    const callbacks = new Map<number, unknown>();
    const tauriWindow = window as Window & {
      __workflowsInvokes?: Array<{ command: string; args?: Record<string, unknown> }>;
      __TAURI_INTERNALS__?: Record<string, unknown>;
      __TAURI_EVENT_PLUGIN_INTERNALS__?: Record<string, unknown>;
    };

    tauriWindow.__workflowsInvokes = [];
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
        tauriWindow.__workflowsInvokes?.push({ command, args });

        if (command === "list_agents") return [];
        if (command === "list_agent_classes") return [];
        if (command === "list_provider_readiness") {
          return [
            { provider: "claude", display_name: "Claude", available: true, executable: "claude", reason: null },
            { provider: "codex", display_name: "Codex", available: true, executable: "codex", reason: null },
            { provider: "gemini", display_name: "Gemini", available: true, executable: "gemini", reason: null },
            { provider: "antigravity", display_name: "Antigravity", available: true, executable: "agy", reason: null },
            { provider: "opencode", display_name: "OpenCode", available: true, executable: "opencode", reason: null },
          ];
        }
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

        if (command === "workflow_list_blueprints") {
          return [{ id: "wf", name: "WF", path: "/x/wf.md" }];
        }
        if (command === "workflow_parse") {
          return { blueprint: blueprintFixture, diagnostics: [] };
        }
        if (command === "workflow_validate") {
          return { ok: true, diagnostics: [] };
        }
        if (command === "workflow_run") {
          return { ok: true, run_id: "run-1", blueprint_id: "wf", run_dir: "/runs/run-1" };
        }
        if (command === "workflow_list_runs") {
          return [{
            run_id: "run-1",
            blueprint_id: "wf",
            status: "completed",
            node_count: 3,
            failure: null,
            path: "/x/wf.md",
          }];
        }
        if (command === "workflow_read_run") {
          return {
            state: {
              run_id: "run-1",
              blueprint_id: "wf",
              status: "completed",
              nodes: { plan: "completed", ship: "completed" },
              registry: { nodes: { plan: { output: { ok: true } }, ship: { output: { ok: true } } }, trigger: { output: {} } },
              loop_iter: {},
              delivered: {},
              skipped_edges: [],
              next_seq: 6,
              failure: null,
            },
            events: eventFixture,
            blueprint: blueprintFixture,
          };
        }

        return null;
      },
    };
  }, { blueprintFixture, eventFixture: events });
}

function nodeById(page: Page, id: string) {
  return page.getByTestId(`run-dag-node-${id}`);
}

function builderNodeById(page: Page, id: string) {
  return page.getByTestId(`rf__node-${id}`);
}

test("unified Workflows view edits, launches, observes, and returns to edit", async ({ page }) => {
  await installWorkflowsIpcMock(page);
  await page.setViewportSize({ width: 1700, height: 980 });
  await page.goto("/", { waitUntil: "domcontentloaded" });
  await page.locator('[data-testid="app-shell"]').waitFor({ timeout: 15_000 });

  const titlebar = page.locator(".titlebar-center");
  await expect(titlebar.getByRole("button", { name: "Workflows" })).toHaveCount(1);
  await expect(titlebar.getByRole("button", { name: "Blueprints" })).toHaveCount(0);
  await expect(titlebar.getByRole("button", { name: "Runs" })).toHaveCount(0);

  await titlebar.getByRole("button", { name: "Workflows" }).click();
  await page.evaluate(async () => {
    const { useSettingsStore } = await import("/src/store/useSettingsStore.ts");
    useSettingsStore.setState({ default_provider: "codex" });
  });
  await expect(page.getByTestId("workflows-view")).toBeVisible();
  await expect(page.getByTestId("workflows-edit-mode")).toBeVisible();

  const blueprintSelect = page.getByTestId("blueprint-selector").getByRole("combobox");
  await blueprintSelect.selectOption("/x/wf.md");
  await expect(blueprintSelect).toHaveValue("/x/wf.md");
  await expect(builderNodeById(page, "plan")).toHaveCount(1);
  await expect(builderNodeById(page, "ship")).toHaveCount(1);

  await page.getByTestId("workflows-view").getByRole("button", { name: /^Run$/ }).click();
  await expect(page.getByRole("dialog")).toBeVisible();
  await expect(page.getByText("New temporary Codex agent")).toHaveCount(2);
  await page.getByRole("dialog").getByRole("button", { name: /^Run$/ }).click();

  await expect(page.getByTestId("workflows-observe-mode")).toBeVisible();
  await expect(page.getByTestId("workflows-observe-mode").getByText("run-1")).toBeVisible();
  await expect(page.getByRole("heading", { name: "Runs" })).toHaveCount(0);
  await expect(nodeById(page, "plan")).toHaveAttribute("data-status", "completed");
  await expect(nodeById(page, "ship")).toHaveAttribute("data-status", "completed");

  await page.getByTestId("workflows-view").getByRole("button", { name: "Edit" }).click();
  await expect(page.getByTestId("workflows-edit-mode")).toBeVisible();
  await expect(builderNodeById(page, "plan")).toHaveCount(1);
  await expect(builderNodeById(page, "ship")).toHaveCount(1);

  const invokes = await page.evaluate(() => window.__workflowsInvokes);
  expect(invokes?.some((call) => call.command === "workflow_run")).toBe(true);
  expect(invokes?.some((call) => call.command === "workflow_read_run")).toBe(true);
});

test("workflow builder renders persisted loop workflow nodes on a visible canvas", async ({ page }) => {
  await installWorkflowsIpcMock(page, loopBlueprint);
  await page.setViewportSize({ width: 1700, height: 980 });
  await page.goto("/", { waitUntil: "domcontentloaded" });
  await page.locator('[data-testid="app-shell"]').waitFor({ timeout: 15_000 });

  const titlebar = page.locator(".titlebar-center");
  await titlebar.getByRole("button", { name: "Workflows" }).click();
  await page.getByTestId("blueprint-selector").getByRole("combobox").selectOption("/x/wf.md");

  await expect(page.getByTestId("workflows-edit-mode")).toBeVisible();
  await expect(page.locator(".react-flow__node")).toHaveCount(loopBlueprint.nodes.length);

  const canvasBox = await page.locator(".react-flow").boundingBox();
  expect(canvasBox?.width).toBeGreaterThan(400);
  expect(canvasBox?.height).toBeGreaterThan(300);

  const nodeBoxes = await page.locator(".react-flow__node").evaluateAll((nodes) => (
    nodes.map((node) => {
      const rect = node.getBoundingClientRect();
      return { width: rect.width, height: rect.height };
    })
  ));
  expect(nodeBoxes.every((box) => box.width > 0 && box.height > 0)).toBe(true);

  const nodeVisibility = await page.locator(".react-flow__node").evaluateAll((nodes) => (
    nodes.map((node) => getComputedStyle(node).visibility)
  ));
  expect(nodeVisibility.every((visibility) => visibility === "visible")).toBe(true);

  const visibleNodeBoxes = await page.locator(".react-flow__node").evaluateAll((nodes) => (
    nodes.map((node) => {
      const rect = node.getBoundingClientRect();
      const canvas = node.closest(".react-flow")?.getBoundingClientRect();
      if (!canvas) return false;
      return rect.right > canvas.left && rect.left < canvas.right && rect.bottom > canvas.top && rect.top < canvas.bottom;
    })
  ));
  expect(visibleNodeBoxes.some(Boolean)).toBe(true);
});

test("workflow run dialog scrolls parameter-heavy forms within the viewport", async ({ page }) => {
  await installWorkflowsIpcMock(page, parameterHeavyBlueprint);
  await page.setViewportSize({ width: 1280, height: 720 });
  await page.goto("/", { waitUntil: "domcontentloaded" });
  await page.locator('[data-testid="app-shell"]').waitFor({ timeout: 15_000 });

  const titlebar = page.locator(".titlebar-center");
  await titlebar.getByRole("button", { name: "Workflows" }).click();
  await page.getByTestId("blueprint-selector").getByRole("combobox").selectOption("/x/wf.md");
  await page.getByTestId("workflows-view").getByRole("button", { name: /^Run$/ }).click();
  const dialog = page.getByTestId("run-launch-dialog");
  await expect(dialog).toBeVisible();
  await dialog.getByRole("radio", { name: "Schedule" }).click();

  const body = page.getByTestId("run-launch-dialog-body");
  const actions = page.getByTestId("run-launch-dialog-actions");
  await expect(body.getByLabel("parameter_28")).toBeVisible();
  await expect(actions.getByRole("button", { name: "Save schedule" })).toBeVisible();

  const geometry = await page.evaluate(() => {
    const dialogElement = document.querySelector('[data-testid="run-launch-dialog"]');
    const bodyElement = document.querySelector('[data-testid="run-launch-dialog-body"]');
    const actionsElement = document.querySelector('[data-testid="run-launch-dialog-actions"]');
    if (!dialogElement || !bodyElement || !actionsElement) return null;
    const dialogRect = dialogElement.getBoundingClientRect();
    const bodyRect = bodyElement.getBoundingClientRect();
    const actionsRect = actionsElement.getBoundingClientRect();
    return {
      dialogBottom: dialogRect.bottom,
      viewportHeight: window.innerHeight,
      bodyClientHeight: bodyElement.clientHeight,
      bodyScrollHeight: bodyElement.scrollHeight,
      bodyBottom: bodyRect.bottom,
      actionsTop: actionsRect.top,
      actionsBottom: actionsRect.bottom,
    };
  });

  expect(geometry).not.toBeNull();
  expect(geometry!.dialogBottom).toBeLessThanOrEqual(geometry!.viewportHeight);
  expect(geometry!.bodyScrollHeight).toBeGreaterThan(geometry!.bodyClientHeight);
  expect(geometry!.bodyBottom).toBeLessThanOrEqual(geometry!.actionsTop);
  expect(geometry!.actionsBottom).toBeLessThanOrEqual(geometry!.viewportHeight);

  await body.evaluate((element) => {
    element.scrollTop = element.scrollHeight;
  });
  await expect(body.getByLabel("parameter_28")).toBeInViewport();

  fs.mkdirSync("e2e/screenshots/workflow-run-dialog", { recursive: true });
  await dialog.screenshot({ path: "e2e/screenshots/workflow-run-dialog/scrollable-parameter-form.png" });
});
