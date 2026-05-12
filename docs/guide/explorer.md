# The File Explorer

The Explorer is a specialized tab in the Left Sidebar that provides a tactile interface for browsing the physical files created and managed by your agents.

![Explorer panel showing a selected agent workspace tree with changed documentation files](../assets/screenshots/explorer/workspace-tree.png)

## 🌳 Root Behavior

The Explorer is context-aware and automatically re-roots itself based on your selection:

### 1. Agent Selected
When you select an agent in the **Roster** (Right Sidebar), the Explorer roots itself in that agent's private workspace directory:
`<wardian-home>/agents/<session-id>/`
Use this to inspect logs, temporary files, and the agent's internal state.

### 2. No Selection (Global Mode)
When no agent is selected, the Explorer roots itself in the main Wardian home directory:
`<wardian-home>/`
This allows you to manually browse common data, shared lineages, and global configuration files.

## 🖱️ File Interactions

The Explorer supports standard right-click actions for rapid file management:

- **Open Preview**: Opens a read-only plaintext viewer within Wardian for quick inspection of markdown, JSON, or log files.
- **Reveal in System Explorer**: Opens your OS file manager (Windows Explorer or macOS Finder) directly to the selected file or folder.
- **Copy Path**: Copies the absolute path of the file to your clipboard.
- **Delete**: Permanently removes the file or directory from your disk (requires confirmation).

## Git Status Markers

When the selected root is a Git repository, the Explorer uses status colors and markers to identify changed, staged, deleted, and untracked paths. Parent folders are highlighted when they contain changed files.
