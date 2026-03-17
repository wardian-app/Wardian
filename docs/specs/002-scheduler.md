# Spec 002: Internal Rust Heartbeat (Scheduler)

* **Status:** Proposed
* **Date:** 2026-03-15
* **Decider:** Architect

## Context and Problem Statement
Wardian currently uses multiple ad-hoc `tokio::spawn` loops for metrics and basic triggers. This is difficult to monitor and doesn't scale well for complex "Scheduled Task" logic or "Agent-to-Agent" synchronization.

## Proposed Decision
We will consolidate all periodic logic into a single **Internal Rust Heartbeat Thread** with a precision of 1 second.

1. **The Loop**: A single `tokio::spawn` loop in `lib.rs` that ticks every 1 second.
2. **The Registry**: A `Vec<Arc<dyn HeartbeatTask>>` in `AppState`.
3. **Dispatch Logic**: On every tick, the Heartbeat iterates over its tasks.
    - **Fast (High-Frequency)**: Metrics collection (every 5s).
    - **Scheduled (Cron)**: Workflow triggers (evaluated via the `cron` crate).
    - **Lifecycle (Health Check)**: PTY status verification and auto-hibernation.
4. **Non-Blocking**: Heavy tasks (like file-system scans or agent spawns) MUST be delegated back to `tokio::spawn` to avoid stalling the Heartbeat.

## Consequences
* **Positive**: Centralized control over all background automation.
* **Positive**: Reduced overhead from multiple active loops.
* **Positive**: Easier to implement global "Pause All Triggers" (Panic Button).
* **Negative**: A crash in the Heartbeat loop could stall all background automation.
