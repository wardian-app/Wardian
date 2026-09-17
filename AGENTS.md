# Project Guidelines: Wardian

Rules that must hold before you have read anything else. Runbooks, commands, and
architecture detail live in [`docs/developer/`](docs/developer/index.md) and are
linked from here — do not copy them back into this file.

## 🎭 Brand Personality & Guiding Principles

- **Tactile**: Physical-first organization. Drag-and-drop grids, local filesystem junctions for skills, visible telemetry.
- **Ecological / Transparent**: A living "Habitat" where agents evolve. "Markdown-as-Truth" — the system's state is always inspectable on disk.
- **High-Tech / Situational**: High-performance orchestration powered by Rust. A live, inspectable view of multiple agent sessions, surfaces, and signals.

## ✅ Pre-Commit Checklist

1. **Validate**: `npm run verify:ci` (narrow with `--only <frontend|backend|docs>`). It reads the authoritative steps from `.github/workflows/ci.yml`.
2. **Document**: a new spec in `docs/specs/` for strategic decisions; update the affected guide in `docs/guide/` or `docs/developer/`, checking [`docs-maintenance.md`](docs/developer/docs-maintenance.md) for release notes, public links, and screenshot refresh; JSDoc or Rust docstrings on public APIs and complex logic.
3. **Check safety**: no API keys, credentials, or `.env` files staged; `git status` shows only intended files; semantic commit message.

### PowerShell Home Safety

- `$home` **is** the built-in `$HOME` (PowerShell variable names are case-insensitive) and points at the OS user profile. Never assign to it. Use `$testHome`, `$wardianHome`, or `$tempRoot`.
- Never pass `$HOME`, `$env:USERPROFILE`, `~`, or a path derived only from one of them to a recursive delete or move. `Remove-Item $home -Recurse -Force` is forbidden under all circumstances.
- Before any recursive delete or move, resolve the target to an absolute path and verify it is inside the workspace or an explicitly created temporary directory. Filesystem mutation scripts fail closed.

### Cross-Platform Documentation

- Docs, bundled skills, examples, and agent instructions are cross-OS and cross-computer by default. Use placeholders such as `<absolute-workspace-path>`, never local machine, drive-letter, or user-home paths.
- Show a POSIX `bash`/`sh` form first and a labeled PowerShell form second. Label Windows-specific examples as such.

## 🏛️ Architecture & Naming

Detail: [`docs/developer/architecture.md`](docs/developer/architecture.md), [`state-management.md`](docs/developer/state-management.md).

- **Folders**: `kebab-case` for frontend and docs, `snake_case` for Rust backend modules.
- **Documents**: `kebab-case.md`. **React components**: `PascalCase.tsx`. **Hooks**: `useCamelCase.ts`. **Utilities and types**: `camelCase.ts`.
- **IPC/data models**: `snake_case` properties in both Rust and TypeScript, so DTOs serialize without translation. Contracts: [`ipc-events.md`](docs/developer/ipc-events.md), [`tauri-command-reference.md`](docs/developer/tauri-command-reference.md).

**Backend** (`src-tauri/src/`): `commands/` Tauri handlers, `models/` DTOs, `state/` app state, `utils/` FS and OS helpers, `automation_engine/` deterministic execution. The Rust backend is the single source of truth for agent session lifecycles, PTY state, and telemetry. Respect the [`portable-pty` lifecycle](docs/developer/pty-lifecycle.md) so ConPTY and Unix PTY behave alike. Use async-aware primitives (`tokio::sync::Mutex`) for state shared across commands, and never hold a global lock while acquiring a per-item one.

**Frontend** (`src/`): `layout/` persistent structure, `views/` page-level containers, `features/` domain modules, `components/` shared atoms. `App.tsx` orchestrates global state and delegates to feature stores.

## 💄 UI & UX Standards

- **Left sidebar (Control)**: persistent icon rail with collapsible panes. **Right sidebar (Roster)**: collapsible, searchable agent list.
- **Status colors**: Emerald (Idle), Cyan (Processing), Amber (Action Required), Gray (Off), Red (Error).
- **Semantic theming**: always use theme variables (`var(--color-wardian-text-muted)`) or themed classes (`.text-muted`), never hardcoded Tailwind colors. Token reference: [`theming.md`](docs/developer/theming.md).

## 🧪 Testing

Commands and setup: [`docs/developer/native-e2e.md`](docs/developer/native-e2e.md), [`ci-verification.md`](docs/developer/ci-verification.md), [`setup.md`](docs/developer/setup.md) (isolated `WARDIAN_HOME`, mock provider).

Write a test at the **lowest** layer that can prove the behavior:

| Layer | Command | Proves | Cannot prove |
| --- | --- | --- | --- |
| Frontend unit | `npm run test` | TypeScript/React logic | anything native |
| Backend unit | `cd src-tauri && cargo test` | Rust logic | IPC, real PTY |
| Browser E2E | `npm run test:e2e` | UI rendering, navigation, forms, mock-provider agent lifecycle | real PTY, Tauri IPC, filesystem ops, provider behavior |
| Native E2E | `npm run test:e2e:native` | PTY resize, `invoke` IPC, junctions and workspace init, CLI/app shared state | provider-specific behavior |
| Real provider E2E | opt-in env vars | provider spawn and token behavior | — |

If a browser E2E test needs a higher layer to be meaningful, wrap it in `test.skip(...)` with a `// @native-only` or `// @real-provider-only` comment so the gap is explicit and machine-readable rather than silently absent.

**Screenshots** are feature-specific PR evidence, not CI artifacts: capture only the interaction or state your PR changed, write it under `e2e/screenshots/<feature>/<timestamp>/`, and embed it in the PR body as an HTTPS image. `npm run check:frontend-screenshot` fails frontend PRs without one; a local path is not enough. Upload with the `gh attach` extension against the linked issue — see [`docs/developer/screenshot-documentation.md`](docs/developer/screenshot-documentation.md#pr-evidence-upload-cli).

## 🛠️ Workflow Rules

- **Surgical changes**: precise, context-aware edits. Do not overwrite whole files except when scaffolding new modules.
- **Resolve failing tests**: a failing test is a decision about which side is wrong, and it is yours. Fix the behavior when the test is right; fix the test when your change intentionally supersedes what it encoded. Never edit a test merely to turn red green. If the failure predates your change, show it reproduces on the base commit before setting it aside. Report the outcome either way.
- **Review has a termination criterion**: every finding is blocking or non-blocking, and deciding which is yours. Only a blocking finding justifies another round; a non-blocking one goes in a linked issue. Review ends at zero blocking findings, not zero possible improvements — "are there more gaps" always answers yes, so it cannot be the stopping condition. Use Wardian's `autoreview` workflow when it is available; otherwise route to a reviewer agent for the verdict instead of re-auditing your own diff.
- **TypeScript sovereignty**: adhere to `src/types/index.ts`. Never use `any` unless an external library forces it.

## 🌿 Git & Pull Request Standards

- **Default delivery (Wardian only)**: implementation tasks in this repository include committing, pushing, and opening or updating an issue-linked PR after verification and zero-blocker local-agent review. This is standing authorization for those steps unless the user explicitly requests local-only work. Never merge or deploy without separate authorization. Procedure: [Pull Request Delivery](docs/developer/pull-requests.md).
- **Local review, not GitHub reviewer requests**: zero-blocker review means review by local agents, obtained through the `autoreview` workflow when available and otherwise routed to a reviewer agent. Never request reviewers on GitHub. GitHub's review status is not the local review verdict.
- **Branching**: never work directly on `main`. Use descriptive branches (`feat/junction-refactor`, `fix/telemetry-bug`).
- **Atomic commits**: small and semantic, using [Conventional Commits](https://www.conventionalcommits.org/).
- **One PR, one issue**: every PR links an existing issue and carries only that issue's work. Open a separate branch for adjacent work rather than bundling it — a reviewer can only accept or reject the whole thing.
- **PR descriptions**: use the template. Explain the "Why" and include verification evidence.
- **CI readiness**: run the full suite before opening. A PR is "ready" only when four separately checkable facts hold, not when a status update says so: every required check is green, merge conflicts are resolved, a local-agent review verdict is on record with zero blocking findings, and the linked issue's behavior is reachable in normal operation without a human running an extra command. Verify each directly. The first three are properties of the artifact and can all pass on a change that fixes nothing.
- **Completion requires hosted CI**: opening or updating a PR does not finish the task. Monitor CI on the latest published commit until all applicable checks have finished successfully; fix failures and revalidate every new commit. Pending, queued, running, cancelled, or failed checks mean the task remains incomplete. Record intentional skips separately and verify they are by design. Keep working without waiting for the user to notice CI failures or ask you to continue. If an external blocker prevents completion, report the task as blocked, with the failing check and next action; never report it as done. Passing CI does not authorize merging or deployment.

### GitHub CLI Bodies

- **Never pass a multi-line body inline.** `--body "## Why\nFixes #123"` posts a literal `\n`, because neither bash double quotes nor PowerShell expand it. Write the body to a file and use `--body-file` for `gh pr create|edit|comment` and `gh issue create|comment`. Single-line bodies may use `--body`.
- Use a **quoted** heredoc delimiter (`<<'EOF'`) or single-quoted here-string (`@'...'@`) so backticks, `$`, and backslashes survive.
- Verify it rendered: `gh pr view <n> --json body --jq '.body' | head`. To patch an existing comment use `gh api -X PATCH repos/<owner>/<repo>/issues/comments/<id> --input <json-file>` — the `-F body=@<file>` form reports success without applying the change.

### No Personal Data in GitHub Content

**The repository is public.** Anything written to an issue, PR body, or comment is published. Never publish the user's username, home-directory paths, drive-letter paths, absolute Wardian home paths, or workspace paths that reveal what the user works on.

| Do not publish | Publish instead |
| --- | --- |
| `C:\Users\alice\.wardian\state.db` | `<wardian-home>/state.db` |
| `/home/alice/projects/thing` | `<workspace-path>` |
| `D:\Comms\Mail` | `<agent-workspace>` |

**Sanitize evidence before posting, not after** — a measurement, stack trace, file listing, or CLI output pasted straight from the terminal carries absolute paths, and the prose around it is usually clean while the evidence block is not. Agent names, workspace names, and directory layouts describe how the user works: include them only when the issue genuinely needs them. If something is already published, edit it rather than leaving it.
