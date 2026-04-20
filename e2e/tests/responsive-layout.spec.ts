import { test, expect } from '@playwright/test';

test.describe('responsive layout', () => {
  test('left sidebar width persists across reload', async ({ page }) => {
    await page.goto('/');
    // First aside is the fixed-width SidebarIconRail; the resize handle's parent aside is the resizable pane.
    const handle = page.getByTestId('sidebar-resize-handle').first();
    const sidebar = handle.locator('xpath=ancestor::aside[1]');
    const initial = await sidebar.evaluate((el) => el.getBoundingClientRect().width);

    const box = await handle.boundingBox();
    if (!box) throw new Error('handle not visible');

    await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
    await page.mouse.down();
    await page.mouse.move(box.x + 60, box.y + box.height / 2, { steps: 10 });
    await page.mouse.up();

    const grown = await sidebar.evaluate((el) => el.getBoundingClientRect().width);
    expect(grown).toBeGreaterThan(initial + 30);

    await page.reload();
    const persisted = await sidebar.evaluate((el) => el.getBoundingClientRect().width);
    expect(Math.round(persisted)).toBe(Math.round(grown));
  });

  test('grid drag past 2/3 enters stacked mode; stack-exit drag restores multi-column', async ({ page }) => {
    await page.goto('/');
    // Pre-condition: at least 2 mock agents present in fixtures.
    const handle = page.locator('[data-resize-handle="h"]').first();
    if (!(await handle.isVisible())) test.skip();

    const grid = page.locator('[data-testid="agent-grid"]');
    const gridBox = await grid.boundingBox();
    const handleBox = await handle.boundingBox();
    if (!gridBox || !handleBox) throw new Error('grid not visible');

    // Drag the inter-column gutter past 2/3 of the grid → stacked.
    await page.mouse.move(handleBox.x + handleBox.width / 2, handleBox.y + handleBox.height / 2);
    await page.mouse.down();
    await page.mouse.move(gridBox.x + gridBox.width - 5, handleBox.y, { steps: 20 });
    await page.mouse.up();

    const exitHandle = page.locator('[data-resize-handle="stack-exit"]').first();
    await expect(exitHandle).toBeVisible();

    // Drag the stack-exit handle inward to the middle of the grid → exit stacked.
    const exitBox = await exitHandle.boundingBox();
    if (!exitBox) throw new Error('stack-exit handle not visible');
    await page.mouse.move(exitBox.x + exitBox.width / 2, exitBox.y + exitBox.height / 2);
    await page.mouse.down();
    await page.mouse.move(gridBox.x + gridBox.width / 2, exitBox.y + exitBox.height / 2, { steps: 20 });
    await page.mouse.up();

    await expect(page.locator('[data-resize-handle="stack-exit"]')).toHaveCount(0);
  });
});
