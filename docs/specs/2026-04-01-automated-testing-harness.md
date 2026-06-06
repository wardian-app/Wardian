# Automated Testing Harness

* **Status:** Proposed
* **Date:** 2026-03-31

## Context and Problem Statement
Wardian still depends heavily on manual verification for agent orchestration, workflow execution, PTY behavior, and provider-specific runtime behavior. That is slow, expensive, and risky because the current runtime shares the user's real `~/.wardian` state, real providers, and real agent sessions.

Issue [#95](https://github.com/wardian-app/Wardian/issues/95) is not just about CI coverage. The more important requirement is that live agents must be able to verify their own changes during execution without interfering with production agents or hallucinating success.

The testing problem has three separate layers that must not be conflated:

* browser-level UI rendering and interaction
* native Tauri IPC and PTY/runtime behavior
* provider-specific behavior, which may be deterministic or real-provider-backed

Without a dedicated layered harness, regressions in workflows, agent runtime behavior, PTY rendering, sidebars, and orchestration logic will continue to escape into manual testing.

## Proposed Decision
Wardian will use a layered automated testing harness built around an isolated runtime home, a deterministic mock provider, a browser Playwright smoke layer, a native Tauri/WebDriver layer for IPC and PTY behavior, and an opt-in real-provider verification layer for cases where mock coverage is insufficient.

### 1. Isolated Runtime Home
The Rust backend will treat `WARDIAN_HOME` as the highest-priority home override.

* `get_wardian_home()` resolves `WARDIAN_HOME` first.
* If `WARDIAN_HOME` is unset, Wardian continues to use `~/.wardian`.
* Tests use repo-root `target/test/` or test-specific temp directories as their isolated runtime home.
* All stateful runtime artifacts must respect that root:
  * agents
  * workflows
  * scheduled runs
  * classes
  * provider bootstrap state
  * logs
  * library data

This gives the app a self-contained, disposable runtime environment that can be deleted by test cleanup.

### 2. Mock Provider
Wardian will keep a backend mock provider for deterministic runtime and UI verification.

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

This provider is the default automated E2E runtime and should cover most orchestration tests without paid or stateful external CLIs.

### 3. Browser Playwright Smoke Layer
Wardian will keep a browser-driven Playwright layer for fast UI smoke coverage.

This layer should:

* boot the frontend shell quickly
* run against isolated `WARDIAN_HOME`
* cover navigation, settings, view switching, and non-native UI regressions
* avoid claiming coverage for native Tauri `invoke`, PTY, or provider behavior

This layer exists to catch UI regressions cheaply. It is not a substitute for native runtime testing.

### 4. Native Tauri Runtime Harness
Wardian will add a native Tauri automation layer using the Tauri-supported WebDriver path.

This harness should:

* launch a native Tauri app instance with isolated `WARDIAN_HOME`
* expose the real Tauri IPC bridge
* exercise PTY-backed terminal behavior, native `invoke` commands, and provider spawn/resume flows
* support seeded isolated fixtures before each suite
* use stable setup commands and repo-local native driver artifacts instead of one-off downloads into the repository root

Initial native coverage should include:

* app boot in isolated mode
* spawning a mock agent through real Tauri commands
* terminal output and status updates rendering through the native PTY path
* sidebar/grid/watchlist rendering and interaction
* workflow execution and monitoring flows

This is the first layer that can truthfully validate PTY and native runtime behavior.

Initial local setup and execution should use:

* `npm run setup:e2e:native`
* `npm run test:e2e:native`

The setup command must be cross-platform and implemented as Node tooling rather than shell-specific scripts. It should prepare `tauri-driver`, detect or install a native WebDriver where the project has a reliable automated path, and print OS-specific guidance when manual installation is required.

### 5. Opt-In Real Provider Verification
Wardian will support a separate, opt-in native test layer for real providers such as OpenCode when provider-specific behavior must be validated directly.

This layer should:

* only run in native Tauri mode, never browser-only mode
* require explicit environment gating
* reuse isolated `WARDIAN_HOME`
* document provider prerequisites such as local auth and installed binaries
* surface backend failure context such as `wardian_debug.log` tails when provider startup fails

The real-provider layer is for validating provider-specific integration seams such as:

* PTY behavior that the mock cannot reproduce faithfully
* provider-native session bootstrap/resume quirks
* provider-specific cwd, trust, or approval behavior

It should remain opt-in locally and out of default CI unless a provider-specific CI strategy becomes stable and cost-safe.

### 6. Agent-Facing Verification Workflow
Wardian will document and standardize how agents should run the automated test harness during their verification phase.

This includes:

* the exact commands to run
* which layers are expected for frontend, backend, PTY, and provider changes
* how to invoke the isolated environment safely
* how to interpret failures and locate artifacts

This guidance can live in docs plus either:

* a dedicated testing skill, or
* explicit testing instructions in `AGENTS.md`

The goal is that a live agent can run the same verification path a human reviewer expects, against safe isolated state, without touching production sessions.

### 7. CI Integration
Once the isolated harness is stable locally, it will be added to PR validation.

CI should eventually run:

* `npm run lint`
* `npm run test`
* `cargo test`
* `cargo check`
* browser Playwright smoke suites
* native mock-provider Tauri runtime suites

Failures should upload useful artifacts where possible, especially for native E2E failures.

Real-provider suites should stay opt-in until they are stable, deterministic enough, and cost-safe.

### 8. Recommended Rollout Order
The implementation should be staged in this order:

1. `WARDIAN_HOME` override and isolated state support
2. mock provider
3. browser Playwright smoke layer
4. native Tauri/WebDriver runtime harness
5. agent-facing verification instructions
6. CI rollout for browser and native mock layers
7. opt-in real-provider native suites

This ordering separates cheap UI confidence from native-runtime confidence and keeps provider-specific testing from blocking the core harness.

## Consequences
* **Positive**: Agents and humans can run verification against isolated state without touching production `~/.wardian`.
* **Positive**: UI and orchestration regressions can be tested deterministically without paid provider calls.
* **Positive**: PTY behavior and Tauri `invoke` paths gain a native test layer instead of being misclassified as browser smoke coverage.
* **Positive**: Real-provider suites remain possible for OpenCode and similar providers once the native harness exists.
* **Positive**: The same verification workflow can be used locally, by live agents, and in CI with clear boundaries between test layers.
* **Negative**: The backend must consistently honor `WARDIAN_HOME`, which increases discipline around path resolution.
* **Negative**: A mock provider adds another maintained provider surface, even if it is test-only.
* **Negative**: Native Tauri/WebDriver tests will increase setup complexity and CI runtime.
* **Negative**: Real-provider native suites remain slower and less deterministic than mock-backed suites, so they must stay explicitly scoped.
