import { test, expect, type Page } from '@playwright/test';
import * as fs from 'node:fs';

async function installResponsiveLayoutIpcMock(page: Page) {
  await page.addInitScript(() => {
    type Agent = {
      session_id: string;
      session_name: string;
      agent_class: string;
      folder: string;
      provider: string;
      is_off: boolean;
    };

    const agents: Agent[] = [
      {
        session_id: 'responsive-agent-1',
        session_name: 'Responsive Alpha',
        agent_class: 'Coder',
        folder: '<absolute-workspace-path>',
        provider: 'mock',
        is_off: false,
      },
      {
        session_id: 'responsive-agent-2',
        session_name: 'Responsive Beta',
        agent_class: 'Reviewer',
        folder: '<absolute-workspace-path>',
        provider: 'mock',
        is_off: false,
      },
    ];
    let callbackId = 1;
    const callbacks = new Map<number, unknown>();
    const tauriWindow = window as Window & {
      __TAURI_INTERNALS__?: Record<string, unknown>;
      __TAURI_EVENT_PLUGIN_INTERNALS__?: Record<string, unknown>;
    };

    tauriWindow.__TAURI_EVENT_PLUGIN_INTERNALS__ = {
      unregisterListener: () => undefined,
    };

    tauriWindow.__TAURI_INTERNALS__ = {
      metadata: {
        currentWindow: { label: 'main' },
        currentWebview: { label: 'main' },
      },
      transformCallback: (callback: unknown) => {
        const id = callbackId++;
        callbacks.set(id, callback);
        return id;
      },
      unregisterCallback: (id: number) => {
        callbacks.delete(id);
      },
      convertFileSrc: (filePath: string) => filePath,
      invoke: async (command: string) => {
        if (command === 'list_agents') return agents;
        if (command === 'list_agent_classes') return [];
        if (command === 'list_provider_readiness') return [];
        if (command === 'load_watchlists') return [];
        if (command === 'load_watchlist_prefs') return null;
        if (command === 'load_agent_interactions') return {};
        if (command === 'load_queue_items') return [];
        if (command === 'load_queue_preferences') return {};
        if (command === 'load_onboarding_hints') return { dismissed_hint_ids: ['spawn-agent-first-run:v1'] };
        if (command === 'list_workflows') return [];
        if (command === 'list_scheduled_runs') return [];
        if (command === 'load_workflow_library') return { folders: [], rootWorkflowIds: [] };
        if (command === 'get_library_tree') return { type: 'Folder', path: '', name: 'Root', children: [] };
        if (command === 'list_deployed_skills') return [];
        if (command === 'sync_provider_theme_settings') return null;
        if (command === 'plugin:event|listen') return callbackId++;
        if (command === 'plugin:event|unlisten') return null;
        return null;
      },
    };
  });
}

test.describe('responsive layout', () => {
  test('left sidebar width persists across reload', async ({ page }) => {
    await page.goto('/', { waitUntil: "domcontentloaded" });
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
    await installResponsiveLayoutIpcMock(page);
    await page.goto('/', { waitUntil: "domcontentloaded" });
    const agentCards = page.locator('[data-testid="agent-card"]');
    await expect(agentCards).toHaveCount(2);

    const handle = page.locator('[data-resize-handle="h"]').first();
    await expect(handle).toBeVisible();

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
    fs.mkdirSync('e2e/screenshots/responsive-layout', { recursive: true });
    await grid.screenshot({ path: 'e2e/screenshots/responsive-layout/stacked-mode.png' });

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
