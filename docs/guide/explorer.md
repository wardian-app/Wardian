# The File Explorer

The Explorer is a specialized tab in the Left Sidebar that provides a tactile interface for browsing the physical files created and managed by your agents.

## 🌳 Root Behavior

The Explorer is context-aware and automatically re-roots itself based on your selection:

### 1. Agent Selected
When you select an agent in the **Roster** (Right Sidebar), the Explorer roots itself in that agent's private workspace directory:
`~/.wardian/agents/[UUID]/`
Use this to inspect logs, temporary files, and the agent's internal state.

### 2. No Selection (Global Mode)
When no agent is selected, the Explorer roots itself in the main Wardian home directory:
`~/.wardian/`
This allows you to manually browse common data, shared lineages, and global configuration files.

## 🖱️ File Interactions

The Explorer supports standard right-click actions for rapid file management:

- **Open Preview**: Opens a read-only plaintext viewer within Wardian for quick inspection of markdown, JSON, or log files.
- **Reveal in System Explorer**: Opens your OS file manager (Windows Explorer or macOS Finder) directly to the selected file or folder.
- **Copy Path**: Copies the absolute path of the file to your clipboard.
- **Delete**: Permanently removes the file or directory from your disk (requires confirmation).

## 🚀 Future Feature: Git Integration
Planned updates will include visual highlighting for files modified by an agent, utilizing Git Worktrees to track changes in real-time.
