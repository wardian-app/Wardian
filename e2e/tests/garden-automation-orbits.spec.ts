import { expect, test } from "@playwright/test";
import path from "node:path";
import { installGardenCompositionMock, GARDEN_AGENT, GARDEN_ROOT } from "../fixtures/gardenComposition";
import { surfacePanel } from "../fixtures/workbench";

test("dense agent automations occupy spatial rings without a failed-run halo", async ({ page }) => {
  await page.setViewportSize({ width: 1600, height: 1000 });
  await installGardenCompositionMock(page, { assignedAutomationCount: 20, scheduledRunStatus: "failed", singleAgentAutomations: true });
  await page.emulateMedia({ reducedMotion: "reduce" });
  await page.goto("/");
  await page.evaluate(async () => {
    const { useSettingsStore } = await import("/src/store/useSettingsStore.ts");
    useSettingsStore.getState().setTheme("dark");
  });
  await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");

  const garden = surfacePanel(page, "garden");
  await expect(garden.locator("canvas").first()).toBeVisible({ timeout: 15_000 });
  await garden.locator(`[data-garden-object="district:workspace:${GARDEN_ROOT}"]`).press("Enter");
  const automations = garden.locator('[data-garden-object^="automation:schedule:"]');
  await expect(automations).toHaveCount(20);
  const positions = await automations.evaluateAll((elements) => elements.map((element) => {
    const style = (element as HTMLElement).style;
    return { left: Math.round(Number.parseFloat(style.left)), top: Math.round(Number.parseFloat(style.top)) };
  }));
  expect(new Set(positions.map((position) => position.left)).size).toBeGreaterThanOrEqual(6);
  expect(new Set(positions.map((position) => position.top)).size).toBeGreaterThanOrEqual(6);

  const stamp = process.env.WARDIAN_GARDEN_STAMP ?? "2026-09-12-agent-composition";
  await expect(page.getByText("Saving workbench changes…", { exact: true })).toBeHidden();
  await garden.screenshot({
    path: path.resolve("e2e/screenshots/garden", stamp, "automation-orbits-no-failed-halo.png"),
    animations: "disabled",
  });

  await garden.locator(`[data-garden-object="agent:${GARDEN_AGENT}"]`).press("Enter");
  const automationRegion = garden.getByRole("region", { name: "Automations", exact: true });
  await expect(automationRegion.locator('[data-garden-ref^="automation:"]')).toHaveCount(20);
  expect(await automationRegion.getByLabel("Automations contents").evaluate((element) => element.scrollHeight > element.clientHeight)).toBe(true);
  await expect(page.getByText("Saving workbench changes…", { exact: true })).toBeHidden();
  await garden.screenshot({
    path: path.resolve("e2e/screenshots/garden", stamp, "dense-agent-organelles.png"),
    animations: "disabled",
  });
});
