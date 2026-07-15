# Workflow Monitor Horizontal Overflow Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Keep the workflow monitor's six-column activity table usable inside narrow workbench panes through pane-local horizontal scrolling.

**Architecture:** The existing history viewport remains the sole vertical virtualization owner and becomes the shared native two-axis scroller. A 960-pixel inner table owns the column layout, while monitor stats, errors, Activity header, filters, and empty state remain outside that intrinsic-width boundary.

**Tech Stack:** React 19, TypeScript, Tailwind CSS utilities, Vitest and Testing Library, Playwright.

## Global Constraints

- The activity table minimum width is exactly 960 pixels.
- Statistics, errors, Activity header, and filters remain fixed to the pane width.
- All activity sections share one horizontal scroll position; do not introduce per-row or per-section scrollbars.
- Existing vertical history virtualization continues to use `workflow-history-scroll`.
- Use Wardian theme tokens and existing component styles; add no dependency.
- Preserve the user-owned `package-lock.json` modification.

---

### Task 1: Add and prove pane-local monitor table overflow

**Files:**
- Modify: `src/features/workflows/monitor/WorkflowMonitor.tsx:173-230`
- Modify: `src/features/workflows/monitor/WorkflowMonitor.tsx:476-482`
- Test: `src/features/workflows/monitor/WorkflowMonitor.test.tsx:748-765`
- Test: `e2e/tests/workflows.spec.ts`
- Modify: `docs/guide/workflows.md:76-90`
- Create: `e2e/screenshots/workflow-monitor-horizontal-overflow/narrow-pane.png`

**Interfaces:**
- Consumes: the existing `historyScrollRef: RefObject<HTMLDivElement | null>` used by `VirtualHistoryRows`.
- Produces: `workflow-history-scroll`, a labeled two-axis scroll region, and `workflow-activity-table`, its 960-pixel-minimum table child.

- [ ] **Step 1: Strengthen the monitor scroll unit test**

Replace the existing bounded-scroll test in `WorkflowMonitor.test.tsx` with:

```tsx
it('keeps wide activity columns inside one labeled two-axis scroll pane', () => {
  runState.runs = [{
    run_id: 'run-scroll',
    blueprint_id: 'audit',
    status: 'completed',
    node_count: 2,
    path: '/runs/scroll',
    updated_at: '2026-06-01T16:00:00Z',
  }];

  render(<WorkflowMonitor onOpenRun={vi.fn()} onEditSchedule={vi.fn()} />);
  fireEvent.click(screen.getByRole('button', { name: /history/i }));

  expect(screen.getByRole('region', { name: 'Workflow activity' }))
    .toHaveClass('flex-1', 'min-h-0', 'overflow-auto');
  expect(screen.getByTestId('workflow-activity-table')).toHaveClass('min-w-[960px]');
});
```

- [ ] **Step 2: Add the narrow-pane Playwright regression**

Append to `e2e/tests/workflows.spec.ts`:

```ts
test('workflow monitor contains wide activity columns inside a narrow surface', async ({ page }) => {
  await installWorkflowsIpcMock(page);
  await page.setViewportSize({ width: 1200, height: 700 });
  await page.goto('/', { waitUntil: 'domcontentloaded' });
  await page.locator('[data-testid="app-shell"]').waitFor({ timeout: 15_000 });

  await openSurface(page, 'workflows');
  await page.getByTestId('workflows-view').getByRole('button', { name: /^monitor$/i }).click();
  await page.evaluate(async () => {
    const { useRunStore } = await import('/src/features/workflows/run/useRunStore.ts');
    useRunStore.setState({
      loadRuns: async () => undefined,
      runs: [{
        run_id: 'run-monitor-overflow',
        blueprint_id: 'wf',
        status: 'completed',
        node_count: 3,
        failure: null,
        path: '/runs/run-monitor-overflow',
        updated_at: '2026-07-15T12:00:00Z',
      }],
    });
  });
  await page.getByTestId('workflow-monitor').getByRole('button', { name: /^history$/i }).click();

  const scroller = page.getByRole('region', { name: 'Workflow activity' });
  const table = page.getByTestId('workflow-activity-table');
  await expect(table).toBeVisible();

  const geometry = await scroller.evaluate((element) => ({
    clientWidth: element.clientWidth,
    scrollWidth: element.scrollWidth,
    documentClientWidth: document.documentElement.clientWidth,
    documentScrollWidth: document.documentElement.scrollWidth,
  }));
  expect(geometry.scrollWidth).toBeGreaterThan(geometry.clientWidth);
  expect(geometry.documentScrollWidth).toBeLessThanOrEqual(geometry.documentClientWidth);
  expect(await table.evaluate((element) => element.getBoundingClientRect().width)).toBeGreaterThanOrEqual(960);

  await scroller.evaluate((element) => {
    element.scrollLeft = element.scrollWidth - element.clientWidth;
  });
  await expect.poll(() => scroller.evaluate((element) => element.scrollLeft)).toBeGreaterThan(0);

  await page.getByTestId('workflow-monitor').screenshot({
    path: 'e2e/screenshots/workflow-monitor-horizontal-overflow/narrow-pane.png',
  });
});
```

- [ ] **Step 3: Run both regressions and verify they fail on the clipped layout**

Run:

```powershell
npm run test -- src/features/workflows/monitor/WorkflowMonitor.test.tsx
npx playwright test --config e2e/playwright.config.ts e2e/tests/workflows.spec.ts --grep "contains wide activity columns"
```

Expected: the unit test cannot find the labeled region or table, and the browser test cannot find `workflow-activity-table`.

- [ ] **Step 4: Implement the shared two-axis scroll boundary**

In `WorkflowMonitor.tsx`, replace the existing `workflow-history-scroll` body with:

```tsx
<div
  ref={historyScrollRef}
  data-testid="workflow-history-scroll"
  role="region"
  aria-label="Workflow activity"
  className="min-h-0 flex-1 overflow-auto p-3"
>
  {hasVisibleActivity ? (
    <div data-testid="workflow-activity-table" className="min-w-[960px]">
      {visibleSections.map((section) => {
        const isHistorySection = historyFilterActive && section === 'history';
        const items = isHistorySection ? [] : groupedActivities[section];
        const historyItems = isHistorySection ? visibleHistoryRuns : [];
        if (items.length === 0 && historyItems.length === 0) return null;
        return (
          <ActivitySection
            key={section}
            title={SECTION_LABELS[section]}
            activities={items}
            olderRuns={historyItems}
            schedulesById={schedulesById}
            remainingOlderRuns={isHistorySection ? Math.max(0, historyRuns.length - visibleHistoryRuns.length) : 0}
            agentLabels={agentLabels}
            historyScrollRef={historyScrollRef}
            onOpenRun={onOpenRun}
            onPause={pause}
            onResume={resume}
            onRunNow={runNow}
            onEditSchedule={onEditSchedule}
            onShowMoreOlderRuns={() => setVisibleOlderHistoryCount((count) => Math.min(historyRuns.length, count + HISTORY_PAGE_SIZE))}
            onResetOlderRuns={() => setVisibleOlderHistoryCount(0)}
            canResetOlderRuns={isHistorySection && visibleOlderHistoryCount > 0}
          />
        );
      })}
    </div>
  ) : (
    <div className="select-text rounded border border-dashed border-wardian-border p-4 text-center text-xs text-muted">
      No workflow activity in this view.
    </div>
  )}
</div>
```

Replace the `ActivityRow` grid class with:

```tsx
className="grid grid-cols-[minmax(120px,170px)_minmax(120px,1fr)_minmax(120px,150px)_minmax(150px,190px)_minmax(140px,220px)_112px] items-start gap-x-4 gap-y-1 border-b border-wardian-border/70 bg-[var(--color-wardian-bg)] px-3 py-2 last:border-b-0 hover:bg-[color-mix(in_srgb,var(--color-wardian-card),transparent_45%)]"
```

- [ ] **Step 5: Document the pane behavior**

Add under `## Monitoring Workflow Activity` in `docs/guide/workflows.md`:

```markdown
In a narrow Workflows pane, the activity table scrolls horizontally while its
summary and filters stay fixed. This preserves the monitor's operational columns
without widening the pane or collapsing the surrounding workbench layout.
```

- [ ] **Step 6: Run focused and repository verification**

Run:

```powershell
npm run test -- src/features/workflows/monitor/WorkflowMonitor.test.tsx
npx playwright test --config e2e/playwright.config.ts e2e/tests/workflows.spec.ts --grep "contains wide activity columns"
npm run lint
npm run build
npm run docs:check-llms
git diff --check
```

Expected: all commands exit 0. The browser test writes `e2e/screenshots/workflow-monitor-horizontal-overflow/narrow-pane.png`.

- [ ] **Step 7: Commit the implementation atomically**

```powershell
git add -- src/features/workflows/monitor/WorkflowMonitor.tsx src/features/workflows/monitor/WorkflowMonitor.test.tsx e2e/tests/workflows.spec.ts docs/guide/workflows.md e2e/screenshots/workflow-monitor-horizontal-overflow/narrow-pane.png
git commit -m "fix(workflows): scroll monitor columns within narrow panes"
```

Do not stage `package-lock.json`.
