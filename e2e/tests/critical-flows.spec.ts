import { expect, test, type Page } from "@playwright/test";

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

  test("command broadcast asks for confirmation when no agents are selected", async () => {
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

  test("workflow builder can add a manual trigger block from the library", async () => {
    await page
      .locator(".titlebar-center")
      .getByRole("button", { name: "Workflows" })
      .click();
    await expect(page.getByTestId("workflows-view")).toBeVisible();
    await expect(page.getByTestId("workflows-edit-mode")).toBeVisible();

    await page.getByTestId("workflows-view").getByRole("button", { name: "Add node" }).click();
    await expect(page.getByTestId("node-library")).toBeVisible();

    await page.getByRole("button", { name: /Manual Trigger/ }).click();

    const manualTriggerNode = page
      .locator(".react-flow__node")
      .filter({ hasText: "Manual Trigger" });
    await expect(manualTriggerNode).toHaveCount(1);
    await expect(manualTriggerNode).toBeVisible();
    await expect(page.getByTestId("workflows-view").getByRole("button", { name: /^Run$/ })).toBeDisabled();
  });
});
