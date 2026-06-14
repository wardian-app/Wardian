# Explorer Filesystem Watch Refresh

* **Status:** Implemented
* **Date:** 2026-06-02

## Context

The Explorer sidebar lazy-loads directory contents with `get_directory_tree`.
Mounted directory components only refetch when their `path` changes, so files
created, deleted, renamed, or modified by agents, terminals, editors, and other
external processes can leave the visible tree stale.

Polling would work, but developer tools generally model file updates as
filesystem events. VS Code uses recursive and non-recursive watchers with
deduplication and excludes, falling back to polling only in limited missing-path
cases. Obsidian exposes vault create, modify, delete, and rename events to
plugins.

## Decision

Explorer will use a backend Tauri filesystem watcher for the active explorer
root. The watcher emits debounced `explorer-changed` events with the root and
changed paths. The frontend subscribes while the Explorer panel is mounted and
refreshes only mounted lazy tree branches affected by the event.

The watcher excludes high-churn implementation folders such as `.git`,
`node_modules`, `target`, `.venv`, `dist`, `build`, `.next`, `.turbo`,
`.cache`, and `.wardian/tmp`.

The frontend normalizes Windows-shaped watcher paths before comparing them with
rendered tree paths. This includes stripping Windows verbatim prefixes such as
`\\?\<absolute-windows-path>` and `\\?\UNC\<server>\<share>\...`, because
platform watcher backends can report canonical paths while `get_directory_tree`
returns display paths. Non-Windows path spelling, case, and significant
whitespace are preserved for comparisons.

## Consequences

* Visible explorer directories update without manual tab switching or polling.
* Expanded folders and scroll position are preserved because the root tree is
  not remounted for ordinary refreshes.
* Idle CPU cost is lower than polling, with a small watcher handle and memory
  cost while Explorer is mounted.
* Large generated-file bursts are debounced and partially filtered, but watcher
  behavior can still vary across platforms and network filesystems.
