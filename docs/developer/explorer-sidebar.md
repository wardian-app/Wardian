# Explorer Sidebar - Developer Guide

## Overview
The Explorer Sidebar is a dedicated panel found in the Wardian sidebar (`SidebarIconRail`), designed to give users direct access to their local workspace. Depending on whether an agent is actively selected, it contextualizes its root directory seamlessly:
1. **Global View**: Shows the configured `<WARDIAN_HOME>/` directory.
2. **Agent View**: Shows the selected agent workspace or assigned Git worktree.

## Key Components

### 1. `ExplorerPanel.tsx`
This is the main container component for the file explorer tab.
- **Root Resolution**: It queries the backend command `get_explorer_root(sessionId)` to identify which path to render.
- **File Opening**: It receives file selections from `FileTree` and routes
  supported text/code, image, and PDF files through the shared
  `fileOpenDestinationForResource` helper and the Settings-backed
  `file_open_actions` preferences. Internal opening sends a resource-keyed
  `files` request through the AppShell-owned
  `WorkbenchNavigationService`; external opening reuses
  `open_in_external_editor`. Unknown and unsupported files force the system
  destination.
- **Filesystem Watch Refresh**: While mounted, the panel subscribes to `explorer-changed`, starts `explorer_watch` for the current root after the listener is ready, and calls `explorer_unwatch` on cleanup. Matching events increment a refresh token and carry changed paths down to `FileTree`.
- **Root Actions**: The Explorer title header can reveal the current Explorer root through `reveal_in_explorer` or open the entire root through the Settings-backed `open_in_external_editor` path.
- **Context Menu Context**: Provides right-click operations tailored to
  `FileTree` items (Open, Open to Side, Open in External App, Reveal in OS,
  Copy Absolute Path, Delete).
- **Navigation Errors**: Contains missing-navigation, synchronous, and rejected
  navigation failures in a themed Explorer-local alert. Explorer never imports
  a Workbench store or creates a second navigation singleton.

### 2. `FileTree.tsx`
A recursive, lazy-loading component responsible for accurately representing nested directory structures.
- **Lazy Loading**: Instead of indexing the entire workspace at once, it fetches child nodes only when a directory is expanded, ensuring optimal performance for large projects.
- **Targeted Refresh**: Each mounted tree refetches its directory when the refresh token changes and one of the changed paths directly affects that directory. Expanded state stays local to the component, so refreshes do not collapse the visible tree.
- **Path Identity**: Explorer path comparisons use `normalizeExplorerPathForCompare` so Windows-specific watcher paths such as `\\?\<absolute-windows-path>` match ordinary display paths from directory reads without rewriting POSIX path spelling, case, or significant whitespace.
- **Open Coordination**: One root-owned interaction controller delays a file
  selection until it can route the path through the shared
  `fileOpenDestinationForResource` helper. Wardian-preferred supported files use
  `openPermanentFileSurface` for a permanent Files surface; external-preferred
  supported files use `open_in_external_editor`; unknown and unsupported files
  always use the system destination. Double-click and `Enter` follow the same
  permanent or external/system route rather than pinning a transient preview.
- **Theming**: Integrates seamlessly with Wardian typography and spacing. Nested items have fixed padding metrics to align correctly underneath parent elements without succumbing to horizontal flex contraction (`shrink-0`). Directory rows use only their expansion chevron; file rows use `lucide-react` icons with colors mapped explicitly to `wardian-*` CSS variables based on file extensions.

### 3. Backend Commands (`src-tauri/src/commands/fs.rs`)
The file system operations strictly enforce security and platform agnosticism:
- `get_explorer_root`: Safely queries `AppState` to determine the correct target directory.
- `get_directory_tree`: Non-recursive listing of immediate children of a given path. Sorts directories first, then alphabetical.
- `open_file_resource` and related Files commands live in
  `src-tauri/src/commands/files.rs`; Explorer does not read preview bytes
  directly. Files Markdown links canonicalize their targets through this
  command before the shared opening router launches an external or system
  destination; inherited agent roots and exact user-file capabilities remain
  enforced when the source resource provides them. The shared router uses the
  returned verified renderer family, so signatures take precedence over a
  misleading filename extension before a family preference is applied.
- `reveal_in_explorer`: OS-specific `std::process::Command` routing to invoke `explorer`, `open`, or `xdg-open`.
- `open_in_external_editor`: Opens folders and editor-friendly files with the Settings-selected external app mode (`system`, `vscode`, or `custom`) by spawning the platform command in Rust. The shared file-opening router explicitly passes `system` for unknown or unsupported content, so VS Code/custom editors are not used as document viewers.
- `delete_file`: Recursively deletes a directory or permanently removes a file string.
- `explorer_watch` / `explorer_unwatch`: Manage debounced recursive filesystem watchers for active explorer roots. Watchers are reference-counted by root and exclude high-churn folders such as `.git`, `node_modules`, `target`, `.venv`, `dist`, `build`, `.next`, `.turbo`, `.cache`, and `.wardian/tmp`.

## Technical Decisions
- **`Option<String>` vs Strict Strings**: Using `null` / `Option` for Session IDs enables elegant toggling between global and localized modes without parallel commands.
- **Scroll Handling**: Native scrollbars (`overflow-auto`) are preserved to prevent users from losing their place in deeply nested directory trees, resolving initial constraints that collapsed items dynamically.
- **Authorization is not tree visibility**: Explorer may display filesystem
  entries, but a Files open is independently authorized by Rust. Current agent
  primary workspaces and `include_directories` are user content grants;
  `system_include_directories` are excluded. Symlinks and junctions are
  canonicalized and cannot escape an authorized root.
- **Exact file grants**: the backend keeps a bounded durable list of canonical
  file paths granted individually. Siblings inherit nothing. Capability
  identifiers and retained handles remain live-only. Workbench restore submits
  only the file path, and the backend resolves it against current agent roots
  or an exact remembered grant; no capability token is serialized.
- **Launcher boundary**: The `files` surface is registered so Explorer and
  restored tabs can render it, but its New Surface contribution remains
  reserved. Do not activate that launcher until artifact review and isolated
  live HTML/SVG are implemented.
