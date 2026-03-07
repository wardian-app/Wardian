# Project Guidelines: Wardian

## Integrated Agent Environment

All agents contributing to the Wardian codebase must adhere to the following architectural and stylistic standards.

### 🏛️ Structural Principles

- **Backend-Driven State**: The Rust backend (`src-tauri/src/manager.rs`) is the single source of truth for agent sessions, PTY states, and telemetry.
- **PTY Integrity**: Ensure that any changes to input/output handling respect the `portable-pty` lifecycle and ConPTY (Windows) vs. Unix PTY differences.
- **Manual Resumption**: All features must maintain the user's ability to resume sessions externally via `gemini --resume [session_id]`.

### 🎨 UI & UX Standards

- **Standardized Sidebars**:
  - **Left**: Persistent icon-based navigation bar with collapsible content panes.
  - **Right**: Collapsible, searchable agent list for rapid status monitoring and selection.
- **Status Indicators**:
  - **Idle**: Emerald (`#10b981`)
  - **Processing**: Cyan (`#22d3ee`)
  - **Action Required**: Amber (`#f59e0b`)
  - **Off**: Gray (`#4b5563`)
  - **Critical/Error**: Red (`#EF4444`)

### 🛠️ Workflow Rules

- **Surgical Code Changes**: Use the `replace` tool for precise, context-aware edits. Avoid overwriting entire files unless scaffolding new modules.
- **Build Verification**: Run the project's build and lint commands to verify changes before concluding a task.
- **Precise Logic**: Strictly adhere to the established TypeScript types (`src/types.ts`) and Rust structs (`manager.rs`).
