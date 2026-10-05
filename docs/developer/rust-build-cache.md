# Rust build cache

Wardian's Rust launcher reuses one bounded sccache store while giving every
worktree a distinct central compiler target. Use the launcher for ordinary
checks and tests; a source checkout's running release artifacts are never a
compiler target for a worktree.

```bash
npm run rust:check
npm run rust:test
npm run rust:clippy
npm run verify:ci -- --only backend
npm run check:rust-deadcode
```

These commands also work in PowerShell. `verify:ci --list` and frontend/docs-only
verification do not acquire a compiler target. Direct `cargo` retains its
normal incremental settings; managed worktree placement is described in the
[worktree guide](./worktree-cache.md).

Public `rust:cache -- cargo <arguments>` supports `build`, `check`, `test`,
`clippy`, `doc`, `metadata`, and `fmt`. Global verbosity, color, configuration,
and offline/locked/frozen flags may precede the command; `+toolchain` stays first.
Unsupported commands and global options are rejected before target claims or
process launch. In particular, `clean`, `run`, `install`, and external Cargo
subcommands are not forwarded. Use the explicit retention command for owned
inactive launcher outputs. Formatting remains available to normal verification.

## Explicit dependency setup

The dependency is official **sccache 0.18.0**. Setup never starts a compiler,
Cargo build, server, installer for Wardian, or a user runtime. There are two
different setup commands:

```bash
# Record an already installed official executable; this does not download it.
WARDIAN_SCCACHE='<absolute-sccache-executable>' npm run rust:cache:setup
```

PowerShell:

```powershell
$env:WARDIAN_SCCACHE = '<absolute-sccache-executable>'
npm run rust:cache:setup
```

On **Windows x64**, explicitly download and verify the official release ZIP:

```powershell
npm run rust:cache:setup -- --install
```

The installer fetches the ZIP and its SHA-256 sidecar from the
[official release](https://github.com/mozilla/sccache/releases/tag/v0.18.0), verifies
the archive before extraction, and stores the executable in a unique private
`<cache-root>/tools/sccache-0.18.0-<attempt>/` directory. It never changes PATH,
replaces an existing executable, or modifies user bins. Attempts and provenance
remain inspectable; setup performs no cleanup. On other platforms, obtain the
matching official executable yourself and use the first form.

`npm run rust:cache -- setup` validates and records an available executable. It
does **not** install anything. `rust:cache setup --install` is rejected; use
`rust:cache:setup -- --install`.

Setup records `sccache-identity.json` under the cache root: schema, pinned
version, absolute wrapper path, executable SHA-256, store path, size limit,
and server port. Later launches verify the recorded executable hash. This is
an operational record, not an authorization grant or proof of daemon identity.

## Layout and concurrency

By default the cache root is the source checkout's sibling
`<source-checkout>.cargo-cache/`. Git's canonical common directory determines
the source checkout. Set `WARDIAN_RUST_CACHE_ROOT` to choose another absolute
root. Copied exports and fixtures must supply an explicit
`WARDIAN_RUST_CACHE_SOURCE_ROOT`; they cannot infer a source checkout without Git.

```text
<cache-root>/
  sccache/                         bounded compiler cache
  sccache-identity.json            setup record
  tools/                          explicit dependency setup attempts
  launcher-targets/<repo-key>/<tree-key>/ claimed, prunable compiler outputs
  direct-targets/<repo-key>/<tree-key>/   ordinary Cargo outputs, inspect only
  claims/<repo-key>/<tree-key>/    exclusive launcher claims
  pruning/                        explicit prune quarantine
```

Ordinary managed direct Cargo uses `direct-targets/`. Launcher commands use
`launcher-targets/` exclusively. A launcher accepts an inherited direct-lane
target matching that worktree, then routes its compilation to the launcher
lane. Direct outputs never receive launcher ownership or enter its prune set.
Do not manually point direct Cargo at a launcher target; that violates the
exclusive routing contract and invalidates prune eligibility.

Keys are the first 16 hexadecimal characters of SHA-256 over canonical paths.
Only Windows converts backslashes to slashes and folds case. POSIX preserves
canonical path bytes, including literal backslashes in filenames. The repository
key uses the source checkout; the tree key uses the worktree. Worktrees keep
separate targets and claims. The launcher uses Cargo arguments to select its
target, removing inherited routing variables only when they agree with that
target. Conflicting custom targets fail before compilation; use direct Cargo
for an intentionally different target.

The existing machine-wide Cargo home continues to share registry archives,
unpacked source, and Git dependency caches. No per-worktree Cargo home is
created. Cargo has download/mutation locks in its source cache; builds may read
the same source cache concurrently.
[Cargo home](https://doc.rust-lang.org/cargo/guide/cargo-home.html) and
[Cargo's cache locking](https://github.com/rust-lang/cargo/blob/rust-1.97.0/src/cargo/util/cache_lock.rs)
describe the separate source-cache responsibilities.

Sharing a mutable Cargo target would serialize matching profile builds behind
Cargo's build-directory lock. Separate targets permit independent compilation;
one launcher's target claim prevents two launcher commands writing the same
tree. There is no global compiler mutex. Shared sccache storage supports
concurrent clients of **one server**; multiple servers using one local store
are unsupported. The managed endpoint defaults to port `4227` and the store cap
to `10G`. Set `SCCACHE_DIR`, `SCCACHE_CACHE_SIZE`, and `SCCACHE_SERVER_PORT` before
explicit setup to record a different arrangement. All clients must agree with
the server's actual configuration; do not assume a reused daemon adopted new
environment variables. Verify its reported cache location and limits before
benchmarking. [sccache local storage](https://github.com/mozilla/sccache/blob/v0.18.0/docs/Local.md)

Stable Cargo supports `build.build-dir` since **1.91**, including 1.97 and 1.99.
The launcher keeps it equal to the isolated target. It does not share dependency
output directories or enable nightly fine-grained locking. Splitting final and
intermediate directories requires separate compatibility work: Wardian's build
script currently derives the ConPTY destination from `OUT_DIR` ancestry.
[Cargo changelog](https://doc.rust-lang.org/cargo/CHANGELOG.html#cargo-191-2025-10-30)

## Cache coverage and diagnostics

The launcher selects `CARGO_INCREMENTAL=0` only when it injects sccache and no
explicit incremental override exists. Uncached commands retain Cargo's default.
Explicit environment wrappers, including empty values and workspace wrappers,
and wrappers declared in Cargo configuration are preserved rather than replaced.
Quoted and dotted wrapper keys, including workspace wrappers, are recognized;
extensionless `config` takes precedence over `config.toml`. The launcher injects
a wrapper only when absence is established in its conservative single-line TOML
subset. Includes, inline tables, multiline values, and unfamiliar syntax leave
the lane uncached and preserve Cargo's configuration handling.
Caller-supplied CLI `--config` also declines injection, since it can override a
wrapper or load another file. Compiler output routing still uses the claimed lane.
Set `WARDIAN_RUST_CACHE_DISABLE=1` to disable wrapper injection.

For debugging with full symbols and incremental compilation:

```bash
CARGO_PROFILE_DEV_DEBUG=2 CARGO_INCREMENTAL=1 npm run rust:check
```

PowerShell:

```powershell
$env:CARGO_PROFILE_DEV_DEBUG = '2'
$env:CARGO_INCREMENTAL = '1'
npm run rust:check
```

Explicit affirmative incremental settings (`1` or `true`), including
`CARGO_BUILD_INCREMENTAL=true`, preserve the override and disable the launcher's
sccache injection. An explicit generic carrier is never superseded by inserting
`CARGO_INCREMENTAL=0`. The pinned sccache CLI rejects the numeric opt-in;
keeping it injected would fail before compilation. An
explicit custom wrapper remains unchanged and is the caller's responsibility.
Crates requiring the system linker,
including binaries, proc macros and cdylibs, also bypass it. Wardian's combined
`staticlib,cdylib,rlib` app target cannot be counted as a cacheable rlib-only
invocation. Build-script execution and generated output remain local; wrapping
rustc does not automatically wrap native C/C++ compilation. MSVC is supported,
but caching native compilation needs its own compatible compiler invocation.

sccache 0.18.0's Rust source accepts metadata-only rlib compilation, despite its
Rust.md still claiming `link` must be emitted. Treat check/test reuse separately.
It hashes compiler identity, source and dependency content, relevant flags,
Cargo environment, and compiler cwd. Registry paths are common; local package
paths and generated output differ between worktrees. Do not strip semantic
paths/environment or assume `SCCACHE_BASEDIRS` normalizes Rust cache keys.
File-reading proc macros require particular care. These are cache eligibility
limits, not measured speedups.
[sccache Rust implementation](https://github.com/mozilla/sccache/blob/v0.18.0/src/compiler/rust.rs) and
[Rust caveats](https://github.com/mozilla/sccache/blob/v0.18.0/docs/Rust.md)

If `WARDIAN_PROTECTED_INPUT_MANIFESTS` is present, the launcher reports an
**uncached protected-input lane** and never injects sccache, even with an identity
record. A reused shared daemon's output roots cannot be qualified by that
record. An existing custom wrapper remains subject to the compiler-input guard;
there is no exemption or bypass. The isolated target and normal profile still
apply. [Compiler input protection](./ci-verification.md)

## Inspect and explicitly prune inactive outputs

```bash
npm run rust:cache -- inspect
npm run rust:cache -- prune --keep 2 --max-bytes 10737418240
```

The commands also work in PowerShell. Inspection reports individual targets,
bytes, modification times, claims and refusal reasons. Prune retains at most
the requested number and byte budget of newest inactive owned targets. It never
runs during startup or verification. The sccache store has its own cap; active
compiler outputs, tools, and retained failed attempts are additional storage.
Central placement alone does not bound total disk usage.

Prune requires the launcher's exact `compiler-writable`, `exclusive-launcher`
ownership marker and an
exclusive target claim. It refuses links, unowned directories, claimed targets,
qualified markers and any overlap with protected-input inventories. Direct
Cargo outputs and historical shared targets are never pruned. Deletion only
addresses the positively claimed launcher generation after renaming it into
private quarantine. Claims alone cannot protect a target used by direct Cargo;
the separate namespaces are necessary for retention safety.

Crashes leave claims visible. A PID alone is not proof of inactivity; there is
no automatic stale-claim repair. Inspect and reconcile ownership before a human
removes a stale claim. A qualified or running artifact belongs outside compiler
targets in an independently hashed immutable bundle. Pruning never migrates
inputs, deletes qualified bundles, or makes a source/release claim.

## Evidence required before performance claims

Compare fixed toolchain/source/features under the ordinary dev profile and
`CARGO_PROFILE_DEV_DEBUG=2`, with separate private output roots. Record wall
time, commands, flags, exit status and artifact bytes. Distinguish first compile,
same-target no-op, and warm shared-cache rebuild into a fresh target. Capture
sccache counters and its actual store before and after each run.

Prove concurrent check/test work with overlapping compiler activity in distinct
targets and bounded jobs, rather than merely two Cargo processes. Benchmark
small fixtures first; serialize tests that share native resources. Repeat
representative `wardian-core` changes before selecting an incremental policy.
No document, source inspection, or static Node test establishes a speedup.

```bash
npm run rust:cache:test
```

The default regression suite uses private Node fixtures only. It never invokes
Cargo, rustc, native tests, the app, the installer, or a cache server.
