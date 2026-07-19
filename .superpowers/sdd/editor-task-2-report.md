# Task 2 implementation report

Date: 2026-07-17

Base: `205062cb204d9a6664c52f8625adc3edd38fe81c`

Status: `DONE`

## Outcome

Implemented guarded atomic UTF-8 text saves for live file-resource
subscriptions and an exact-target native Save As authority flow.

- Core saves bind the backend-private revision token and expected hash to the
  retained file authorization, stage and flush a sibling file, revalidate the
  path/root/identity immediately before replacement, and return a newly
  authorized private revision.
- Runtime saves are serialized with watcher refresh and close through a
  per-resource operation mutex. IPC exposes only tagged `saved`, `unchanged`,
  or `stale_conflict` metadata, never the private revision capability.
- Save As grants are 60-second, one-shot capabilities bound to an opened
  parent-directory identity and one exact basename. Missing targets use an
  atomic no-replace commit; existing targets use the guarded replacement
  primitive. Save As does not accept or retarget a source/artifact resource.
- Windows retained handles opt into read/write/delete sharing, existing target
  replacement uses `ReplaceFileW`, and missing-target commits use
  `MoveFileExW` without replacement.

## RED evidence

The initial focused core test build failed with six missing-method errors for
`AuthorizedPath::guarded_atomic_replace_text`. The initial runtime and Save As
test builds then failed on the missing save result variants and missing runtime
methods. These compile failures established the new API surface before its
implementation.

## Verification

Focused core tests:

```text
CARGO_BUILD_JOBS=1 cargo test -p wardian-core guarded_atomic_text_replace -- --test-threads=1
4 passed; 0 failed
Exit code 0
```

Focused runtime tests:

```text
CARGO_BUILD_JOBS=1 cargo test -p Wardian file_resources_save_ -- --test-threads=1
8 passed; 0 failed
Exit code 0
```

Workspace compile gate:

```text
CARGO_BUILD_JOBS=1 cargo check --workspace
Exit code 0
```

Strict workspace lint gate:

```text
CARGO_BUILD_JOBS=1 cargo clippy --workspace --all-targets --all-features -- -D warnings
Exit code 0
```

Full workspace tests:

```text
CARGO_BUILD_JOBS=1 cargo test --workspace
All unit, integration, and doc tests passed
Exit code 0
```

Two diagnostic full runs forced through `--test-threads=1` each passed all
1,198 other Wardian library tests but exposed the same unrelated
`terminal_session` activation-timeout timing assertion. That test passed
immediately in isolation, and the required default-concurrency workspace gate
above passed in full.

The focused tests cover stale frontend revision/base hash, external
same-identity mutation, revoked roots, changed identities, symlink/junction
retargeting, permission preservation, unchanged writes, no partial bytes on a
rejected write, one logical revision with watcher echo suppression, exact
one-shot Save As, parent/target binding races, and source-resource
non-retargeting. A two-subscription regression saves through each subscription
in turn and proves both can read the replacement identities.

## Documentation

- Added `docs/specs/2026-07-17-files-guarded-text-saves.md` with the authority,
  atomicity, concurrency, conflict, and Save As decisions.
- Updated `docs/developer/tauri-command-reference.md` with all three IPC
  commands and cross-platform request/response examples.
- Added Rust documentation for the new guarded result and IPC DTOs.

## Hygiene and self-review

- Scoped `rustfmt` completed for all changed Rust files; `git diff --check`
  passed.
- The pre-existing `package-lock.json` modification was not edited and will
  not be staged.
- No frontend behavior changed, so frontend tests/build and screenshot evidence
  are outside this task.
- Native and real-provider E2E were not run; the behavior is covered by
  cross-platform unit/integration tests, including Windows-native filesystem
  execution in this checkout.
- Every same-resource live subscription is prevalidated before replacement.
  The guarded core result will rebind only the exact pre-write identity and
  canonical target, preserving each subscription's requested path and root;
  runtime installs those rebound handles together while the resource operation
  lock is held. No known Task 2 security concern remains.

## Security review fix wave (2026-07-18)

The independent Task 2 review found four commit-boundary gaps. All four were
fixed as one security wave:

1. After sibling staging and flush, core now invokes the backend commit check,
   rescans the expected size and hash through the same locked retained handle,
   and only then performs the final pathname/root/identity check and replace.
   The deterministic barrier test mutates the original in place after staging;
   it receives `stale_revision` and the newer bytes survive.
2. Existing-resource subscription admission now acquires `entry.operation`,
   reauthorizes after the lock, and retries across entry-incarnation changes.
   A concurrent open launched after save candidate capture blocks until commit,
   then reads and saves the replacement; the original subscription reads the
   next replacement.
3. `save_file_resource_text` no longer resolves or passes a command-layer
   `AgentConfig` snapshot. Runtime resolves all live claims from its
   backend-owned resolver initially and again through the core's final commit
   callback. Deterministic revocation after initial validation rejects the save,
   preserves original bytes, and emits no revision event.
4. Save As acquires an owned ordinary-capability table reservation before it
   consumes the target grant or commits bytes. The guard holds capacity stable
   through commit and makes publication infallible. Saturated-table tests prove
   both missing and existing destinations remain unchanged on
   `grant_limit_reached`; the missing-target test also proves the target grant
   was not consumed.

Focused fix evidence:

```text
CARGO_BUILD_JOBS=1 cargo test -p wardian-core guarded_atomic_text_replace -- --test-threads=1
5 passed; 0 failed

CARGO_BUILD_JOBS=1 cargo test -p Wardian file_resources_save_ -- --test-threads=1
12 passed; 0 failed
```

The final scan is the optimistic-concurrency boundary. Cross-process filesystem
APIs do not supply an atomic compare-and-replace on file content, so an external
writer beginning after that scan can still race replacement; the spec now
states that limitation instead of claiming exclusion beyond the last scan.

Final security-wave gates:

```text
CARGO_BUILD_JOBS=1 cargo check --workspace
Exit code 0

CARGO_BUILD_JOBS=1 cargo clippy --workspace --all-targets --all-features -- -D warnings
Exit code 0

CARGO_BUILD_JOBS=1 cargo test --workspace
Wardian library: 1203 passed; 0 failed
All remaining unit, integration, and doc tests passed
Exit code 0
```

Security-wave self-review confirmed that existing-entry admission and save use
the same operation-lock order; commit validation compares exact subscriber
membership and re-resolves every current claim; staged-write rejection removes
the sibling without publishing a revision; and Save As holds the ordinary grant
table reservation from before target-grant consumption until infallible
publication. No new secret, environment, frontend, artifact-retargeting, or
private-token surface was introduced. The pre-existing unstaged
`package-lock.json` modification remains outside the task.
