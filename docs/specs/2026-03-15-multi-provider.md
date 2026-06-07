# Multi-Provider Support (The Provider Trait)

* **Status:** Implemented
* **Date:** 2026-03-15

## Context and Problem Statement
Currently, Wardian's backend logic in `manager.rs` is tightly coupled to the `gemini-cli`. To achieve our goal of being a universal agent habitat, we need to support multiple providers (Claude Code, Codex, OpenClaw, etc.) without rewriting the core spawning and telemetry logic.

## Proposed Decision
We will implement a **Provider Trait** in `src-tauri/src/models/provider.rs` that abstracts the specific CLI commands and log parsing.

### 1. The Provider Trait
```rust
pub trait AgentProvider: Send + Sync {
    /// Returns the executable name and OS-specific base arguments.
    fn get_executable(&self) -> (String, Vec<String>);

    /// Returns the full list of CLI flags based on AgentConfig.
    fn get_spawn_args(&self, config: &AgentConfig, is_resume: bool) -> Vec<String>;

    /// Parses raw PTY output for provider-specific JSON events or status updates.
    fn parse_output(&self, line: &str) -> Option<AgentEvent>;

    /// Returns the provider-specific instruction filename (e.g., "GEMINI.md").
    fn get_instruction_filename(&self) -> String;
}
```

### 2. Native Instruction Inclusion (@AGENTS.md)
To maintain a single source of truth across all providers:
- `AGENTS.md` is established as the **Master Instruction Set**.
- Provider-specific files (`GEMINI.md`, `CLAUDE.md`) will be created as **Stub Files** containing only `@AGENTS.md`.
- This leverages the agent's native inclusion syntax to route context back to the master file without content duplication.

### 3. Manager Refactoring
The `manager.rs` will be refactored to use `Arc<dyn AgentProvider>`. It will no longer care *which* CLI is being spawned, only that it satisfies the trait.

## Consequences
*   **Positive**: Full decoupling of the UI and core manager from specific agent CLIs.
*   **Positive**: Native Agent-level resolution of instructions (High performance, low complexity).
*   **Negative**: Requires a significant refactor of `manager.rs` and `AgentConfig`.
