# Spec 025: Testing Coverage and Screenshot Documentation

- **Status:** Implemented
- **Date:** 2026-04-26
- **Decider:** Tan Gemicioglu

## Context and Problem Statement

Wardian's test suite has three gaps that reduce confidence in PRs and make agent contributions harder to verify:

1. **No screenshot documentation**: the Playwright config only captures screenshots on failure. Agents opening PRs have no standard artifact to demonstrate what the UI looks like after their changes.
2. **No coverage reporting**: there is no measure of how much of the frontend or backend is exercised by tests. The README contains no coverage badges.
3. **Thin E2E coverage**: the five existing browser E2E specs test only the empty-state/smoke layer. No spec exercises agent lifecycle (spawn → running → idle → kill) or workflow execution, which are the most critical user-facing paths. Additionally, the boundary between what browser E2E can prove vs. what requires the native harness is not documented in a machine-readable way.

## Proposed Decision

### Component A — Screenshot Documentation

Add a dedicated Playwright project `screenshots` that captures named PNGs of every major view and key agent states. This project is **not** part of the default test run — it is invoked explicitly before opening a PR.

**Script**: `npm run screenshots`

**Output**: `e2e/screenshots/<timestamp>/` (gitignored). CI uploads the folder as a workflow artifact named `pr-screenshots` on every PR branch push.

**Playwright project config** (`e2e/playwright.config.ts`):

```ts
{
  name: "screenshots",
  testMatch: "screenshots.spec.ts",
  use: { screenshot: "on", video: "off" },
}
```

**Coverage** (`e2e/tests/screenshots.spec.ts`):
| Screenshot name | How to reach it |
|---|---|
| `dashboard.png` | default view on load |
| `agent-spawn.png` | sidebar Agent Config tab |
| `workflow-builder.png` | sidebar Workflows tab |
| `settings.png` | sidebar Settings tab |
| `class-manager.png` | sidebar Classes tab |
| `explorer.png` | sidebar Explorer tab |
| `grid-empty.png` | Grid view, no agents |
| `agent-running.png` | Grid view after spawning mock agent (`basic` scenario) |
| `agent-action-needed.png` | Grid view after spawning mock agent (`action_needed` scenario) |
| `watchlist-populated.png` | right sidebar after mock agent spawned |

**PR requirement** (added to AGENTS.md Pre-Commit Checklist): before opening a PR that touches UI components or layout, run `npm run screenshots` and attach the CI artifact link (or embed one representative screenshot) in the PR description.

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

- **Positive**: agents have a deterministic, artifact-backed way to document UI changes in PRs.
- **Positive**: Codecov gives per-PR coverage delta without blocking merges; coverage can only improve over time.
- **Positive**: agent-lifecycle and workflow E2E specs cover the most important user-facing paths that were entirely untested.
- **Positive**: the `@native-only` convention makes coverage gaps explicit and machine-readable.
- **Negative**: `npm run screenshots` requires the app to be running (either `tauri dev` or via the `webServer` in playwright config); CI must start the app first, adding ~3 min to PR pipelines.
- **Negative**: `cargo-llvm-cov` is a one-time developer install not managed by Cargo.toml; needs documentation and CI bootstrapping.
- **Negative**: the mock provider fixture for agent-lifecycle specs calls `spawn_agent` IPC, which means these tests run on the browser E2E layer but depend on the Tauri backend being alive. They will not work in pure browser mode without the Tauri webview — document this limitation.
