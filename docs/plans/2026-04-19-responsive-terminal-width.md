# Responsive Terminal Width Implementation Plan

> **For agentic workers:** Steps use checkbox (`- [ ]`) syntax for tracking. Mark each step as you finish it. Commit after every task.

**Goal:** Let the user reclaim cramped terminal real estate on physically small displays via (a) draggable sidebar widths and (b) a user-forced single-column "stacked" grid mode triggered by dragging a grid column past 2/3 width.

**Architecture:** Extend the existing `useLayoutStore` (Zustand + persist) with three new fields: `leftSidebarWidth`, `rightSidebarWidth`, `gridStacked`. Replace the static CSS variables `--sidebar-content-width` / `--sidebar-secondary-width` with values written from the store at runtime. Add inner-edge drag handles to both sidebars. In `useGridResize`, when the magnetic snap selects `1.0`, flip `gridStacked: true`; `GridView` then forces `gridTemplateColumns: '1fr'`.

**Tech Stack:** React, TypeScript, Zustand (with `persist` middleware), Vitest, React Testing Library, Playwright.

**Spec:** `docs/specs/022-responsive-terminal-width.md`

---

## File Structure

| Path | Action | Responsibility |
|---|---|---|
| `src/store/useLayoutStore.ts` | Modify | Add `leftSidebarWidth`, `rightSidebarWidth`, `gridStacked` and setters; clamp width setters. |
| `src/store/useLayoutStore.test.ts` | Create | Unit tests for new store fields and clamp behavior. |
| `src/views/App.tsx` | Modify | Bridge store → CSS variables on `documentElement.style`; pass `gridStacked` props through to `GridView` (or read directly from store). |
| `src/components/SidebarResizeHandle.tsx` | Create | Reusable 4px drag handle component (left or right edge), emits `(deltaPx) => void`, double-click resets. |
| `src/components/SidebarResizeHandle.test.tsx` | Create | Unit tests for the handle. |
| `src/layout/SidebarContentPane.tsx` | Modify | Render `SidebarResizeHandle` on inner (right) edge when not collapsed; wire to `setLeftSidebarWidth`. |
| `src/layout/watchlist/AgentWatchlist.tsx` | Modify | Render `SidebarResizeHandle` on inner (left) edge when not collapsed; wire to `setRightSidebarWidth`. |
| `src/styles/App.css` | Modify | Keep CSS variables but note they are runtime-controlled (defaults remain as fallback). |
| `src/features/grid/useGridResize.ts` | Modify | When global drag weight snaps to `1.0`, call `setGridStacked(true)` on release. |
| `src/features/grid/useGridResize.test.ts` | Create (if missing) | Unit test for the new stacked-trigger behavior. |
| `src/views/GridView.tsx` | Modify | Honor `gridStacked` in `gridStyle` (`isMobile \|\| gridStacked \|\| isMaximized`); add an "Exit stacked" button when active. |
| `src/views/GridView.test.tsx` | Modify | Extend existing tests with stacked-mode coverage. |
| `e2e/tests/responsive-layout.spec.ts` | Create | Playwright spec: sidebar drag persists across reload; clamp at 40% viewport; grid drag-to-stacked snaps and exit restores. |

---

## Task 1: Extend `useLayoutStore` with sidebar widths

**Files:**
- Modify: `src/store/useLayoutStore.ts`
- Test: `src/store/useLayoutStore.test.ts` (create)

- [ ] **Step 1: Write the failing test**

Create `src/store/useLayoutStore.test.ts`:

```ts
import { describe, it, expect, beforeEach } from 'vitest';
import { act } from '@testing-library/react';
import { useLayoutStore } from './useLayoutStore';

describe('useLayoutStore — sidebar widths', () => {
  beforeEach(() => {
    localStorage.clear();
    act(() => useLayoutStore.getState().resetLayout());
  });

  it('exposes default sidebar widths', () => {
    const s = useLayoutStore.getState();
    expect(s.leftSidebarWidth).toBe(260);
    expect(s.rightSidebarWidth).toBe(240);
  });

  it('setLeftSidebarWidth clamps below 200px to 200', () => {
    act(() => useLayoutStore.getState().setLeftSidebarWidth(50));
    expect(useLayoutStore.getState().leftSidebarWidth).toBe(200);
  });

  it('setLeftSidebarWidth clamps above 40% of window width', () => {
    Object.defineProperty(window, 'innerWidth', { value: 1000, configurable: true });
    act(() => useLayoutStore.getState().setLeftSidebarWidth(900));
    expect(useLayoutStore.getState().leftSidebarWidth).toBe(400);
  });

  it('setRightSidebarWidth applies the same clamps', () => {
    Object.defineProperty(window, 'innerWidth', { value: 1000, configurable: true });
    act(() => useLayoutStore.getState().setRightSidebarWidth(50));
    expect(useLayoutStore.getState().rightSidebarWidth).toBe(200);
    act(() => useLayoutStore.getState().setRightSidebarWidth(900));
    expect(useLayoutStore.getState().rightSidebarWidth).toBe(400);
  });

  it('resetLayout restores sidebar defaults', () => {
    act(() => {
      useLayoutStore.getState().setLeftSidebarWidth(320);
      useLayoutStore.getState().setRightSidebarWidth(320);
    });
    act(() => useLayoutStore.getState().resetLayout());
    expect(useLayoutStore.getState().leftSidebarWidth).toBe(260);
    expect(useLayoutStore.getState().rightSidebarWidth).toBe(240);
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `npm run test -- src/store/useLayoutStore.test.ts`
Expected: FAIL — `leftSidebarWidth`/`setLeftSidebarWidth` undefined.

- [ ] **Step 3: Implement**

Replace `src/store/useLayoutStore.ts` with:

```ts
import { create } from 'zustand';
import { persist } from 'zustand/middleware';
import { GridLayout } from '../types';

const DEFAULT_LEFT_SIDEBAR_WIDTH = 260;
const DEFAULT_RIGHT_SIDEBAR_WIDTH = 240;
const MIN_SIDEBAR_WIDTH = 200;
const MAX_SIDEBAR_FRACTION = 0.4;

const DEFAULT_LAYOUT: GridLayout = {
  column_tracks: [0.5, 0.5],
  row_height: 450,
};

const clampSidebarWidth = (px: number): number => {
  const max = Math.max(MIN_SIDEBAR_WIDTH, Math.floor(window.innerWidth * MAX_SIDEBAR_FRACTION));
  return Math.max(MIN_SIDEBAR_WIDTH, Math.min(max, Math.round(px)));
};

interface LayoutState {
  layout: GridLayout;
  leftSidebarWidth: number;
  rightSidebarWidth: number;
  gridStacked: boolean;
  setColumnTracks: (tracks: number[]) => void;
  setRowHeight: (height: number) => void;
  setLeftSidebarWidth: (px: number) => void;
  setRightSidebarWidth: (px: number) => void;
  setGridStacked: (v: boolean) => void;
  resetLayout: () => void;
}

export const useLayoutStore = create<LayoutState>()(
  persist(
    (set) => ({
      layout: DEFAULT_LAYOUT,
      leftSidebarWidth: DEFAULT_LEFT_SIDEBAR_WIDTH,
      rightSidebarWidth: DEFAULT_RIGHT_SIDEBAR_WIDTH,
      gridStacked: false,
      setColumnTracks: (column_tracks) => set((state) => ({ layout: { ...state.layout, column_tracks } })),
      setRowHeight: (row_height) => set((state) => ({ layout: { ...state.layout, row_height } })),
      setLeftSidebarWidth: (px) => set({ leftSidebarWidth: clampSidebarWidth(px) }),
      setRightSidebarWidth: (px) => set({ rightSidebarWidth: clampSidebarWidth(px) }),
      setGridStacked: (gridStacked) => set({ gridStacked }),
      resetLayout: () => set({
        layout: DEFAULT_LAYOUT,
        leftSidebarWidth: DEFAULT_LEFT_SIDEBAR_WIDTH,
        rightSidebarWidth: DEFAULT_RIGHT_SIDEBAR_WIDTH,
        gridStacked: false,
      }),
    }),
    { name: 'wardian-layout' }
  )
);
```

- [ ] **Step 4: Run test to verify it passes**

Run: `npm run test -- src/store/useLayoutStore.test.ts`
Expected: PASS (5 tests).

- [ ] **Step 5: Commit**

```bash
git add src/store/useLayoutStore.ts src/store/useLayoutStore.test.ts
git commit -m "feat(layout): extend useLayoutStore with sidebar widths and gridStacked"
```

---

## Task 2: Bridge store → CSS variables in `App.tsx`

**Files:**
- Modify: `src/views/App.tsx` (around line 600 — top of `return`)

- [ ] **Step 1: Add the bridge effect**

In `src/views/App.tsx`, near other top-level hooks in the `App` component, add:

```tsx
import { useLayoutStore } from '../store/useLayoutStore';

// inside App component:
const leftSidebarWidth = useLayoutStore((s) => s.leftSidebarWidth);
const rightSidebarWidth = useLayoutStore((s) => s.rightSidebarWidth);

useEffect(() => {
  const root = document.documentElement;
  root.style.setProperty('--sidebar-content-width', `${leftSidebarWidth}px`);
  root.style.setProperty('--sidebar-secondary-width', `${rightSidebarWidth}px`);
}, [leftSidebarWidth, rightSidebarWidth]);
```

(Place near existing `useEffect` blocks; do not duplicate the import if already present.)

- [ ] **Step 2: Manual sanity check**

Run: `npm run tauri dev` (or `npm run dev` if web-only). Open devtools, run in console:
```js
getComputedStyle(document.documentElement).getPropertyValue('--sidebar-content-width')
```
Expected: `260px`. Then in console: `useLayoutStore.setState({ leftSidebarWidth: 320 })` — sidebar should grow.
(If you don't have a debug hook for the store, skip this and rely on the unit tests + Task 4 visual check.)

- [ ] **Step 3: Lint**

Run: `npm run lint`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add src/views/App.tsx
git commit -m "feat(layout): wire useLayoutStore sidebar widths to CSS variables"
```

---

## Task 3: `SidebarResizeHandle` component

**Files:**
- Create: `src/components/SidebarResizeHandle.tsx`
- Test: `src/components/SidebarResizeHandle.test.tsx`

- [ ] **Step 1: Write the failing test**

```tsx
import { describe, it, expect, vi } from 'vitest';
import { render, fireEvent, screen } from '@testing-library/react';
import { SidebarResizeHandle } from './SidebarResizeHandle';

describe('SidebarResizeHandle', () => {
  it('emits cumulative width during pointer drag', () => {
    const onResize = vi.fn();
    render(<SidebarResizeHandle baseWidth={260} edge="right" onResize={onResize} onReset={() => {}} />);
    const handle = screen.getByTestId('sidebar-resize-handle');

    fireEvent.pointerDown(handle, { clientX: 100 });
    fireEvent.pointerMove(window, { clientX: 140 });
    expect(onResize).toHaveBeenLastCalledWith(300); // 260 + 40

    fireEvent.pointerUp(window, { clientX: 140 });
  });

  it('inverts delta when edge="left"', () => {
    const onResize = vi.fn();
    render(<SidebarResizeHandle baseWidth={260} edge="left" onResize={onResize} onReset={() => {}} />);
    const handle = screen.getByTestId('sidebar-resize-handle');

    fireEvent.pointerDown(handle, { clientX: 100 });
    fireEvent.pointerMove(window, { clientX: 60 });
    expect(onResize).toHaveBeenLastCalledWith(300); // 260 + (100 - 60)

    fireEvent.pointerUp(window, { clientX: 60 });
  });

  it('calls onReset on double click', () => {
    const onReset = vi.fn();
    render(<SidebarResizeHandle baseWidth={260} edge="right" onResize={() => {}} onReset={onReset} />);
    fireEvent.doubleClick(screen.getByTestId('sidebar-resize-handle'));
    expect(onReset).toHaveBeenCalledOnce();
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `npm run test -- src/components/SidebarResizeHandle.test.tsx`
Expected: FAIL — module not found.

- [ ] **Step 3: Implement the component**

Create `src/components/SidebarResizeHandle.tsx`:

```tsx
import React, { useCallback, useRef } from 'react';

interface Props {
  baseWidth: number;
  edge: 'left' | 'right';
  onResize: (newWidthPx: number) => void;
  onReset: () => void;
}

export const SidebarResizeHandle: React.FC<Props> = ({ baseWidth, edge, onResize, onReset }) => {
  const startXRef = useRef<number | null>(null);
  const baseRef = useRef<number>(baseWidth);

  const onPointerMove = useCallback((e: PointerEvent) => {
    if (startXRef.current === null) return;
    const delta = e.clientX - startXRef.current;
    const next = edge === 'right' ? baseRef.current + delta : baseRef.current - delta;
    onResize(next);
  }, [edge, onResize]);

  const onPointerUp = useCallback(() => {
    startXRef.current = null;
    window.removeEventListener('pointermove', onPointerMove);
    window.removeEventListener('pointerup', onPointerUp);
    document.body.style.userSelect = '';
    document.body.style.cursor = '';
  }, [onPointerMove]);

  const onPointerDown = (e: React.PointerEvent) => {
    startXRef.current = e.clientX;
    baseRef.current = baseWidth;
    document.body.style.userSelect = 'none';
    document.body.style.cursor = 'col-resize';
    window.addEventListener('pointermove', onPointerMove);
    window.addEventListener('pointerup', onPointerUp);
  };

  return (
    <div
      data-testid="sidebar-resize-handle"
      onPointerDown={onPointerDown}
      onDoubleClick={onReset}
      className={`absolute top-0 bottom-0 w-1 hover:w-1.5 cursor-col-resize z-20 transition-[width] ${
        edge === 'right' ? 'right-0 hover:bg-[var(--color-wardian-accent)]/40' : 'left-0 hover:bg-[var(--color-wardian-accent)]/40'
      }`}
      title="Drag to resize · double-click to reset"
    />
  );
};
```

- [ ] **Step 4: Run test to verify it passes**

Run: `npm run test -- src/components/SidebarResizeHandle.test.tsx`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add src/components/SidebarResizeHandle.tsx src/components/SidebarResizeHandle.test.tsx
git commit -m "feat(layout): add SidebarResizeHandle component"
```

---

## Task 4: Wire resize handle into `SidebarContentPane`

**Files:**
- Modify: `src/layout/SidebarContentPane.tsx`

- [ ] **Step 1: Update the aside to be `relative` and host the handle**

In `src/layout/SidebarContentPane.tsx`, change line 45 from:

```tsx
<aside className={`h-full bg-[var(--color-wardian-sidebar-secondary)]/30 border-r border-wardian-border sidebar-transition overflow-hidden flex flex-col ${leftCollapsed ? 'w-0' : 'w-[var(--sidebar-content-width)]'}`}>
```

to:

```tsx
<aside className={`relative h-full bg-[var(--color-wardian-sidebar-secondary)]/30 border-r border-wardian-border sidebar-transition overflow-hidden flex flex-col ${leftCollapsed ? 'w-0' : 'w-[var(--sidebar-content-width)]'}`}>
```

- [ ] **Step 2: Import the store and the handle**

At top of file, add:

```tsx
import { useLayoutStore } from '../store/useLayoutStore';
import { SidebarResizeHandle } from '../components/SidebarResizeHandle';
```

- [ ] **Step 3: Render the handle inside the aside, after the inner div**

Inside the component, after the existing `<div className="px-4 py-6 ...">…</div>` and before `</aside>`:

```tsx
{!leftCollapsed && (
  <SidebarResizeHandle
    baseWidth={useLayoutStore.getState().leftSidebarWidth}
    edge="right"
    onResize={(px) => useLayoutStore.getState().setLeftSidebarWidth(px)}
    onReset={() => useLayoutStore.getState().setLeftSidebarWidth(260)}
  />
)}
```

- [ ] **Step 4: Lint and run unit tests**

Run: `npm run lint && npm run test`
Expected: PASS.

- [ ] **Step 5: Manual visual check**

Run: `npm run tauri dev`. Hover over the right edge of the left sidebar — cursor should become `col-resize`. Drag — sidebar grows/shrinks. Double-click handle — resets to 260.

- [ ] **Step 6: Commit**

```bash
git add src/layout/SidebarContentPane.tsx
git commit -m "feat(layout): make left sidebar resizable"
```

---

## Task 5: Wire resize handle into `AgentWatchlist`

**Files:**
- Modify: `src/layout/watchlist/AgentWatchlist.tsx` (around line 753)

- [ ] **Step 1: Make the aside `relative` and host a left-edge handle**

Change line 755 to add `relative` to the className. Then inside the aside, after the existing inner `<div>` block (the `p-4 h-full flex flex-col …`), before `</aside>`, add:

```tsx
{!collapsed && (
  <SidebarResizeHandle
    baseWidth={useLayoutStore.getState().rightSidebarWidth}
    edge="left"
    onResize={(px) => useLayoutStore.getState().setRightSidebarWidth(px)}
    onReset={() => useLayoutStore.getState().setRightSidebarWidth(240)}
  />
)}
```

Add the imports near the top of the file:

```tsx
import { useLayoutStore } from '../../store/useLayoutStore';
import { SidebarResizeHandle } from '../../components/SidebarResizeHandle';
```

- [ ] **Step 2: Lint and run unit tests**

Run: `npm run lint && npm run test`
Expected: PASS.

- [ ] **Step 3: Manual visual check**

Run dev mode; verify the right sidebar resizes symmetrically.

- [ ] **Step 4: Commit**

```bash
git add src/layout/watchlist/AgentWatchlist.tsx
git commit -m "feat(layout): make right sidebar (AgentWatchlist) resizable"
```

---

## Task 6: Trigger `gridStacked` from `useGridResize`

**Files:**
- Modify: `src/features/grid/useGridResize.ts`
- Test: `src/features/grid/useGridResize.test.ts` (create if missing)

- [ ] **Step 1: Write the failing test**

Create or extend `src/features/grid/useGridResize.test.ts`:

```ts
import { describe, it, expect, beforeEach } from 'vitest';
import { renderHook, act } from '@testing-library/react';
import React from 'react';
import { useGridResize } from './useGridResize';
import { useLayoutStore } from '../../store/useLayoutStore';

const makeContainer = (width = 1000) => {
  const el = document.createElement('div');
  Object.defineProperty(el, 'getBoundingClientRect', {
    value: () => ({ left: 0, top: 0, right: width, bottom: 600, width, height: 600, x: 0, y: 0, toJSON: () => '' }),
  });
  Object.defineProperty(el, 'clientWidth', { value: width });
  return el;
};

describe('useGridResize — stacked trigger', () => {
  beforeEach(() => {
    localStorage.clear();
    act(() => useLayoutStore.getState().resetLayout());
  });

  it('sets gridStacked when horizontal drag releases past 2/3 weight', () => {
    const ref = { current: makeContainer(1000) } as React.RefObject<HTMLDivElement>;
    const { result } = renderHook(() => useGridResize(ref));

    act(() => result.current.startResize('h', 0));
    // Move mouse to x=900 → globalWeight 0.9 → snaps to 1.0
    act(() => window.dispatchEvent(new MouseEvent('mousemove', { clientX: 900, clientY: 0 })));
    act(() => window.dispatchEvent(new MouseEvent('mouseup')));

    expect(useLayoutStore.getState().gridStacked).toBe(true);
  });

  it('does not set gridStacked when drag stays below 2/3', () => {
    const ref = { current: makeContainer(1000) } as React.RefObject<HTMLDivElement>;
    const { result } = renderHook(() => useGridResize(ref));

    act(() => result.current.startResize('h', 0));
    act(() => window.dispatchEvent(new MouseEvent('mousemove', { clientX: 500, clientY: 0 })));
    act(() => window.dispatchEvent(new MouseEvent('mouseup')));

    expect(useLayoutStore.getState().gridStacked).toBe(false);
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `npm run test -- src/features/grid/useGridResize.test.ts`
Expected: FAIL.

- [ ] **Step 3: Implement**

In `src/features/grid/useGridResize.ts`:

1. Pull `setGridStacked` from the store at the top:
   ```ts
   const { layout, setColumnTracks, setRowHeight, setGridStacked } = useLayoutStore();
   ```
2. Track the latest snapped global weight in a ref so `stopResize` can read it:
   ```ts
   const lastGlobalWeightRef = useRef<number | null>(null);
   ```
3. Inside `handleMouseMove`, in the `'h'` branch, after the snap loop assigns `globalWeight`, set:
   ```ts
   lastGlobalWeightRef.current = globalWeight;
   ```
4. Replace `stopResize`:
   ```ts
   const stopResize = useCallback(() => {
     if (resizing?.type === 'h' && lastGlobalWeightRef.current !== null) {
       if (lastGlobalWeightRef.current >= 1.0 - 1e-6) {
         setGridStacked(true);
       }
     }
     lastGlobalWeightRef.current = null;
     setResizing(null);
     setGuidePos(null);
   }, [resizing, setGridStacked]);
   ```

(Add the missing `useRef` import.)

- [ ] **Step 4: Run test to verify it passes**

Run: `npm run test -- src/features/grid/useGridResize.test.ts`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
git add src/features/grid/useGridResize.ts src/features/grid/useGridResize.test.ts
git commit -m "feat(grid): trigger stacked mode when drag snaps to full width"
```

---

## Task 7: Honor `gridStacked` in `GridView` + add exit button

**Files:**
- Modify: `src/views/GridView.tsx`
- Modify: `src/views/GridView.test.tsx`

- [ ] **Step 1: Extend the existing failing test**

In `src/views/GridView.test.tsx`, add (use the existing test scaffolding/mocks):

```tsx
it('renders single column when gridStacked is true', () => {
  act(() => useLayoutStore.getState().setGridStacked(true));
  const { container } = render(<GridView {...baseProps} filteredAgents={twoMockAgents} />);
  const grid = container.querySelector('[role="grid"], div[style*="grid"]') as HTMLElement;
  expect(grid.style.gridTemplateColumns).toBe('1fr');
});

it('renders an Exit Stacked button when gridStacked is true', () => {
  act(() => useLayoutStore.getState().setGridStacked(true));
  render(<GridView {...baseProps} filteredAgents={twoMockAgents} />);
  expect(screen.getByRole('button', { name: /exit stacked/i })).toBeInTheDocument();
});
```

(Adapt `baseProps` / `twoMockAgents` to existing test fixtures in this file.)

- [ ] **Step 2: Run tests to verify they fail**

Run: `npm run test -- src/views/GridView.test.tsx`
Expected: FAIL on both new cases.

- [ ] **Step 3: Implement**

In `src/views/GridView.tsx`:

1. Read `gridStacked` and `setGridStacked` from the store alongside `layout`/`resetLayout`:
   ```ts
   const { layout, resetLayout, gridStacked, setGridStacked } = useLayoutStore();
   ```
2. Update `gridStyle` (currently around line 121–131): change every `(isMaximized || isMobile)` to `(isMaximized || isMobile || gridStacked)`.
3. Add an exit button. Just above `return (` (around line 141), or in the return alongside the existing toolbar/affordances if present:
   ```tsx
   {gridStacked && !isMaximized && (
     <button
       type="button"
       onClick={() => setGridStacked(false)}
       className="absolute top-2 right-2 z-30 px-2 py-1 text-xs bg-[var(--color-wardian-sidebar-secondary)] border border-wardian-border rounded hover:text-[var(--color-wardian-accent)]"
     >
       Exit stacked
     </button>
   )}
   ```
   (If the existing `<div ref={containerRef} …>` is not `position: relative`, add `relative` to its className.)

- [ ] **Step 4: Run tests to verify they pass**

Run: `npm run test -- src/views/GridView.test.tsx`
Expected: PASS.

- [ ] **Step 5: Run full suites**

Run: `npm run lint && npm run test && npm run build`
Expected: all PASS.

- [ ] **Step 6: Commit**

```bash
git add src/views/GridView.tsx src/views/GridView.test.tsx
git commit -m "feat(grid): honor gridStacked mode with exit button"
```

---

## Task 8: E2E spec for resize + stacked mode

**Files:**
- Create: `e2e/tests/responsive-layout.spec.ts`

- [ ] **Step 1: Write the spec**

```ts
import { test, expect } from '@playwright/test';

test.describe('responsive layout', () => {
  test('left sidebar width persists across reload', async ({ page }) => {
    await page.goto('/');
    const sidebar = page.locator('aside').first();
    const initial = await sidebar.evaluate((el) => el.getBoundingClientRect().width);

    const handle = page.getByTestId('sidebar-resize-handle').first();
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

  test('grid drag past 2/3 enters stacked mode and exit restores', async ({ page }) => {
    await page.goto('/');
    // Pre-condition: at least 2 mock agents present in fixtures.
    const handle = page.locator('[data-resize-handle="h"]').first();
    if (!(await handle.isVisible())) test.skip();

    const grid = page.locator('[data-testid="agent-grid"]');
    const gridBox = await grid.boundingBox();
    const handleBox = await handle.boundingBox();
    if (!gridBox || !handleBox) throw new Error('grid not visible');

    await page.mouse.move(handleBox.x + handleBox.width / 2, handleBox.y + handleBox.height / 2);
    await page.mouse.down();
    await page.mouse.move(gridBox.x + gridBox.width - 5, handleBox.y, { steps: 20 });
    await page.mouse.up();

    await expect(page.getByRole('button', { name: /exit stacked/i })).toBeVisible();
    await page.getByRole('button', { name: /exit stacked/i }).click();
    await expect(page.getByRole('button', { name: /exit stacked/i })).toBeHidden();
  });
});
```

NOTE: the second test references selectors `[data-resize-handle="h"]` and `[data-testid="agent-grid"]`. If these are not already present in `GridView`, add them as part of this task — they are read-only hooks for tests, low blast radius. The first test gates on the sidebar handle's existing `data-testid="sidebar-resize-handle"`.

- [ ] **Step 2: Add missing test hooks**

In `src/views/GridView.tsx`:
- Add `data-testid="agent-grid"` to the outer `<div ref={containerRef} …>`.
- Add `data-resize-handle="h"` to whichever element renders horizontal resize handles inside the grid (search for `startResize('h'` to find where).

- [ ] **Step 3: Run E2E**

Run: `npm run test:e2e -- responsive-layout.spec.ts`
Expected: PASS. (The grid test will skip if no horizontal handle is rendered for the seeded fixture; that's acceptable as long as the sidebar test passes.)

- [ ] **Step 4: Commit**

```bash
git add e2e/tests/responsive-layout.spec.ts src/views/GridView.tsx
git commit -m "test(e2e): cover sidebar resize persistence and stacked grid mode"
```

---

## Task 9: Pre-PR verification

- [ ] **Step 1: Frontend full sweep**

Run: `npm run lint && npm run test && npm run build`
Expected: all PASS.

- [ ] **Step 2: Backend sanity (no Rust changes expected)**

Run: `cd src-tauri && cargo check`
Expected: PASS.

- [ ] **Step 3: Push branch and open PR**

```bash
git push -u origin feat/responsive-terminal-width
gh pr create --title "feat(layout): resizable sidebars and forced stacked grid mode" \
  --body "$(cat <<'EOF'
## Summary
- Adds resizable sidebars (drag handles on inner edges, persisted width in `useLayoutStore`).
- Adds a user-forced stacked grid mode triggered by dragging a column past 2/3 (snaps to 1.0 weight).
- Closes the small-display cramping problem documented in spec 022.

Implements `docs/specs/022-responsive-terminal-width.md`.

## Test plan
- [x] `npm run lint`
- [x] `npm run test`
- [x] `npm run build`
- [x] `npm run test:e2e -- responsive-layout.spec.ts`
- [x] Manual: laptop (1080p physical) — drag left sidebar narrower, grid breathes; drag a grid cell past 2/3 → stacks; exit button restores.
- [x] Manual: desktop — defaults unchanged on first load.
EOF
)"
```

---

## Notes for the implementer

- **Read the spec first** (`docs/specs/022-responsive-terminal-width.md`). It has the *why*; this plan has the *how*.
- **TDD strictly**: every code change has a test that fails before it and passes after. Do not skip the "verify failure" steps — they catch tests that pass for the wrong reason.
- **DRY**: the resize handle is one component used twice. If you find yourself duplicating handle logic for the right sidebar, you went wrong in Task 3.
- **YAGNI**: do not add a "Compact density" preset, do not add window-size auto-hiding, do not add per-workspace persistence. None of those are in the spec.
- **Project conventions**: `snake_case` for IPC properties, `camelCase` for TS, `PascalCase.tsx` for components, theme variables not hardcoded Tailwind colors. See `AGENTS.md`.
- **Commit cadence**: one commit per task, conventional-commit style.
