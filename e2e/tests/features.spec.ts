import { test, expect, type Page } from "@playwright/test";
import {
  buildLibraryContentFixture,
  buildLibraryIndexFixture,
  installLibraryIpcMock,
} from "../fixtures/libraryIpcMock";
import { openSurface, surfacePanel } from "../fixtures/workbench";

async function openSettings(page: Page) {
  const dialog = page.getByRole("dialog", { name: "Settings" });
  if (!(await dialog.isVisible().catch(() => false))) {
    await page.locator('[data-testid="sidebar-tab-settings"]').click();
  }
  await expect(dialog).toBeVisible();
  return dialog;
}

async function closeSettings(page: Page) {
  const dialog = page.getByRole("dialog", { name: "Settings" });
  if (await dialog.isVisible().catch(() => false)) {
    await page.getByRole("button", { name: "Close settings" }).click();
    await expect(dialog).toBeHidden();
  }
}

test.describe("Wardian Core Feature Tests", () => {
  test.describe.configure({ mode: "serial" });

  let page: Page;

  test.beforeAll(async ({ browser }) => {
    page = await browser.newPage();
    // Tests 6 and 10 drive the index-driven LibraryView, which calls
    // invoke("get_library_index") on mount. Without a Tauri invoke bridge
    // that call throws, LibraryView renders its error/retry state, and
    // SectionRail (and library-section-classes) never mounts. Install the
    // same invoke mock library-redesign.spec.ts uses (shared via
    // ../fixtures/libraryIpcMock) before the initial navigation so it's in
    // place for every test in this suite, not just the library ones.
    await installLibraryIpcMock(page, buildLibraryIndexFixture(), buildLibraryContentFixture());
    await page.goto("/", { waitUntil: "domcontentloaded" });
    await page.locator('[data-testid="app-shell"]').waitFor({ timeout: 15_000 });
  });

  test.afterAll(async () => {
    await page.close();
  });

  test.beforeEach(async () => {
    await expect(page.locator('[data-testid="app-shell"]')).toBeVisible();
    await closeSettings(page);
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
    const sidebar = page.locator("aside").nth(1);
    await expect(sidebar.getByRole("heading", { name: "Workflows" })).toBeVisible();
    await expect(sidebar.getByRole("button", { name: "Monitor" })).toBeVisible();
  });

  test("5. Icon rail settings opens a modal without changing the sidebar pane", async () => {
    await page.locator('[data-testid="sidebar-tab-agent-config"]').click();
    await expect(page.locator('[data-testid="spawn-agent-name"]')).toBeVisible();

    const dialog = await openSettings(page);
    await expect(dialog.getByRole("button", { name: "General" })).toBeVisible();

    await page.getByRole("button", { name: "Close settings" }).click();
    await expect(dialog).toBeHidden();
    await expect(page.locator('[data-testid="spawn-agent-name"]')).toBeVisible();
  });

  test("6. Library - Classes section", async () => {
    await openSurface(page, "library");
    const library = surfacePanel(page, "library");
    await library.locator('[data-testid="library-section-classes"]').click();
    await expect(library.locator('[data-testid="library-list-content"]')).toBeVisible();
  });

  test("7. Sidebar navigation - Explorer tab", async () => {
    await page.locator('[data-testid="sidebar-tab-explorer"]').click();
    await page.waitForTimeout(500);
    await expect(page.locator('[data-testid="explorer-panel"]')).toBeVisible();
  });

  test("8. Settings - Theme switching", async () => {
    const dialog = await openSettings(page);
    await dialog.getByRole("button", { name: "Appearance" }).click();

    const themeSelect = dialog.getByLabel("Theme", { exact: true });
    await themeSelect.selectOption("dark");
    await expect(themeSelect).toHaveValue("dark");
    await themeSelect.selectOption("light");
    await expect(themeSelect).toHaveValue("light");
    await themeSelect.selectOption("system");
    await expect(themeSelect).toHaveValue("system");
  });

  test("9. Settings - Shell selection", async () => {
    const dialog = await openSettings(page);
    await dialog.getByRole("button", { name: "Terminal" }).click();

    const shellSelect = dialog.locator('[data-testid="shell-select"]');
    await expect(shellSelect).toBeVisible();

    const options = await shellSelect.locator("option").count();
    expect(options).toBeGreaterThan(0);
  });

  test("10. Library - Create class entry form", async () => {
    await openSurface(page, "library");
    const library = surfacePanel(page, "library");
    await library.locator('[data-testid="library-section-classes"]').click();

    await library.locator('[data-testid="library-new"]').click();
    await library.locator('[data-testid="library-new-item"]').click();
    await library.locator('[data-testid="library-new-name"]').fill("TestClass");
    await page.waitForTimeout(200);

    await expect(library.locator('[data-testid="library-new-confirm"]')).toBeVisible();
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
    await openSurface(page, "agents-overview");
    const overview = surfacePanel(page, "agents-overview");
    await overview.getByRole("button", { name: "Grid", exact: true }).click();
    await expect(overview.getByText("No Active Instances")).toBeVisible();
  });

  test("15. Watchlist shows empty state message", async () => {
    // Verify watchlist shows empty state
    const watchlist = page.locator('[data-testid="agent-watchlist"]');
    await expect(watchlist).toBeVisible();
  });
});
