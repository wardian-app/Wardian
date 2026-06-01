/**
 * Workflow Builder E2E tests.
 *
 * Tests are split into two groups:
 *   1. UI-only (browser E2E): builder renders, block palette, navigation.
 *   2. @native-only: tests that require live agent blocks executing inside
 *      a running workflow. These need the native E2E harness.
 *      Run via: npm run test:e2e:native
 */

import { test, expect, type Page } from "@playwright/test";

test.describe("Workflow Builder UI", () => {
  test.describe.configure({ mode: "serial" });

  let page: Page;

  test.beforeAll(async ({ browser }) => {
    page = await browser.newPage();
    await page.goto("/", { waitUntil: "domcontentloaded" });
    await page.locator('[data-testid="app-shell"]').waitFor({ timeout: 15_000 });
    await page.locator('[data-testid="sidebar-tab-workflows"]').click();
    await page.locator("aside").nth(1).getByRole("heading", { name: "Workflows" }).waitFor();
  });

  test.afterAll(async () => {
    await page.close();
  });

  test("workflow glance pane renders", async () => {
    const sidebar = page.locator("aside").nth(1);
    await expect(sidebar.getByRole("heading", { name: "Workflows" })).toBeVisible();
    await expect(sidebar.getByRole("button", { name: "Monitor" })).toBeVisible();
  });

  test("switching to Workflows view renders the edit canvas", async () => {
    await page.locator(".titlebar-center").getByRole("button", { name: "Workflows" }).click();
    await expect(page.getByTestId("workflows-view")).toBeVisible();
    await expect(page.getByTestId("workflows-edit-mode")).toBeVisible();
    await expect(page.locator(".react-flow")).toBeVisible();
  });

  test("workflow edit mode opens the node library", async () => {
    await page.locator(".titlebar-center").getByRole("button", { name: "Workflows" }).click();
    await page.getByTestId("workflows-view").getByRole("button", { name: "Add node" }).click();
    await expect(page.getByTestId("node-library")).toBeVisible();
    await page.getByTestId("node-library").getByRole("button", { name: "Close" }).click();
    await expect(page.getByTestId("node-library")).toHaveCount(0);
  });

  test("add-node button is visible in edit mode", async () => {
    await page.locator(".titlebar-center").getByRole("button", { name: "Workflows" }).click();
    await expect(page.getByTestId("workflows-view").getByRole("button", { name: "Add node" })).toBeVisible();
  });

  test("run button is disabled for an unsaved empty workflow", async () => {
    await page.locator(".titlebar-center").getByRole("button", { name: "Workflows" }).click();
    await expect(page.getByTestId("workflows-view").getByRole("button", { name: /^Run$/ })).toBeDisabled();
  });
});

// @native-only: the tests below require live agent blocks and real workflow execution.
// Workflow execution triggers agent spawning which requires native IPC + PTY.
test.describe("Workflow Execution (@native-only)", () => {
  test.skip("creating a workflow with two mock-agent blocks shows them on canvas", async () => {
    // Open builder, click add-block, add two mock-agent blocks.
    // Assert two block nodes visible on canvas.
  });

  test.skip("running a workflow transitions block status to Processing", async () => {
    // Create workflow with mock agents.
    // Click run-workflow-button.
    // Assert block status indicators show Processing.
  });

  test.skip("workflow completes and block status transitions to Idle", async () => {
    // Run workflow with basic mock scenario.
    // Assert all blocks reach Idle/completed state.
  });

  test.skip("cancelling a running workflow stops block execution", async () => {
    // Start workflow, then cancel mid-run.
    // Assert blocks stop and workflow shows cancelled state.
  });
});
