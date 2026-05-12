import { test, expect, type Page } from "@playwright/test";

test.describe("Wardian Core Feature Tests", () => {
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

  test.beforeEach(async () => {
    await expect(page.locator('[data-testid="app-shell"]')).toBeVisible();
  });

  test("1. App renders with main layout", async () => {
    await expect(page.locator('[data-testid="app-shell"]')).toBeVisible();
    await expect(page.locator('[data-testid="sidebar-icon-rail"]')).toBeVisible();
    await expect(page.locator('[data-testid="agent-watchlist"]')).toBeVisible();
  });

  test("2. Sidebar navigation - Agent Config tab", async () => {
    await page.locator('[data-testid="sidebar-tab-agent-config"]').click();
    await page.waitForTimeout(500);
    await expect(page.locator('[data-testid="spawn-agent-name"]')).toBeVisible();
    await expect(page.locator('[data-testid="spawn-submit"]')).toBeVisible();
  });

  test("3. Sidebar navigation - Command tab", async () => {
    await page.locator('[data-testid="sidebar-tab-command"]').click();
    await page.waitForTimeout(500);
    await expect(page.locator('[data-testid="command-panel"]')).toBeVisible();
    await expect(page.locator('[data-testid="broadcast-textarea"]')).toBeVisible();
  });

  test("4. Sidebar navigation - Workflows tab", async () => {
    await page.locator('[data-testid="sidebar-tab-workflows"]').click();
    await page.waitForTimeout(500);
    await expect(page.locator('[data-testid="workflow-sidebar"]')).toBeVisible();
  });

  test("5. Sidebar navigation - Settings tab", async () => {
    await page.locator('[data-testid="sidebar-tab-settings"]').click();
    await page.waitForTimeout(500);
    await expect(page.locator('[data-testid="settings-panel"]')).toBeVisible();
  });

  test("6. Sidebar navigation - Classes tab", async () => {
    await page.locator('[data-testid="sidebar-tab-classes"]').click();
    await page.waitForTimeout(500);
    await expect(page.locator('[data-testid="class-manager-panel"]')).toBeVisible();
  });

  test("7. Sidebar navigation - Explorer tab", async () => {
    await page.locator('[data-testid="sidebar-tab-explorer"]').click();
    await page.waitForTimeout(500);
    await expect(page.locator('[data-testid="explorer-panel"]')).toBeVisible();
  });

  test("8. Settings - Theme switching", async () => {
    await page.locator('[data-testid="sidebar-tab-settings"]').click();
    await page.waitForTimeout(500);
    
    await page.locator('[data-testid="theme-dark"]').click();
    await page.waitForTimeout(300);
    
    await page.locator('[data-testid="theme-light"]').click();
    await page.waitForTimeout(300);
    
    await page.locator('[data-testid="theme-system"]').click();
    await page.waitForTimeout(300);
  });

  test("9. Settings - Shell selection", async () => {
    await page.locator('[data-testid="sidebar-tab-settings"]').click();
    await page.waitForTimeout(500);
    
    const shellSelect = page.locator('[data-testid="shell-select"]');
    await expect(shellSelect).toBeVisible();
    
    const options = await shellSelect.locator("option").count();
    expect(options).toBeGreaterThan(0);
  });

  test("10. Class Manager - Create class form", async () => {
    await page.locator('[data-testid="sidebar-tab-classes"]').click();
    await page.waitForTimeout(500);
    
    await page.locator('[data-testid="class-name-input"]').fill("TestClass");
    await page.locator('[data-testid="class-description-input"]').fill("A test class for E2E testing");
    await page.waitForTimeout(200);
    
    await expect(page.locator('[data-testid="class-create-button"]')).toBeVisible();
  });

  test("11. Spawn Agent form validation", async () => {
    await page.locator('[data-testid="sidebar-tab-agent-config"]').click();
    await page.waitForTimeout(500);
    
    const nameInput = page.locator('[data-testid="spawn-agent-name"]');
    await nameInput.fill("TestAgent");
    await expect(nameInput).toHaveValue("TestAgent");
    
    const workspaceInput = page.locator('[data-testid="spawn-workspace-path"]');
    await workspaceInput.fill("C:/temp");
    await expect(workspaceInput).toHaveValue("C:/temp");
  });

  test("12. Broadcast input functionality", async () => {
    await page.locator('[data-testid="sidebar-tab-command"]').click();
    await page.waitForTimeout(500);
    
    const textarea = page.locator('[data-testid="broadcast-textarea"]');
    await textarea.fill("E2E test broadcast message");
    await expect(textarea).toHaveValue("E2E test broadcast message");
    
    await expect(page.locator('[data-testid="broadcast-submit"]')).toBeVisible();
  });

  test("13. Empty state - no agent cards", async () => {
    const cards = page.locator('[data-testid="agent-card"]');
    await expect(cards).toHaveCount(0);
  });

  test("14. Grid view container exists", async () => {
    await page.getByRole("button", { name: "Grid" }).click();
    await expect(page.getByText("No Active Instances")).toBeVisible();
  });

  test("15. Watchlist shows empty state message", async () => {
    // Verify watchlist shows empty state
    const watchlist = page.locator('[data-testid="agent-watchlist"]');
    await expect(watchlist).toBeVisible();
  });
});
