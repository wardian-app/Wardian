# Local CI verification

Run the project-owned core CI checks with:

```sh
npm run verify:ci
```

To rerun one category after a failure:

```sh
npm run verify:ci -- --only backend
```

The runner reads the marked `run:` steps from `.github/workflows/ci.yml`, so
the command list and flags stay coupled to CI. It fails fast and echoes the
literal failing command. Use `npm run verify:ci -- --list` to inspect the
resolved sequence without executing it.

`--only` accepts exactly one of `frontend`, `backend`, or `docs`. Missing,
empty, unknown, and repeated category options fail before any check starts.
Each workflow marker must immediately precede an equally indented, literal
single-line `run:` command. YAML block scalars are not supported.

Backend verification lints and tests all workspace targets, including tests
and examples. Documentation tests run separately because `--all-targets` does
not include them. The command-contract tests in `src/verify-ci.test.ts` pin
this coverage and exercise invalid arguments and workflow declarations.

## Dead-code gates

Two [knip](https://knip.dev) passes over `src/` run in the frontend job:

- `npm run check:deadcode` treats tests, E2E specs, and scripts as entry
  points. It catches files and exports that nothing reaches, including unused
  test helpers.
- `npm run check:deadcode:production` runs `knip --production`, which drops
  every test entry. Only `src/main.tsx`, reached through `index.html`, remains.
  It catches production code that only its own tests import.

When the production pass reports an export, delete it and the tests that only
exercise it. If it is deliberate test support, such as a test seam, a fixture
builder, or an invariant checker, keep it and add an `@internal` JSDoc tag with
the reason:

```ts
/** @internal Test support, no production caller: resets module state between tests. */
export function resetForTesting() {}
```

knip ignores `@internal` exports only in production mode. The default pass
still reports one that tests stop using.

A file that only tests import is excluded from the production project in
`knip.json` with a `"!<path>!"` pattern. The trailing `!` limits the exclusion
to production mode. `src/test/**` is excluded as a directory. Every other
exclusion names one file. `src/config/vite*.ts` is excluded because only
`vite.config.ts` imports it, and knip does not trace plugin config files in
production mode.

For provider fixtures, shared environment locks, and deliberate contention,
use [Test Reliability](./test-reliability.md). That guide maps each pattern to
its executable check and states the limits of the evidence.

The local sequence covers the frontend, backend, and documentation quality
steps. PR screenshot/code-claim checks, dependency audits, coverage uploads,
and browser/native suites remain CI- or environment-specific.

The hosted `Backend (macOS - Codex Home ACL)` job runs the focused Codex home
platform tests on `macos-latest`. Its native fixtures check that a private
directory without an extended ACL is accepted and one with an ACL entry is
rejected. This is filesystem validation, not a Codex spawn or provider test;
that runtime acceptance still requires a separate Mac run.

A local pass does not complete PR delivery. Follow [Pull Request
Delivery](./pull-requests.md) to monitor hosted checks on the latest published
commit, resolve failures, and verify that all applicable checks have finished
successfully before declaring the task complete.

CI validates pull requests against any base branch, including stacked PRs.
The `Wardian Docs` workflow also accepts any PR base when its existing docs,
package metadata, or workflow path filters match. Pull requests build docs;
Pages configuration, artifact upload, and deployment remain disabled for PRs.
Both workflows retain `main`-only push triggers, and the docs workflow retains
its manual `workflow_dispatch` trigger. Routing and Pages guards are pinned in
`src/config/ciWorkflow.test.ts`.

## Rust dead code

`npm run check:rust-deadcode` fails when Rust production code contains an item
that no production code uses. It is the Rust counterpart of
`check:deadcode` (knip). It runs in the Windows backend job and in
`verify:ci -- --only backend`. Tests never count as callers: an item that only
tests call is dead in production. `telemetry::maintain` (#1082) had tests and
no production caller.

The script runs three checks:

| Check | Scope | Method |
| --- | --- | --- |
| rustc | The `src-tauri` library | The script copies the workspace under `<cargo-target-dir>/rust-deadcode/`. In the copy, every `pub` item in `src-tauri/src` becomes `pub(crate)`, except the functions that `main.rs` calls (`run`). Then it runs `cargo check --workspace --lib` without `cfg(test)`. Each `dead_code` warning that rustc then reports is a finding. |
| Token search | Shared library crates (`wardian-core`) | An item is dead when production code cannot reach it by name. Production code in another workspace crate, and crate code outside any item (for example a trait impl), are the roots. A name used inside an item counts only once that item is reachable, so a chain or cycle of items that only call each other is dead. Definitions, `impl` headers, `use` declarations, comments, `#[cfg(test)]` code, tests, and examples do not count. |
| Commands | `tauri::generate_handler!` | Every registered command must be invoked by name from non-test frontend code or Rust production code. Frontend names are string literals found with the TypeScript parser, so a name in a comment never counts. A `debug_*` command can also be invoked from `e2e/`, `e2e-native/`, or `scripts/`. |

The script never writes to the checkout. It updates the copy in place and
rewrites only files whose content changed, so cargo reuses its incremental
state. When nothing changed, a run takes a few seconds. After a change to
`src-tauri` or `wardian-core`, the script checks the copy's crates again.

**Platform.** CI runs the check on Windows, and the baseline is recorded on
Windows. rustc cannot see a caller that the current platform compiles out,
such as `#[cfg(unix)]` code on Windows. If compiled-out code names a reported
item, the check sets that item aside. For a method, field, or variant, the
compiled-out code must also name the item's type. `--verbose` lists the items
set aside. A baseline entry whose item this platform compiles out is never
reported as stale. On Linux or macOS, the check can report `#[cfg(unix)]`
items that the Windows CI run does not see.

### When the check fails

The output names each item and the exact baseline entry it would need. Do one
of the following:

1. Delete the item. This is the expected fix for code that nothing calls.
2. Call the item from production code, if a call is missing. A cleanup or
   retention function with no caller is often a missing call.
3. Keep a test-only helper next to the tests that use it. Put it inside the
   test module (`#[cfg(test)] mod tests { ... }`), or in a test-support module
   declared once as `#[cfg(test)] mod test_support;`. Do not add
   `#[cfg(test)]` to the function itself. `check:budgets` counts a
   `#[cfg(test)]` attribute directly on a `fn` in production files as a
   test seam, and fails when that count rises.
4. Add the entry to `scripts/rust-deadcode-baseline.json`. Use this option
   only when the item must stay and none of the options above applies, for
   example a field that holds a resource until `Drop`. Add the entry to the
   group whose reason matches, or add a new group with a one-line reason. The
   reviewer must accept the reason.

`#[allow(dead_code)]` also silences rustc. Prefer the baseline, because the
baseline keeps each kept item and its reason in one reviewed file.

### Shrinking the baseline

When you delete or start calling a baselined item, the check fails until you
remove its entry. Remove the entry by hand, or run:

```sh
npm run check:rust-deadcode -- --prune
```

`--prune` removes only entries that match no finding on this platform. It
never adds entries.
