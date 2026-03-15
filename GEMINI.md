---
# Project Guidelines: Wardian

All agents contributing to the Wardian codebase must adhere to the following architectural and stylistic standards.

## ✅ Pre-Commit Checklist
Before requesting a commit or finalizing a task, ensure the following steps are completed:

1. **Code Quality (Frontend)**:
   - [ ] Run `npm run lint` to ensure style consistency.
   - [ ] Run `npm run test` (Vitest) to verify UI logic and utility functions.
   - [ ] Run `npm run build` to ensure no production build breaks.

2. **Code Quality (Backend)**:
   - [ ] Run `cd src-tauri; cargo clippy` for idiomatic Rust linting.
   - [ ] Run `cd src-tauri; cargo test` to verify backend logic and state management.
   - [ ] Run `cd src-tauri; cargo check` to verify cross-platform build integrity.

3. **Documentation**:
   - [ ] If a strategic decision was made, document it in a new **ADR** in `docs/adrs/`.
   - [ ] Update any relevant files in `docs/guide/` or `docs/developer/`.
   - [ ] Ensure any new public APIs or complex logic have appropriate JSDoc/Docstrings.

4. **Safety & Integrity**:
   - [ ] **Secrets Check**: Verify no API keys, credentials, or `.env` files are being committed.
   - [ ] **Git Status**: Run `git status` to ensure only intended files are staged.
   - [ ] **Commit Message**: Use a clear, semantic commit message (e.g., `feat(kts): implement YAML parser`).

## 🏛️ Backend Architecture (Rust/Tauri)

- **Modular Domain Design**: The backend is organized into specialized modules within `src-tauri/src/`:
    - `commands/`: Individual modules for Tauri `#[command]` handlers.
    - `models/`: Plain data structures (DTOs) and serialization logic.
    - `state/`: Application-level state management (e.g., `AppState`, `ActiveAgent`).
    - `utils/`: Global helpers for filesystem, logging, and OS interop.
- **Single Source of Truth**: The Rust backend is the definitive authority for agent session lifecycles, PTY states, and telemetry.
- **PTY Integrity**: All terminal logic must respect the `portable-pty` lifecycle, ensuring consistent behavior between ConPTY (Windows) and Unix PTY systems.
- **Thread Safety**: Use async-aware primitives (e.g., `tokio::sync::Mutex`) for state shared across Tauri commands and background metrics tasks.

## 🎨 Frontend Architecture (React/TypeScript)

- **Clean Component Hierarchy**:
    - `layout/`: Persistent structural infrastructure (e.g., Sidebar, Watchlist).
    - `views/`: Page-level layout containers and display modes (e.g., Dashboard, Grid).
    - `features/`: Domain-driven functional modules (e.g., `agents`, `terminal`, `commands`).
    - `components/`: Shared, atomic UI parts.
- **Strict Naming Conventions**:
    - **React Components**: `PascalCase.tsx`.
    - **Hooks**: `useCamelCase.ts`.
    - **Utilities/Types**: `camelCase.ts`.
    - **Folders**: `kebab-case`.
- **State Management**: The root `App.tsx` acts as the primary orchestrator for global state, delegating specific logic to feature-based components.

## 💄 UI & UX Standards

- **Standardized Sidebars**:
  - **Left (Control)**: Persistent icon-based navigation rail with collapsible content panes.
  - **Right (Roster)**: Collapsible, searchable agent list for rapid status monitoring.
- **Status Indicators**:
  - **Idle**: Emerald (`#10b981`)
  - **Processing**: Cyan (`#22d3ee`)
  - **Action Required**: Amber (`#f59e0b`)
  - **Off**: Gray (`#4b5563`)
  - **Critical/Error**: Red (`#EF4444`)
- **Semantic Theming**: ALWAYS use theme variables (e.g., `var(--color-wardian-text-muted)`) or themed classes (`.text-muted`) instead of hardcoded Tailwind colors.

## 🛠️ Workflow Rules

- **Surgical Code Changes**: Use the `replace` tool for precise, context-aware edits. Avoid overwriting entire files unless scaffolding new modules.
- **Verification-First**: Always run `npm run build` and `cd src-tauri; cargo check` before concluding a task to ensure architectural and path integrity.
- **TypeScript Sovereignty**: Strictly adhere to the types defined in `src/types/index.ts`. Never use `any` unless required by external library constraints.
