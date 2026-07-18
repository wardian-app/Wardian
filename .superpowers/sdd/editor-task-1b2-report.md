# Task 1B.2 integration report

Date: 2026-07-17

Base: `586597573c1fdab4b514514991d95bb5abb2f7fe`

## Outcome

Integrated the reviewed pure close transaction coordinator into every scoped
destructive Workbench navigation route:

- canonical resource convergence;
- explicit resource rebind;
- surface reset;
- surface close;
- group close; and
- Workbench reset, including the injected durable reset boundary.

Navigation now captures the immutable document, transaction version, and
complete closing-surface IDs; observes all resource memberships; collects every
choice; revalidates live transaction, identity, generation, and membership;
runs Saves; compare-and-applies layout; then runs Discard cleanup.

The registry now exposes resource observation, preparation, and exact
revalidation through narrow resource adapters. Library uses presentation-keyed
editor bridges with store-owned monotonic generations, Workflows uses the shared
builder and a monotonic resource revision, and Files uses an injected adapter
whose default reports clean/no effects until Task 4.
The sequential `can_close` contract and Files placeholder that treated Save or
Discard as unconditional permission were removed.

## RED evidence

Initial navigation RED:

```text
npm run test -- src/features/workbench/navigationService.test.ts
Test Files  1 failed (1)
Tests       1 failed | 49 passed (50)
TypeError: registry.register_close_adapter is not a function
Exit code 1
```

Expanded registry/dirty/navigation RED:

```text
npm run test -- src/features/workbench/navigationService.test.ts src/features/workbench/surfaceRegistry.test.ts src/features/workbench/surfaces/dirtySurfaceGuards.test.ts
Test Files  3 failed (3)
Tests       3 failed | 76 passed (79)
```

The failures identified the absent registry adapter protocol and absent
Library/Workflows choice-only preparation.

## Verification

Focused coordinator/navigation/registry/dirty surfaces:

```text
npm run test -- src/features/workbench/closeTransactionCoordinator.test.ts src/features/workbench/navigationService.test.ts src/features/workbench/surfaceRegistry.test.ts src/features/workbench/coreSurfaceRegistry.test.ts src/features/workbench/surfaces/dirtySurfaceGuards.test.ts
Test Files  5 passed (5)
Tests       92 passed (92)
Exit code 0
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
3797 modules transformed
Exit code 0
```

Vite emitted only the repository's existing non-fatal large-chunk advisory.

## Coverage

Integration tests prove:

- every scoped route supplies its complete closing set during initial
  observation and pre-effect revalidation;
- a duplicate opened while a rebind choice is pending makes the transaction
  stale before effects;
- closing one of multiple presentations produces no prompt or resource effect;
- mixed Files/Library/Workflow close ordering is Saves, layout, then Discard;
- a failed later Save cancels layout and performs no Discard;
- Library and Workflows preparation is choice-only;
- group close and Workbench reset do not partially discard when another
  resource cancels; and
- registry generation changes and navigation membership changes fail
  revalidation; and
- concrete Library and Workflows identity/generation changes cancel prepared
  effects before Save, layout, or Discard.

## Documentation

- Added `docs/specs/2026-07-17-workbench-resource-close-transactions.md`.
- Updated `docs/developer/workbench-surfaces.md` to describe the two-phase
  resource close contract and the Task 4 Files adapter boundary.

## Hygiene and concerns

The pre-existing `package-lock.json` modification was not edited or staged.
No Files editor buffer, native save, recovery state, or recovery UI was added.
No implementation concerns remain within Task 1B.2.

## Review remediation evidence

Review RED, before the generation owners and effect bindings were implemented:

```text
npm run test -- src/store/useLibraryStore.test.ts src/store/useBuilderStore.test.ts src/features/library/DetailPane.discardGuard.test.tsx src/features/workbench/surfaces/dirtySurfaceGuards.test.ts src/features/workbench/navigationService.test.ts
Test Files  4 failed | 1 passed (5)
Tests       4 failed | 96 passed (100)
Exit code 1
```

Focused GREEN after remediation:

```text
npm run test -- src/features/workbench/closeTransactionCoordinator.test.ts src/features/workbench/navigationService.test.ts src/features/workbench/surfaceRegistry.test.ts src/features/workbench/coreSurfaceRegistry.test.ts src/features/workbench/surfaces/dirtySurfaceGuards.test.ts src/store/useLibraryStore.test.ts src/features/library/DetailPane.test.tsx src/features/library/DetailPane.discardGuard.test.tsx src/store/useBuilderStore.test.ts src/store/useWorkflowsView.test.ts
Test Files  10 passed (10)
Tests       152 passed (152)
Exit code 0
```

Final gates:

```text
npm run test
Test Files  182 passed (182)
Tests       2179 passed | 1 skipped (2180)

npm run lint
tsc --noEmit
Exit code 0

npm run build
tsc && vite build
3797 modules transformed
Exit code 0
```

## Review remediation self-review

- Library generations are store-owned and advance on every editor-state
  publication, including a second draft edit while the surface remains dirty
  and an entry-identity switch.
- Workflows `resourceRevision` never resets and advances through
  load/initialize/edit/save/discard/reset; a separate monotonic identity token
  prevents an in-flight save response from applying across resource ABA.
- Prepared Library and Workflows Save/Discard closures are bound to the exact
  observed identity and generation and fail closed before invoking stale work.
- Integration regressions hold choices pending across already-dirty Library
  edits and Workflows reset/initialize ABA, proving cancellation before Save,
  layout, or Discard.
- Canonical convergence onto an existing transient observes and revalidates
  both the permanent source and transient target surface IDs.
- The earlier coverage claim was narrowed to match the exact direct and
  integration evidence. The pre-existing `package-lock.json` modification
  remains untouched and unstaged.
