# Source Control VS Code Parity

- **Status:** In progress
- **Date:** 2026-06-25

## Context

Wardian already has a Source Control sidebar with branch status, staging,
commit, push/publish, diffs, recent history, and worktree controls. The user
reported two high-value parity gaps with VS Code:

- the Source Control activity icon should show a pending-change badge
- the history/graph surface should be richer, structurally clear, and easy to
  expand or shrink

The reference implementation is the official `microsoft/vscode` repository.
Key source files inspected:

- `src/vs/workbench/contrib/scm/browser/activity.ts`
- `src/vs/workbench/contrib/scm/browser/scmViewPane.ts`
- `src/vs/workbench/contrib/scm/browser/scmHistory.ts`
- `src/vs/workbench/contrib/scm/browser/scmHistoryViewPane.ts`
- `src/vs/workbench/contrib/scm/browser/media/scm.css`
- `extensions/git/src/repository.ts`
- `extensions/git/src/commands.ts`
- `extensions/git/src/staging.ts`
- `extensions/git/src/historyProvider.ts`
- `extensions/git/package.json`

## VS Code Feature Inventory

### 1. Activity Bar Pending-Change Badge

VS Code computes a source-control badge from repository resource counts. The
workbench-level SCM controller supports `scm.countBadge` as `all`, `focused`,
or `off`, then shows a `NumberBadge` only when the count is nonzero. The Git
extension separately computes its provider count from merge, index, working
tree, and untracked groups, honoring `git.countBadge` and
`git.untrackedChanges`.

User value: the SCM icon answers "are there uncommitted changes?" before the
panel is opened.

Wardian parity decision: show a compact numeric badge on the Source Control
rail icon for the single selected agent workspace. Count all `git_status.files`
for now, suppress zero, cap display at `99+`, and reset to zero for ambiguous
multi-selection or non-git roots. This matches VS Code's visible behavior while
respecting Wardian's selected-agent workspace model.

### 2. Resource Groups and Source-Control Tree Mechanics

VS Code models SCM as repositories, inputs, action buttons, resource groups,
resource folders, and resources. Git maps porcelain statuses into merge, index,
working tree, and untracked groups, with status letters, colors, optional icons,
tooltips, strike-through for deletions, and propagated file decorations.
Resource groups expose counts and inline/context-menu actions.
The Git extension contributes group-specific commands to
`scm/resourceGroup/context`, including stage-all merge, unstage-all index,
stage-all tracked, stage-all untracked, and group-level open-diff actions for
staged, working-tree, and untracked resources.
It also contributes `scm/resourceFolder/context` actions so tree folder nodes
can stage, unstage, discard, or ignore only the resources under that folder.
For individual SCM resources, VS Code also contributes navigation actions such
as `Open File` and `Reveal in Explorer View`, alongside change viewing and
modification commands.
VS Code's Git extension also contributes diff-editor commands such as
`git.diff.stageHunk`, `git.diff.stageSelection`,
`git.stageSelectedRanges`, and `git.unstageSelectedRanges`, implemented through
line-change patch application helpers in `extensions/git/src/staging.ts`.

User value: users can scan what changed, where it changed, and what actions are
available without opening a terminal.

Wardian parity backlog:

- add an explicit conflict/merge group when porcelain reports conflict states
- keep staged, changes, and untracked groups, but align labels, counts, status
  letters, and deletion strike-through more closely with VS Code
- add list/tree display mode for nested paths, with tree mode as the default
  for deep directory changes
- keep hover-only inline actions, but add context menus for file and group
  actions
- add folder-scoped actions in tree mode so users can stage or unstage a
  directory without affecting sibling changes
- add group-scoped open-diff actions so a user can inspect all resources in a
  staged, tracked, or untracked group from its header menu
- add resource navigation actions so changed files can be opened externally or
  revealed from the Source Control context menu
- add diff-review hunk actions, then selected-range actions once Wardian's
  diff renderer supports stable text selections

### 3. Commit Input and Action Button

VS Code renders the commit input as part of the SCM tree, supports visibility
through `git.showCommitInput`, and can show a prominent action button for
commit, publish, or sync through `git.showActionButton`.
Its SCM input widget also supports validation messages. The Git extension
contributes 50/72 commit-message rulers and validation settings, so Wardian can
surface long-line guidance without blocking the commit.
The input toolbar can also expose a primary action plus a chevron menu for
commit variants such as commit staged, commit all, amend, and signed/no-verify
forms.
VS Code stores the last SCM input action id in profile storage and promotes it
to the next primary action when the toolbar has multiple actions.

User value: the next useful Git action is obvious and close to the commit
message.

Wardian parity backlog:

- preserve the existing commit box
- add a dynamic primary action row: Commit when changes exist, Publish Branch
  when no upstream exists, Sync Changes when ahead or behind exists
- make commit placeholder branch-aware
- show non-blocking SCM-style validation warnings for long commit subject/body
  lines
- add a compact input action menu for the commit variants Wardian can safely
  support in the selected workspace
- remember the last selected commit input action and surface it as the next
  primary action

### 4. History Graph Structure

VS Code's graph view is not a decorative timeline. It renders commit lanes with
fixed 22px row height, 11px swimlanes, colored SVG paths, node circles, special
HEAD and incoming/outgoing markers, labels for refs, and expandable history item
changes. The title area includes repository and ref filters, reveal-current,
refresh, and list/tree mode actions. CSS keeps the graph compact enough to live
in a sidebar, auxiliary bar, or panel.

User value: users can understand branch shape, remote divergence, and commit
context at a glance, then expand individual commits for changed files.

Wardian parity backlog:

- replace the current simple history line with a compact graph row renderer
  using fixed lane metrics and theme colors
- include short hash, subject, author/date metadata, HEAD/current marker, and
  ahead/behind remote markers
- make graph rows expandable to changed files
- add a tiny/detailed density control and preserve collapsed state
- add a ref filter for current branch, upstream/base, or all

### 5. Refresh, Progress, and Error States

VS Code debounces file changes, avoids refreshing while operations are running,
shows source-control progress through `ProgressLocation.SourceControl`, and
keeps welcome/error states explicit for missing Git, no repositories, unsafe
repositories, and closed repositories.

User value: Git state updates feel live without flicker or stale UI, and failure
states explain what can be done next.

Wardian parity backlog:

- keep the current watcher plus polling fallback
- show refresh/sync progress on the Source Control rail and panel header
- expose VS Code-like checkout in the title/header area so users can switch
  local branches without leaving the sidebar
- expose VS Code-like branch creation from the source-control branch controls
- expose the most common VS Code stash operations from a compact
  source-control overflow menu
- expose VS Code's staged-only stash action for index-focused workflows
- expose VS Code's selected-stash apply action with the same compact stash
  selector used by preview
- expose VS Code's selected-stash pop action with the same compact stash
  selector used by preview and apply
- expose VS Code's selected-stash drop action with the same compact stash
  selector and a Wardian confirmation step
- expose VS Code's destructive stash cleanup action with a Wardian
  confirmation step
- expose VS Code's stash preview action with a compact stash selector
- expose VS Code-like fetch in the title/header area so users can update
  remote refs without merging
- organize the Source Control overflow into compact command groups so VS
  Code-like secondary actions do not bloat the title row or root menu
- improve non-git and git-missing states with direct actions where Wardian can
  offer them
- avoid duplicating expensive git status work where the badge and panel observe
  the same selected root

## Implementation Order

1. Activity rail pending-change badge for selected agent source control.
2. Shared selected-agent git status observer to reduce duplicate polling.
3. Resource group parity: conflict group, status metadata, deletion styling, and
   group/file context actions.
4. Primary action button row for commit/publish/sync.
5. Compact graph renderer with fixed swimlanes and ref labels.
6. Expandable graph rows with per-commit changed files.
7. Graph density and collapse controls, persisted by selected root.
8. Source-control progress and richer empty/error states.
9. Resource list/tree display mode for nested file paths.
10. Explicit Source Control refresh action wired to status and history.
11. History graph ref filter for auto, all, current branch, and upstream refs.
12. Resource sorting by VS Code-style status priority inside groups.
13. Non-blocking SCM input validation warning for long commit messages.
14. Source-control input action menu for Commit Staged and Commit All.
15. Persisted last commit input action for the primary button.
16. Scoped resource group context menus for staged, tracked, and untracked
    batch actions.
17. Resource folder context menus for tree-mode directory actions.
18. Resource group open-diff actions for staged, tracked, and untracked
    changes.
19. Resource file context navigation actions for opening and revealing changed
    files.
20. Source Control title/header Fetch action.
21. Source Control title/header Checkout action for local branches.
22. Source Control branch-menu Create Branch action.
23. Source Control overflow Stash actions.
24. Source Control overflow Apply Latest Stash action.
25. Source Control overflow Stash Staged action.
26. Source Control overflow Drop All Stashes action.
27. Source Control overflow View Stash action.
28. Source Control overflow Apply Stash action.
29. Source Control overflow Pop Stash action.
30. Source Control overflow Drop Stash action.
31. Source Control resource-folder Add to .gitignore action.
32. Source Control non-git Initialize Repository action.
33. Source Control resource-file Add to .gitignore action.
34. Source Control resource-file Open File (HEAD) action.
35. Source Control commit-menu Undo Last Commit action.
36. Source Control commit-menu Commit (Amend) action.
37. Source Control commit-menu Commit (No Verify) action.
38. Source Control commit-menu Commit Empty action.
39. Source Control commit-menu Commit All (Amend) action.
40. Source Control commit-menu Commit Staged (Amend) action.
41. Source Control commit-menu Commit All (No Verify) action.
42. Source Control commit-menu Commit Staged (No Verify) action.
43. Source Control commit-menu Commit (Amend, No Verify) action.
44. Source Control commit-menu Commit Staged (Amend, No Verify) action.
45. Source Control commit-menu Commit All (Amend, No Verify) action.
46. Source Control commit-menu Commit Empty (No Verify) action.
47. Source Control commit-menu Commit (Signed Off) action.
48. Source Control commit-menu Commit Staged (Signed Off) action.
49. Source Control commit-menu Commit All (Signed Off) action.
50. Source Control commit-menu Commit (Signed Off, No Verify) action.
51. Source Control commit-menu Commit Staged (Signed Off, No Verify) action.
52. Source Control commit-menu Commit All (Signed Off, No Verify) action.
53. Source Control commit-menu Abort Rebase action.
54. Source Control staged-resource Compare with Workspace action.
55. Source Control non-git Clone Repository action.
56. Source Control untracked-resource Discard Changes action.
57. Source Control untracked-group Discard All Untracked Changes action.
58. Source Control tracked-group Discard All Tracked Changes action.
59. Source Control tracked/untracked inline group discard controls.
60. Source Control resource sort menu for path, name, and status ordering.
61. Source Control diff review actions for staging and unstaging the opened
    file.
62. Source Control diff hunk actions for staging and unstaging a selected diff
    hunk.
63. Source Control overflow command grouping for Branch, Sync, View, and Stash
    actions.
64. History graph incoming/outgoing divergence markers with dashed graph nodes.
65. History graph expanded commit changes tree/list view mode.
66. History graph paged loading through a compact `Load More...` row.
67. History graph commit context menu for viewing changes and copying commit
    details.
68. History graph `Go to Current History Item` title action.
69. History graph changed-file `Open File` action.
70. History graph commit `View Changes` patch action.
71. History graph ref badge filter/all mode.
72. History graph changed-file context menu `Open File` action.

## Current Slice

Implemented the first seventy-two slices:

- `useSourceControlBadge` resolves the selected agent root, observes
  `git_status`, listens for `git-changed`, polls as a fallback, and returns a
  pending-change count.
- `SidebarIconRail` renders the Source Control badge when the count is nonzero.
- `App` wires the selected-agent badge count into the rail.
- `useSelectedAgentGitStatus` now owns the selected-agent root resolution,
  `git_status` refreshes, `git-changed` listener, and polling fallback.
- `App` passes the shared observer to both the activity rail badge and the
  Source Control pane so opening the pane does not create a second status
  watcher for the same selected repository.
- `GitPanel` consumes the shared status observer while keeping commit, sync,
  worktree, diff, and history behavior local to the panel.
- `GitPanel` now separates unresolved merge/conflict porcelain statuses into a
  `Merge Changes` resource group ahead of ordinary changes, matching VS Code's
  merge/index/working-tree/untracked grouping model.
- `GitFileList` exposes VS Code-like status metadata for status badges,
  including conflict codes such as `UU`, and visually strikes deleted resources.
- File rows and the merge group header expose right-click context actions for
  the same stage, unstage, discard, diff, and group stage-all mechanics already
  available inline.
- The commit area now uses a branch-aware message placeholder and a dynamic
  primary action: `Commit` when changes exist, `Publish Branch` for clean
  branches without an upstream, and `Sync Changes` with ahead/behind counts for
  clean diverged branches.
- `git_log` now preserves parent hashes and decorated refs from Git, giving the
  frontend enough data to render branch/ref-aware graph rows instead of a
  purely decorative vertical timeline.
- `GitHistoryGraph` renders compact fixed-height history rows using 22px row
  height, 11px swimlane spacing, themed lane colors, HEAD/current markers,
  branch/upstream labels, short hashes, and author/date metadata.
- Commit graph rows now expand lazily through `git_commit_changes`, using the
  commit's first parent when present and preserving the graph placeholder
  alignment for each changed file row, matching VS Code's tree mechanics.
- The history graph now exposes detailed and tiny density controls, a collapse
  all control, and root-keyed persistence for density and expanded rows so each
  selected repository can keep its own graph presentation.
- The shared selected-agent Git observer now distinguishes initial loading from
  background refreshes. The Source Control rail shows a compact progress marker
  while Git status refreshes, and the panel keeps loaded files visible while
  announcing refresh/sync/commit/worktree progress through a live status row.
- Non-git/error states now include the affected workspace path and a reveal
  action so users can inspect the folder directly instead of seeing a dead-end
  message.
- Non-git workspace states now expose `Initialize Repository`, matching VS
  Code's `git.init` command. Wardian runs `git init` in the affected workspace
  and refreshes the shared Source Control observer so the panel can transition
  into the normal clean-repository state.
- Resource groups now default to an expandable tree presentation for nested
  paths, with a compact list/tree toggle in the Source Control header and
  root-keyed persistence for the selected display mode.
- The Source Control header now includes an explicit refresh action that
  refreshes the shared Git status observer and commit history together.
- The history graph now includes a compact ref-filter toolbar for auto refs,
  all refs, current branch, and upstream. The filter is persisted by repository
  root alongside density and expanded-row state.
- Resource rows now sort by VS Code-like status priority within each group:
  conflict states first, modified/copied/type-changed resources next, then
  ordinary added/deleted/renamed/untracked resources by path. Tree mode
  preserves directory hierarchy while applying the same priority ordering within
  each folder.
- The Source Control overflow now includes VS Code-style resource sort actions:
  `Sort by Path`, `Sort by Name`, and `Sort by Status`. Wardian keeps status
  ordering as the default, applies the selected mode to both list and tree file
  rows, and persists the choice for each repository root alongside the
  list/tree display mode.
- The diff modal now exposes review-context actions for single-file resource
  diffs. Working-tree diffs show `Stage Changes`, staged diffs show `Unstage
  Changes`, and aggregate, stash, HEAD, and compare-with-workspace views remain
  read-only. This is Wardian's first bounded step toward VS Code's
  `git.stageChange` / diff-editor staging workflow while keeping the current
  line renderer stable.
- Single-file diff hunks now expose per-hunk review actions. Working-tree hunk
  headers show `Stage Hunk`, staged hunk headers show `Unstage Hunk`, and
  Wardian applies only that unified-diff hunk to the index with
  `git apply --cached` or `git apply --cached --reverse`. This matches the
  next stable part of VS Code's `git.diff.stageHunk` and selected-range staging
  workflow without pretending the current diff renderer can yet support
  arbitrary selected line ranges.
- The Source Control overflow root menu is now grouped into `Branch`, `Sync`,
  `View`, and `Stash` submenus. Checkout/create-branch, fetch/pull/push,
  display/sort, and stash commands stay discoverable while the root menu
  remains compact enough for the sidebar.
- The history graph now renders VS Code-style synthetic `Outgoing Changes` and
  `Incoming Changes` rows when Git reports branch divergence. The rows use
  dashed graph markers, local/remote commit counts, and non-expandable graph
  row behavior while preserving the real commit lanes below them.
- Expanded history graph commits now default to a collapsible changed-file tree
  with graph-aligned folder and file rows, plus a persisted list/tree toggle for
  users who want the flat full-path change list.
- The history graph now renders a VS Code-style `Load More...` row when the
  current page is full. Selecting it requests the next page of commits while
  preserving the graph density, ref filter, expanded rows, and change view mode.
- History graph commit rows now expose a VS Code-like right-click context menu
  with `View Changes`, `Copy Commit ID`, and `Copy Commit Message`. `View
  Changes` expands the commit without collapsing already-open history rows, and
  the copy actions use the browser clipboard for fast commit sharing.
- The history graph title controls now include a VS Code-like `Go to Current
  History Item` target action. It scrolls and focuses the current `HEAD` row
  when that row is visible, marks the row with `aria-current`, and disables the
  action when the active ref filter hides the current commit.
- Expanded history graph file rows now expose a VS Code-like `Open File`
  action. Wardian reads the selected path at the expanded commit hash and opens
  it in the existing read-only diff modal with a short-hash label, matching the
  stable history-file inspection path without adding Monaco-backed editing.
- Expanded history graph file rows now expose the same `Open File` action from
  a right-click context menu, matching VS Code's separate history item change
  context menu without adding another visible graph toolbar control.
- History graph commit context menus now route `View Changes` through a
  commit-vs-parent patch command when the graph is hosted in `GitPanel`.
  Wardian opens the resulting patch in the existing read-only diff modal while
  preserving row-click expansion for changed-file tree inspection.
- The history graph now exposes a VS Code-like ref badge mode. Detailed rows
  default to badges that follow the active ref filter, while a compact title
  action switches to all decorated refs and persists the choice by repository
  root.
- The commit input now shows an advisory SCM-style validation warning when the
  subject line exceeds 50 characters or a body line exceeds 72 characters, while
  leaving the commit action available.
- The commit action row now behaves like a compact SCM input toolbar: the
  primary action remains close to the message box, and a `More Actions` chevron
  opens commit variants for `Commit Staged` and `Commit All`. `Commit All`
  stages remaining unstaged files even when other files are already staged.
- The commit input toolbar now remembers the last selected commit variant in
  profile-local storage and promotes it to the next primary action, while
  falling back to the generic `Commit` action when no variant has been chosen.
- The commit input toolbar now exposes `Undo Last Commit`, matching VS Code's
  `git.undoCommit` command in the Git commit menu. Wardian confirms the action,
  resets `HEAD` to the previous commit while preserving the undone changes in
  the working tree, restores the undone commit message into the commit input,
  and refreshes status and history.
- The commit input toolbar now exposes `Commit (Amend)`, matching VS Code's
  `git.commitAmend` command in the Git commit menu. Wardian runs
  `git commit --amend` with the current commit input message, keeps the action
  explicit in the chevron menu, and refreshes status and history after the
  amended commit is written.
- The commit input toolbar now exposes `Commit Staged (Amend)`, matching VS
  Code's `git.commitStagedAmend` command in the Git commit menu. Wardian runs
  `git commit --amend` without staging additional files, so already-staged
  changes are folded into the latest commit while unstaged and untracked work
  remains pending.
- The commit input toolbar now exposes `Commit All (Amend)`, matching VS
  Code's `git.commitAllAmend` command in the Git commit menu. Wardian stages
  all working-tree changes, including untracked files, then runs
  `git commit --amend` with the current commit input message and refreshes
  status and history.
- The commit input toolbar now exposes `Commit (Amend, No Verify)`, matching
  VS Code's `git.commitAmendNoVerify` command in the Git commit menu. Wardian
  rewrites the latest commit with `git commit --amend --no-verify`, bypassing
  local commit hooks while preserving the explicit amend workflow.
- The commit input toolbar now exposes `Commit Staged (Amend, No Verify)`,
  matching VS Code's `git.commitStagedAmendNoVerify` command in the Git commit
  menu. Wardian runs `git commit --amend --no-verify` without staging
  additional files, so already-staged changes are folded into the latest commit
  while unstaged and untracked work remains pending.
- The commit input toolbar now exposes `Commit All (Amend, No Verify)`,
  matching VS Code's `git.commitAllAmendNoVerify` command in the Git commit
  menu. Wardian stages all working-tree changes, including untracked files,
  then runs `git commit --amend --no-verify` so the latest commit is rewritten
  while local commit hooks are bypassed.
- The commit input toolbar now exposes `Commit Empty (No Verify)`, matching
  VS Code's `git.commitEmptyNoVerify` command-palette contribution. Wardian
  runs `git commit --allow-empty --no-verify` from a clean repository so users
  can create an explicit marker commit while bypassing local hooks.
- The commit input toolbar now exposes `Commit (Signed Off)`, matching VS
  Code's `git.commitSigned` command in the Git commit menu. Wardian preserves
  its normal smart-staging behavior for unstaged-only work, then runs
  `git commit --signoff` so Git appends the DCO `Signed-off-by` trailer from
  the configured user name and email.
- The commit input toolbar now exposes `Commit Staged (Signed Off)`, matching
  VS Code's `git.commitStagedSigned` command in the Git commit menu. Wardian
  runs `git commit --signoff` without staging additional files, so only
  already-staged changes are committed while unstaged and untracked work
  remains pending.
- The commit input toolbar now exposes `Commit All (Signed Off)`, matching VS
  Code's `git.commitAllSigned` command in the Git commit menu. Wardian stages
  all working-tree changes, including untracked files, then runs
  `git commit --signoff` so every pending change is captured with the DCO
  signoff trailer.
- The commit input toolbar now exposes `Commit (Signed Off, No Verify)`,
  matching VS Code's `git.commitSignedNoVerify` command in the Git commit
  menu. Wardian preserves its normal smart-staging behavior for unstaged-only
  work, then runs `git commit --signoff --no-verify` so the DCO trailer is
  appended while local commit hooks are bypassed.
- The commit input toolbar now exposes `Commit Staged (Signed Off, No
  Verify)`, matching VS Code's `git.commitStagedSignedNoVerify` command in the
  Git commit menu. Wardian runs `git commit --signoff --no-verify` without
  staging additional files, so already-staged changes are committed with a DCO
  trailer while unstaged work remains pending and hooks are bypassed.
- The commit input toolbar now exposes `Commit All (Signed Off, No Verify)`,
  matching VS Code's `git.commitAllSignedNoVerify` command in the Git commit
  menu. Wardian stages all working-tree changes, including untracked files,
  then runs `git commit --signoff --no-verify` so every pending change is
  captured with a DCO trailer while local hooks are bypassed.
- The commit input toolbar now exposes `Abort Rebase` when Git reports a
  rebase in progress, matching VS Code's `git.rebaseAbort` command in the Git
  commit menu. Wardian detects `.git/rebase-merge` and `.git/rebase-apply`
  through Git's `rev-parse --git-path`, runs `git rebase --abort`, then
  refreshes status and history.
- Staged resource rows now expose `Compare with Workspace`, matching VS Code's
  `git.compareWithWorkspace` resource-state contribution for index resources.
  Wardian runs `git diff -- <path>` to compare the staged index version against
  the working-tree version and opens the result in the existing diff modal.
- Non-git workspace states now expose `Clone Repository...`, matching VS Code's
  `git.clone` Source Control title action. Wardian opens a compact repository
  URL field, runs `git clone <repository> .` inside the selected workspace, and
  refreshes Source Control when the clone completes.
- Untracked resource rows now expose `Discard Changes`, matching VS Code's
  `git.clean` contribution for untracked resources. Wardian routes the action
  through the existing confirmed discard flow, removes the selected untracked
  file or directory from disk, and refreshes Source Control.
- The Untracked group header now exposes `Discard All Untracked Changes`,
  matching VS Code's `git.cleanAllUntracked` resource-group contribution.
  Wardian confirms the destructive action, passes only untracked resource paths
  to `git_discard_changes`, and refreshes Source Control.
- The Changes group header now exposes `Discard All Tracked Changes`, matching
  VS Code's `git.cleanAllTracked` resource-group contribution. Wardian
  confirms the destructive action, passes only tracked working-tree paths to
  `git_discard_changes`, and refreshes Source Control without touching
  untracked files.
- The Changes and Untracked group headers now expose inline discard buttons
  for the tracked and untracked clean actions, matching VS Code's `inline@2`
  placement for `git.cleanAllTracked` and `git.cleanAllUntracked`.
- The commit input toolbar now exposes `Commit (No Verify)`, matching VS
  Code's `git.commitNoVerify` command in the Git commit menu. Wardian runs
  `git commit --no-verify` with the current commit input message, preserving
  the normal Wardian rule that unstaged files are staged first only when no
  files are already staged.
- The commit input toolbar now exposes `Commit Staged (No Verify)`, matching
  VS Code's `git.commitStagedNoVerify` command in the Git commit menu. Wardian
  runs `git commit --no-verify` without staging additional files, so
  already-staged changes are committed while unstaged and untracked work stays
  pending.
- The commit input toolbar now exposes `Commit All (No Verify)`, matching VS
  Code's `git.commitAllNoVerify` command in the Git commit menu. Wardian stages
  all working-tree changes first, including untracked files, then runs
  `git commit --no-verify` and refreshes status and history.
- The commit input toolbar now exposes `Commit Empty`, matching VS Code's
  `git.commitEmpty` command in the Git commit menu. Wardian enables the action
  for clean repositories with a commit message, runs
  `git commit --allow-empty`, clears the input on success, and refreshes
  status and history.
- Staged, tracked, and untracked resource group headers now expose VS Code-like
  right-click context actions. Their batch operations are group-scoped:
  `Unstage All Changes` touches staged files, `Stage All Tracked Changes`
  touches tracked working-tree files, and `Stage All Untracked Changes` touches
  untracked files.
- Tree-mode folder rows now expose VS Code-like right-click actions. `Stage
  Changes`, `Unstage Changes`, and `Discard Changes` operate only on descendant
  resources under the clicked folder, so directory-level source-control work
  does not affect sibling files.
- Tree-mode folder rows now expose `Add to .gitignore`, matching VS Code's
  `git.ignore` resource-folder contribution for working-tree and untracked
  folders. Wardian appends the selected folder pattern to the repository
  `.gitignore`, deduplicates existing entries, then refreshes status and
  history.
- File resource rows now expose `Add to .gitignore` for unstaged resources,
  matching VS Code's `git.ignore` resource-state contribution. Wardian appends
  the selected repository-relative file path to `.gitignore` through the same
  deduplicating backend command used by folder ignores.
- Staged, tracked, and untracked resource group headers now expose VS Code-like
  open-diff actions. `Open Staged Changes`, `Open Changes`, and `Open
  Untracked Changes` collect only the files in the selected group and render a
  combined diff in Wardian's existing diff modal.
- Individual resource context menus now include `Open File` and `Reveal in
  Explorer View`. Wardian resolves Git-relative paths against the selected
  repository root and reuses the configured external editor and explorer reveal
  commands already used by the Explorer panel.
- Individual resource context menus now include `Open File (HEAD)`, matching
  VS Code's `git.openHEADFile` resource-state contribution. Wardian reads the
  selected file from `HEAD` with Git and displays that committed content in the
  existing source-control modal without touching the working tree.
- The Source Control header now exposes a `Fetch` action, matching VS Code's
  `git.fetch` SCM title contribution. Wardian runs `git fetch`, refreshes the
  selected workspace status, and reloads commit history without merging remote
  changes into the current branch.
- The Source Control header now exposes a `Checkout to...` action, matching VS
  Code's `git.checkout` SCM title contribution. Wardian lists local branches,
  marks the current branch in the menu, checks out the selected branch, then
  refreshes status and commit history for the selected workspace.
- The branch menu now includes `Create Branch...`, matching VS Code's
  `git.branch` command. Wardian opens a compact inline branch-name field,
  validates the branch through Git, creates and checks out the new local branch,
  then refreshes status and commit history.
- The Source Control header now has an overflow action menu with common VS
  Code stash commands: `Stash Changes`, `Stash Changes Including Untracked`,
  and `Pop Latest Stash`. Wardian runs the matching Git stash command for the
  selected workspace, then refreshes status and commit history.
- The overflow action menu now includes `Apply Latest Stash`, matching VS
  Code's `git.stashApplyLatest` command. Wardian applies the latest stash
  without dropping it, then refreshes status and commit history.
- The overflow action menu now includes `Apply Stash...`, matching VS Code's
  `git.stashApply` command. Wardian lists stash selectors and messages in the
  compact stash picker, applies the selected stash without dropping it, then
  refreshes status and commit history.
- The overflow action menu now includes `Pop Stash...`, matching VS Code's
  `git.stashPop` command. Wardian lists stash selectors and messages in the
  compact stash picker, applies the selected stash, drops that stash entry, then
  refreshes status and commit history.
- The overflow action menu now includes `Drop Stash...`, matching VS Code's
  `git.stashDrop` command. Wardian lists stash selectors and messages in the
  compact stash picker, confirms the destructive selected-stash removal, runs
  `git stash drop <selector>`, then refreshes status and commit history.
- The overflow action menu now includes `Stash Staged`, matching VS Code's
  `git.stashStaged` command. Wardian runs `git stash push --staged` so users
  can park only index changes while leaving unstaged working-tree edits in
  place, then refreshes status and commit history.
- The overflow action menu now includes `Drop All Stashes...`, matching VS
  Code's `git.stashDropAll` command. Wardian confirms the destructive action,
  runs `git stash clear`, then refreshes status and commit history.
- The overflow action menu now includes `View Stash...`, matching VS Code's
  `git.stashView` command. Wardian lists stash selectors and messages in a
  compact picker, then renders the selected stash patch in the existing diff
  modal without changing the worktree.

## Testing

Focused tests:

- `npm run test -- src/features/git/GitPanel.test.tsx src/features/git/GitFileList.test.tsx` (resource sort mode and compact overflow coverage)
- `npm run test -- src/features/git/GitDiffView.test.tsx src/features/git/GitPanel.test.tsx` (diff review stage/unstage coverage)
- `cargo test git_apply_diff_hunk --manifest-path src-tauri/Cargo.toml`
- `npm run lint`
- `npm run test`
- `npm run build`
- `npm run test -- src/features/git/useSourceControlBadge.test.ts src/layout/SidebarIconRail.test.tsx src/views/App.test.tsx`
- `npm run test -- src/features/git/useSelectedAgentGitStatus.test.ts src/features/git/useSourceControlBadge.test.ts src/features/git/GitPanel.test.tsx src/layout/SidebarContentPane.test.tsx src/views/App.test.tsx`
- `npm run test -- src/features/git/GitFileList.test.tsx src/features/git/GitPanel.test.tsx`
- `npm run test -- src/features/git/GitPanel.test.tsx`
- `npm run test -- src/features/git/GitHistoryGraph.test.tsx src/features/git/GitPanel.test.tsx`
- `npm run test -- src/features/git/GitHistoryGraph.test.tsx`
- `npm run test -- src/features/git/GitHistoryGraph.test.tsx` (expanded commit change tree/list coverage)
- `npm run test -- src/features/git/GitHistoryGraph.test.tsx src/features/git/GitPanel.test.tsx` (history graph load-more paging coverage)
- `npm run test -- src/features/git/GitHistoryGraph.test.tsx` (history graph commit context menu coverage)
- `npm run test -- src/features/git/GitHistoryGraph.test.tsx` (history graph current-item reveal coverage)
- `npm run test -- src/features/git/GitHistoryGraph.test.tsx src/features/git/GitPanel.test.tsx` (history graph changed-file open coverage)
- `npm run test -- src/features/git/GitHistoryGraph.test.tsx src/features/git/GitPanel.test.tsx` (history graph commit patch view coverage)
- `cargo test git_commit_diff_compares_commit_with_parent --manifest-path src-tauri/Cargo.toml`
- `npm run test -- src/features/git/GitHistoryGraph.test.tsx` (history graph ref badge filter/all mode coverage)
- `npm run test -- src/features/git/GitHistoryGraph.test.tsx` (history graph changed-file context menu coverage)
- `npm run test -- src/features/git/useSelectedAgentGitStatus.test.ts src/features/git/GitPanel.test.tsx src/layout/SidebarIconRail.test.tsx src/views/App.test.tsx`
- `npm run test -- src/features/git/GitFileList.test.tsx src/features/git/GitPanel.test.tsx`
- `npm run test -- src/features/git/GitPanel.test.tsx`
- `npm run test -- src/features/git/GitHistoryGraph.test.tsx`
- `npm run test -- src/features/git/GitFileList.test.tsx`
- `npm run test -- src/features/git/GitPanel.test.tsx`
- `npm run test -- src/features/git/GitPanel.test.tsx`
- `npm run test -- src/features/git/GitPanel.test.tsx`
- `npm run test -- src/features/git/GitPanel.test.tsx`
- `npm run test -- src/features/git/GitPanel.test.tsx`
- `npm run test -- src/features/git/GitPanel.test.tsx`
- `npm run test -- src/features/git/GitPanel.test.tsx`
- `npm run test -- src/features/git/GitPanel.test.tsx`
- `cargo test parse_git_log_entries_preserves_parent_hashes_and_refs`
- `cargo test parse_git_commit_changes_preserves_status_and_rename_target`
- `cargo test git_fetch_updates_remote_tracking_refs_without_merging`
- `cargo test git_checkout_branch_lists_and_switches_local_branches`
- `cargo test git_create_branch_creates_and_checks_out_local_branch`
- `cargo test git_stash_push_include_untracked_and_pop_latest_round_trips_worktree`
- `cargo test git_stash_apply_latest_restores_worktree_and_keeps_stash`
- `cargo test git_stash_apply_applies_selected_stash_and_keeps_stash`
- `cargo test git_stash_pop_applies_selected_stash_and_removes_it`
- `cargo test git_stash_staged_stashes_only_staged_changes`
- `cargo test git_stash_drop_removes_selected_stash_only`
- `cargo test git_stash_drop_all_clears_stash_entries`
- `cargo test git_ignore_adds_relative_folder_pattern_to_gitignore`
- `cargo test git_init_initializes_workspace_as_repository`
- `cargo test git_show_file_revision_returns_committed_content`
- `cargo test git_undo_last_commit_restores_message_and_keeps_changes`
- `cargo test git_commit_amend_updates_last_commit_without_new_commit`
- `cargo test git_commit_amend_no_verify_bypasses_failing_pre_commit_hook`
- `cargo test git_commit_staged_amend_no_verify_preserves_unstaged_changes_and_bypasses_hook`
- `cargo test git_commit_all_amend_no_verify_stages_all_changes_and_bypasses_hook`
- `cargo test git_commit_staged_amend_preserves_unstaged_changes`
- `cargo test git_commit_all_amend_stages_all_changes_before_amending`
- `cargo test git_commit_no_verify_bypasses_failing_pre_commit_hook`
- `cargo test git_commit_staged_no_verify_preserves_unstaged_changes_and_bypasses_hook`
- `cargo test git_commit_all_no_verify_stages_all_changes_and_bypasses_hook`
- `cargo test git_commit_empty_no_verify_bypasses_failing_pre_commit_hook`
- `cargo test git_commit_empty_creates_commit_without_file_changes`
- `cargo test git_commit_signed_adds_signed_off_by_trailer`
- `cargo test git_commit_staged_signed_preserves_unstaged_changes_and_adds_trailer`
- `cargo test git_commit_all_signed_stages_all_changes_and_adds_trailer`
- `cargo test git_commit_signed_no_verify_bypasses_hook_and_adds_trailer`
- `cargo test git_commit_staged_signed_no_verify_preserves_unstaged_changes_bypasses_hook_and_adds_trailer`
- `cargo test git_commit_all_signed_no_verify_stages_all_changes_bypasses_hook_and_adds_trailer`
- `cargo test git_rebase_abort_clears_in_progress_rebase_state`
- `cargo test git_diff_file_against_workspace_compares_index_to_worktree`
- `cargo test git_clone_repository_clones_into_empty_workspace`
- `cargo test git_discard_changes_removes_untracked_file`
- `cargo test parse_git_stashes_preserves_selector_and_message`
- `cargo test git_list_and_show_stashes_returns_selected_stash_diff`
