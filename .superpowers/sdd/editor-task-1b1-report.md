# Task 1B.1 close coordinator report

Date: 2026-07-17

Base: `b8835635`

## Outcome

Added the pure, framework-agnostic Workbench close transaction coordinator.
The coordinator:

- accepts an immutable Workbench snapshot, transaction version, and complete
  closing-surface set;
- groups repeated observations by canonical resource identity and prepares each
  final-closing resource once;
- skips resource preparation when any exact presentation remains open;
- collects every choice before running effects and cancels safely on malformed,
  failed, or cancelling preparation;
- revalidates transaction, identity, generation, and exact presentation
  membership before effects;
- runs all successful saves before the injected layout compare-and-apply;
- runs discard/release cleanup only after an accepted layout commit; and
- converts revalidation, save, and commit rejection into cancellation without
  pre-commit discard.

The module imports only the Workbench document type. It has no React, Monaco,
Files-buffer, registry, navigation, Tauri, or surface-adapter dependency.

## Files

- `src/features/workbench/closeTransactionCoordinator.ts`
- `src/features/workbench/closeTransactionCoordinator.test.ts`
- `.superpowers/sdd/editor-task-1b1-report.md`

The pre-existing `package-lock.json` modification was not edited or staged.

## RED evidence

Command:

```text
npm run test -- src/features/workbench/closeTransactionCoordinator.test.ts
```

Outcome before implementation:

```text
Test Files  1 failed (1)
Tests       no tests
Error: Failed to resolve import "./closeTransactionCoordinator"
Exit code  1
```

## GREEN verification

Focused Vitest:

```text
npm run test -- src/features/workbench/closeTransactionCoordinator.test.ts
Test Files  1 passed (1)
Tests       7 passed (7)
Exit code   0
```

TypeScript lint:

```text
npm run lint
tsc --noEmit
Exit code 0
```

Production build:

```text
npm run build
tsc && vite build
3795 modules transformed
Exit code 0
```

Vite printed the repository's existing large-chunk advisories; they were
non-fatal.

Full frontend suite:

```text
npm run test
Test Files  182 passed (182)
Tests       2169 passed | 1 skipped (2170)
Exit code   0
```

The first full run had one unrelated GraphView layout timing failure while
2,168 tests passed. The exact GraphView test passed immediately in isolation,
and the complete suite then passed. Existing jsdom canvas notices were
non-fatal.

## Coverage

The focused tests prove:

- one cancelling prepared resource produces zero save, discard, revalidation,
  or commit calls after all choices are collected;
- two final-closing presentations of one canonical resource produce one
  preparation and one choice;
- a remaining duplicate presentation produces no resource preparation or
  resource effect while still allowing the layout close;
- failed exact-state revalidation produces zero effects;
- all saves precede layout commit and all discards follow an accepted commit;
- a failed later save preserves earlier successful saves, cancels layout, and
  runs no discard; and
- a rejected layout compare-and-apply runs no discard.

## Concerns

None within Task 1B.1. Navigation, registry, and concrete Files/Library/Workflow
adapters remain intentionally deferred to Task 1B.2.

## Independent review fixes

The independent review found two Important contract gaps: the public snapshot
type was only shallowly readonly at the context level and not frozen at runtime,
and a final-closing resource could return `null`, disappear from exact-state
revalidation, and still permit layout commit.

### Resolution

- Added a pure recursive `SurfaceCloseDeepReadonly<T>` type and exposed the
  Workbench snapshot as `SurfaceCloseSnapshot`, without importing the Workbench
  store module.
- Captured one context before grouping or awaiting preparation. The captured
  context, complete closing-ID copy, Workbench document, nested records, arrays,
  surfaces, and opaque state are recursively frozen before any injected callback
  receives them. The same captured context reaches preparation, revalidation,
  and layout commit.
- A `null` result for an observed final-closing resource is now a preparation
  failure. The coordinator still collects later resources' choices, then
  cancels before revalidation, saves, commit, or discard.

### Review-fix RED evidence

Command:

```text
npm run test -- src/features/workbench/closeTransactionCoordinator.test.ts
```

Outcome before the fix:

```text
Test Files  1 failed (1)
Tests       2 failed | 7 passed (9)
Exit code   1
```

The runtime immutability regression received `cancel` because its freeze
assertions failed inside preparation, and the missing-preparation regression
received `allow` because the empty revalidation set still reached layout commit.

### Review-fix GREEN verification

```text
npm run test -- src/features/workbench/closeTransactionCoordinator.test.ts
Test Files  1 passed (1)
Tests       9 passed (9)
Exit code   0

npm run lint
tsc --noEmit
Exit code   0

npm run build
tsc && vite build
3795 modules transformed
Exit code   0
```

The build printed only the repository's existing non-fatal large-chunk
advisories.

### Review-fix self-review

- Confirmed compile-time assertions reject mutation of surface records, group
  membership arrays, and the complete closing-ID set.
- Confirmed runtime preparation observes a frozen context, document, surface
  record graph, opaque state, group membership array, and closing-ID array.
- Confirmed attempted array mutation throws and leaves both the captured
  Workbench snapshot and closing membership unchanged.
- Confirmed context capture occurs synchronously before the first preparation
  await, and every later phase receives that same frozen capture.
- Confirmed `null` cannot omit a final-closing resource from revalidation and
  cannot reach layout commit.
- Confirmed `null` failure does not short-circuit preparation of later resources,
  preserving the all-choice-collection rule.
- Confirmed no store, navigation, registry, React, Monaco, Files buffer, or Tauri
  dependency was added.
- Confirmed the pre-existing `package-lock.json` change remains untouched and
  unstaged.

No remaining concerns from the two Important review findings.
