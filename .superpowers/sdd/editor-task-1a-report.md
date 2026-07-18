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
