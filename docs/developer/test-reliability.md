# Test reliability

Tests are examples that contributors copy. Keep their inputs explicit and their
failure signals meaningful. These are local test-infrastructure conventions;
they do not change provider ingestion or application runtime behavior.

## Choose the owning mechanism

| Failure mode | Implementation pattern | Executable enforcement |
| --- | --- | --- |
| Test code bypasses Rust linting | CI and local verification use `clippy --workspace --all-targets -- -D warnings` | Clippy rejects blocking guards across awaits and default-then-reassign fixtures; command-contract tests pin the target flags |
| Sync environment locks block async tests | Sync and async fixture entry points share one Tokio mutex | The shared-lock regression proves mutual exclusion; the sync entry point rejects runtime use; the full backend suite exercises migrated constructors |
| A contention test holds a blocking guard across `await` | A test-only `HeldMutex` owns the guard on a separate thread | Readiness, release, and panic-cleanup tests; Clippy checks the async callers |
| A provider test reads changing personal logs or passes without evidence | Committed fixtures for defaults; explicit-path, owned snapshots for external diagnostics | Fixture goldens, forced-reparse checks, snapshot-mutation tests, and invalid-input tests |
| A malformed verification selector expands to the whole suite | Validate selectors before reading or executing the plan | Parser tests and subprocess tests assert failure without command output |
| A workflow block is mistaken for a literal shell command | Adjacent, equally indented marker and single-line command | Workflow negative fixtures reject block scalars and malformed steps |

These checks prevent the named regressions. They are not a proof that arbitrary
new tests are hermetic, that external provider formats remain compatible, or
that every application lock ordering is correct.

## Shared process state in Rust tests

Use `crate::utils::wardian_test_env_lock()` only outside an async runtime.
Inside async tests or async fixture constructors, await
`crate::utils::wardian_test_env_lock_async()` instead. Both entry points acquire
the same process-wide mutex. Keep the guard until shared-state restoration is
complete. An unrelated local mutex does not serialize against other fixtures.

Give mixed-use fixtures separate sync and async constructors that delegate to
the same initialization code. `control/test_support.rs` is a working example.
Use an owned temporary directory and restore environment values in `Drop`.
Construct the cleanup owner before fallible initialization after an environment
change, so failed setup does not leave the next test with that environment.

For deliberate contention, see `remote/operations/test_support.rs`. Its
`HeldMutex` acknowledges acquisition before the test reads the contended state,
then releases and joins the holder on drop, including during assertion failure.
Do not acquire that same mutex before dropping the holder. When the contract is
only that a lock is immediately available, assert `try_lock()` directly.

## Provider logs

Keep arithmetic and parser regressions in committed, sanitized fixtures under
`crates/wardian-core/tests/fixtures/`. Missing fixtures and missing usage facts
are failures, not successful early returns. Default Codex and Pi tests do not
consult `WARDIAN_TEST_CODEX_LOG`, `WARDIAN_TEST_PI_LOG`, or personal session
directories.

The shared diagnostic implementation is
`crates/wardian-core/tests/common/provider_log.rs`. It requires a provider and
file path, captures an owned copy, and uses that copy for declared totals,
ingestion, and forced reparse. Its types stay out of the production API.

### Check an external log

Prerequisites: run from the repository root with the Rust toolchain installed.
Use a completed JSONL file for either `codex` or `pi`. The diagnostic reads that
file but does not modify it or the application database. Do not publish private
log contents as test evidence.

POSIX shell:

```sh
cargo run -p wardian-core --example verify_provider_log -- codex '<provider-log-path>'
```

PowerShell:

```powershell
cargo run -p wardian-core --example verify_provider_log -- codex '<provider-log-path>'
```

Replace `codex` with `pi` for a Pi session. Require exit status zero before
reporting success. Missing arguments, unreadable or unusable input, mismatched
totals, and reparse drift must fail. A snapshot prevents later source changes
from changing the comparison; it cannot make a concurrently written source an
atomic or complete provider session. A diagnostic failure still requires
investigation and must not be hidden by changing the fixture expectation.

## Changing verification

Edit the marked commands in `.github/workflows/ci.yml`, then update and run
`src/config/ciWorkflow.test.ts`. Keep documentation tests in the sequence when changing
Cargo target flags. Add invalid-input cases when extending the runner syntax;
do not infer a default category from malformed input.

Run `npm run verify:ci -- --list` to inspect the resolved plan without execution.
Run `npm run verify:ci` for the full local contract. See
[CI Verification](./ci-verification.md) for checks that require other environments.
