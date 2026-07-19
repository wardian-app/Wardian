# Task 1A review-fix report

Date: 2026-07-17

Branch: `feat/files-surface-foundation`

Reviewed starting head: `0082a43c`

Final fix commit: recorded in the agent handoff because a commit cannot embed its own
stable SHA without changing that SHA.

## Outcome

Both Important review findings are resolved without expanding into later editor
tasks.

- Baseline discovery is tri-state: `available`, `unavailable`, or `unknown`.
  Historical checkpoint/version baselines retain their identity and open comparison
  while discovery is unknown, and are cleared only after an explicit unavailable
  result.
- Schema-1 Preview and historical-baseline migrations remain provisional. `App`
  carries the validated Preview renderer-default intent into `FilesSurface`, which
  resolves it against the discovered renderer and writes canonical V2 state once.
- A renderer with both rendered/editor presentations and an editor default now
  migrates legacy Preview to editor.
- Duplicate/stale normalization callbacks are suppressed in `FilesSurface`, and
  `App` compares the callback's rendered state snapshot against the live canonical
  surface before applying it. This prevents stale pre-pin state from replaying over
  a newer transient-state update.
- Accepted legacy normalization is flushed immediately. This keeps the migration
  save acknowledgment from racing a newly interactive renderer remount.

## Files changed

- `src/features/files/filesSurfaceState.ts`
- `src/features/files/filesSurfaceState.test.ts`
- `src/features/files/FilesSurface.tsx`
- `src/features/files/FilesSurface.test.tsx`
- `src/views/App.tsx`
- `.superpowers/sdd/editor-task-1a-report.md`

The unrelated user-owned `package-lock.json` deletion was preserved exactly and was
not staged.

## Verification

### Focused integration coverage

Command:

```text
npm run test -- src/features/files/filesSurfaceState.test.ts src/features/files/FilesSurface.test.tsx src/views/App.test.tsx
```

Outcome:

```text
Test Files  3 passed (3)
Tests       106 passed (106)
Exit code   0
```

### Full frontend tests

Command:

```text
npm run test
```

Outcome:

```text
Test Files  181 passed (181)
Tests       2161 passed | 1 skipped (2162)
Exit code   0
```

The run printed the repository's existing jsdom
`HTMLCanvasElement.getContext()` notices; they were non-fatal.

### TypeScript lint

Command:

```text
npm run lint
```

Outcome: `tsc --noEmit` passed with exit code 0.

### Production build

Command:

```text
npm run build
```

Outcome: `tsc && vite build` passed with exit code 0; Vite transformed 3,795
modules and completed the production build. Existing large-chunk advisories were
non-fatal.

### Diff hygiene

Command:

```text
git diff --check -- . ':(exclude)package-lock.json'
```

Outcome: passed with exit code 0.

## Iteration evidence

- One focused Files rerun transiently timed out loading two existing lazy renderer
  modules (23 passed, 2 failed); the identical immediate retry passed all 25 tests.
- The first full suite exposed a deterministic App integration regression: 2,160
  tests passed, one transient-preview pin test failed, and one test was skipped.
  Focused reproduction showed schema-1 Preview normalization could race the
  workbench save acknowledgment and detach a Markdown link before its click reached
  `on_open_file`. Immediate migration flush plus live-state snapshot validation fixed
  it; the focused App test and final full suite both passed.

## Self-review

- Confirmed V1 validation remains strict and byte-bearing legacy Draft data is still
  rejected.
- Confirmed only V1 Preview and V1 Changes with a candidate historical baseline are
  deferred; clean Draft and Changes-without-baseline migrations remain idempotent.
- Confirmed `unknown` never closes comparison or clears its baseline, while explicit
  `unavailable` clears both.
- Confirmed renderer-default intent is one-shot, including when the normalized
  presentation equals the provisional V2 projection.
- Confirmed a dual-capability text renderer reports rendered and editor availability
  while retaining editor as its default.
- Confirmed stale prop objects and changing callback identities do not issue duplicate
  normalization writes.
- Confirmed App rejects state callbacks after the live surface has advanced, covering
  both legacy schema changes and ordinary V2 transient pinning.
- Confirmed no backend Rust, later-task editor UI, or package-lock content was changed.

## Concerns

No remaining Task 1A correctness concerns. Historical baseline providers are still a
downstream capability; until one reports a conclusive result, their migration remains
intentionally provisional.

## Independent re-review fix

The independent re-review found one remaining canonical-resource race: a canonical
replacement rebuilt from render-captured request state could replay stale pre-pin or
pre-normalization Files state after an asynchronous close guard.

### Resolution

`canonicalize_resource` is now identity-only for a live source surface. On every CAS
attempt it:

1. reads the current transaction snapshot;
2. verifies that the source type matches and validates the complete persisted source
   surface through the registry;
3. derives the canonical resource key using that current persisted state;
4. replaces only the resource identity while preserving the validated source's exact
   schema version, state, and presentation provenance; and
5. relies on the existing stale-transaction retry to repeat the derivation from a
   newer pinned/normalized snapshot.

The existing-canonical/transient collision branch now also builds its surviving
replacement from the current source snapshot rather than the callback request.

### Race regression

The new navigation regression uses the core Files registry and a real asynchronous
dirty-close prompt. It starts with different provisional and canonical keys, holds
canonicalization in flight, pins the transient before releasing the guard, proves the
first CAS becomes stale and retries, then verifies the final canonical surface exactly
retains the pinned V2 state.

RED command:

```text
npm run test -- src/features/workbench/navigationService.test.ts -t "preserves a transient pin while canonical resource replacement is in flight"
```

RED outcome:

```text
Test Files  1 failed (1)
Tests       1 failed | 48 skipped (49)
Expected transient_preview: false; received true after canonical replacement
Exit code   1
```

The identical command passed after the fix: 1 passed, 48 skipped, exit code 0.

### Final verification

Requested App/navigation/Files focused command:

```text
npm run test -- src/views/App.test.tsx src/features/workbench/navigationService.test.ts src/features/files/FilesSurface.test.tsx src/features/files/filesSurfaceState.test.ts
```

Outcome:

```text
Test Files  4 passed (4)
Tests       155 passed (155)
Exit code   0
```

Additional gates:

- `npm run test -- src/features/workbench/navigationService.test.ts`: passed, 49/49 tests.
- `npm run test`: passed, 181 files; 2,162 tests passed and 1 skipped.
- `npm run lint`: passed; `tsc --noEmit` exited 0.
- `npm run build`: passed; `tsc && vite build` transformed 3,795 modules and exited 0.
- `git diff --check -- . ':(exclude)package-lock.json'`: passed.

The full test run printed the existing jsdom `HTMLCanvasElement.getContext()` notices,
and the build printed the existing large-chunk advisories; both remained non-fatal.

### Re-review self-review

- Confirmed `request.state` is no longer authoritative when the canonicalized source
  still exists.
- Confirmed source state is preserved only after full registry validation, including
  resource identity validation.
- Confirmed the exact persisted schema/state is retained rather than opportunistically
  upgrading provisional historical V1 state during identity convergence.
- Confirmed a stale guard causes a retry that re-reads the newest pinned/normalized
  source before rebuilding commands.
- Confirmed collision convergence carries current source state into the surviving
  existing transient while retaining the existing surface's applicable provenance.
- Confirmed missing, invalid, or cross-type live sources fail closed instead of using
  captured request state.
- Confirmed the fix is in the navigation transaction boundary, not an App-only Files
  key special case.
- Confirmed the user-owned `package-lock.json` diff remains untouched and unstaged.

No remaining concerns from the independent re-review finding.
