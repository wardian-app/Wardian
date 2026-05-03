# Skill Library UI Refresh Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Refresh skill-library-backed UI while Wardian is running when skills are added, removed, renamed, or edited on disk.

**Architecture:** Skill-library UI consumers request freshness only while mounted. The backend owns filesystem watching for `library/skills`, emits a debounced `library-changed` event, and the frontend refetches only the affected library tree through `useLibraryStore`. Consumers also fetch on mount so changes that occurred while no library UI was open are detected immediately when the user opens a skill-related surface.

**Tech Stack:** Tauri v2 commands/events, Rust `notify`, React, Zustand, Vitest/Testing Library, existing Wardian library commands.

---

## Scope

This plan is intentionally UI-scoped. It does not design or implement future Wardian CLI behavior.

In scope:
- `LibraryView` refreshes skills when its Skills tab is active and skill files change.
- `ManageSkills` refreshes available skills while mounted in agent/class configuration.
- External filesystem changes under `WARDIAN_HOME/library/skills` are detected while one of those UI consumers is active.
- The frontend fetches a fresh snapshot on mount to catch changes made while no consumer was active.
- Mount freshness is required even when `skillTree` is already populated from app startup preload or a prior UI session.

Out of scope:
- Prompt-library watching.
- Always-on global app watcher.
- CLI watch/subscription behavior.
- Watching deployed target `.agents/skills` directories for external edits.

## File Structure

- Modify `src-tauri/src/state/app_state.rs`
  - Add library watch registrations keyed by library type.
  - Store the watcher and reference count in the same registration value so lifecycle changes are serialized under one mutex.

- Modify `src-tauri/src/commands/library.rs`
  - Add `library_watch(library_type)` and `library_unwatch(library_type)` commands.
  - Create `WARDIAN_HOME/library/skills` before watching.
  - Watch `library/skills` recursively, plus resolved linked skill roots, and debounce `notify` events before emitting `library-changed`.
  - Reconcile linked-root watch targets after change bursts so newly added linked skills are watched after their create/rename event.
  - Discover linked skill roots anywhere in the nested skill tree, matching the recursive behavior of `get_library_tree`.
  - Keep helper functions small enough to unit test path/type validation, watcher target resolution, and watcher generation checks.

- Modify `src-tauri/src/lib.rs`
  - Register `library_watch` and `library_unwatch`.

- Modify `src/store/useLibraryStore.ts`
  - Add stale-response protection for `fetchLibraryTree`.
  - Add `subscribeToLibraryChanges(libraryType)` helper with frontend ref counting so multiple mounted consumers share one `listen` handler and one backend watch lease.
  - Refetch on matching `library-changed` events only.

- Modify `src/store/useLibraryStore.test.ts`
  - Cover request sequencing for stale successes and stale failures.
  - Cover event subscription, matching payload refresh, non-matching payload ignore, and cleanup.
  - Cover multiple local subscribers producing one backend watch and one refetch per event.

- Modify `src/views/LibraryView.tsx`
  - Fetch on mount/active tab changes as it does today through `setActiveTab`, but subscribe only while `activeTab === 'skills'`.

- Modify or create `src/views/LibraryView.test.tsx`
  - Cover Skills-tab subscription lifecycle if a focused existing test harness is practical.
  - If `LibraryView` requires too much unrelated app wiring, cover the same lifecycle in store tests and `App.test.tsx` only.

- Modify `src/features/library/ManageSkills.tsx`
  - Fetch `skillTree` on every mount, even when a cached tree already exists, so changes made while no skill UI was active are detected when the panel opens.
  - Subscribe to skill library changes while mounted.
  - Recompute available skills from `skillTree`.
  - Refresh deployed skills when `skillTree` changes so removed/renamed library skills do not leave the panel stale while open.
  - Guard deployed-skill refreshes so stale responses cannot update state after target changes or unmount.

- Create or modify `src/features/library/ManageSkills.test.tsx`
  - Cover that available options update when `skillTree` changes.
  - Cover deployed skill refresh after `skillTree` changes.

- Modify `src/views/App.tsx`
  - Keep initial preload of prompts and skills.
  - Do not add a global library watcher here.

- Modify `src/views/App.test.tsx`
  - Ensure initial library preload behavior remains unchanged.
  - No global `library_watch` expectation should be added.

- Modify `docs/developer/tauri-command-reference.md`
  - Add `library_watch`, `library_unwatch`, and `library-changed`.

## Event Contract

Backend emits:

```ts
type LibraryChangedEvent = {
  library_type: 'skills';
};
```

Only `skills` is supported in this plan. Backend commands should reject unsupported library types with a clear error instead of silently watching prompts.

Frontend behavior:
- `subscribeToLibraryChanges('skills')` calls `library_watch`.
- It listens for `library-changed`.
- On `{ library_type: 'skills' }`, it calls `fetchLibraryTree('skills')`.
- Cleanup removes the listener and calls `library_unwatch`.

## Backend Watcher Semantics

- `library_watch('skills')` increments a ref count.
- When the count transitions from `0` to `1`, create the skills directory and start one recursive watcher.
- `library_unwatch('skills')` decrements the ref count.
- When the count reaches `0`, drop the watcher.
- Extra unwatch calls should be safe and leave the count at `0`.
- Rapid filesystem events are debounced to one `library-changed` event.
- The watcher should emit after create, modify, remove, and rename events under `library/skills`.
- Watcher creation must be all-or-nothing: if directory creation or watcher setup fails, do not increment or persist a reference count.
- Watcher/count updates must happen through a single `Mutex<HashMap<String, LibraryWatchRegistration>>` so concurrent watch/unwatch calls cannot split count and watcher state.

Symlink/junction behavior:
- Required implementation watches `library/skills` recursively and also watches resolved roots for skill directories that are links/junctions.
- Discover linked skill roots by recursively scanning the skill tree and canonicalizing directories that contain `SKILL.md`, matching `get_library_tree`'s nested-folder behavior.
- De-duplicate watch targets so a normal local skill under `library/skills` does not add a redundant second watch, but a linked root outside `library/skills` is watched recursively.
- After each debounced change burst, reconcile the watch target set before emitting `library-changed`. This lets a newly added linked skill become watched for future edits after the link create/rename event.
- Add a real watcher test for normal skill create/edit/remove and a linked-root watcher test on platforms where the repo's link helper can create directory links. If linked-root creation is unavailable, skip with an explicit reason and document the missing platform coverage.
- Each watcher registration has a monotonically increasing generation id. Debounce tasks must check that the registration still exists and has the same generation before reconciling targets or emitting. A dropped or replaced registration must not emit stale events.

## Frontend Subscription Semantics

- Multiple UI consumers may call `subscribeToLibraryChanges('skills')` at the same time.
- The store must keep one frontend subscription record per library type:
  - `refCount`
  - `unlisten?: () => void`
  - `listenPromise?: Promise<() => void>`
  - `disposed`
- The first subscriber invokes `library_watch` and registers one `listen('library-changed', ...)` handler.
- The first subscriber also triggers one `fetchLibraryTree('skills')` so mount freshness is shared across multiple `ManageSkills` instances.
- Later subscribers increment the frontend ref count only.
- Cleanup decrements the frontend ref count. When it reaches `0`, unlisten and invoke `library_unwatch`.
- One backend `library-changed` event should trigger at most one frontend `fetchLibraryTree('skills')`, regardless of how many `ManageSkills` or `LibraryView` consumers are mounted.
- Cleanup must handle the case where `listen` resolves after the final consumer already unsubscribed.

## Task 1: Backend Watch Commands

**Files:**
- Modify: `src-tauri/src/state/app_state.rs`
- Modify: `src-tauri/src/commands/library.rs`
- Modify: `src-tauri/src/lib.rs`

- [ ] **Step 1: Add failing Rust tests for library type validation and watch target creation**

Add tests in `src-tauri/src/commands/library.rs` for helpers:
- `library_dir_for_type(home, "skills")` returns `home/library/skills`.
- Unsupported types return an error.
- `ensure_library_watch_dir(home, "skills")` creates the directory when missing.
- `discover_skill_watch_targets(home)` includes `library/skills`.
- `discover_skill_watch_targets(home)` includes a canonicalized linked skill root outside `library/skills` when a linked directory contains `SKILL.md`.
- `discover_skill_watch_targets(home)` includes a canonicalized linked skill root in a nested folder such as `library/skills/category/planner`.
- `discover_skill_watch_targets(home)` de-duplicates targets.
- `is_current_library_watch_generation(state, "skills", generation)` returns false after the registration is removed or replaced.

Run:

```bash
cd src-tauri
cargo test commands::library::tests::library_watch
```

Expected: fail until helpers are implemented.

- [ ] **Step 2: Implement backend helpers**

Implement small helpers in `commands/library.rs`:
- `fn library_dir_for_type(home: &Path, library_type: &str) -> Result<PathBuf, String>`
- `fn ensure_library_watch_dir(home: &Path, library_type: &str) -> Result<PathBuf, String>`
- `fn is_supported_watch_library_type(library_type: &str) -> bool`
- `fn discover_skill_watch_targets(skills_dir: &Path) -> Result<Vec<PathBuf>, String>`

Reuse `LIBRARY_SKILLS_DIR`. `discover_skill_watch_targets` should return the root skills directory plus canonicalized linked skill directories outside that root. It must recursively scan nested library folders so every UI-visible skill node with `SKILL.md` can contribute a linked-root watch target.

- [ ] **Step 3: Add watcher state**

In `AppState`, add one serialized map:

```rust
pub library_watchers: Mutex<HashMap<String, LibraryWatchRegistration>>,
```

Add a focused state type near `AppState`:

```rust
pub struct LibraryWatchRegistration {
    pub watcher: notify::RecommendedWatcher,
    pub ref_count: usize,
    pub generation: u64,
    pub watched_paths: Vec<std::path::PathBuf>,
}
```

Initialize the map in `Default`. Do not use separate watcher and count maps.

- [ ] **Step 4: Implement `library_watch` and `library_unwatch`**

Add Tauri commands in `commands/library.rs`.

Core behavior:
- Validate `library_type`.
- Lock `state.library_watchers`.
- If a registration exists, increment its `ref_count` and return.
- If no registration exists, create the skills directory, discover watch targets, create the watcher, allocate a new generation id, and only then insert a registration with `ref_count: 1`.
- If watcher creation fails, return the error without changing state.
- Debounce events in an async task.
- After each debounce window, verify the registration still exists with the same generation id. If not, return without reconciling or emitting.
- If the generation is current, rediscover linked watch targets and add newly discovered targets to the live watcher before emitting.
- Emit `library-changed` with payload `{ library_type }`.
- On unwatch, lock the same map, decrement count, and remove the whole registration when count reaches zero.
- If unwatch is called with no registration, return `Ok(())`.

- [ ] **Step 5: Register commands**

Add both commands to the `tauri::generate_handler!` list in `src-tauri/src/lib.rs`.

- [ ] **Step 6: Add real watcher behavior tests**

Add Rust tests for a helper or command-internal watcher constructor where practical:
- Start a temporary skills directory watcher.
- Create a skill directory with `SKILL.md`.
- Assert a debounced change notification is observed.
- Modify `SKILL.md`.
- Assert a notification is observed.
- Remove or rename the skill.
- Assert a notification is observed.

Add a linked-root test:
- Create an external skill directory with `SKILL.md`.
- Link/junction it into `library/skills`.
- Start or reconcile the watcher.
- Modify the external `SKILL.md`.
- Assert a notification is observed through the linked-root watch.

If the link helper cannot create directory links on the current platform, skip only that linked-root test with a precise skip reason. Do not skip normal filesystem watcher coverage.

- [ ] **Step 7: Run backend-focused tests**

Run:

```bash
cd src-tauri
cargo test commands::library
```

Expected: pass.

## Task 2: Store Subscription and Request Sequencing

**Files:**
- Modify: `src/store/useLibraryStore.ts`
- Modify: `src/store/useLibraryStore.test.ts`

- [ ] **Step 1: Add failing store tests**

Add tests for:
- `subscribeToLibraryChanges('skills')` invokes `library_watch`.
- Matching `library-changed` event refetches only skills.
- Non-matching events do not refetch.
- Cleanup unlistens and invokes `library_unwatch`.
- A slower old `fetchLibraryTree('skills')` response cannot overwrite a newer response.
- A slower old failed `fetchLibraryTree('skills')` request cannot overwrite `error` or `isLoading` after a newer request succeeds.
- Two active `subscribeToLibraryChanges('skills')` subscribers invoke `library_watch` once, register one event listener, and produce one skill refetch for one matching event.
- The first cleanup keeps the shared subscription alive while another subscriber remains.
- The final cleanup unlistens and invokes `library_unwatch` once.

Mock both:

```ts
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn() }));
```

Run:

```bash
npm run test -- useLibraryStore.test.ts
```

Expected: fail until store changes are implemented.

- [ ] **Step 2: Add request sequencing**

In `useLibraryStore.ts`, keep a module-level request id per library type:

```ts
const fetchRequestIds: Record<'prompts' | 'skills', number> = { prompts: 0, skills: 0 };
```

When fetching, increment the type's id and only apply the result if it matches the latest id.
Guard success, error, and `isLoading` updates with the same id. A stale request must not update the tree, clear a newer error, set a new error, or flip `isLoading` after a newer request has completed.

- [ ] **Step 3: Add subscription helper**

Add to `LibraryState`:

```ts
subscribeToLibraryChanges: (type: 'skills') => () => void;
```

Implementation:
- Maintain a module-level subscription map by library type.
- On the first subscriber, call `invoke('library_watch', { libraryType: type })`.
- On the first subscriber, register `listen<LibraryChangedEvent>('library-changed', ...)`.
- On later subscribers, increment `refCount` without adding another listener or backend watch.
- On first subscription and on matching events, call `get().fetchLibraryTree(type)`.
- Return cleanup that decrements `refCount`.
- Only when `refCount` reaches `0`, unlisten when available and call `invoke('library_unwatch', { libraryType: type })`.

Use a local `disposed` guard so a late `listen` promise cannot leave an active listener after cleanup.

- [ ] **Step 4: Run store tests**

Run:

```bash
npm run test -- useLibraryStore.test.ts
```

Expected: pass.

## Task 3: ManageSkills Refresh Behavior

**Files:**
- Modify: `src/features/library/ManageSkills.tsx`
- Create or modify: `src/features/library/ManageSkills.test.tsx`

- [ ] **Step 1: Add failing ManageSkills tests**

Tests:
- When mounted, `ManageSkills` fetches skills and subscribes to library changes even if `skillTree` already contains cached data.
- When mounted with a stale non-null `skillTree`, `ManageSkills` keeps rendering cached options until the fresh fetch resolves, then updates to the fresh tree.
- When `skillTree` changes to include a new skill, the select options update.
- When `skillTree` changes, deployed skills are refreshed for the same target.
- A slower deployed-skill response from an old target cannot overwrite the current target's deployed list.
- A deployed-skill response resolving after unmount does not update state.
- On unmount, the subscription cleanup is called.

Run:

```bash
npm run test -- ManageSkills.test.tsx
```

Expected: fail until component changes are implemented.

- [ ] **Step 2: Subscribe while mounted**

In `ManageSkills`, pull `subscribeToLibraryChanges` from the store and add:

```ts
useEffect(() => subscribeToLibraryChanges('skills'), [subscribeToLibraryChanges]);
```

Do not keep the current `if (!skillTree) fetchLibraryTree('skills')` behavior as the only freshness path; a cached tree may be stale after no skill UI was mounted. The shared `subscribeToLibraryChanges('skills')` helper owns the mount freshness fetch so multiple `ManageSkills` instances do not each issue duplicate library-tree requests.

- [ ] **Step 3: Refresh deployed skills when the library tree changes**

After `skillTree` changes and the component recomputes `availableSkills`, call `refreshSkills()`.

Use `useCallback` for `refreshSkills` so effects have stable dependencies and do not loop unnecessarily.

Guard `refreshSkills` with:
- a monotonically increasing request id in a `useRef`
- an `isMounted`/disposed ref
- current `targetType` and `targetId` captured per request

Only apply `setDeployedSkills` and `setIsLoading(false)` if the request id is still current, the component is mounted, and the target still matches.

- [ ] **Step 4: Run ManageSkills tests**

Run:

```bash
npm run test -- ManageSkills.test.tsx
```

Expected: pass.

## Task 4: LibraryView Skills-Tab Subscription

**Files:**
- Modify: `src/views/LibraryView.tsx`
- Modify or create: `src/views/LibraryView.test.tsx`

- [ ] **Step 1: Add failing LibraryView test if practical**

Test:
- When `activeTab` is `skills`, `LibraryView` subscribes to skill changes.
- Switching to `prompts` cleans up the skill subscription.

Run:

```bash
npm run test -- LibraryView.test.tsx
```

Expected: fail until view changes are implemented.

- [ ] **Step 2: Subscribe only on the Skills tab**

In `LibraryView`, pull `subscribeToLibraryChanges` from the store and add:

```ts
useEffect(() => {
  if (activeTab !== 'skills') return;
  return subscribeToLibraryChanges('skills');
}, [activeTab, subscribeToLibraryChanges]);
```

- [ ] **Step 3: Keep mount freshness**

Do not remove existing preload or tab-change fetching. Opening the Library view should still fetch the current tab snapshot, so changes made while no watcher was active are detected.

- [ ] **Step 4: Run LibraryView test**

Run:

```bash
npm run test -- LibraryView.test.tsx
```

Expected: pass, or document why lifecycle is covered through store/consumer tests instead.

## Task 5: Documentation

**Files:**
- Modify: `docs/developer/tauri-command-reference.md`

- [ ] **Step 1: Update command list**

Under Library commands, add:
- `library_watch`
- `library_unwatch`

- [ ] **Step 2: Update event list**

Under app-level backend events, add:
- `library-changed`

Note payload:

```json
{ "library_type": "skills" }
```

- [ ] **Step 3: Run docs diff check**

Run:

```bash
git diff --check -- docs/developer/tauri-command-reference.md
```

Expected: no output.

## Task 6: Verification

**Files:**
- All changed files.

- [ ] **Step 1: Frontend focused tests**

Run:

```bash
npm run test -- useLibraryStore.test.ts ManageSkills.test.tsx LibraryView.test.tsx App.test.tsx
```

Expected: all targeted tests pass.

- [ ] **Step 2: Full frontend checks**

Run:

```bash
npm run lint
npm run test
npm run build
```

Expected: all pass. Existing unrelated warnings may remain, but no new warnings should be introduced.

- [ ] **Step 3: Backend checks**

Run:

```bash
cd src-tauri
cargo clippy
cargo test
cargo check
```

Expected: all pass.

- [ ] **Step 4: Native watcher validation**

Add or run a native/runtime watcher validation. Browser tests are not sufficient for the filesystem watcher acceptance criteria.

Preferred native test:
- Starts the app with isolated `WARDIAN_HOME`.
- Opens a skill-library consumer.
- Creates a skill directory with `SKILL.md` under `library/skills`.
- Verifies the UI updates without relaunch.
- Removes or renames the skill and verifies the UI updates.
- Creates or uses a linked/junction skill root when supported on the host.
- Verifies edits inside the linked target trigger the UI refresh.

Run:

```bash
npm run setup:e2e:native
npm run test:e2e:native
```

Expected: native test passes.

Fallback if native harness is unavailable locally:
- Add backend real watcher tests for normal skills and linked roots where link creation is supported.
- Add explicit manual verification steps to the PR.
- Open or link a follow-up issue for any platform where linked-root watcher coverage could not run.
- Do not mark the symlink/junction acceptance criterion complete unless linked-root behavior was verified on at least one supported platform or covered by a backend test using the repo's directory-link helper.

- [ ] **Step 5: Final diff hygiene**

Run:

```bash
git diff --check
git status --short
```

Expected: no whitespace errors and only intended files changed.
