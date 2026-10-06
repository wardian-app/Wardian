# Central isolated compiler outputs for managed worktrees

## Decision

Use `<source-checkout>.cargo-cache/direct-targets/<repo-key>/<worktree-key>` for
eligible ordinary managed worktree Cargo outputs. Canonical primary Git source identity
and canonical worktree identity determine independent SHA-256 keys. Keep build
and target directories equal to preserve native build-script ancestry.

Hash canonical absolute UTF-8 paths, converting backslash separators to forward
slashes and folding case only on Windows. POSIX preserves literal backslashes
and case: `a\b` and `a/b` remain distinct worktree identities and output lanes.
Containment checks use the same platform-specific path semantics.

Reuse compiler results through one shared sccache store and server, rather than
sharing Cargo's mutable output directory across source branches. Cargo home is
unchanged. The cache launcher owns positive target markers, exclusive claims
outside targets, and explicit inspection/pruning for its separate
`launcher-targets/<repo-key>/<worktree-key>` lane. Direct lanes remain non-prunable;
claim absence does not establish that ordinary Cargo is idle. Report their
storage as skipped rather than implying it is bounded. Routing alone creates
no claim or deletion authority, and qualified artifacts remain protected inputs.
Separate lanes duplicate Cargo working sets. Only the launcher's owned lanes
have explicit bounded retention; both may reuse the same 10 GB compiler store.

## Configuration ownership

The repository's tracked minimal local `target-dir = "target"` config stays
unchanged. Provider environment routing and the supported launcher make central
outputs reachable without tracked modifications or permanent agent creation.

Only the exact untracked historical generated source-target template migrates.
Preserve custom configs, compiler wrappers, legacy config precedence, and tracked
deletions and explicit inherited output settings. Reject linked paths and failed ownership checks before mutation or
provider launch. Worktree cleanup can remove recognized generated configs but
never central compiler output or claims.

## Evidence boundary

Backend regressions prove real fixture worktree creation, config ownership and
migration, central routing at the shared provider-environment entry point,
disjoint concurrent output paths, and preserved main artifacts. Compilation,
native operation, sccache hit measurements, independent review, and hosted CI
remain separate acceptance evidence.

The operational contract is documented in
[Managed worktree build caches](../developer/worktree-cache.md).
