# Task 1B combined outcome: two-phase Workbench close coordination

Date: 2026-07-17

## Task 1B.1: pure coordinator

Task 1B.1 introduced the reviewed framework-agnostic coordinator. It captures
and deeply freezes close context, groups exact presentations by canonical
resource, prepares each final-closing resource once, collects all choices before
effects, revalidates exact state, runs Saves before layout, and runs Discard only
after an accepted layout commit. Missing or malformed preparation fails closed.

The detailed Task 1B.1 evidence is recorded in
`.superpowers/sdd/editor-task-1b1-report.md`.

## Task 1B.2: Workbench integration

Task 1B.2 connected that coordinator to canonicalize, rebind, surface reset,
surface close, group close, and Workbench reset. The surface registry now owns a
UI-neutral resource adapter boundary for observation, deferred preparation, and
exact revalidation. Library and Workflows use choice-only adapters; Files uses a
narrow injected clean/no-effect adapter pending Task 4.

The sequential effectful `can_close(surface)` contract and Files placeholder
guard were removed. Mixed resources cannot partially discard, remaining
presentations do not prompt, concurrent duplicate/rebind changes go stale before
effects, and save failure prevents layout and post-commit discard.

The detailed Task 1B.2 RED/GREEN evidence is recorded in
`.superpowers/sdd/editor-task-1b2-report.md`.

## Combined verification

```text
Focused Vitest: 5 files passed, 92 tests passed
npm run lint: passed
npm run build: passed (3797 modules transformed)
```

The pre-existing `package-lock.json` modification remained untouched and
unstaged. Task 1B adds no Files buffers, native saving, recovery state, or UI.

## Task 1B.2 review remediation

The integration review exposed two non-monotonic generation paths: Library
draft edits made while already dirty and Workflows state replacement that reset
its edit revision. Library now publishes a store-owned monotonic generation for
every draft/baseline/entry identity change. Workflows now advances a monotonic
resource revision across load, initialize, edit, save, discard, and reset.
Prepared Save/Discard effects bind to their observed identity/generation and
fail closed if either changes before invocation.

```text
Review RED: 5 files, 4 failed | 1 passed; 4 failed | 96 passed tests
Focused GREEN: 10 files passed, 152 tests passed
Full Vitest: 182 files passed, 2179 passed | 1 skipped tests
npm run lint: passed
npm run build: passed (3797 modules transformed)
```

## Task 1B.2 remediation self-review

- Pending choices now cancel before Save, layout, and Discard when an
  already-dirty Library draft changes or Workflows switches through an ABA
  reset/initialize sequence.
- Canonical convergence onto an existing transient proves that both closing IDs
  are observed and revalidated.
- Deferred effects independently reject stale identity/generation bindings
  immediately before their resource actions.
- The detailed report's earlier exact-change claim was narrowed to distinguish
  direct registry/navigation coverage from concrete adapter integration
  coverage.
- The pre-existing `package-lock.json` modification remains untouched and
  unstaged. No Files buffer, native save, recovery, or UI scope was added.
