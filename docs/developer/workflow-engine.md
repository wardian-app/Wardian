# Workflow Engine Architecture

The Wardian Workflow Engine is a **Deterministic, Event-Driven Execution Environment** designed for multi-agent orchestration. It supports manual, scheduled, and listener-style triggers; conditional logic; loops; waits; agent execution modes; shared storage; and workflow run telemetry.

## 🧱 Core Concepts

### 1. The Registry (Stateful Context)
The engine maintains a transient `HashMap<String, Value>` called the **Registry** during a workflow run.
- **`nodes.[id]`**: Stores the output of each node.
- **`trigger`**: Stores the payload that initiated the run (e.g., Cron timestamp, File Watcher event).
- **`storage`**: Provides access to `shared_storage.json` for cross-workflow persistence.

### 2. Transactional Consumption Logic
Unlike traditional DAGs, Wardian supports **Cycles** and **Loops** through a pulse-based consumption model:
- **Pulses**: When a node finishes, it "pulses" its output ports.
- **Consumption**: A downstream node only executes if it has "unconsumed pulses" from its dependencies.
- **Wait Nodes**: Specifically require a pulse from *every* dependency before triggering.

### 3. Internal Candidate Queue
Each run owns a FIFO queue of candidate node IDs. The engine seeds the queue from trigger entry points, pops one node at a time, validates dependency consumption, executes the node, stores output in the registry, and enqueues downstream candidates. This keeps graph execution inspectable without making each agent node responsible for orchestrating the rest of the graph.

## 🚀 Execution Flow

1. **Trigger Phase**:
   - **Cron**: Uses the `cron` crate to evaluate standard expressions.
   - **File Watcher**: Uses the `notify` crate to monitor filesystem events.
   - **Manual**: Triggered via the `run_workflow` Tauri command.
2. **Initialization**:
   - Resolves the `WorkflowDefinition` from JSON.
   - Sets up the `Registry` and execution queue by identifying entry-point candidates.
3. **The Loop**:
   - Pops a candidate node ID from the execution queue.
   - Validates dependency satisfaction (Transactional Logic).
   - Executes node-specific logic (see below).
   - Updates the `Registry` and pulses downstream nodes.
4. **Finalization**:
   - Emits telemetry events (`workflow-telemetry`) for real-time UI visualization.
   - Emits workflow completion status for app-level subscribers.
   - Persists `shared_storage.json` if memory nodes were used.

## 🧩 Node Types & Logic

### Agent Node
The most complex node type. The builder exposes one execution policy selector:

- **`ephemeral`**: build a fresh workflow-run agent config from the node's class and folder fields.
- **`inherit_fresh`**: clone the selected agent's provider, class, workspace, skills, and scoped read configuration, but clear provider resume state and run under a workflow-run session ID.
- **`inherit_resume`**: intentionally use the selected agent's provider session and mutable runtime state.

The backend resolves that selector into an `AgentExecutionContext` before launch. Fresh modes use headless execution and must not kill or mutate a live source agent. Resume mode may use the live PTY for text output, or temporarily switch to provider-native headless execution for structured JSON output when the provider requires it.

Workflow-spawned agent runs skip interactive startup prompts. Provider resume flags are emitted only when the resolved context is `inherit_resume`.

### Logic Node
Evaluates a string condition (e.g., `nodes.gatekeeper.output.decision === 'PROCEED'`) using a regex-based parser. Pulses either the `on_true` or `on_false` port.

### Loop Node
Maintains an internal iterator count in the `Registry`. Pulses the `body` port until the limit is reached, then pulses `done`.

### Memory Node
Performs `get`, `set`, or `delete` operations on the `shared_storage.json` file.

## 🔒 Security & Isolation
- **Headless Execution**: Uses `tokio::process::Command` with strictly limited `current_dir` validation via `validate_workspace_path`.
- **Interpolation**: All strings (prompts, paths, commands) are safely interpolated using `\{\{nodes.id.output.path\}\}` syntax before execution.
