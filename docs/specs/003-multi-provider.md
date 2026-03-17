# Spec 003: Multi-Provider Support (The Provider Trait)

* **Status:** Proposed
* **Date:** 2026-03-15
* **Decider:** Architect

## Context and Problem Statement
Currently, Wardian's backend logic in `manager.rs` is tightly coupled to the `gemini-cli`. To achieve our goal of being a universal agent habitat, we need to support multiple providers (Claude Code, Codex, OpenClaw, etc.) without rewriting the core spawning and telemetry logic.

## Proposed Decision
We will implement a **Provider Trait** in `src-tauri/src/models/provider.rs` that abstracts the specific CLI commands and log parsing.

### 1. The Provider Trait
```rust
pub trait AgentProvider: Send + Sync {
    /// Returns the executable name and arguments for spawning the agent.
    fn get_spawn_command(&self, config: &AgentConfig) -> (String, Vec<String>);
    
    /// Parses raw PTY output for provider-specific JSON events or status updates.
    fn parse_output(&self, line: &str) -> Option<AgentEvent>;
    
    /// Returns the specific CLI flags for features like sandboxing or including directories.
    fn get_feature_flags(&self, config: &AgentConfig) -> Vec<String>;
}
```

### 2. Implementation Strategy
- **`GeminiProvider`**: The first implementation, wrapping existing `gemini-cli` logic.
- **`OpenClawProvider` / `ClaudeProvider`**: Future implementations that map their specific CLI flags and output formats to our internal `AgentEvent` schema.
- **Factory Pattern**: A `ProviderFactory` will resolve the correct implementation based on the `provider_type` field in `AgentConfig`.

### 3. Manager Refactoring
The `manager.rs` will be refactored to take an `Arc<dyn AgentProvider>`. It will no longer care *which* CLI is being spawned, only that it satisfies the trait.

## Consequences
* **Positive**: Full decoupling of the UI and core manager from specific agent CLIs.
* **Positive**: Enables rapid support for new AI models and agent tools.
* **Negative**: Requires a significant refactor of `manager.rs` and `AgentConfig`.
* **Negative**: Parsing logic becomes more complex as we handle varying output formats (JSON stream vs. raw text).
