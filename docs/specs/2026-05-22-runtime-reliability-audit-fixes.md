# Runtime Reliability Audit Fixes

## Goal

Close the highest-risk failure modes found in the May 22, 2026 audit of `origin/main`: destructive native E2E cleanup, silent native test skips, terminal output stalls, unbounded PTY buffers, linked-worktree git watching, workflow trigger races, stale frontend async responses, incomplete Rust CI coverage, and oversized production chunks.

## Decisions

- Native E2E uses `WARDIAN_E2E_NATIVE_HOME` for disposable state and ignores ambient `WARDIAN_HOME` when choosing a cleanup target.
- Native E2E infrastructure failures mark the process as failed by default; local exploratory runs can opt into skip behavior with `WARDIAN_E2E_ALLOW_INFRA_SKIP=1`.
- PTY output buffers are capped with a shared UTF-8-preserving helper that drops oldest unread output when the UI is disconnected or slow.
- User terminal output drains until the backend buffer is empty and uses a generation guard to avoid stale writes after remount.
- Git file watching resolves linked-worktree `.git` files before selecting `HEAD` and `index` watch targets.
- Workflow file triggers use an unbounded event channel with coalescing and logged setup failures instead of panics, silent no-ops, or one-slot drops.
- Workflow run tasks wait for cancellation-handle registration before executing so fast runs cannot leave stale handles.
- Frontend polling paths use monotonic request ids so older responses cannot overwrite newer state.
- CI Rust checks run against the full Cargo workspace.
- Vite splits major vendor groups into separate chunks to reduce the main production bundle.
- Remote/browser and native E2E fixtures track current UI and IPC contracts instead of stale button labels, expired session timestamps, missing chat endpoints, or legacy shell settings payloads.

## Verification

Completed verification in this branch:

```bash
npm run lint
npm run test
npm run build
npm run test:e2e
npm run test:e2e:native:fast -- e2e-native/tests/user-terminal-native.test.mjs
cargo check --workspace
cargo test --workspace
cargo clippy --workspace -- -D warnings
node --test e2e-native/tests/native-preflight.test.mjs
```

`cargo fmt --all --check` was also run. It still reports formatting drift in pre-existing, unrelated Rust files outside this audit fix set; the audit-touched Rust files were formatted without broadening the change.
