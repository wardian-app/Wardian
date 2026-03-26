# Spec 007: Reliable Workflow Execution Engine

* **Status:** Proposed
* **Date:** 2026-03-25
* **Decider:** Architect

## Context and Problem Statement
The current workflow engine relies on hardcoded message types to detect when an agent has finished a turn. This is brittle and fails when introducing new providers like Claude. Additionally, template management is cumbersome for parallel runs.

## Proposed Decision

### 1. Reliable Agent Turn Completion
We will update the `AgentProvider` trait to include a formal "Turn Completion" detection mechanism.
*   **Gemini/Claude**: The providers will now emit an `AgentEvent::TurnCompleted` whenever the stream indicates the model has finished speaking (e.g., specific JSON flags or PTY escape sequences).
*   **Engine Update**: The `AgentNode` in the engine will wait specifically for this event instead of searching for arbitrary message strings.

### 2. Role-Based Agent Mapping
To streamline management, workflows will now use **Dynamic Roles** instead of hardcoded `agent_id`s.
*   **Template Definition**: A node is assigned to role `PrimaryCoder`.
*   **Run Initialization**: The user maps `PrimaryCoder` to a specific active agent (e.g., "Agent Alpha").
*   **Benefit**: This allows the same workflow template to be reused across different projects or agent swarms without editing the JSON file.

### 3. State Isolation (The Instance Model)
Every execution of a workflow is an independent **Instance**.
*   **Registry**: A unique in-memory key-value store for each run.
*   **Pulse History**: Track unconsumed pulses specifically for that run to ensure parallel executions don't bleed into each other.

### 4. Headless Persistence
If an agent is offline when a workflow hits an Agent Node, the engine will use the provider's `run_headless` method but **must** verify that the session state is updated correctly so that subsequent nodes see the same context.

## Consequences
* **Positive**: 100% reliable turn detection across all providers (Gemini, Claude, etc.).
* **Positive**: Massive reduction in template duplication.
* **Negative**: Requires a significant refactor of the `workflow_engine/mod.rs` execution loop.
