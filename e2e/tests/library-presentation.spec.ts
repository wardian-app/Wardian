import { test, expect } from '@playwright/test';
import { mkdirSync } from 'node:fs';
import { buildLibraryContentFixture, buildLibraryIndexFixture, installLibraryIpcMock } from '../fixtures/libraryIpcMock';
import { openSurface } from '../fixtures/workbench';
import type { LibraryIndex } from '../../src/types';

test('browses class contents, previews a skill, and saves a draft from Preview', async ({ page }) => {
  const errors: string[] = [];
  page.on('pageerror', (error) => errors.push(error.message));
  page.on('console', (message) => { if (message.type() === 'error') errors.push(message.text()); });
  const index: LibraryIndex = buildLibraryIndexFixture();
  index.deployments = {
    'skills/dev/planner': [{ target_type: 'class', target_id: 'Architect', linked: true }],
    'skills/ops/reviewer': [{ target_type: 'class', target_id: 'Architect', linked: false }, { target_type: 'user', target_id: 'global', linked: true }],
  };
  const content = buildLibraryContentFixture();
  content['classes/Architect'] = '# Architect\n\nTurn product requirements into clear, reviewable system designs.\n\n## Working principles\n\n- Identify ownership and boundaries.\n- Make tradeoffs explicit.\n- Keep designs grounded in the existing system.\n\n## Deliverables\n\n| Artifact | Purpose |\n| --- | --- |\n| Design brief | Explain the proposed behavior |\n| Implementation plan | Define bounded work and evidence |';
  content['skills/dev/planner'] = '---\nname: planner\ndescription: Plans work\n---\n# Planner\n\nCreate a concrete plan that connects the requested outcome to implementation and verification.\n\n## Steps\n\n1. Inspect the current system.\n2. Define the smallest coherent change.\n3. Choose evidence for each requirement.\n\n## Checklist\n\n- [x] Scope and owner identified\n- [ ] Verification recorded\n\n```sh\nnpm run verify:ci\n```\n\n<details><summary>Planning notes</summary>Keep each phase independently reviewable.</details>';
  await installLibraryIpcMock(page, index, content);
  await page.setViewportSize({ width: 1920, height: 1080 });
  await page.goto('/', { waitUntil: 'domcontentloaded' });
  await expect(page.getByTestId('app-shell')).toBeVisible({ timeout: 15_000 });
  await openSurface(page, 'library');
  await page.getByTestId('library-section-classes').click();
  await expect(page.getByTestId('library-row-classes/Architect')).toContainText('2 class skills');
  await page.getByTestId('library-row-classes/Architect').click();
  const contents = page.getByRole('region', { name: 'Class contents' });
  await expect(contents.getByText('Plans work')).toBeVisible();
  await expect(contents.getByText("copied — edits won't sync")).toBeVisible();
  await expect(page.getByRole('table')).toBeVisible();
  await expect(page.getByText(/Invalid workbench/)).toHaveCount(0);
  const evidence = `e2e/screenshots/library/${new Date().toISOString().replace(/[:.]/g, '-')}`;
  mkdirSync(evidence, { recursive: true });
  await expect(page.getByTestId('workbench-persistence-notice')).toHaveCount(0);
  await page.getByTestId('library-view').screenshot({ path: `${evidence}/class-contents.png` });
  await contents.getByRole('button', { name: 'planner', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'Planner', exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Copy Sh code' })).toBeVisible();
  await expect(page.getByTestId('workbench-persistence-notice')).toHaveCount(0);
  await page.getByTestId('library-view').screenshot({ path: `${evidence}/skill-preview.png` });
  await page.getByRole('button', { name: 'Edit', exact: true }).click();
  const source = page.getByRole('textbox', { name: 'Markdown source' });
  await expect(source).toHaveValue(content['skills/dev/planner']);
  await source.fill(`${content['skills/dev/planner']}\n\n## Saved from Preview\nA reviewable draft.`);
  await expect(page.getByTestId('workbench-persistence-notice')).toHaveCount(0);
  await page.getByTestId('library-view').screenshot({ path: `${evidence}/skill-editor.png` });
  await page.getByRole('button', { name: 'Preview', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'Saved from Preview' })).toBeVisible();
  await page.getByRole('button', { name: 'Save', exact: true }).click();
  await expect(page.getByTestId('markdown-editor-dirty-dot')).toHaveCount(0);
  await page.getByTestId('library-section-prompts').click();
  await page.getByTestId('library-row-prompts/greeting.md').click();
  await page.getByTestId('library-section-skills').click();
  await page.getByTestId('library-search').fill('planner');
  await page.getByTestId('library-row-skills/dev/planner').click();
  await expect(page.getByRole('heading', { name: 'Saved from Preview' })).toBeVisible();
  await page.setViewportSize({ width: 800, height: 900 });
  await expect(page.getByRole('button', { name: 'Edit', exact: true })).toBeInViewport();
  await expect(page.getByRole('button', { name: 'Back to library list' })).toBeInViewport();
  await expect(page.getByTestId('workbench-persistence-notice')).toHaveCount(0);
  await page.getByTestId('library-view').screenshot({ path: `${evidence}/narrow-preview.png` });
  // At an intermediate width CSS caps the pane below its preferred width.
  // Resizing must start at that visible edge, without a dead zone or jump.
  await page.setViewportSize({ width: 1380, height: 900 });
  const detail = page.getByTestId('library-detail');
  const handle = detail.getByTestId('sidebar-resize-handle');
  await expect(handle).toBeInViewport();
  const before = await detail.boundingBox();
  const grip = await handle.boundingBox();
  expect(before).not.toBeNull();
  expect(grip).not.toBeNull();
  await page.mouse.move(grip!.x + 2, grip!.y + 100);
  await page.mouse.down();
  await page.mouse.move(grip!.x + 42, grip!.y + 100, { steps: 4 });
  await page.mouse.up();
  await expect.poll(async () => (await detail.boundingBox())?.width).toBeCloseTo(before!.width - 40, 0);
  expect(errors).toEqual([]);
});
