# Isolated Rust targets for managed worktrees

## Decision

Managed Git worktrees receive a worktree-local `CARGO_TARGET_DIR` instead of the source checkout's shared `target` directory.

## Why

Provider and interactive terminal processes inherit the same environment. A shared target directory can therefore compile one worktree while reusing dependency and crate artifacts produced by another checkout or branch. On Windows, that cross-worktree state can surface as an opaque `rustc` `STATUS_ACCESS_VIOLATION` during release builds.

The repository's `.cargo/config.toml` remains useful for ordinary commands, but it cannot override an inherited `CARGO_TARGET_DIR` environment variable. The environment value must therefore be safe for every command launched inside the managed worktree.

## Contract

- A worktree agent with a valid `git_worktree_folder` receives `<worktree>/target` as `CARGO_TARGET_DIR`.
- Agents without a worktree folder receive no override.
- Source checkout targets are not shared through this environment variable.
- Node and Python cache sharing is unchanged.

## Verification

The manager unit tests assert the worktree-local target path and the missing-folder fallback. A native release build should be run from a newly spawned worktree agent after the change so the process receives the corrected environment.
