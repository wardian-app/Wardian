# Spec 012: Automated Testing Harness

* **Status:** Accepted
* **Date:** 2026-03-31
* **Decider:** Architect

## Context and Problem Statement
Wardian still depends heavily on manual verification for agent orchestration, workflow execution, and UI behavior. That is slow, expensive, and risky because the current runtime shares the user's real `~/.wardian` state, real providers, and real agent sessions.

Issue [#95](https://github.com/tangemicioglu/Wardian/issues/95) is not just about CI coverage. The more important requirement is that live agents must be able to verify their own changes during execution without interfering with production agents or hallucinating success.

Today, Wardian is missing the core infrastructure needed for that:

* The backend always resolves its home to `~/.wardian`, so tests cannot run in a safe isolated state root.
* There is no mock provider for deterministic multi-agent UI and workflow verification.
* There is no Playwright + Tauri end-to-end harness for native app behavior.
* There is no documented, standardized verification path that agents themselves can invoke reliably.

Without a dedicated automated testing harness, regressions in workflows, agent runtime behavior, sidebars, and orchestration logic will continue to escape into manual testing.

## Proposed Decision
Wardian will add a layered automated testing harness built around an isolated runtime home, a deterministic mock provider, a native Playwright + Tauri E2E runner, and explicit instructions for agent-driven verification.

### 1. Isolated Runtime Home
The Rust backend will treat `WARDIAN_HOME` as the highest-priority home override.

* `get_wardian_home()` will resolve `WARDIAN_HOME` first.
* If `WARDIAN_HOME` is unset, Wardian will continue to use `~/.wardian`.
* Tests will use `src-tauri/target/test/` or test-specific temp directories as their isolated runtime home.
* All stateful runtime artifacts must respect that root:
  * agents
  * workflows
  * scheduled runs
  * classes
  * provider bootstrap state
  * logs
  * library data

This gives the app a self-contained, disposable runtime environment that can be deleted by `cargo clean` or test cleanup.

### 2. Mock Provider
Wardian will add a backend mock provider for deterministic runtime and UI verification.

The mock provider must support:

* fresh session init
* resumed sessions
* processing/status transitions
* action-needed prompts
* successful completion
* failure cases
* long terminal output / scrollback scenarios
* workflow-compatible headless execution

The mock provider should emit the same event shapes used by real providers so the frontend, workflow engine, and telemetry paths can be exercised without special-case UI logic.

This provider is primarily a testing tool and should be used by automated E2E flows instead of real Claude, Codex, or Gemini sessions.

### 3. Playwright + Tauri Native E2E Harness
Wardian will add a Playwright-based native app automation layer.

The harness should:

* launch the Tauri app with an isolated `WARDIAN_HOME`
* seed test fixtures into that runtime home before each suite
* use the mock provider for deterministic behavior
* exercise native UI behavior rather than browser-only components

Initial smoke coverage should include:

* app boot in isolated mode
* spawning a mock agent
* terminal output and status updates rendering
* sidebar/grid/watchlist rendering and interaction
* workflow execution and monitoring flows

The first milestone is not exhaustive UI coverage. It is a stable, repeatable native harness that can catch orchestration regressions.

### 4. Agent-Facing Verification Workflow
Wardian will document and standardize how agents should run the automated test harness during their verification phase.

This includes:

* the exact commands to run
* which tests are expected for frontend, backend, and E2E changes
* how to invoke the isolated environment safely
* how to interpret failures and locate artifacts

This guidance can live in docs plus either:

* a dedicated Playwright/testing skill, or
* explicit testing instructions in `AGENTS.md`

The goal is that a live agent can run the same verification path a human reviewer expects, against safe isolated state, without touching production sessions.

### 5. CI Integration
Once the isolated harness is stable locally, it will be added to PR validation.

CI should eventually run:

* `npm run lint`
* `npm run test`
* `cargo test`
* `cargo check`
* targeted Playwright + Tauri E2E suites

Failures should upload useful artifacts where possible, especially for native E2E failures.

### 6. Recommended Rollout Order
The implementation should be staged in this order:

1. `WARDIAN_HOME` override and isolated state support
2. mock provider
3. first Playwright + Tauri smoke tests
4. agent-facing verification instructions
5. CI rollout

This ordering minimizes risk and gives immediate value after each step.

## Consequences
* **Positive**: Agents and humans can run verification against isolated state without touching production `~/.wardian`.
* **Positive**: UI and orchestration regressions can be tested deterministically without paid provider calls.
* **Positive**: Native Tauri behavior becomes testable in a repeatable way instead of relying on manual visual verification.
* **Positive**: The same verification workflow can be used locally, by live agents, and in CI.
* **Positive**: Workflow, sidebar, terminal, and orchestration regressions should be caught earlier and more reliably.
* **Negative**: The backend must consistently honor `WARDIAN_HOME`, which increases discipline around path resolution.
* **Negative**: A mock provider adds another maintained provider surface, even if it is test-only.
* **Negative**: Playwright + Tauri E2E tests will increase setup complexity and CI runtime.
* **Negative**: The initial harness will need fixture design and maintenance to stay deterministic as the runtime model evolves.
