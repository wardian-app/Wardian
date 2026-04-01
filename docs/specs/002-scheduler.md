# Spec 002: Unified Workflow Scheduler

- **Status:** Implemented
- **Date:** 2026-03-25
- **Decider:** Architect

## Context and Problem Statement

Wardian's current scheduler is split between a hardcoded "Cron" trigger in the engine and an experimental sidebar UI. We need a unified system that allows users to instantiate workflow templates into "Scheduled Runs" with specific agent mappings.

## Proposed Decision

### 1. The Scheduled Trigger (Replaces Cron)

We will move away from a single "Cron" string to a structured **Schedule Definition**:

```json
{
  "type": "one_time" | "recurring" | "cron",
  "value": "2026-04-01T12:00:00Z" | "60m" | "0 0 * * *",
  "active": true
}
```

### 2. Workflow "Templates" vs "Runs"

- **Template**: The static JSON definition of the workflow nodes and logic.
- **Run (Instance)**: A specific execution of a template.
  - Stores **Agent Mappings**: (e.g., Template Role "Worker" -> Agent ID "agent-123").
  - Stores **State**: The current registry and pulse history.

### 3. Unified Sidebar Logic

The Workflow Sidebar will be refactored to handle two primary tabs:

- **Library**: Browse and edit templates.
- **Monitoring (Scheduler)**:
  - Displays all "Active Runs" (Scheduled or currently executing).
  - Allows "Quick-Start" of a template by picking an agent from a dropdown.

### 4. Implementation (Rust)

The **Internal Heartbeat Thread** (from the original Spec 002) will remain the driver, but it will now query a `scheduled_runs.json` store instead of individual workflow files.

## Consequences

- **Positive**: Allows starting multiple runs of the same workflow with different agents.
- **Positive**: Centralized management of all automation via the sidebar.
- **Negative**: Requires a migration of existing "Cron" trigger configurations.
