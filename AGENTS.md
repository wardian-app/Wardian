# Project Guidelines: Wardian

All agents contributing to the Wardian codebase must adhere to the following architectural and stylistic standards.

## 🎭 Brand Personality & Guiding Principles
These clusters guide both the architectural integrity and the user experience of Wardian.

- **Tactile**: Physical-first organization. Drag-and-drop grids, local filesystem junctions for skills, and visible telemetry.
- **Ecological / Transparent**: A living "Habitat" where agents evolve. "Markdown-as-Truth" ensuring the system's state is always inspectable on disk.
- **High-Tech / Situational**: High-performance orchestration powered by Rust. A live, inspectable view of multiple agent sessions, surfaces, and signals.

## ✅ Pre-Commit Checklist
Before requesting a commit or finalizing a task, ensure the following steps are completed:

1. **Validation & Build**:
   - [ ] **Frontend**: Run `npm run lint`, `npm run test`, and `npm run build`.
   - [ ] **Backend**: Run `cargo clippy`, `cargo test`, and `cargo check` (in `src-tauri`).
2. **Documentation**:
   - [ ] Document strategic decisions in a new **Spec** in `docs/specs/`.
   - [ ] Update related guides in `docs/guide/` or `docs/developer/`.
   - [ ] Ensure public APIs/complex logic have appropriate JSDoc or Rust docstrings.
   - [ ] **Frontend/UI changes**: If frontend behavior or visual behavior changed, capture feature-specific screenshots that demonstrate the changed interaction/state and embed at least one representative image directly in the PR description using markdown image syntax or an `<img>` tag. A local path alone does not satisfy the requirement.
3. **Safety & Integrity**:
   - [ ] **Secrets Check**: Verify no API keys, credentials, or `.env` files are being committed.
   - [ ] **Git Status**: Run `git status` to ensure only intended files are staged.
   - [ ] **Commit Message**: Use a clear, semantic commit message (e.g., `feat(workflows): implement parallel execution`).

### PowerShell Home Safety
- PowerShell variable names are case-insensitive: `$home` is the built-in `$HOME` variable and points to the OS user profile. Never assign to or reuse `$home` as a scratch, test, temporary, or Wardian-home variable. Use a descriptive name such as `$testHome`, `$wardianHome`, or `$tempRoot`.
- Never pass `$HOME`, `$home`, `$env:USERPROFILE`, `~`, or a path derived only from one of them to a recursive delete or move. Commands such as `Remove-Item $home -Recurse -Force` are forbidden under all circumstances.
- Before any recursive delete or move, resolve the intended target to an absolute path, verify that it is inside the workspace or an explicitly created temporary directory, and abort if assignment or path resolution failed. Filesystem mutation scripts must fail closed instead of continuing after a setup error.

### Cross-Platform Documentation
- User-facing docs, bundled skills, examples, and agent instructions must be cross-OS and cross-computer by default.
- Use placeholders such as `<absolute-workspace-path>` instead of local machine paths, drive-letter paths, or user-home paths.
- When commands differ by shell, show a POSIX `bash`/`sh` form first and a labeled PowerShell form second. Do not make PowerShell-only syntax the default unless the section is explicitly Windows-only.
- Keep Windows-specific examples only when documenting Windows behavior, and label them as Windows-specific.

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

## 🧪 Automated Testing & Verification

Wardian has multiple test layers. Before marking a task as complete, run the appropriate ones:

1. **Frontend Unit Tests**: `npm run test`
   - Run after any TypeScript/React changes.
   - Coverage report: `npm run test:coverage` → `coverage/lcov.info`
2. **Backend Unit Tests**: `cd src-tauri && cargo test`
   - Run after any Rust changes. Use `--test-threads=1` if tests involve env vars.
   - Coverage report: `npm run test:coverage:rust` → `coverage/rust-lcov.info` (requires `cargo install cargo-llvm-cov`)
3. **Browser E2E Smoke Tests**: `npm run test:e2e`
   - Run after UI or orchestration changes.
   - Uses an isolated `WARDIAN_HOME` (temp directory) with seeded fixtures.
   - **Scope**: browser-level UI behavior and mock-provider agent lifecycle flows only.
   - **Cannot prove**: real PTY behavior, native Tauri IPC, filesystem operations (junctions, workspace init), or provider-specific behavior. Use a higher layer for those.
4. **Native Runtime E2E**: use the Tauri/WebDriver-native harness when validating PTY behavior, `invoke` commands, or provider spawning.
   - This is the required layer for real terminal and provider-runtime claims.
   - Setup: `npm run setup:e2e:native`
   - Run: `npm run test:e2e:native`
   - CLI shared-state smoke: `npm run test:e2e:native:fast -- e2e-native/tests/cli-shared-state-native.test.mjs`
   - User terminal smoke: `npm run test:e2e:native:fast -- e2e-native/tests/user-terminal-native.test.mjs`
   - Generated native driver artifacts live under `tools/e2e-native/` and are intentionally ignored by git.
5. **Real Provider E2E**: run only when a change specifically depends on provider-specific native behavior and the native runtime harness is available.
   - Keep these runs isolated and opt-in.
   - Example (PowerShell): ``$env:WARDIAN_E2E_REAL_OPENCODE='1'; $env:WARDIAN_E2E_REAL_WORKSPACE='<absolute path to your Wardian checkout>'; npm run test:e2e:native``

### E2E Layer Boundary Rules

When writing a test, pick the **lowest** layer that can prove the behavior:

| What you're testing | Layer to use |
|---|---|
| UI rendering, navigation, form inputs | Browser E2E |
| Agent lifecycle with mock provider (spawn/status/kill) | Browser E2E |
| PTY resize, `invoke` IPC commands | Native E2E |
| App-created agent readable through CLI state | Native E2E |
| Standalone user terminal PTY behavior | Native E2E |
| Real filesystem ops (junctions, workspace init) | Native E2E |
| Provider-specific spawn or token behavior | Real Provider E2E |

If a browser E2E test **requires** a higher layer to be meaningful, wrap it in `test.skip(...)` with a `// @native-only` or `// @real-provider-only` comment. This makes the gap explicit and machine-readable rather than silently absent.

### Screenshot Documentation

Screenshots are feature-specific PR evidence rather than generic CI artifacts. For frontend changes, use Playwright or the running app to capture only the interaction/state changed by the PR, write images under `e2e/screenshots/<feature>/<timestamp>/`, upload or attach at least one representative image, and embed it in the PR description. Do not add empty-window or app-tour screenshots that do not explain the change.

The PR workflow runs `npm run check:frontend-screenshot` and fails frontend PRs without an embedded HTTPS image in the PR body. A local-only path such as `e2e/screenshots/<feature>/<timestamp>/<name>.png` is not enough. Use the GitHub web attachment flow or another approved GitHub-hosted image URL, then embed it with markdown:

```markdown
![feature-state](https://github.com/<owner>/<repo>/.../<screenshot>.png)
```

### Mock Provider

The `mock` provider (`scripts/mock-agent.cjs`) simulates deterministic agent behavior for offline testing. Configure via environment variables:
- `WARDIAN_MOCK_SCENARIO`: `basic`, `resume`, `action_needed`, `failure`, `long_output`, `headless`, `multi_turn`
- `WARDIAN_MOCK_DELAY_MS`: Delay between events (default `100`)

Use `e2e/fixtures/mockAgent.ts` to set up an isolated `WARDIAN_HOME` and seed a mock agent before assertions in browser E2E specs.

### Isolated Test Home

Set `WARDIAN_HOME` to redirect all state to an isolated directory:
```bash
WARDIAN_HOME="$(mktemp -d)" npm run tauri dev
```
This prevents test runs from interfering with production `~/.wardian` state.

When testing the dev app and CLI together, set the same explicit `WARDIAN_HOME` in both terminals. The CLI defaults to production `~/.wardian`, while the dev app may use debug state unless this is set.

macOS/Linux shell:
```bash
export WARDIAN_HOME="$PWD/.tmp/wardian-cli-dev"
npm run dev
```

Second terminal:
```bash
export WARDIAN_HOME="$PWD/.tmp/wardian-cli-dev"
cargo run -p wardian-cli -- agent list --scope all
```

PowerShell:
```powershell
$env:WARDIAN_HOME = "$PWD\.tmp\wardian-cli-dev"
npm run dev
```

Second terminal:
```powershell
$env:WARDIAN_HOME = "$PWD\.tmp\wardian-cli-dev"
cargo run -p wardian-cli -- agent list --scope all
```

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
