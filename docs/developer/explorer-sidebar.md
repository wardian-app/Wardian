# Explorer Sidebar - Developer Guide

## Overview
The Explorer Sidebar is a dedicated panel found in the Wardian sidebar (`SidebarIconRail`), designed to give users direct access to their local workspace. Depending on whether an agent is actively selected, it contextualizes its root directory seamlessly:
1. **Global View**: Shows the configured `<WARDIAN_HOME>/` directory.
2. **Agent View**: Shows the selected agent workspace or assigned Git worktree.

## Key Components

### 1. `ExplorerPanel.tsx`
This is the main container component for the file explorer tab.
- **Root Resolution**: It queries the backend command `get_explorer_root(sessionId)` to identify which path to render.
- **File Click Action**: It receives file selections from `FileTree` and routes them through the Settings-backed `explorerFileClickAction` preference. Preview mode uses `read_file_preview`; external mode reuses `open_in_external_editor`.
- **Filesystem Watch Refresh**: While mounted, the panel subscribes to `explorer-changed`, starts `explorer_watch` for the current root after the listener is ready, and calls `explorer_unwatch` on cleanup. Matching events increment a refresh token and carry changed paths down to `FileTree`.
- **Root Actions**: The Explorer title header can reveal the current Explorer root through `reveal_in_explorer` or open the entire root through the Settings-backed `open_in_external_editor` path.
- **Context Menu Context**: Provides right-click operations tailored to `FileTree` items (Open Preview, Open in External App, Reveal in OS, Copy Absolute Path, Delete).
- **Preview Modal**: Implements a themed modal overlay to securely display raw text file contents queried from the OS.

### 2. `FileTree.tsx`
A recursive, lazy-loading component responsible for accurately representing nested directory structures.
- **Lazy Loading**: Instead of indexing the entire workspace at once, it fetches child nodes only when a directory is expanded, ensuring optimal performance for large projects.
- **Targeted Refresh**: Each mounted tree refetches its directory when the refresh token changes and one of the changed paths directly affects that directory. Expanded state stays local to the component, so refreshes do not collapse the visible tree.
- **Path Identity**: Explorer path comparisons use `normalizeExplorerPathForCompare` so Windows-specific watcher paths such as `\\?\<absolute-windows-path>` match ordinary display paths from directory reads without rewriting POSIX path spelling, case, or significant whitespace.
- **Theming**: Integrates seamlessly with Wardian typography and spacing. Nested items have fixed padding metrics to align correctly underneath parent elements without succumbing to horizontal flex contraction (`shrink-0`). Directory rows use only their expansion chevron; file rows use `lucide-react` icons with colors mapped explicitly to `wardian-*` CSS variables based on file extensions.

### 3. Backend Commands (`src-tauri/src/commands/fs.rs`)
The file system operations strictly enforce security and platform agnosticism:
- `get_explorer_root`: Safely queries `AppState` to determine the correct target directory.
- `get_directory_tree`: Non-recursive listing of immediate children of a given path. Sorts directories first, then alphabetical.
- `read_file_preview`: Simple text reader.
- `reveal_in_explorer`: OS-specific `std::process::Command` routing to invoke `explorer`, `open`, or `xdg-open`.
- `open_in_external_editor`: Opens folders and editor-friendly files with the Settings-selected external app mode (`system`, `vscode`, or `custom`) by spawning the platform command in Rust. Known binary, media, archive, executable, and document files fall back to the system default handler so VS Code/custom editors are not used for non-editor file types.
- `delete_file`: Recursively deletes a directory or permanently removes a file string.
- `explorer_watch` / `explorer_unwatch`: Manage debounced recursive filesystem watchers for active explorer roots. Watchers are reference-counted by root and exclude high-churn folders such as `.git`, `node_modules`, `target`, `.venv`, `dist`, `build`, `.next`, `.turbo`, `.cache`, and `.wardian/tmp`.

## Technical Decisions
- **`Option<String>` vs Strict Strings**: Using `null` / `Option` for Session IDs enables elegant toggling between global and localized modes without parallel commands.
- **Scroll Handling**: Native scrollbars (`overflow-auto`) are preserved to prevent users from losing their place in deeply nested directory trees, resolving initial constraints that collapsed items dynamically.
