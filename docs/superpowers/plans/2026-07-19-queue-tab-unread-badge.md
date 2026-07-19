# Queue Tab Unread Badge Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Restore the Queue Workbench tab's numeric unread notification badge with live updates.

**Architecture:** Extend the existing UI-neutral `SurfaceBadge` metadata with an optional display value. Queue's surface definition derives its unread count from `useQueueStore` and subscribes the Workbench registry to queue-store changes; `WorkbenchTab` renders value-bearing badges as compact pills while preserving dot-only badges for existing surfaces.

**Tech Stack:** React, TypeScript, Zustand, Vitest, Testing Library, CSS custom properties.

## Global Constraints

- Use snake_case for IPC/data model properties and existing Workbench naming conventions.
- Use theme variables/classes instead of hardcoded Tailwind colors.
- Keep the Queue unread definition scoped to `useQueueStore`; do not change queue persistence or event generation.
- Write the regression test before production code and verify the red-green cycle.
- Frontend/UI changes require feature-specific screenshot evidence in the PR description.

---

## File Map

- Modify `src/types/index.ts`: add the optional display value to `SurfaceBadge`.
- Modify `src/layout/workbench/WorkbenchTab.tsx`: render value-bearing badges without changing existing dot badges.
- Modify `src/layout/workbench/workbench.css`: style numeric badge values using Wardian theme variables.
- Modify `src/layout/workbench/WorkbenchTab.test.tsx`: prove numeric values render in a Workbench tab.
- Modify `src/features/workbench/surfaces/coreSurfaceMetadata.ts`: derive and subscribe Queue unread badge metadata.
- Modify `src/features/workbench/surfaces/coreSurfaceDefinitions.test.tsx`: prove Queue presentation metadata returns the correct badge states.
- Modify `src/layout/workbench/WorkbenchHost.test.tsx`: prove a queue-store update refreshes a mounted Queue tab.
- Update `docs/specs/2026-07-19-queue-tab-unread-badge-design.md` only if implementation details materially change during execution.

### Task 1: Render value-bearing Workbench badges

**Interfaces:** `SurfaceBadge` gains `value?: string`; `WorkbenchTab` consumes it and keeps the existing `data-surface-badge` identity and `title` label.

- [ ] **Step 1: Write the failing test**

Add this test to `src/layout/workbench/WorkbenchTab.test.tsx`:

```tsx
it("renders a value-bearing badge without changing the tab title", () => {
  const surface = makeSurface("surface-queue", { surface_type: "queue" });
  const view = render(
    <div role="tab" aria-label="Queue">
      <WorkbenchTab
        surface={surface}
        title="Queue"
        group_id="group-1"
        badges={[{ badge_id: "unread", label: "3 unread queue items", value: "3" }]}
      />
    </div>,
  );

  expect(screen.getByRole("tab", { name: "Queue" })).toHaveTextContent("Queue");
  expect(view.container.querySelector('[data-surface-badge="unread"]'))
    .toHaveTextContent("3")
    .toHaveAttribute("title", "3 unread queue items");
});
```

- [ ] **Step 2: Run the focused test and verify it fails**

Run:

```powershell
npm run test -- src/layout/workbench/WorkbenchTab.test.tsx
```

Expected: the test fails because `SurfaceBadge` does not accept `value` and the badge does not render its value.

- [ ] **Step 3: Add the minimal type and rendering support**

In `src/types/index.ts`, extend `SurfaceBadge` with:

```ts
readonly value?: string;
```

In `src/layout/workbench/WorkbenchTab.tsx`, render the optional value inside the existing badge span:

```tsx
<span
  key={badge.badge_id}
  className="wardian-workbench-tab-badge"
  data-surface-badge={badge.badge_id}
  {...(badge.value === undefined ? {} : { "data-surface-badge-value": badge.value })}
  title={badge.label}
>
  {badge.value}
</span>
```

- [ ] **Step 4: Style numeric values and rerun the focused test**

Add a value selector to `src/layout/workbench/workbench.css`:

```css
.wardian-workbench-tab-badge[data-surface-badge-value] {
  width: auto;
  min-width: 1rem;
  height: 1rem;
  padding: 0 0.25rem;
  border-radius: 999px;
  color: var(--color-wardian-bg);
  font-size: 0.5625rem;
  font-weight: 700;
  line-height: 1rem;
  text-align: center;
}
```

Run the same focused test. Expected: PASS.

- [ ] **Step 5: Commit the independently testable badge renderer**

```powershell
git add src/types/index.ts src/layout/workbench/WorkbenchTab.tsx src/layout/workbench/workbench.css src/layout/workbench/WorkbenchTab.test.tsx
git commit -m "feat(workbench): render numeric surface badges"
```

### Task 2: Supply Queue unread badge metadata

**Interfaces:** `QUEUE_SURFACE_DEFINITION` returns `SurfaceBadge[]` with `badge_id: "unread"`, `label`, and capped `value`; its `presentation_subscribe` uses the existing Zustand store subscription contract.

- [ ] **Step 1: Write the failing Queue metadata tests**

In `src/features/workbench/surfaces/coreSurfaceDefinitions.test.tsx`, import `useQueueStore` and add:

```tsx
function queueSurfaceSnapshot() {
  return {
    surface_id: "queue-1",
    surface_type: "queue" as const,
    state_schema_version: CORE_VIEW_SURFACE_STATE_SCHEMA_VERSION,
    state: {},
  };
}

it("derives a capped unread badge for Queue presentation metadata", () => {
  const queue = CORE_VIEW_SURFACE_DEFINITIONS.find((definition) => definition.type === "queue")!;

  useQueueStore.setState({ items: [] });
  expect(queue.badges?.(queueSurfaceSnapshot())).toEqual([]);

  useQueueStore.setState({
    items: Array.from({ length: 3 }, (_, index) => ({
      id: `unread-${index}`,
      type: "agent_completed" as const,
      timestamp: Date.now(),
      read: false,
    })),
  });
  expect(queue.badges?.(queueSurfaceSnapshot())).toEqual([{
    badge_id: "unread",
    label: "3 unread queue items",
    value: "3",
  }]);

  useQueueStore.setState({
    items: Array.from({ length: 11 }, (_, index) => ({
      id: `unread-${index}`,
      type: "agent_completed" as const,
      timestamp: Date.now(),
      read: false,
    })),
  });
  expect(queue.badges?.(queueSurfaceSnapshot())).toEqual([{
    badge_id: "unread",
    label: "11 unread queue items",
    value: "9+",
  }]);
});

it("subscribes Queue presentation metadata to the queue store", () => {
  const queue = CORE_VIEW_SURFACE_DEFINITIONS.find((definition) => definition.type === "queue")!;
  const listener = vi.fn();
  const unsubscribe = queue.presentation_subscribe?.(listener);

  useQueueStore.setState({ items: [{
    id: "unread-1",
    type: "agent_completed",
    timestamp: Date.now(),
    read: false,
  }] });

  expect(listener).toHaveBeenCalled();
  unsubscribe?.();
});
```

Reset `useQueueStore` items in the existing test cleanup so these tests cannot leak state.

- [ ] **Step 2: Run the focused metadata tests and verify they fail**

Run:

```powershell
npm run test -- src/features/workbench/surfaces/coreSurfaceDefinitions.test.tsx
```

Expected: the badge metadata assertions fail because Queue currently returns `[]`, and the subscription assertion fails because no Queue source is registered.

- [ ] **Step 3: Implement the minimal Queue definition behavior**

Import `useQueueStore` in `src/features/workbench/surfaces/coreSurfaceMetadata.ts`, add this helper near the core definitions:

```ts
function queueUnreadBadges(): SurfaceBadge[] {
  const unreadCount = useQueueStore.getState().items.filter((item) => !item.read).length;
  if (unreadCount === 0) return [];
  return [{
    badge_id: "unread",
    label: `${unreadCount} unread queue item${unreadCount === 1 ? "" : "s"}`,
    value: unreadCount > 9 ? "9+" : String(unreadCount),
  }];
}
```

Override the Queue definition's default empty badge contract:

```ts
export const QUEUE_SURFACE_DEFINITION = {
  ...defineCoreViewSurface(
    "queue", "Queue", "recreate_from_state", {
      default_state: () => EMPTY_STATE,
      serialize_state: () => EMPTY_STATE,
      restore_state: (value, version) => restoreEmptyState(value, version, "queue"),
    },
  ),
  presentation_subscribe: (listener: () => void) => useQueueStore.subscribe(listener),
  badges: queueUnreadBadges,
} satisfies SurfaceDefinition;
```

- [ ] **Step 4: Run the metadata tests and verify they pass**

Run the same focused command. Expected: all tests in `coreSurfaceDefinitions.test.tsx` PASS.

- [ ] **Step 5: Commit Queue metadata**

```powershell
git add src/features/workbench/surfaces/coreSurfaceMetadata.ts src/features/workbench/surfaces/coreSurfaceDefinitions.test.tsx
git commit -m "fix(queue): restore unread badge metadata"
```

### Task 3: Verify Workbench integration and finish documentation

**Interfaces:** `createCoreWorkbenchSurfaceRegistry()` exposes the reactive Queue definition to `WorkbenchHost`; no new public component props are introduced.

- [ ] **Step 1: Add a mounted Workbench Queue refresh regression**

Add a test to `src/layout/workbench/WorkbenchHost.test.tsx` that creates a Queue-only document, renders `WorkbenchHost`, asserts no unread badge, updates `useQueueStore` with one unread item, and waits for `[data-surface-badge="unread"]` with text `1`. Reset the queue store in cleanup.

- [ ] **Step 2: Run the integration regression and verify it passes**

```powershell
npm run test -- src/layout/workbench/WorkbenchHost.test.tsx src/layout/workbench/WorkbenchTab.test.tsx src/features/workbench/surfaces/coreSurfaceDefinitions.test.tsx
```

Expected: all focused Workbench and Queue badge tests PASS.

- [ ] **Step 3: Capture feature-specific screenshot evidence**

Run the focused Workbench browser fixture or the approved running-app capture path and save a screenshot under:

```text
e2e/screenshots/queue-tab-unread-badge/<timestamp>/queue-tab-unread-badge.png
```

The image must show the Workbench Queue tab with an unread numeric badge, not an empty shell or generic app tour.

- [ ] **Step 4: Run repository verification**

Run:

```powershell
npm run lint
npm run test
npm run build
npm run check:frontend-screenshot -- origin/main HEAD
git diff --check
git status --short --branch
```

Expected: lint, unit tests, build, screenshot gate, and diff check exit successfully; status contains only the intended implementation, documentation, plan, and screenshot evidence files.

- [ ] **Step 5: Commit the integration test and evidence references**

```powershell
git add src/layout/workbench/WorkbenchHost.test.tsx docs/superpowers/plans/2026-07-19-queue-tab-unread-badge.md e2e/screenshots/queue-tab-unread-badge
git commit -m "test(queue): verify unread badge refresh"
```
