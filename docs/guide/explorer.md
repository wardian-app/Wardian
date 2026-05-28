# The File Explorer

The Explorer is a specialized tab in the Left Sidebar that provides a tactile interface for browsing the physical files created and managed by your agents.

Use it when you need to inspect generated files, logs, prompt assets, or the selected agent's workspace without leaving Wardian.

![Explorer panel showing a selected agent workspace tree with changed documentation files](../assets/screenshots/explorer/workspace-tree.png)

## When to Use It

- Browse the workspace for the agent selected in [Watchlists](./watchlists.md).
- Inspect files after an agent reports completion in [Queue](./queue.md).
- Open a quick preview before deciding whether to edit files in an external tool.
- Open a file or folder in your configured local app or editor.
- Reveal a file in the system file manager when you need native OS actions.

## Basic Workflow

1. Select an agent in the right roster, or clear selection for global Wardian home browsing.
2. Open the **Explorer** tab in the left sidebar.
3. Expand folders to inspect files.
4. Use preview, open externally, reveal, copy path, or delete from the file context menu.
5. Move to [Source Control](./source-control.md) when the selected root is a Git workspace and you need to review changes.

## Root Behavior

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
- **Open in External App**: Opens the selected file or folder using the configured Explorer editor preference. The default is the operating system's default app for that path. You can switch to VS Code or a custom executable in [Settings](./settings.md).
- **Reveal in System Explorer**: Opens your OS file manager (Windows Explorer or macOS Finder) directly to the selected file or folder.
- **Copy Path**: Copies the absolute path of the file to your clipboard.
- **Delete**: Permanently removes the file or directory from your disk (requires confirmation).

## Git Status Markers

When the selected root is a Git repository, the Explorer uses status colors and markers to identify changed, staged, deleted, and untracked paths. Parent folders are highlighted when they contain changed files.

## Important Limits

- Delete removes files from disk after confirmation; it is not a soft-hide operation.
- Explorer context follows selection. If the tree is not showing the workspace you expect, check the selected agent in the roster.
- Previews are for quick inspection. Use your editor or Source Control for deeper code review.

## Related Links

- [Getting Started](./getting-started.md)
- [Watchlists](./watchlists.md)
- [Source Control](./source-control.md)
- [Queue](./queue.md)
