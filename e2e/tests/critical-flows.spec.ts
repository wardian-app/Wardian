import { expect, test, type Page } from "@playwright/test";
import { openSurface, surfacePanel } from "../fixtures/workbench";

test.describe("Critical browser flows", () => {
  test.describe.configure({ mode: "serial" });

  let page: Page;

  test.beforeAll(async ({ browser }) => {
    page = await browser.newPage();
    await page.goto("/", { waitUntil: "domcontentloaded" });
    await page.locator('[data-testid="app-shell"]').waitFor({ timeout: 15_000 });
  });

  test.afterAll(async () => {
    await page.close();
  });

  test("command broadcast requires an agent selection", async () => {
    await page.locator('[data-testid="sidebar-tab-command"]').click();
    await expect(page.locator('[data-testid="command-panel"]')).toBeVisible();

    const textarea = page.locator('[data-testid="broadcast-textarea"]');
    await expect(textarea).toBeDisabled();
    await expect(page.locator('[data-testid="broadcast-submit"]')).toBeDisabled();
  });

  test("workflow builder can add a manual trigger block from the library", async () => {
    await openSurface(page, "workflows");
    const workflows = surfacePanel(page, "workflows");
    await expect(workflows.getByTestId("workflows-view")).toBeVisible();
    await expect(workflows.getByTestId("workflows-edit-mode")).toBeVisible();

    await workflows.getByTestId("workflows-view").getByRole("button", { name: "Add node" }).click();
    await expect(page.getByTestId("node-library")).toBeVisible();

    await page.getByRole("button", { name: /Manual Trigger/ }).click();

    const manualTriggerNode = page
      .locator(".react-flow__node")
      .filter({ hasText: "Manual Trigger" });
    await expect(manualTriggerNode).toHaveCount(1);
    await expect(manualTriggerNode).toBeVisible();
    await expect(workflows.getByTestId("workflows-view").getByRole("button", { name: /^Run$/ })).toBeDisabled();
  });
});
