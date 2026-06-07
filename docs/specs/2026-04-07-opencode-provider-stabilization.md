# OpenCode Provider Stabilization

* **Status:** Implemented
* **Date:** 2026-04-06

## Context and Problem Statement

OpenCode support exists in Wardian, but the provider remains unstable in packaged builds and noticeably slower than the other providers during startup and some session actions.

The audit found three main causes:

- Wardian creates fresh OpenCode sessions through a full headless `opencode run --format json` bootstrap before spawning the interactive PTY session.
- The interactive OpenCode launch path does not consistently anchor the session to the real project workspace in the same way OpenCode's own TUI expects.
- OpenCode status recovery is lighter than the other providers, so UI state depends heavily on live PTY parsing and can feel stale or delayed.

OpenCode also has an important provider-specific constraint: it should run in the real target workspace rather than a projected habitat workspace, while Wardian-specific instructions and skills are injected through OpenCode runtime config.

## Decision

We will stabilize OpenCode in two stages, starting with a compatibility pass that keeps the existing bootstrap model but makes the bootstrap and interactive session contexts consistent.

### 1. Real Workspace Interactive Launch

- OpenCode interactive PTY sessions will run in the real resolved workspace.
- Wardian will stop treating OpenCode as a projected-workspace provider.
- The interactive launch contract will match OpenCode's documented TUI model as closely as possible, including explicit project anchoring where appropriate.

### 2. Bootstrap / Interactive Context Consistency

- The existing `obtain_session_id` bootstrap will remain for now.
- Bootstrap and interactive launch must use the same effective workspace and the same OpenCode runtime context assumptions.
- Wardian will continue using runtime configuration for extra instruction files and skill roots, but only in ways proven compatible with the specific launch mode.

### 3. Telemetry Recovery Improvement

- OpenCode will gain lightweight persisted-session enrichment so the UI can recover status and metadata even when live PTY parsing is incomplete.
- This pass is intentionally narrower than a full OpenCode log parser rewrite.

### 4. Deferred Optimization

- Removing the bootstrap round-trip entirely is deferred to a later pass.
- The likely long-term direction is to adopt the OpenCode session id from the interactive stream instead of creating it through a separate headless prompt.

## Consequences

- **Positive**: Packaged and dev launches use a more consistent OpenCode project model.
- **Positive**: OpenCode sessions are less likely to start in the wrong cwd or lose project-config discovery.
- **Positive**: Some perceived lag should improve once status recovery is less dependent on live PTY state.
- **Negative**: Fresh OpenCode sessions will still pay the bootstrap round-trip cost in this pass.
- **Negative**: Runtime-config asymmetry between headless and interactive modes may still require a second pass if OpenCode's TUI remains sensitive to inline config.
