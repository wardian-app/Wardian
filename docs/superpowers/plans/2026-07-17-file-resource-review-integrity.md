# File Resource Review Integrity Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the grant-eviction, membership-reconciliation, and live-claim refresh races found in the independent review of subscription-scoped Files authorization.

**Architecture:** Keep the canonical watcher and revision stream shared, but make grant activity authoritative inside the grant mutex and make every membership transition schedule reconciliation. Refresh resolves each candidate's live claim through a testable current-agent resolver (AppState in production, opening snapshots in standalone tests), then revalidates the candidate's exact requested pathname before scanning.

**Tech Stack:** Rust 2021, Tauri 2 managed state, Tokio mutexes/tasks/barriers, `notify`, cross-platform junction/symlink tests.

## Global Constraints

- Do not modify or stage concurrent renderer/frontend files.
- Preserve one watcher and revision stream per canonical file.
- Preserve exact requested-path authorization per subscription.
- Never hold the entries, grant, AppState-agent, or resolver locks across another subsystem lock.
- Run Files/core tests, strict Clippy, workspace check, documentation checks, and diff ownership checks before committing.

---

### Task 1: Authoritative picker grant activity

**Files:**
- Modify: `src-tauri/src/state/file_resources.rs`

**Interfaces:**
- Consumes: `UserFileGrant`, `open_user_file`, `open_authorized`, `close`, `upsert_user_file_grant`.
- Produces: `UserFileGrant::active_subscriptions: usize` and eviction based only on `in_flight_uses == 0 && active_subscriptions == 0` under `user_file_grants`.

- [ ] **Step 1: Write the deterministic eviction interleaving test**

Add a test barrier immediately before `upsert_user_file_grant` acquires the grant mutex. Pause selection of a second file, fully open the only existing capability while selection is paused, then resume selection and assert `grant_limit_reached`, the first capability still exists, close releases it, and the second selection then succeeds.

- [ ] **Step 2: Run the regression and verify the stale membership snapshot fails**

Run:

```powershell
$env:CARGO_BUILD_JOBS='1'
cargo test -p Wardian state::file_resources::tests::active_picker_subscription_cannot_be_evicted_after_membership_interleaving -- --exact --test-threads=1
```

Expected before the fix: the active capability is evicted or the assertion that it remains live fails.

- [ ] **Step 3: Coordinate open/close accounting under the grant mutex**

Add `active_subscriptions` to `UserFileGrant`. Keep `in_flight_uses` nonzero from pre-authorization until `open_authorized` finishes; on success, decrement in-flight and increment active in the same grant-lock section. On close, remove the subscription from the resource first, then decrement the removed user claim's active count. Replace the entries snapshot in eviction with:

```rust
grant.in_flight_uses == 0 && grant.active_subscriptions == 0
```

- [ ] **Step 4: Run the eviction and existing picker tests**

Run the exact regression, then `picker_grants_are_exact_path_deduplicated_and_lru_bounded`. Both must pass with one grant retained until its final subscription closes.

### Task 2: Membership reconciliation

**Files:**
- Modify: `src-tauri/src/state/file_resources.rs`

**Interfaces:**
- Consumes: `schedule_refresh_for_incarnation`, `open_authorized`, `close`.
- Produces: one debounced reconciliation after an existing-entry open or a close that leaves subscribers.

- [ ] **Step 1: Add transition regressions**

Add one test that makes an alias-only resource unavailable, opens a valid direct subscriber without changing the file, and expects a recovery revision. Add another that leaves an invalid alias plus a valid direct subscriber, closes the direct subscriber, and expects one unavailable revision without a filesystem event. After each expected event, wait beyond two stability windows and assert no additional event.

- [ ] **Step 2: Run both tests and verify they time out before the fix**

Run each exact test with `--test-threads=1`. Expected before the fix: no recovery/unavailable event is received.

- [ ] **Step 3: Schedule reconciliation on membership changes**

When `open_authorized` inserts into an existing entry, capture its incarnation, publish the subscription mapping, and call `schedule_refresh_for_incarnation`. When `close` removes a subscriber but leaves the entry nonempty, capture its incarnation and schedule once after releasing entries and grant locks. Generation checks continue to discard superseded scans.

- [ ] **Step 4: Run membership and debounce tests**

Run both new tests plus the existing burst, unchanged-hash, and old-incarnation tests. Expected: one semantic event per transition and no refresh loop.

### Task 3: Live claim validation before refresh scans

**Files:**
- Modify: `src-tauri/src/state/file_resources.rs`
- Modify: `docs/developer/workbench-surfaces.md`
- Modify: `docs/specs/2026-07-17-files-subscription-authorization-provenance.md`

**Interfaces:**
- Consumes: `tauri::AppHandle`, `AppState::agents`, `FileAccessClaim`, `AuthorizedPath::requested_path`, `user_file_grants`.
- Produces: `CurrentAgentConfigResolver`, `validate_refresh_candidate`, and refresh candidates carrying their full `FileSubscriptionAccess`.

- [ ] **Step 1: Add revoked-claim tests**

Add deterministic tests proving a revoked agent or picker candidate does not reach the descriptor scanner, a valid later candidate refreshes the shared revision, and an invalid-only resource publishes unavailable while preserving the prior content hash. Track descriptor scan calls with a test-only atomic counter.

- [ ] **Step 2: Run the revoked-claim tests and verify the current refresh violates them**

Run each exact test with `--test-threads=1`. Expected before the fix: the revoked candidate increments the descriptor-scan counter or drives the new hash.

- [ ] **Step 3: Implement the resolver and validation boundary**

Use a cloneable resolver enum. The production variant resolves `AgentConfig` from the AppHandle's managed `AppState`; the standalone variant retains opening snapshots and supports deterministic test revocation. Clone the resolver before awaiting so no `RwLock` guard crosses an await. For every candidate:

```rust
let authorized = self.validate_refresh_candidate(&candidate.access).await?;
let refreshed = self.refresh_from_authorization(authorized).await?;
```

Agent validation requires a live matching session and current authorized roots. Picker validation requires the live capability and matching canonical target. Both first preserve exact alias provenance with `reauthorize_same_target`.

- [ ] **Step 4: Document and verify the contract**

Document that refresh authority has parity with reads/tickets and that membership changes reconcile availability. Run `npm run docs:check-llms` and `npm run docs:build`.

### Task 4: Full validation and commit

**Files:**
- Verify only the files above plus this plan are staged.

**Interfaces:**
- Consumes: all three completed tasks.
- Produces: one semantic follow-up commit to `300a4071`.

- [ ] **Step 1: Run focused and full Rust gates**

```powershell
$env:CARGO_BUILD_JOBS='1'
cargo test -p Wardian state::file_resources::tests -- --test-threads=1
cargo test -p wardian-core files:: -- --test-threads=1
cargo clippy -p wardian-core -p Wardian --all-targets -- -D warnings
cargo check --workspace
```

Expected: all commands exit zero.

- [ ] **Step 2: Audit formatting, ownership, and secrets**

Run `rustfmt --edition 2021 --check src-tauri/src/state/file_resources.rs`, scoped `git diff --check`, `git status --short`, and inspect the staged name list. Renderer and frontend changes must remain unstaged.

- [ ] **Step 3: Commit without amending**

```powershell
git commit -m "fix(files): reconcile live resource authority"
```

Expected: a new commit after `300a4071` containing only runtime, test, plan, and relevant clean documentation files.
