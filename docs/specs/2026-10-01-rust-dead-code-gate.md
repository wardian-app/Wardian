# Rust dead-code gate

- **Status:** Implemented
- **Date:** 2026-10-01

## Context and Problem Statement

`check:deadcode` (knip) fails on an unreferenced TypeScript export. Rust had no
equivalent. rustc's `dead_code` lint does not fire for a `pub` item in a
library crate, because a library's public items are its API. Both Rust
libraries in the workspace have no external consumers. `src-tauri` is used
only by its own binary. `wardian-core` is used only by the app and the CLI. As
a result, an uncalled `pub fn` passed every gate. PR #1082 merged
`wardian_core::telemetry::maintain`, a retention path that had tests and no
production caller (#1085).

PRs #1515 and #1517 removed about 1,500 lines found by a manual procedure. That
procedure was not repeatable in CI, and nothing stopped new dead items.

## Proposed Decision

`npm run check:rust-deadcode` (`scripts/verify-rust-deadcode.mjs`) runs three
checks. Tests never count as callers in any of them.

1. **rustc on the app crate.** The script copies the workspace into the cargo
   target directory. In the copy, every `pub` item in `src-tauri/src` becomes
   `pub(crate)`, except the entry points that the binary calls. Then the
   script runs a non-test `cargo check`. rustc's own `dead_code` analysis then
   applies to the whole app library. The analysis is transitive and resolves
   types.
2. **Token search on shared library crates.** rustc cannot see across the
   `wardian-core` boundary. Every item in that crate is a node in a name
   graph. Production code in another crate, and crate code outside any item
   (trait impls, macro bodies), are roots. A name used inside an item is an
   edge from that item. An item whose name no root reaches is dead, so chains
   and cycles of items that only call each other are dead as a whole.
   Definitions, `impl` headers, `use` declarations, comments, and test-only
   code are not references. The script finds test-only code by following
   `mod`, `#[path]`, and `include!` from each crate root, and by evaluating
   `cfg` attributes. A predicate it cannot evaluate, such as a cargo feature,
   counts as possibly enabled.
3. **Tauri commands.** Every command in `generate_handler!` must be invoked by
   name from production code. `debug_*` commands can be invoked from the E2E
   suites or scripts instead. Frontend names come from the TypeScript parser's
   string literals, not a text search, so a call left in a comment never
   counts; the Windows backend job installs the npm dependencies for this.

Existing findings are recorded in `scripts/rust-deadcode-baseline.json`,
grouped by the reason each is kept. A new finding fails the gate. A baseline
entry that matches no finding also fails, and `--prune` removes such entries.
Shrinking the baseline is therefore one deletion, and growing it is a reviewed
diff.

The gate runs in the Windows backend job, after `cargo check --workspace`.
At that point every dependency is already checked, so the gate compiles only
the copy's workspace crates. Windows is also the platform where the baseline
is recorded. If code that the platform compiles out names a reported item,
the gate sets that item aside. For a method, field, or variant, that code
must also name the item's type.

### Alternatives considered

- **Rewrite the checkout in place and restore it afterwards.** This saves the
  copy's compile. An interrupted run would leave the checkout modified, and
  the restored files would get new modification times, which forces rebuilds.
- **Token search for the app crate as well.** This is cheaper, but it is not
  transitive, and a common method name such as `new` or `id` hides a dead
  item. rustc resolves both cases.
- **Run on Linux.** The Linux job builds instrumented coverage artifacts, so
  every dependency would need an additional plain check build. Most
  contributors also develop on Windows.

## Consequences

- **Positive**: A `pub fn` with no production caller fails CI. A function
  called only from tests fails too, which is the `maintain` failure mode.
- **Positive**: The first run found unwired cleanup and retention paths, such
  as `db::prune_events`. #1536 tracks them, so they were not deleted silently.
- **Negative**: The baseline starts at 118 entries. Most are app or core code
  that only tests call. It shrinks as that code is deleted or moved into test
  modules.
- **Negative**: Each CI run rechecks the copy of `wardian-core` and the app
  crate. The Windows backend job takes about one to two minutes longer.
- **Negative**: The token search for `wardian-core` matches names, not
  resolved paths. A dead item with a common name, such as `new`, can still
  pass.
