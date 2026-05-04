import { expect, test } from "@playwright/test";

test.describe("Critical browser flows", () => {
  test.beforeEach(async ({ page }) => {
    await page.goto("/");
    await page.locator('[data-testid="app-shell"]').waitFor({ timeout: 15_000 });
  });

  test("command broadcast asks for confirmation when no agents are selected", async ({ page }) => {
    await page.locator('[data-testid="sidebar-tab-command"]').click();
    await expect(page.locator('[data-testid="command-panel"]')).toBeVisible();

    const textarea = page.locator('[data-testid="broadcast-textarea"]');
    await textarea.fill("status check");
    await page.locator('[data-testid="broadcast-submit"]').click();

    const dialog = page.locator("#confirm-dialog-panel");
    await expect(dialog).toBeVisible();
    await expect(dialog).toContainText(
      "No agents selected. This will broadcast to ALL agents. Are you sure?",
    );

    await page.getByRole("button", { name: "Cancel" }).click();
    await expect(dialog).toBeHidden();
    await expect(textarea).toHaveValue("status check");
  });

  test("workflow builder can add a manual trigger block from the library", async ({ page }) => {
    await page
      .locator(".titlebar-center")
      .getByRole("button", { name: "Workflows" })
      .click();
    await expect(page.locator('[data-testid="workflow-builder"]')).toBeVisible();

    await page.locator('[data-testid="add-block-button"]').click();
    await expect(page.getByRole("heading", { name: "Block Library" })).toBeVisible();

    await page.getByRole("button", { name: /Manual Trigger/ }).click();

    const manualTriggerNode = page
      .locator(".react-flow__node")
      .filter({ hasText: "Manual Trigger" });
    await expect(manualTriggerNode).toHaveCount(1);
    await expect(manualTriggerNode).toBeVisible();
    await expect(page.locator('[data-testid="run-workflow-button"]')).toBeDisabled();
  });
});
