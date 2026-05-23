# VS Code-Like Source Control Graph

## Context

Wardian's Source Control panel already provides staging, diffing, commits, sync actions, and worktree controls for the selected agent workspace. The prior history list was useful but too thin compared with the Source Control Graph experience in VS Code, and the push action failed silently for local branches that had not been published.

This spec tracks the scoped improvement for [issue #368](https://github.com/wardian-app/Wardian/issues/368).

## Reference Behavior

The implementation was checked against VS Code source:

- `extensions/git/src/actionButton.ts` switches the primary Git action between commit, publish, and sync depending on branch upstream state.
- `extensions/git/src/historyProvider.ts` supplies history items with parent IDs, refs, author metadata, display hash, timestamp, and tooltips.
- `src/vs/workbench/contrib/scm/browser/scm.contribution.ts` registers a Source Control Graph view and graph-specific settings.
- `src/vs/workbench/contrib/scm/browser/scmHistoryViewPane.ts` renders graph rows, repository/ref controls, selected item details, refresh actions, context menus, and paged history loading.

Wardian should not clone all of VS Code's SCM surface in one change. The first parity step is to make the Wardian panel expose the same core state signals: upstream status, branch publication, decorated refs, parent hashes, and selected commit details.

## Decisions

- `git_status` now includes `upstream` so the frontend can distinguish **Push** from **Publish Branch** without parsing errors.
- `git_push` detects whether the current branch has an upstream before running Git. Branches with upstream still use `git push`; branches without upstream use `git push --set-upstream <remote> <branch>`.
- Publish remote selection prefers `origin` and falls back to the first configured remote.
- Detached `HEAD` and repositories with no remotes return explicit errors rather than doing nothing.
- `git_log` now returns parent hashes, decorated refs, author email, and ISO dates using record and field separators so messages do not break parsing.
- The frontend history section is renamed **Graph** and renders compact graph rows with refs, author/date metadata, short hashes, and a commit detail panel.
- Sync failures are shown in the Source Control panel instead of being logged only to the developer console.

## UI Shape

The graph remains a sidebar-native control, not a full workbench clone. Rows are dense and scannable:

- left rail: commit node and vertical history line
- center: message, refs, author, and date
- right: short hash
- detail panel: message, hash, author, email, date, and parents for the selected commit

Theme variables and existing Wardian utility classes remain the styling authority.

## Verification

Automated coverage:

- `src/features/git/GitPanel.test.tsx` verifies graph rows, refs, commit details, publish labeling, and surfaced push failures.
- `src-tauri/src/commands/git.rs` tests verify upstream parsing and publish behavior for a local branch with no upstream.

Targeted commands:

```bash
npm run test -- src/features/git/GitPanel.test.tsx
cargo test git::tests:: --manifest-path src-tauri/Cargo.toml
```

PowerShell:

```powershell
npm run test -- src/features/git/GitPanel.test.tsx
cargo test git::tests:: --manifest-path src-tauri\Cargo.toml
```
