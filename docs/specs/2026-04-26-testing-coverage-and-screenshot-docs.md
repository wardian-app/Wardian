# Testing Coverage and Screenshot Documentation

- **Status:** Implemented
- **Date:** 2026-04-26
- **Decider:** Tan Gemicioglu

## Context and Problem Statement

Wardian's test suite has three gaps that reduce confidence in PRs and make agent contributions harder to verify:

1. **No screenshot documentation**: the Playwright config only captures screenshots on failure. Agents opening PRs need a standard way to provide useful visual evidence for the UI behavior they changed.
2. **No coverage reporting**: there is no measure of how much of the frontend or backend is exercised by tests. The README contains no coverage badges.
3. **Thin E2E coverage**: the five existing browser E2E specs test only the empty-state/smoke layer. No spec exercises agent lifecycle (spawn → running → idle → kill) or workflow execution, which are the most critical user-facing paths. Additionally, the boundary between what browser E2E can prove vs. what requires the native harness is not documented in a machine-readable way.

## Proposed Decision

### Component A — Screenshot Documentation

Use local, feature-specific screenshot evidence rather than a generic CI-generated app tour. Agents should drive the changed behavior with Playwright or a running local app, save only meaningful screenshots under `e2e/screenshots/<feature>/<timestamp>/`, and embed representative images directly in the PR description.

Do not maintain a default screenshot command that captures empty windows or unchanged top-level views. Generic screenshots create low-signal artifacts and do not prove the PR behavior.

**PR requirement** (added to AGENTS.md Pre-Commit Checklist): for UI changes, include screenshots only when they explain the changed interaction/state. Omit screenshots for non-visual changes and avoid generic empty-state captures.

### Component B — Coverage Reporting

#### Frontend (Vitest)

Add `@vitest/coverage-v8` to devDependencies. Extend `vitest.config.ts`:

```ts
test: {
  coverage: {
    provider: "v8",
    reporter: ["text", "lcov"],
    include: ["src/**/*.{ts,tsx}"],
    exclude: ["src/test/**", "src/**/*.test.*", "src/types/**"],
  },
}
```

Add script: `"test:coverage": "vitest run --coverage"`

Output: `coverage/lcov.info` (gitignored).

#### Backend (Rust)

Requires `cargo-llvm-cov` installed once per machine (`cargo install cargo-llvm-cov`). Document in `docs/developer/setup.md`.

Add script alias to `package.json`:

```json
"test:coverage:rust": "cd src-tauri && cargo llvm-cov --lcov --output-path ../coverage/rust-lcov.info"
```

#### Codecov Integration

Add `.codecov.yml` to the repo root defining two flags:

```yaml
coverage:
  status:
    project:
      frontend:
        flags: [frontend]
        informational: true
      backend:
        flags: [backend]
        informational: true
```

`informational: true` means Codecov comments on PRs but does **not** block merges. Coverage is reported, not gated.

Update CI (`release.yml` or a dedicated `coverage.yml`) to:

1. Run `npm run test:coverage` and upload `coverage/lcov.info` with `flags: frontend`
2. Run `npm run test:coverage:rust` and upload `coverage/rust-lcov.info` with `flags: backend`

Add badges to `README.md`:

```md
[![Frontend Coverage](https://codecov.io/gh/ORG/wardian/branch/main/graph/badge.svg?flag=frontend)](...)
[![Backend Coverage](https://codecov.io/gh/ORG/wardian/branch/main/graph/badge.svg?flag=backend)](...)
```

### Component C — E2E Layer Expansion

#### Layer Boundaries

The three E2E layers have hard boundaries. These must be respected when writing tests:

| Layer                 | Command                   | What it can prove                                                     | What it cannot prove                                                  |
| --------------------- | ------------------------- | --------------------------------------------------------------------- | --------------------------------------------------------------------- |
| **Browser E2E**       | `npm run test:e2e`        | UI rendering, state transitions visible in DOM, mock provider flows   | Real PTY, real filesystem ops, native IPC, provider-specific behavior |
| **Native E2E**        | `npm run test:e2e:native` | PTY resize, `invoke` commands, real workspace init, junction creation | Multi-instance parallelism, provider-specific UI quirks               |
| **Real Provider E2E** | opt-in env flag           | Provider-specific spawn, real token flow                              | Everything else                                                       |

Tests that require a higher layer must include a `// @native-only` or `// @real-provider-only` comment and be wrapped in `test.skip(...)` when run in browser E2E. This makes the gap visible rather than silently absent.

#### New Browser E2E Specs

**`e2e/tests/agent-lifecycle.spec.ts`**

Uses `WARDIAN_MOCK_SCENARIO=basic` and `action_needed`. Covers:

- Spawn agent form submission → agent card appears in watchlist
- Status indicator transitions: Off → Processing → Idle (emerald)
- `action_needed` scenario: status indicator goes Amber
- Kill agent → card disappears from watchlist and grid

**`e2e/tests/workflow.spec.ts`**

Covers:

- Open workflow builder
- Create a workflow with two mock-agent blocks
- Run workflow → observe block status transitions in UI
- Cancel workflow

**`e2e/tests/watchlist.spec.ts`**

Covers:

- Watchlist item renders correct status color for each status (Idle=emerald, Processing=cyan, Action Required=amber, Error=red)
- Search/filter by name
- Collapse/expand watchlist panel

#### Mock Provider Fixture

E2E specs that require a running mock agent must use a `test.beforeAll` fixture that:

1. Sets `WARDIAN_HOME` to an isolated temp dir
2. Seeds a `projects.json` with one project containing a mock agent config
3. Calls the Tauri `spawn_agent` IPC to start the mock provider before assertions begin

This fixture will live in `e2e/fixtures/mockAgent.ts`.

## Consequences

- **Positive**: agents have a targeted, artifact-backed way to document UI changes in PRs without producing empty-state noise.
- **Positive**: Codecov gives per-PR coverage delta without blocking merges; coverage can only improve over time.
- **Positive**: agent-lifecycle and workflow E2E specs cover the most important user-facing paths that were entirely untested.
- **Positive**: the `@native-only` convention makes coverage gaps explicit and machine-readable.
- **Negative**: screenshot evidence is not a uniform CI artifact; agents must intentionally capture the UI state that matters for each feature.
- **Negative**: `cargo-llvm-cov` is a one-time developer install not managed by Cargo.toml; needs documentation and CI bootstrapping.
- **Negative**: the mock provider fixture for agent-lifecycle specs calls `spawn_agent` IPC, which means these tests run on the browser E2E layer but depend on the Tauri backend being alive. They will not work in pure browser mode without the Tauri webview — document this limitation.
