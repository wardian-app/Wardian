import { expect, test, type Locator } from "@playwright/test";
import * as path from "node:path";

import { installWorkbenchIpcMock, makeWorkbenchDocument } from "../fixtures/workbenchIpcMock";
import {
  parentAttributionBreakdown,
  parentAttributionFleet,
  parentAttributionMatrix,
} from "../fixtures/telemetryParentAttribution";

const SHOTS = process.env.WARDIAN_E2E_SCREENSHOT_DIR
  ?? path.join("e2e", "screenshots", "analytics-parent-attribution-1370");

async function installTelemetryFixture(page: import("@playwright/test").Page) {
  const document = makeWorkbenchDocument();
  const parentBreakdown = parentAttributionBreakdown("fixture-parent");
  const leafBreakdown = parentAttributionBreakdown("fixture-leaf");
  const ipc = await installWorkbenchIpcMock(page, {
    load_result: {
      source: "primary",
      document,
      notice: null,
      durable_revision: document.revision,
      durable_token: "fixture-token",
    },
    responses: {
      telemetry_fleet: parentAttributionFleet(),
      telemetry_matrix: parentAttributionMatrix(),
      telemetry_agent_breakdown: parentBreakdown,
      load_dashboard_prefs: null,
      telemetry_refresh: { advanced: 0 },
    },
  });
  await page.addInitScript(
    ({ parent, leaf }) => {
      type TauriInternals = {
        invoke?: (command: string, args?: Record<string, unknown>) => Promise<unknown>;
      };
      const tauriWindow = window as Window & { __TAURI_INTERNALS__?: TauriInternals };
      const originalInvoke = tauriWindow.__TAURI_INTERNALS__?.invoke;
      if (!originalInvoke || !tauriWindow.__TAURI_INTERNALS__) {
        throw new Error("Workbench IPC mock was not installed before telemetry fixture setup");
      }
      tauriWindow.__TAURI_INTERNALS__.invoke = async (command, args) => {
        const response = await originalInvoke(command, args);
        if (command !== "telemetry_agent_breakdown") return response;
        return structuredClone(String(args?.session_id ?? "") === "fixture-leaf" ? leaf : parent);
      };
    },
    { parent: parentBreakdown, leaf: leafBreakdown },
  );
  return ipc;
}

async function expectLeafOwnOnly(dialog: Locator) {
  await expect(dialog).toHaveRole("dialog");
  await expect(dialog.getByRole("heading", { name: "Leaf Atlas" })).toBeVisible();
  await expect(dialog.locator("tbody tr").filter({ hasText: "Active agent time" }).locator("td"))
    .toHaveText(["6m", "6m", "0m"]);
  await expect(dialog.locator("tbody tr").filter({ hasText: "Turns" }).locator("td"))
    .toHaveText(["4", "4", "0"]);
  await expect(dialog.locator("tbody tr").filter({ hasText: "Files touched" }).locator("td"))
    .toHaveText(["2", "2", "0"]);
}

test.describe("Issue 1370 parent telemetry attribution", () => {
  test("keeps parent and leaf rows aligned and uses the shared Dashboard detail overlay", async ({ page }) => {
    const ipc = await installTelemetryFixture(page);
    await page.goto("/");
    await page.getByText("Review habitat telemetry.").click();
    await expect(page.getByText("Parent Atlas")).toBeVisible({ timeout: 20_000 });

    const parentRow = page.locator(".dashboard-view__row", { hasText: "Parent Atlas" });
    const leafRow = page.locator(".dashboard-view__row", { hasText: "Leaf Atlas" });
    await expect(parentRow).toHaveCount(1);
    await expect(leafRow).toHaveCount(1);
    const [parentBox, leafBox] = await Promise.all([parentRow.boundingBox(), leafRow.boundingBox()]);
    expect(parentBox?.width).toBe(leafBox?.width);
    expect(parentBox?.height).toBe(leafBox?.height);
    await expect(parentRow.locator("[data-subagent-badge]")).toHaveCount(0);
    await expect(leafRow.locator("[data-subagent-badge]")).toHaveCount(0);
    await page.locator(".dashboard-view__table").screenshot({
      path: path.join(SHOTS, "dashboard-table-before-details.png"),
    });

    await parentRow.click();
    const dashboardDialog = page.getByRole("dialog", { name: "Parent Atlas" });
    await expect(dashboardDialog).toBeVisible();
    await expect(dashboardDialog.getByRole("columnheader", { name: "Combined" })).toBeVisible();
    await expect(dashboardDialog.getByRole("columnheader", { name: "Own work" })).toBeVisible();
    await expect(dashboardDialog.getByRole("columnheader", { name: "Subagents" })).toBeVisible();
    await expect(dashboardDialog.getByText("—").first()).toBeVisible();
    await expect(dashboardDialog.getByRole("button", { name: "Open agent" })).toBeVisible();

    const detailCalls = await ipc.calls("telemetry_agent_breakdown");
    expect(detailCalls.at(-1)?.args).toEqual({
      session_id: "fixture-parent",
      from: "2026-08-14T23:00:00.000Z",
      to: "2026-08-15T00:00:00.000Z",
    });
    await dashboardDialog.screenshot({ path: path.join(SHOTS, "dashboard-parent-details.png") });

    await dashboardDialog.getByRole("button", { name: "Close telemetry details" }).click();
    await expect(dashboardDialog).toHaveCount(0);
    await expect(parentRow).toHaveCount(1);
    await expect(leafRow).toHaveCount(1);
    const [parentAfterClose, leafAfterClose] = await Promise.all([
      parentRow.boundingBox(),
      leafRow.boundingBox(),
    ]);
    expect(parentAfterClose?.width).toBe(leafAfterClose?.width);
    expect(parentAfterClose?.height).toBe(leafAfterClose?.height);

    await leafRow.click();
    const leafDialog = page.getByRole("dialog", { name: "Leaf Atlas" });
    await expect(leafDialog).toBeVisible();
    await expectLeafOwnOnly(leafDialog);
    const leafDetailCalls = await ipc.calls("telemetry_agent_breakdown");
    expect(leafDetailCalls.at(-1)?.args).toEqual({
      session_id: "fixture-leaf",
      from: "2026-08-14T23:00:00.000Z",
      to: "2026-08-15T00:00:00.000Z",
    });
    await leafDialog.getByRole("button", { name: "Close telemetry details" }).click();
    await expect(leafDialog).toHaveCount(0);
  });

  test("keeps the same row geometry and detail columns in Analytics", async ({ page }) => {
    const ipc = await installTelemetryFixture(page);
    await page.goto("/");
    await page.getByText("Look up what agents did over a period.").click();
    await expect(page.getByText("Parent Atlas")).toBeVisible({ timeout: 20_000 });

    const parentRow = page.locator(".analytics-view__row", { hasText: "Parent Atlas" });
    const leafRow = page.locator(".analytics-view__row", { hasText: "Leaf Atlas" });
    await expect(parentRow).toHaveCount(1);
    await expect(leafRow).toHaveCount(1);
    const [parentBox, leafBox] = await Promise.all([parentRow.boundingBox(), leafRow.boundingBox()]);
    expect(parentBox?.width).toBe(leafBox?.width);
    expect(parentBox?.height).toBe(leafBox?.height);
    await page.locator(".analytics-view__matrix").screenshot({
      path: path.join(SHOTS, "analytics-table-before-details.png"),
    });

    await parentRow.click();
    const analyticsDialog = page.getByRole("dialog", { name: "Parent Atlas" });
    await expect(analyticsDialog).toBeVisible();
    const headers = await analyticsDialog.locator("thead th").allTextContents();
    expect(headers).toEqual([
      "Metric",
      "Combined",
      "Own work",
      "Subagents",
    ]);
    const detailCalls = await ipc.calls("telemetry_agent_breakdown");
    expect(detailCalls.at(-1)?.args).toEqual({
      session_id: "fixture-parent",
      from: "2026-08-14T23:00:00.000Z",
      to: "2026-08-15T00:00:00.000Z",
    });
    await analyticsDialog.screenshot({ path: path.join(SHOTS, "analytics-parent-details.png") });

    await analyticsDialog.getByRole("button", { name: "Close telemetry details" }).click();
    await expect(analyticsDialog).toHaveCount(0);
    await expect(parentRow).toHaveCount(1);
    await expect(leafRow).toHaveCount(1);
    const [parentAfterClose, leafAfterClose] = await Promise.all([
      parentRow.boundingBox(),
      leafRow.boundingBox(),
    ]);
    expect(parentAfterClose?.width).toBe(leafAfterClose?.width);
    expect(parentAfterClose?.height).toBe(leafAfterClose?.height);

    await leafRow.click();
    const leafDialog = page.getByRole("dialog", { name: "Leaf Atlas" });
    await expect(leafDialog).toBeVisible();
    await expectLeafOwnOnly(leafDialog);
    const leafDetailCalls = await ipc.calls("telemetry_agent_breakdown");
    expect(leafDetailCalls.at(-1)?.args).toEqual({
      session_id: "fixture-leaf",
      from: "2026-08-14T23:00:00.000Z",
      to: "2026-08-15T00:00:00.000Z",
    });
    await leafDialog.getByRole("button", { name: "Close telemetry details" }).click();
    await expect(leafDialog).toHaveCount(0);
  });
});
