# Project Guidelines: Wardian

All agents contributing to the Wardian codebase must adhere to the following architectural and stylistic standards.

## 🎭 Brand Personality & Guiding Principles
These clusters guide both the architectural integrity and the user experience of Wardian.

- **Tactile**: Physical-first organization. Drag-and-drop grids, local filesystem junctions for skills, and visible telemetry.
- **Ecological / Transparent**: A living "Habitat" where agents evolve. "Markdown-as-Truth" ensuring the system's state is always inspectable on disk.
- **High-Tech / Omniscient**: High-performance orchestration powered by Rust. A "Command Center" view of multiple agent minds in real-time.

## ✅ Pre-Commit Checklist
Before requesting a commit or finalizing a task, ensure the following steps are completed:

1. **Validation & Build**:
   - [ ] **Frontend**: Run `npm run lint`, `npm run test`, and `npm run build`.
   - [ ] **Backend**: Run `cargo clippy`, `cargo test`, and `cargo check` (in `src-tauri`).
2. **Documentation**:
   - [ ] Document strategic decisions in a new **Spec** in `docs/specs/`.
   - [ ] Update related guides in `docs/guide/` or `docs/developer/`.
   - [ ] Ensure public APIs/complex logic have appropriate JSDoc or Rust docstrings.
3. **Safety & Integrity**:
   - [ ] **Secrets Check**: Verify no API keys, credentials, or `.env` files are being committed.
   - [ ] **Git Status**: Run `git status` to ensure only intended files are staged.
   - [ ] **Commit Message**: Use a clear, semantic commit message (e.g., `feat(workflows): implement parallel execution`).

## 🏛️ Architecture & Naming Standards

### 1. Naming Conventions (Cross-Cutting)
- **Folders**: `kebab-case` for Frontend/Docs, `snake_case` for Backend modules (required for Rust module identifiers).
- **Documents**: `kebab-case.md` (e.g., `workflow-engine.md`).
- **IPC/Data Models**: `snake_case` for properties in both Rust and TypeScript to ensure seamless DTO serialization.

### 2. Backend Architecture (Rust/Tauri)
- **Modular Domain Design**: The backend is organized into specialized modules within `src-tauri/src/`:
    - `commands/`: Individual modules for Tauri `#[command]` handlers.
    - `models/`: Plain data structures (DTOs) and serialization logic.
    - `state/`: Application-level state management (e.g., `AppState`, `ActiveAgent`).
    - `utils/`: Global helpers for filesystem, logging, and OS interop.
    - `workflow_engine/`: Deterministic execution logic for multi-agent workflows.
- **Single Source of Truth**: The Rust backend is the definitive authority for agent session lifecycles, PTY states, and telemetry.
- **PTY Integrity**: All terminal logic must respect the `portable-pty` lifecycle, ensuring consistent behavior between ConPTY (Windows) and Unix PTY systems.
- **Thread Safety**: Use async-aware primitives (e.g., `tokio::sync::Mutex`) for state shared across Tauri commands.

### 3. Frontend Architecture (React/TypeScript)
- **Clean Component Hierarchy**:
    - `layout/`: Persistent structural infrastructure (e.g., Sidebar, Watchlist).
    - `views/`: Page-level layout containers and display modes (e.g., Dashboard, Grid).
    - `features/`: Domain-driven functional modules (e.g., `agents`, `terminal`, `commands`).
    - `components/`: Shared, atomic UI parts.
- **Naming Standards**:
    - **React Components**: `PascalCase.tsx` (e.g., `WorkflowBuilderView.tsx`).
    - **Hooks**: `useCamelCase.ts` (e.g., `useWorkflowStore.ts`).
    - **Utilities/Types**: `camelCase.ts` (e.g., `statusUtils.ts`).
- **State Management**: The root `App.tsx` acts as the primary orchestrator for global state, delegating specific logic to feature-based stores (e.g., Zustand).

## 💄 UI & UX Standards
- **Standardized Sidebars**:
  - **Left (Control)**: Persistent icon-based navigation rail with collapsible content panes.
  - **Right (Roster)**: Collapsible, searchable agent list for rapid status monitoring.
- **Status Indicators**: Emerald (Idle), Cyan (Processing), Amber (Action Required), Gray (Off), Red (Error).
- **Semantic Theming**: ALWAYS use theme variables (e.g., `var(--color-wardian-text-muted)`) or themed classes (`.text-muted`) instead of hardcoded Tailwind colors.

## 🛠️ Workflow Rules
- **Surgical Code Changes**: Use the `replace` tool for precise, context-aware edits. Avoid overwriting entire files unless scaffolding new modules.
- **Verification-First**: A task is only complete once the behavioral correctness has been verified via the pre-commit checklist.
- **TypeScript Sovereignty**: Strictly adhere to the types defined in `src/types/index.ts`. Never use `any` unless required by external library constraints.

## 🌿 Git & Pull Request Standards
All agents must follow these standards to ensure a clean, high-governance repository state:

- **Branching**: Never work directly on `main`. Create descriptive feature branches (e.g., `feat/junction-refactor` or `fix/telemetry-bug`).
- **Atomic Commits**: Group related changes into small, semantic commits. Use [Conventional Commits](https://www.conventionalcommits.org/) (e.g., `feat:`, `fix:`, `docs:`, `chore:`).
- **Issue Linking**: Every PR must link to an existing GitHub issue. If no issue exists, create one before starting the implementation.
- **PR Descriptions**: Always use the provided PR template. Explain the "Why" behind the change and include evidence of successful verification (logs or test results).
- **CI Readiness**: Before opening a PR, run the full verification suite (`npm run lint/test` and `cargo clippy/test`) to ensure green status on GitHub Actions.
