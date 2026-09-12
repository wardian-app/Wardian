import { expect, test } from "@playwright/test";
import path from "node:path";
import { installGardenCompositionMock, GARDEN_AGENT, GARDEN_ROOT } from "../fixtures/gardenComposition";
import { surfacePanel } from "../fixtures/workbench";

test("Garden omits dense terminal one-off automation history", async ({ page }) => {
  await page.setViewportSize({ width: 1600, height: 1000 });
  const mock = await installGardenCompositionMock(page, { historicalOneOffRunCount: 80 });
  await page.goto("/");

  const garden = surfacePanel(page, "garden");
  await expect(garden.locator("canvas").first()).toBeVisible({ timeout: 15_000 });
  await expect(garden.locator('[data-garden-object="automation:schedule:daily-design"]')).toBeAttached();
  await expect(garden.locator('[data-garden-object^="automation:run:historical-review-"]')).toHaveCount(0);

  const runReads = await mock.calls("automation_read_run");
  const invocationReads = await mock.calls("read_file_preview");
  expect(runReads).toHaveLength(1);
  expect(invocationReads).toHaveLength(1);

  await garden.locator(`[data-garden-object="district:workspace:${GARDEN_ROOT}"]`).press("Enter");
  await expect(page.getByText("Saving workbench changes…", { exact: true })).toBeHidden();
  const stamp = process.env.WARDIAN_GARDEN_STAMP ?? "terminal-one-off-history";
  await garden.screenshot({
    path: path.resolve("e2e/screenshots/garden", stamp, "terminal-one-off-history-omitted.png"),
    animations: "disabled",
  });

  await garden.locator(`[data-garden-object="agent:${GARDEN_AGENT}"]`).press("Enter");
  const activity = garden.getByRole("region", { name: "Automations, Conversations, Inbox", exact: true });
  await expect(activity.getByRole("button", { name: /Daily design review/ })).toBeVisible();
  await expect(activity.getByRole("button", { name: /historical review/i })).toHaveCount(0);
});
