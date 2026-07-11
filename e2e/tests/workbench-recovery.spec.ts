import { expect, test, type Page } from "@playwright/test";

import { surfacePanel, surfaceTab } from "../fixtures/workbench";
import {
  installWorkbenchIpcMock,
  makeWorkbenchDocument,
  makeWorkbenchSurface,
  type WorkbenchAgentFixture,
} from "../fixtures/workbenchIpcMock";

const candidateAgent: WorkbenchAgentFixture = {
  session_id: "agent-live",
  session_name: "Live Agent",
  agent_class: "Architect",
  folder: "C:/workspace/live-agent",
  provider: "claude",
  is_off: false,
};

async function executeWorkbenchCommand(page: Page, query: string): Promise<void> {
  await page.getByRole("tab", { selected: true }).focus();
  await page.keyboard.press("ControlOrMeta+Shift+P");
  const palette = page.getByRole("dialog", { name: "Command Palette", exact: true });
  await expect(palette).toBeVisible();
  await palette.getByRole("combobox", { name: "Search commands", exact: true }).fill(query);
  await page.keyboard.press("Enter");
}

test.describe("Workbench recovery", () => {
  test("restores the exact split document, shell sizes, and an inert unknown surface", async ({ page }) => {
    const overview = makeWorkbenchSurface("surface-overview", "agents-overview", {
      state: {
        mode: "single",
        focused_agent_id: null,
        search_query: "restored query",
        status_filter: ["Idle"],
      },
    });
    const unknown = makeWorkbenchSurface("surface-plugin", "unknown-plugin", {
      state_schema_version: 7,
      state: { opaque: ["preserve", 42] },
    });
    const missing = makeWorkbenchSurface("surface-missing-agent", "agent-session", {
      resource_key: "missing-agent",
      state: { restored: true },
    });
    const restored = makeWorkbenchDocument({
      revision: 7,
      root: {
        kind: "split",
        node_id: "restored-split",
        direction: "horizontal",
        ratio: 0.61,
        first: { kind: "group", group_id: "group-left" },
        second: { kind: "group", group_id: "group-right" },
      },
      groups: {
        "group-left": {
          group_id: "group-left",
          surface_ids: [overview.surface_id, unknown.surface_id],
          active_surface_id: unknown.surface_id,
        },
        "group-right": {
          group_id: "group-right",
          surface_ids: [missing.surface_id],
          active_surface_id: missing.surface_id,
        },
      },
      surfaces: [overview, unknown, missing],
      active_group_id: "group-right",
      shell: {
        left_sidebar_width: 318,
        right_sidebar_width: 276,
        bottom_terminal_open: true,
        bottom_terminal_height: 444,
      },
    });
    const ipc = await installWorkbenchIpcMock(page, {
      load_result: {
        source: "primary",
        document: restored,
        notice: null,
        durable_revision: restored.revision,
        durable_token: "restore-token-7",
      },
    });

    await page.goto("/");
    await expect(page.getByTestId("workbench-host")).toBeVisible();
    await expect(page.getByTestId("workbench-group")).toHaveCount(2);
    await expect(page.getByTestId("workbench-group")
      .and(page.locator('[data-group-id="group-right"]'))).toHaveAttribute("data-active", "true");
    await expect(page.getByRole("tab")).toHaveCount(3);
    await expect(surfaceTab(page, "agents-overview")).toHaveAttribute(
      "data-surface-id",
      overview.surface_id,
    );
    await expect(surfaceTab(page, "agent-session", "missing-agent")).toHaveAttribute(
      "data-surface-id",
      missing.surface_id,
    );

    const unknownTab = page.getByRole("tab")
      .and(page.locator('[data-surface-id="surface-plugin"]'));
    await expect(unknownTab).toHaveAttribute("aria-selected", "true");
    const unknownPanel = page.getByTestId("surface-panel")
      .and(page.locator('[data-surface-id="surface-plugin"]'));
    await expect(unknownPanel).toBeVisible();
    await expect(unknownPanel.getByRole("heading", { name: "unknown-plugin" })).toBeVisible();
    await expect(unknownPanel.getByRole("button")).toHaveCount(0);

    await expect.poll(() => page.evaluate(() => ({
      left: document.documentElement.style.getPropertyValue("--sidebar-content-width"),
      right: document.documentElement.style.getPropertyValue("--sidebar-secondary-width"),
    }))).toEqual({ left: "318px", right: "276px" });
    const titlebar = page.locator(".titlebar");
    await expect(titlebar).toHaveCSS("--titlebar-left-width", "366px");
    await expect(titlebar).toHaveCSS("--titlebar-right-width", "276px");
    await expect(page.getByTestId("user-terminal-panel")).toHaveCSS("height", "444px");

    const snapshot = await ipc.snapshot();
    expect(snapshot.load_result.document).toEqual(restored);
    expect(await ipc.calls("load_workbench_state")).toHaveLength(1);
    expect(await ipc.calls("save_workbench_state")).toHaveLength(0);
  });

  test("shows a nonblocking backup recovery notice", async ({ page }) => {
    const restored = makeWorkbenchDocument({
      revision: 3,
      surfaces: [makeWorkbenchSurface("backup-overview", "agents-overview")],
    });
    await installWorkbenchIpcMock(page, {
      load_result: {
        source: "backup",
        document: restored,
        notice: "Recovered the workbench from backup.",
        durable_revision: restored.revision,
        durable_token: "backup-token-3",
      },
    });

    await page.goto("/");
    const notice = page.getByTestId("workbench-persistence-notice");
    await expect(notice).toHaveAttribute("role", "status");
    await expect(notice).toContainText("Recovered the workbench from backup.");
    await expect(surfacePanel(page, "agents-overview")).toBeVisible();
  });

  test("keeps a future-schema workbench read-only without attempting V1 writes", async ({ page }) => {
    const ipc = await installWorkbenchIpcMock(page, {
      load_result: {
        source: "future_schema",
        document: null,
        notice: "Workbench schema 99 requires a newer Wardian version.",
        durable_revision: null,
        durable_token: null,
      },
    });

    await page.goto("/");
    const dialog = page.getByRole("dialog", { name: "Newer workbench version" });
    await expect(dialog).toBeVisible();
    await expect(dialog).toContainText("read-only");
    await expect(dialog.getByRole("button", { name: "Export Local JSON" })).toBeVisible();
    await expect(dialog.getByRole("button", { name: "Use Disk" })).toHaveCount(0);
    await expect(dialog.getByRole("button", { name: "Replace Disk" })).toHaveCount(0);
    await expect(page.getByTestId("workbench-persistence-notice")).toContainText(
      "Workbench schema 99 requires a newer Wardian version.",
    );

    await page.waitForTimeout(350);
    expect(await ipc.calls("save_workbench_state")).toHaveLength(0);
    expect(await ipc.calls("reset_workbench_state")).toHaveLength(0);
  });

  test("recovers a missing Agent Session through refresh, reset, and close actions", async ({ page }) => {
    const missing = makeWorkbenchSurface("missing-session", "agent-session", {
      resource_key: "agent-gone",
      state: { stale: true },
    });
    const resetMissing = makeWorkbenchSurface("reset-missing-session", "agent-session", {
      resource_key: "agent-reset",
      state: { stale: true },
    });
    const restored = makeWorkbenchDocument({
      revision: 2,
      surfaces: [missing, resetMissing],
      groups: {
        "group-1": {
          group_id: "group-1",
          surface_ids: [missing.surface_id, resetMissing.surface_id],
          active_surface_id: missing.surface_id,
        },
      },
    });
    const ipc = await installWorkbenchIpcMock(page, {
      load_result: {
        source: "primary",
        document: restored,
        notice: null,
        durable_revision: restored.revision,
        durable_token: "missing-token-2",
      },
    });

    await page.goto("/");
    const placeholder = surfacePanel(page, "agent-session", "agent-gone")
      .getByTestId("agent-session-surface");
    await expect(placeholder).toHaveAttribute("data-missing-agent", "true");
    await expect(placeholder).toContainText("agent-gone");
    await expect(placeholder.getByRole("button", { name: "Reset Surface" })).toBeVisible();
    await expect(placeholder.getByRole("button", { name: "Close" })).toBeVisible();

    const listCallsBeforeRefresh = (await ipc.calls("list_agents")).length;
    await ipc.setAgents([candidateAgent], { emit: false });
    await placeholder.getByRole("button", { name: "Refresh agents" }).click();
    await expect.poll(async () => (await ipc.calls("list_agents")).length)
      .toBeGreaterThan(listCallsBeforeRefresh);
    await expect(placeholder.getByLabel("Rebind Agent Session")).toHaveValue("agent-live");
    await expect(placeholder.getByRole("button", { name: "Rebind" })).toBeEnabled();
    await placeholder.getByRole("button", { name: "Rebind" }).click();
    await expect(surfaceTab(page, "agent-session", "agent-gone")).toHaveCount(0);
    await expect(surfaceTab(page, "agent-session", "agent-live")).toHaveCount(1);
    await expect(surfacePanel(page, "agent-session", "agent-live")
      .getByTestId("agent-session-surface"))
      .not.toHaveAttribute("data-missing-agent", "true");

    await surfaceTab(page, "agent-session", "agent-reset").click();
    const resetPlaceholder = surfacePanel(page, "agent-session", "agent-reset")
      .getByTestId("agent-session-surface");
    await expect(resetPlaceholder).toHaveAttribute("data-missing-agent", "true");

    await resetPlaceholder.getByRole("button", { name: "Reset Surface" }).click();
    await expect.poll(async () => {
      const calls = await ipc.calls("save_workbench_state");
      return calls.some((call) => {
        const request = call.args as { document?: typeof restored } | undefined;
        return request?.document?.surfaces[resetMissing.surface_id]?.state
          && Object.keys(request.document.surfaces[resetMissing.surface_id].state as object).length === 0;
      });
    }).toBe(true);
    await expect(resetPlaceholder).toHaveAttribute("data-missing-agent", "true");

    await resetPlaceholder.getByRole("button", { name: "Close" }).click();
    await expect(surfaceTab(page, "agent-session", "agent-reset")).toHaveCount(0);
    await expect.poll(async () => {
      const calls = await ipc.calls("save_workbench_state");
      const last = calls.at(-1)?.args as { document?: typeof restored } | undefined;
      return last?.document?.surfaces[resetMissing.surface_id] === undefined;
    }).toBe(true);
    expect(await ipc.calls("kill_agent")).toHaveLength(0);
  });

  test("Reset Workbench evaluates dirty-surface guards before issuing reset_workbench_state", async ({ page }) => {
    const dashboard = makeWorkbenchSurface("reset-dashboard", "dashboard");
    const workflows = makeWorkbenchSurface("reset-workflows", "workflows");
    const restored = makeWorkbenchDocument({
      revision: 4,
      surfaces: [dashboard, workflows],
      groups: {
        "group-1": {
          group_id: "group-1",
          surface_ids: [dashboard.surface_id, workflows.surface_id],
          active_surface_id: workflows.surface_id,
        },
      },
    });
    const ipc = await installWorkbenchIpcMock(page, {
      reset_delay_ms: 500,
      load_result: {
        source: "primary",
        document: restored,
        notice: null,
        durable_revision: restored.revision,
        durable_token: "reset-token-4",
      },
    });

    await page.goto("/");
    const workflowName = page.getByRole("textbox", { name: "Workflow name" });
    await workflowName.fill("Edited workflow");
    await executeWorkbenchCommand(page, "Reset Workbench");

    const prompt = page.getByRole("dialog", { name: "Unsaved Workflows changes" });
    await expect(prompt).toBeVisible();
    await prompt.getByRole("button", { name: "Cancel", exact: true }).click();
    expect(await ipc.calls("reset_workbench_state")).toHaveLength(0);
    await expect(surfaceTab(page, "dashboard")).toHaveCount(1);
    await expect(surfaceTab(page, "workflows")).toHaveCount(1);

    await executeWorkbenchCommand(page, "Reset Workbench");
    await page.getByRole("dialog", { name: "Unsaved Workflows changes" })
      .getByRole("button", { name: "Discard", exact: true })
      .click();

    const workbench = page.getByTestId("workbench-host");
    await expect(workbench).toHaveAttribute("data-reset-pending", "true");
    await expect(workbench).toHaveAttribute("inert", "");
    expect(await workflowName.evaluate((element) => {
      element.focus();
      return document.activeElement === element;
    })).toBe(false);
    await expect.poll(async () => (await ipc.calls("reset_workbench_state")).length).toBe(1);
    await expect(page.getByRole("tab")).toHaveCount(0);
    await expect(page.getByRole("heading", { name: "New Surface" })).toBeVisible();
  });

  test("a delayed reset conflict locks shell mutations and outside-host navigation", async ({ page }) => {
    const dashboard = makeWorkbenchSurface("conflict-dashboard", "dashboard");
    const restored = makeWorkbenchDocument({
      revision: 6,
      surfaces: [dashboard],
      shell: { left_sidebar_width: 300 },
    });
    await installWorkbenchIpcMock(page, {
      reset_delay_ms: 500,
      reset_outcome: "revision_conflict",
      load_result: {
        source: "primary",
        document: restored,
        notice: null,
        durable_revision: restored.revision,
        durable_token: "conflict-token-6",
      },
    });

    await page.goto("/");
    const sidebar = page.getByTestId("sidebar-resize-handle").first()
      .locator("xpath=ancestor::aside[1]");
    await expect.poll(async () => Math.round(await sidebar.evaluate(
      (element) => element.getBoundingClientRect().width,
    ))).toBe(300);
    const widthBefore = await sidebar.evaluate((element) => element.getBoundingClientRect().width);
    await executeWorkbenchCommand(page, "Reset Workbench");

    const content = page.getByTestId("app-shell-content");
    await expect(content).toHaveAttribute("inert", "");
    await expect(page.getByRole("button", { name: "Hide Left Sidebar" })).toBeDisabled();
    await expect(page.getByRole("button", { name: "Hide Agent Roster" })).toBeDisabled();
    await expect(page.getByTestId("sidebar-tab-workflows").click({
      trial: true,
      timeout: 200,
    })).rejects.toThrow();
    await expect(page.getByTestId("sidebar-resize-handle").first().click({
      trial: true,
      timeout: 200,
    })).rejects.toThrow();

    await expect(page.getByRole("dialog", { name: "Workbench changed on disk" })).toBeVisible();
    await expect(content).not.toHaveAttribute("inert");
    const widthAfter = await sidebar.evaluate((element) => element.getBoundingClientRect().width);
    expect(Math.round(widthAfter)).toBe(Math.round(widthBefore));
    await expect(surfaceTab(page, "dashboard")).toHaveCount(1);
    await expect(surfaceTab(page, "workflows")).toHaveCount(0);
  });
});
