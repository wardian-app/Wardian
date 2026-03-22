# Spec 006: Explorer Left Sidebar

* **Status:** Proposed
* **Date:** 2026-03-22
* **Decider:** Architect

## Context and Problem Statement
Agents often work within specific local directories, but there is currently no way for the user to visually browse the agent's workspace files within Wardian. A file explorer is a standard expectation for developer productivity tools.

## Proposed Decision
Implement a **File Explorer** as the primary tab in the Left Sidebar.

### 1. Root Behavior
*   **Agent-Centric**: When an agent is selected, the explorer roots itself in that agent's local directory (`~/.wardian/agents/[UUID]/`).
*   **System-Fallback**: When no agent is selected, the explorer roots itself in the `.wardian/` home directory for global management.

### 2. Implementation Architecture
*   **Backend**: A Rust command `get_directory_tree(path: String)` that performs a recursive scan (with lazy-loading for large folders).
*   **Frontend**: A recursive `FileTree` component in `src/features/explorer/`.
*   **Icons**: Integration with `Lucide` icons for file-type associations.

### 3. Core Interactions (Right-Click Menu)
*   **Open Preview**: A read-only plaintext viewer for quick inspection.
*   **Reveal in OS Explorer**: Direct shortcut to the native file manager.
*   **Copy Path**: Copies the absolute path to the clipboard.
*   **Delete**: Physical file deletion (with a mandatory confirmation dialog).

### 4. Integration
*   The Explorer will be placed as the **first icon** in the `SidebarIconRail`.
*   Highlighting of modified files is deferred until **Git Worktrees** are implemented, following standard IDE patterns.

## Consequences
*   **Positive**: Provides a tactile way to inspect agent outputs and project structure.
*   **Positive**: Aligns with the "Physical-First" brand principle.
*   **Negative**: Increases backend I/O load during directory scans (mitigated by lazy-loading).
