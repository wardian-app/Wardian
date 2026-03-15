# Workflow Engine Architecture

The Wardian Workflow Engine is a **Deterministic, Event-Driven Execution Environment** designed for multi-agent orchestration. It supports periodic triggers, conditional logic, and stateful memory.

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

## 🚀 Execution Flow

1. **Trigger Phase**:
   - **Cron**: Uses the `cron` crate to evaluate standard expressions.
   - **File Watcher**: Uses the `notify` crate to monitor filesystem events.
   - **Manual**: Triggered via the `run_workflow` Tauri command.
2. **Initialization**:
   - Resolves the `WorkflowDefinition` from JSON.
   - Sets up the `Registry` and `Queue` (identifying entry points).
3. **The Loop**:
   - Pops a node ID from the `Queue`.
   - Validates dependency satisfaction (Transactional Logic).
   - Executes node-specific logic (see below).
   - Updates the `Registry` and pulses downstream nodes.
4. **Finalization**:
   - Emits telemetry events (`workflow-telemetry`) for real-time UI visualization.
   - Persists `shared_storage.json` if memory nodes were used.

## 🧩 Node Types & Logic

### Agent Node
The most complex node type. It can run in two modes:
- **PTY (Text) Mode**: Injects a prompt into a live agent's terminal and waits for an "Idle" JSON event from the CLI.
- **Headless (JSON) Mode**: Temporarily kills the live PTY, runs a one-off `gemini-cli` command for structured JSON output, and then restores the PTY.

### Logic Node
Evaluates a string condition (e.g., `nodes.gatekeeper.output.decision === 'PROCEED'`) using a regex-based parser. Pulses either the `on_true` or `on_false` port.

### Loop Node
Maintains an internal iterator count in the `Registry`. Pulses the `body` port until the limit is reached, then pulses `done`.

### Memory Node
Performs `get`, `set`, or `delete` operations on the `shared_storage.json` file.

## 🔒 Security & Isolation
- **Headless Execution**: Uses `tokio::process::Command` with strictly limited `current_dir` validation via `validate_workspace_path`.
- **Interpolation**: All strings (prompts, paths, commands) are safely interpolated using `{{nodes.id.output.path}}` syntax before execution.
