# Managed worktree build caches

Git remains the source of truth for worktree registration. Agent assignment is
owned by Wardian's workspace records. Use `wardian agent worktree enable` and
`join` for new managed assignments; ordinary project worktrees belong beside
the primary checkout under `<source-checkout>.wt/`. These commands preserve the
target provider conversation through a workspace handover. A harness subagent uses its coordinator's prepared
checkout with an explicit working directory; creating a worktree does not
require creating another permanent agent.

Ephemeral baseline source exports can live in private temporary directories.
They are unassigned measurement inputs, with no implementation branch,
compiler output, or provider lifecycle. Preserve existing owners when changing
policy; moving an active checkout is a separate operation.

## Cargo configuration and runtime routing

Ordinary managed provider commands use the central direct lane:

```text
<source-checkout>.cargo-cache/direct-targets/<repo-key>/<worktree-key>
```

Resolve the primary Git checkout even when an agent's recorded source is a
linked worktree. Both keys are the first 16 lowercase hex characters of the
SHA-256 of the canonical absolute UTF-8 path. Only Windows converts backslash
separators to forward slashes and folds case. POSIX retains literal backslashes
and case, so `a\b` and `a/b` are distinct identities and output lanes. Containment
checks follow the same platform distinction. `WARDIAN_RUST_CACHE_ROOT` can select
a different absolute cache root without parent traversal. Existing linked
output ancestors and lanes overlapping the source checkout's `target` are
refused. This routing does not initialize Cargo home.

The backend distinguishes three configurations:

| Configuration | Managed setup | Provider environment |
| --- | --- | --- |
| Tracked minimal `[build] target-dir = "target"` | Preserve bytes and Git cleanliness | Route target and build directory centrally |
| No config, or exact untracked legacy generated source-target config | Generate or atomically migrate to the central lane | Use the same central lane |
| Custom config, tracked deletion, or legacy `.cargo/config` | Preserve the user's policy | Add no managed Cargo output override |

Current Wardian sources track the minimal local config. Ordinary Cargo in an
external shell already writes to that checkout's local `target`. The historical
source-target redirection affects generated configs in checkouts lacking a
checked-in config; it does not describe every current Wardian worktree.

Generated configs set `target-dir` and `build-dir` to the same central lane.
Matching ancestry preserves native build-script assumptions. Interactive and
headless managed provider launches use `CARGO_TARGET_DIR` and
`CARGO_BUILD_BUILD_DIR` for eligible configs. Explicit inherited Cargo output
variables remain unchanged and bypass managed source resolution. They never overwrite tracked
defaults or select a compiler wrapper. Linked Cargo config files/directories,
unexpected file types, failed Git ownership checks, and changed config during
migration fail closed. Custom configs are not silently repaired.

## Compiler caching and lifecycle

The Rust cache launcher shares one sccache store and server endpoint. Its
default store limit is 10 GB. Existing wrapper choices and Cargo home remain
under user control. Before invoking Cargo it removes inherited output-lane
environment variables and supplies explicit target/build arguments, avoiding
those variables in sccache keys for otherwise reusable registry compilations.

The launcher uses a separate `launcher-targets/<repo-key>/<worktree-key>` lane. Target
ownership markers and exclusive claims belong to that launcher lane, with
claims outside compiler output directories. A crashed claim is refused until
an explicit ownership decision; no automatic stale-claim cleanup occurs.
For one operator-qualified ended claim, use the receipt-bound recovery command
in the [Rust cache guide](./rust-build-cache.md); it archives claim metadata
without cleaning or moving compiler outputs.

Direct Cargo invocations do not acquire the launcher's claim. Their
`direct-targets` lanes are unowned and non-prunable even when no claim exists; inspection
must report their storage as skipped. This output storage is not bounded by
the sccache store limit. The launcher can recognize the matching managed direct
environment, remove its lane variables, and route into its exclusively claimed
wrapper lane. An unmarked target must not be treated as owned merely because
its directory name matches the layout. Separate lanes duplicate Cargo working
sets; the launcher's owned lanes have explicit bounded retention, while direct
lanes are reported without being automatically adopted or pruned. Both lanes
can reuse the same compiler cache when the user's wrapper settings permit it.

Worktree deletion recognizes the exact old and new untracked generated config
templates. It removes neither central compiler outputs nor claims. Inspection
and pruning remain explicit and must refuse linked, unowned, claimed, or
protected targets. Qualified runtime artifacts remain outside compiler-writable
lanes. See [CI verification](./ci-verification.md) for protected-input admission;
output routing does not replace that guard.

## Verification

After obtaining a compiler resource with a disjoint target, run the focused
backend regression tests from the checkout root:

```bash
npm run rust:cache -- cargo test -p Wardian --lib commands::git::cargo_worktree_config::tests -- --test-threads=1
npm run rust:cache -- cargo test -p Wardian --lib worktree_build_env -- --test-threads=1
npm run rust:cache -- cargo test -p Wardian --lib worktree_with -- --test-threads=1
```

PowerShell uses the same commands. These tests create fixture Git worktrees,
exercise managed creation and exact legacy migration, preserve tracked/custom
configs, compare runtime routing, and check concurrent output paths and main
artifact preservation. Unix-only regressions register literal-backslash and
nested-separator paths, checking distinct keys, direct lanes, and registration;
Windows regressions retain separator and case equivalence. They do not establish
provider restart or native build acceptance. Complete the repository's normal verification and independent review
before publication.

Worktree enable, join, and disable preserve the exact provider conversation identity.
Off agents remain Off; active agents resume in the selected folder through the ordinary
replacement transaction. The explicit move temporarily uses Resume while retaining the
configured persistence policy. An unconfirmed candidate leaves a durable
`<wardian-home>/runtime/workspace-handovers/` reservation that blocks target deletion
until candidate exit and the reservation have been inspected.

Worktree handover drains the stopped provider's remaining history before replacement can rotate its log, using the existing conversation logging policy. A capture failure withholds replacement and leaves the published folder unchanged.

Active handover requires an owned runtime-tree exit receipt and is qualified only for Codex and mock on Windows. Other providers and platforms, and legacy or uncontained active runtimes, are rejected during preflight until separately qualified. Off assignment remains available across platforms. Unconfirmed shutdown retains conversation and target protection instead of rotating the log.

Before shutdown, handover also acquires an exact prior-provider hold using the existing conversation lease architecture. Unlike the lifecycle lease, that hold survives expiry and restart. Only confirmed owned exit releases the newly acquired hold; pre-existing holds are preserved.

### Provider handover qualification

Worktree preparation is independent of providers. Off assignment preserves the
conversation without launching a provider. Active assignment asks the provider
adapter for its typed `WorkspaceHandoverQualification`; ordinary Resume and
persistent messaging support do not qualify workspace handover. Adapters default
to an explicit unqualified reason until exact identity, effective destination cwd
and retained history are verified. Current qualified adapters are Codex and mock.

The actual runtime must independently supply an owned tree-exit receipt before
closing-history drain and replacement publication. This currently limits active
handover to supported Windows runtimes. New adapter qualification must preserve
these shared transaction rules and prove actual relative tool execution at the
destination, prior context and durable/rendered history with the same native
conversation identity. Missing-history fresh-session fallback is not continuity.

Workspace assignment also records an identity-bound `exact_resume` requirement
in the saved agent configuration. Read-only adapter preflight checks it before
shutdown, and native launch checks it again after restore or delayed dispatch.
Missing history cannot become a same-ID fresh conversation. Explicit New Session
or an ordinary Fresh resume clears the prior requirement at its intentional
conversation boundary. The roster owns this agent-scoped contract; runtime launch plans consume it.
