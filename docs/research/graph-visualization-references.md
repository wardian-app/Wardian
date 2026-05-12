# Graph Visualization References

This document maps public graph-visualization libraries, workflow graph UIs, topology maps, and trace-oriented visualizations to design patterns relevant to Wardian's workflow builder, workflow run graph, and future agent communication graph.

This is not an endorsement, affiliation claim, product evaluation, or competitive teardown. The notes below describe public architecture and design pressure only.

Last reviewed: 2026-05-12.

Source basis: entries were selected through public research and checked against public project pages, repositories, or documentation where available.

## Design Axes

- **Graph purpose**: construction graph, dependency graph, communication graph, topology map, trace graph, or artifact graph.
- **Source of truth**: whether the graph is hand-authored, generated from code/configuration, projected from runtime events, or reconstructed from traces.
- **Scale behavior**: whether the UI handles tens, hundreds, thousands, or tens of thousands of nodes and edges.
- **Layout model**: manual placement, automatic layout, hierarchical DAG layout, force-directed layout, clustered topology, or timeline-adjacent trace layout.
- **Status overlay**: how health, freshness, ownership, stuckness, current execution state, or causality is layered onto graph structure.
- **Drill-down path**: how users move from a node or edge to logs, terminal output, messages, files, approvals, artifacts, traces, or run history.
- **Text as source**: whether graph definitions can be saved, reviewed, versioned, and regenerated.

## Summary Map

| System | Primary Pattern | Wardian Takeaway |
|---|---|---|
| [React Flow](https://reactflow.dev/) | Node-based editor and interactive diagram library. | Relevant to Wardian's workflow construction surface. |
| [Cytoscape.js](https://github.com/cytoscape/cytoscape.js/) | Graph theory and interactive network visualization library. | Relevant to agent interaction, communication, dependency, and ownership networks. |
| [sigma.js](https://v4.sigmajs.org/) | WebGL-powered large-graph rendering. | Useful if Wardian needs dense agent/message/trace graphs at large scale. |
| [Graphviz](https://graphviz.org/) and [Mermaid](https://mermaid.js.org/) | Text-defined graph rendering. | Reinforces "code/text as construction, graph as projection." |
| [Kiali](https://kiali.io/) | Service-mesh topology visualization. | Relevant to overlaying health, traffic, traces, and ownership onto communication graphs. |
| [Airflow](https://github.com/apache/airflow) | Workflow UI with DAG graph and run status. | Reference for dependency graph plus per-run status projection. |
| [Dagster](https://github.com/dagster-io/dagster) | Asset graph and orchestration observability. | Useful for scaling dependency visualization and artifact/run status. |
| [Temporal UI](https://docs.temporal.io/web-ui/) | Durable workflow execution inspection. | Reference for workflow history, retries, and long-running execution evidence. |
| [Prefect](https://github.com/PrefectHQ/prefect) | Flow-run graph and task/subflow observability. | Reference for API-backed run graphs with update limits and artifacts. |
| [Jaeger UI](https://github.com/jaegertracing/jaeger-ui) | Distributed trace detail UI. | Relevant to Wardian run traces, causality, and sequence/flame views. |

## Reference Profiles

### React Flow

**Source basis:** Public project site checked.

**What it includes:** React Flow is an open-source React library for building node-based editors and interactive diagrams. It provides nodes, edges, selection, keyboard operations, panning, zooming, and a component model for custom workflow-style surfaces.

**Distinctive components:**

- Custom node and edge rendering.
- Interactive editing with keyboard and pointer support.
- Library posture for builders, workflows, and node-based tools.
- Broad ecosystem adoption and examples.

**Wardian relevance:** React Flow is a construction-surface reference. Wardian should use similar interaction concepts for workflow editing, but the graph should remain a projection of Wardian's normalized workflow model rather than becoming a separate source of truth.

### Cytoscape.js

**Source basis:** Public repo and documentation checked.

**What it includes:** Cytoscape.js is a graph-theory library for visualization and analysis. It includes a graph model, optional renderer, layouts, traversal, filtering, and interaction support for rich network views.

**Distinctive components:**

- Graph model and renderer are separable.
- Directed, undirected, mixed, compound, and other graph use cases.
- Traversal and graph algorithms.
- Layout and interaction ecosystem.
- Usable headlessly for analysis or in-browser for UI.

**Wardian relevance:** Cytoscape.js is relevant to non-construction graphs: agent communication, dependency maps, memory/reference networks, skill usage, workflow run causality, and ownership. These graphs should support analysis and filtering, not only manual node placement.

### sigma.js

**Source basis:** Public project site checked.

**What it includes:** sigma.js is a WebGL-powered library for rendering large interactive graphs in the browser. It is built on graphology and emphasizes smooth rendering for complex networks.

**Distinctive components:**

- GPU-accelerated graph rendering.
- Built-in pan, zoom, hover, and multi-touch.
- Framework-agnostic integration.
- Graphology algorithms for layouts, metrics, and community detection.
- Large-graph examples such as thousands of research-paper nodes.

**Wardian relevance:** sigma.js is relevant if Wardian eventually visualizes thousands of events, messages, memories, or interactions. The lesson is to separate "large graph exploration" from "workflow editing"; they have different performance and interaction needs.

### Graphviz and Mermaid

**Source basis:** Public project docs checked at a high level.

**What they include:** Graphviz and Mermaid generate diagrams from text definitions. Graphviz emphasizes graph layout algorithms and DOT; Mermaid emphasizes Markdown-friendly diagrams and documentation.

**Distinctive components:**

- Text-defined graph source.
- Repeatable rendering.
- Good fit for documentation and generated diagrams.
- Different balance between layout power and authoring convenience.

**Wardian relevance:** These tools reinforce Wardian's documentation posture. Workflow graphs, class relationships, and reference maps should be exportable to text-defined diagrams for specs, reviews, and agent-authored documentation.

### Kiali

**Source basis:** Public site and docs checked.

**What it includes:** Kiali is a service-mesh observability and configuration dashboard. Its graph surface shows service topology, health, traffic, and links to deeper metrics and tracing systems.

**Distinctive components:**

- Service topology graph.
- Health overlays.
- Metrics and tracing integration.
- Ownership/custom information patterns.
- Links out to deeper dashboards.

**Wardian relevance:** Kiali is a useful analogy for agent communication graphs. Wardian can show which agents, workflows, skills, and tools are interacting, then overlay stuckness, cost, error rate, approvals, and ownership.

### Airflow

**Source basis:** Public repo checked.

**What it includes:** Airflow is a platform to programmatically author, schedule, and monitor workflows. Its UI includes DAG overview, asset dependencies, grid timelines, graph views with per-run status, environment summaries, backfill views, and source-code viewing.

**Distinctive components:**

- DAG dependency graph.
- Per-run status projection.
- Code view for workflow definitions.
- Time-oriented run grid next to graph-oriented dependency views.

**Wardian relevance:** Airflow is relevant for combining source-defined workflows with graph and run-state projections. Wardian should similarly show definitions, graph structure, current run status, historical run evidence, and logs without splitting them into unrelated screens.

### Dagster

**Source basis:** Public repo/docs and UI scaling material checked.

**What it includes:** Dagster is an orchestration platform for developing, producing, and observing data assets. It emphasizes software-defined assets, asset dependency graphs, materialization status, and scalable graph visualization for large asset sets.

**Distinctive components:**

- Asset graph rather than only task graph.
- Dependency visualization.
- Materialization and freshness status.
- Large graph scaling work.
- Code-defined assets and UI observation.

**Wardian relevance:** Dagster's asset graph is relevant because Wardian workflows should not only show tasks; they should show produced artifacts, files, PRs, reports, memories, and skill outputs. Asset-centered graphs may be more useful than task-only graphs for long agent work.

### Temporal UI

**Source basis:** Public docs and repo checked.

**What it includes:** Temporal UI is used to inspect durable workflow executions: running workflows, workflow history, retries, failures, and execution details for long-running business logic.

**Distinctive components:**

- Durable workflow execution inspection.
- Event history and replay-oriented debugging.
- Long-running execution visibility.
- Retry/failure state inspection.

**Wardian relevance:** Temporal is relevant where Wardian workflows become long-lived and retryable. A node graph is not enough; Wardian needs durable execution history and a way to inspect what the engine believed at each step.

### Prefect

**Source basis:** Public docs/API reference checked.

**What it includes:** Prefect exposes flow run dashboards and APIs for flow run graphs, task runs, subflow runs, artifacts, and graph limits. It treats flow-run graphs as an API-backed view with max-node and max-artifact settings.

**Distinctive components:**

- Flow and flow-run graph APIs.
- Task/subflow run graph APIs.
- Artifact display tied to run graph.
- Limits for large run graphs.
- Flow visualization through Graphviz in SDK contexts.

**Wardian relevance:** Prefect is useful for implementation discipline. Wardian should treat graph visualization as a bounded API problem: max nodes, changed-since queries, artifacts, and progressive loading matter once agent workflows get large.

### Jaeger UI

**Source basis:** Public repo checked.

**What it includes:** Jaeger UI visualizes distributed traces collected by Jaeger. It supports trace search and trace detail views around spans, services, timing, and causality.

**Distinctive components:**

- Trace search.
- Trace detail and span timing views.
- Distributed causality inspection.
- Integration with tracing backends and OpenTelemetry pipelines.

**Wardian relevance:** Wardian workflow and agent runs are effectively distributed traces across humans, agents, tools, terminals, and files. Jaeger-style views are relevant for "why did this run take so long?", "which agent blocked the flow?", and "which node caused this failure?"

## Wardian Positioning

Wardian should treat graph surfaces as projections over specific relationship models:

```text
workflow definition graph -> workflow run graph -> agent communication graph -> trace/detail graph
```

These views can share event data and entity IDs, but they should not share one universal interaction model. A workflow editor, agent communication map, dependency graph, and trace view each answer different questions.

Near-term implications:

- Keep the workflow builder graph separate from large-scale communication/trace graphs.
- Add graph exports for docs and specs.
- Design graph node drill-downs to PTY output, logs, artifacts, approvals, skills, files, and run history.
- Use status overlays consistently: processing, idle, action required, failed, stale, unknown.
- Add API limits and progressive loading before graph data grows large.
- Preserve dense list and keyboard workflows alongside visual graphs.
