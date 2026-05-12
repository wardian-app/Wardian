# Dashboard Observability References

This document maps public complex-system dashboards, Kubernetes/operator UIs, monitoring tools, and trace dashboards to design patterns relevant to Wardian's dashboard view, command-center telemetry, agent roster, watchlists, and drill-down monitoring paths.

This is not an endorsement, affiliation claim, product evaluation, or competitive teardown. The notes below describe public architecture and design pressure only.

Last reviewed: 2026-05-12.

Source basis: entries were selected through public research and checked against public project pages, repositories, or documentation where available.

## Design Axes

- **Operator surface**: dashboard, resource browser, dense table, topology summary, trace search, log viewer, or alert panel.
- **Status projection**: how health, freshness, ownership, stuckness, current execution state, and incident severity are summarized.
- **Drill-down path**: how users move from overview to resource details, logs, terminal output, metrics, traces, events, files, and actions.
- **Information density**: whether the UI supports repeated monitoring at scale without forcing decorative or low-density cards.
- **Layout persistence**: whether dashboards and views can be saved, provisioned, versioned, or reproduced across machines.
- **Action locality**: whether pause, restart, inspect, filter, port-forward, logs, or other actions sit near the monitored object.
- **Dashboard scope**: whether the surface is cluster-wide, team-wide, workflow-wide, resource-specific, or time-window-specific.

## Summary Map

| System | Primary Pattern | Wardian Takeaway |
|---|---|---|
| [Headlamp](https://github.com/kubernetes-sigs/headlamp) | Extensible Kubernetes web UI. | Reference for resource trees, plugin-driven panes, and operator workflows. |
| [Octant](https://octant.dev/) | Developer-centric Kubernetes dashboard. | Useful for related-object views, live logs, filtered resources, and extensibility. |
| [K9s](https://github.com/derailed/k9s) | Keyboard-first Kubernetes TUI. | Reference for dense, fast, repeated monitoring without losing command access. |
| [Grafana](https://grafana.com/) | Provisionable monitoring dashboards. | Reference for dashboard-as-code, panel composition, variables, alerts, and time-series drill-down. |
| [Jaeger UI](https://github.com/jaegertracing/jaeger-ui) | Distributed trace search and trace detail UI. | Relevant to Wardian run traces, causality, and time-window investigation. |
| [Kiali](https://kiali.io/) | Service-mesh observability dashboard. | Reference for combining topology, health, metrics, traces, and external dashboard links. |
| [Airflow](https://github.com/apache/airflow) | Workflow monitoring dashboard with DAG and run status. | Reference for joining run grids, environment summaries, code view, and per-run details. |
| [Temporal UI](https://docs.temporal.io/web-ui/) | Durable workflow execution inspection. | Reference for long-running execution history, retries, and failure evidence. |
| [Prefect](https://github.com/PrefectHQ/prefect) | Flow-run dashboard and task/artifact observability. | Reference for API-backed run visibility with graph, artifact, and size limits. |

## Reference Profiles

### Headlamp

**Source basis:** Public repo checked.

**What it includes:** Headlamp is a Kubernetes web UI described as fully-featured, user-friendly, and extensible. Its public surface emphasizes Kubernetes debugging, monitoring, orchestration, dashboarding, and plugins.

**Distinctive components:**

- Resource browser for Kubernetes objects.
- Extensible plugin model.
- Cluster/operator dashboard posture.
- Debugging and monitoring orientation.

**Wardian relevance:** Headlamp is a command-center reference. Wardian has a similar problem shape: many live objects, nested resource kinds, status badges, logs, actions, plugins, and operator workflows. The dashboard should make the object graph navigable without hiding raw state.

### Octant

**Source basis:** Public project site checked.

**What it includes:** Octant is a developer-centric web interface for inspecting Kubernetes clusters and applications. It emphasizes visualization, extensibility, real-time updates, related-object views, filtering labels, streaming logs, port-forward actions, and gRPC plugins.

**Distinctive components:**

- Related-object views.
- Real-time cluster updates.
- Plugin API.
- Label filtering and log streaming.
- Developer debugging actions such as port forwarding.

**Wardian relevance:** Octant's related-object model maps well to Wardian's dashboard: from an agent, a user should be able to reach workspace, PTY, workflow run, skills, files changed, logs, messages, approvals, and errors. Dashboard cards and tables should be navigation into concrete evidence.

### K9s

**Source basis:** Public repo checked at a high level.

**What it includes:** K9s is a terminal UI for managing and debugging Kubernetes clusters. It is known for dense keyboard-driven navigation over live resource tables, logs, describes, and actions.

**Distinctive components:**

- Keyboard-first live resource navigation.
- Dense tabular status views.
- Fast switching between contexts and resources.
- Logs and command actions near the resource list.
- TUI posture for repeated operator use.

**Wardian relevance:** K9s is relevant because a serious Wardian dashboard needs dense lists, fast keyboard control, filters, and logs at hand. The dashboard view should not become a slow, ornamental overview that sends operators elsewhere for every action.

### Grafana

**Source basis:** Public docs checked.

**What it includes:** Grafana is a monitoring/dashboard platform. Its provisioning docs show dashboards and data sources managed through files, making dashboards version-controllable and reproducible.

**Distinctive components:**

- Panel-based dashboards.
- Data-source abstraction.
- JSON/YAML dashboard definitions.
- File-based provisioning and GitOps workflows.
- Time range, variables, drill-downs, and alerts.

**Wardian relevance:** Grafana is a reference for dashboard-as-code. Wardian dashboard layouts, watchlists, and observability panels should be saveable as files, reviewed in specs, and recreated across machines where practical.

### Jaeger UI

**Source basis:** Public repo checked.

**What it includes:** Jaeger UI visualizes distributed traces collected by Jaeger. It supports trace search and trace detail views around spans, services, timing, and causality.

**Distinctive components:**

- Trace search.
- Trace detail and span timing views.
- Distributed causality inspection.
- Integration with tracing backends and OpenTelemetry pipelines.

**Wardian relevance:** Wardian dashboard drill-downs should answer operational questions such as "why did this run take so long?", "which agent blocked the flow?", and "which step caused this failure?" Jaeger-style trace search and detail views are relevant once the dashboard links into run evidence.

### Kiali

**Source basis:** Public site and docs checked.

**What it includes:** Kiali is a service-mesh observability and configuration dashboard. It integrates service topology, health, metrics, tracing, and links to systems such as Grafana and Jaeger.

**Distinctive components:**

- Service topology summary.
- Health overlays.
- Metrics and tracing integration.
- Ownership/custom information patterns.
- Links out to deeper dashboards.

**Wardian relevance:** Kiali is useful for the dashboard-to-detail pattern: a high-level system view should show health and relationships while preserving fast links into metrics, traces, configuration, and ownership.

### Airflow

**Source basis:** Public repo checked.

**What it includes:** Airflow is a platform to programmatically author, schedule, and monitor workflows. Its UI includes DAG overview, asset dependencies, grid timelines, graph views with per-run status, environment summaries, backfill views, and source-code viewing.

**Distinctive components:**

- Grid view across time/runs.
- Per-run status projection.
- Environment and scheduling summaries.
- Code view for workflow definitions.
- Backfill and scheduling surfaces.

**Wardian relevance:** Airflow is relevant where Wardian's dashboard needs to summarize workflow health across many runs. The useful pattern is not only the DAG graph; it is the combination of current status, historical run grid, schedules, code, and logs.

### Temporal UI

**Source basis:** Public docs and repo checked.

**What it includes:** Temporal UI is used to inspect durable workflow executions: running workflows, workflow history, retries, failures, and execution details for long-running business logic.

**Distinctive components:**

- Durable workflow execution inspection.
- Event history and replay-oriented debugging.
- Long-running execution visibility.
- Retry/failure state inspection.

**Wardian relevance:** Temporal is relevant where Wardian dashboard cards need to represent long-lived, retryable work. A status badge is not enough; operators need durable execution history and a way to inspect what the engine believed at each step.

### Prefect

**Source basis:** Public docs/API reference checked.

**What it includes:** Prefect exposes flow run dashboards and APIs for flow run graphs, task runs, subflow runs, artifacts, and graph limits. It treats flow-run graphs as an API-backed view with max-node and max-artifact settings.

**Distinctive components:**

- Flow and flow-run dashboards.
- Task/subflow run graph APIs.
- Artifact display tied to run state.
- Limits for large run graphs and artifacts.
- API-backed run details.

**Wardian relevance:** Prefect is useful for dashboard implementation discipline. Wardian should treat dashboard data as bounded API output: changed-since queries, max rows, artifacts, and progressive loading matter once agent workflows and run histories get large.

## Wardian Positioning

Wardian's dashboard should be a monitoring surface, not a universal graph surface:

```text
system dashboard -> roster/watchlists -> focused detail panes -> logs/traces/artifacts/actions
```

The dashboard should summarize active agents, stuck work, resource pressure, recent failures, approvals, workflow runs, schedules, and provider health. Graph surfaces can be linked from the dashboard, but the dashboard itself should prioritize fast scanning, filtering, and drill-down.

Near-term implications:

- Keep dashboard view separate from workflow graph editing and communication graph exploration.
- Use dense rows, tables, and compact panels where they beat large cards.
- Put common actions near the monitored object: pause, resume, inspect, jump to terminal, open logs, open run details.
- Treat dashboard layouts and watchlists as versionable artifacts where practical.
- Preserve direct drill-downs from dashboard summaries into PTY output, logs, artifacts, approvals, skills, files, and run history.
- Add API limits, time windows, and changed-since queries before dashboard data grows large.
- Keep keyboard navigation and filtering central to repeated operator use.
