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
