/**
 * Agent Lifecycle E2E tests.
 *
 * Tests are split into two groups:
 *   1. UI-only (browser E2E): spawn form interaction, validation, empty-state guards.
 *   2. @native-only: tests that require an actually-running mock agent.
 *      These are skipped here and belong in the native E2E harness.
 *      Run via: npm run test:e2e:native
 *
 * Why @native-only? The mock provider is not exposed in the spawn form dropdown,
 * and status indicator transitions require real PTY/IPC events from the backend.
 * See e2e/fixtures/mockAgent.ts for the unlock path.
 */

import { test, expect, type Page } from "@playwright/test";

test.describe("Agent Spawn Form", () => {
  test.describe.configure({ mode: "serial" });

  let page: Page;

  test.beforeAll(async ({ browser }) => {
    page = await browser.newPage();
    await page.goto("/", { waitUntil: "domcontentloaded" });
    await page.locator('[data-testid="app-shell"]').waitFor({ timeout: 15_000 });
    await page.locator('[data-testid="sidebar-tab-agent-config"]').click();
    await page.locator('[data-testid="spawn-agent-name"]').waitFor();
  });

  test.afterAll(async () => {
    await page.close();
  });

  test("spawn form renders all required fields", async () => {
    await expect(page.locator('[data-testid="spawn-agent-name"]')).toBeVisible();
    await expect(page.locator('[data-testid="spawn-workspace-path"]')).toBeVisible();
    await expect(page.locator('[data-testid="spawn-provider"]')).toBeVisible();
    await expect(page.locator('[data-testid="spawn-submit"]')).toBeVisible();
  });

  test("submit button is present when name is filled", async () => {
    await page.locator('[data-testid="spawn-agent-name"]').fill("test-agent");
    await expect(page.locator('[data-testid="spawn-submit"]')).toBeVisible();
  });

  test("provider dropdown has expected options", async () => {
    const select = page.locator('[data-testid="spawn-provider"]');
    await expect(select).toBeVisible();
    const options = await select.locator("option").allTextContents();
    expect(options).toContain("Claude");
    expect(options.length).toBeGreaterThanOrEqual(3);
  });

  test("workspace path field accepts input", async () => {
    const input = page.locator('[data-testid="spawn-workspace-path"]');
    await input.fill("C:/projects/test");
    await expect(input).toHaveValue("C:/projects/test");
  });

  test("grid is empty before any agent is spawned", async () => {
    await page.getByRole("button", { name: "Grid" }).click();
    const cards = page.locator('[data-testid="agent-card"]');
    await expect(cards).toHaveCount(0);
  });

  test("watchlist is empty before any agent is spawned", async () => {
    const watchlist = page.locator('[data-testid="agent-watchlist"]');
    await expect(watchlist).toBeVisible();
    // No agent rows expected in empty state.
    const cards = page.locator('[data-testid="agent-card"]');
    await expect(cards).toHaveCount(0);
  });
});

// @native-only: the tests below require a running mock agent.
// The mock provider is not selectable via the spawn form dropdown.
// Unlock path: add "mock" to SpawnAgentPanel provider options, then
// use seededHome() fixture from e2e/fixtures/mockAgent.ts.
test.describe("Agent Status Transitions (@native-only)", () => {
  test.skip("spawning agent appears in watchlist with Processing status", async () => {
    // Spawn mock agent (basic scenario).
    // Assert agent card appears in watchlist.
    // Assert status indicator has cyan (Processing) color.
  });

  test.skip("agent transitions to Idle after turn completes", async () => {
    // Spawn mock agent (basic scenario).
    // Wait for turn_completed event.
    // Assert status indicator has emerald (Idle) color.
  });

  test.skip("action_needed scenario shows Amber status indicator", async () => {
    // Spawn mock agent (action_needed scenario).
    // Assert status indicator has amber color.
  });

  test.skip("killing agent removes it from watchlist and grid", async () => {
    // Spawn mock agent.
    // Trigger kill via UI.
    // Assert agent card count drops to 0.
  });
});
