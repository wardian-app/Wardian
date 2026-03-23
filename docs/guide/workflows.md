# Visual Workflows

Wardian includes a deterministic, event-driven execution engine that allows you to automate complex multi-agent sequences through a visual node-based canvas.

## 🧱 Core Concepts

### 1. The Canvas
The Workflow view provides a grid-based workspace where you can drag and drop functional "Nodes" and connect them with "Edges" to define the flow of execution.

### 2. Node Types
- **Agent Node**: Injects a prompt into a live agent session or runs a headless command.
- **Logic Node**: Branches the flow based on a condition (e.g., "If the test passed, continue").
- **Loop Node**: Repeats a sequence of actions a specific number of times or until a condition is met.
- **Trigger Node**: The entry point for the workflow (e.g., a Cron schedule or File Watcher).

### 3. Execution (The Pulse Model)
Wardian uses a unique "Pulse" model for execution. A node only triggers when it has received an unconsumed pulse from all of its upstream dependencies. This allows for complex cycles and loops that are impossible in traditional DAG engines.

## 🚀 Creating a Workflow

1. Click **WORKFLOWS** in the top bar.
2. Drag nodes from the **Workflow Library** sidebar onto the canvas.
3. Connect the output port of one node to the input port of another.
4. **Configure Node**: Click a node to open its settings (e.g., selecting the target agent or writing the prompt).
5. **Save & Run**: Click the **Run** button in the top header to execute the sequence manually.

## 🕰️ Automation & Triggers

You can automate your workflows using built-in triggers:
- **Cron**: Schedule workflows to run at specific times (e.g., `0 0 * * *` for every midnight).
- **File Watcher**: Trigger a workflow whenever a file in your project changes (perfect for auto-testing).

## 📊 Monitoring
During execution, nodes will change color to indicate their status (Processing, Success, or Error). You can monitor the real-time registry values to see the data being passed between nodes.
