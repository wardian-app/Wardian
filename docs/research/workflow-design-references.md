# Workflow Design References

This document maps public workflow and agent-orchestration systems to design patterns relevant to Wardian's workflow builder and runtime.

This is not an endorsement, affiliation claim, product evaluation, or competitive teardown. The notes below describe public architecture and design pressure only.

Last reviewed: 2026-05-12.

Source basis: Orla and Archon were rechecked against their public repositories because they anchor the "code as construction, graph as observability" framing. Other entries are prior-art references based on public project documentation and repositories.

## Design Axes

- **Code-first construction**: workflows can be created, reviewed, versioned, and modified by agents or developers as text.
- **Observable graph projection**: workflow state can be inspected as nodes, edges, ports, and execution status.
- **Durable execution**: runs can tolerate retries, pauses, resumes, and long-lived state.
- **Human-in-the-loop control**: humans can approve, interrupt, redirect, or inspect execution without losing the underlying run context.
- **Trace and replay evidence**: execution leaves enough telemetry to understand what ran, why it ran, and what each step produced.

## Summary Map

| System | Relevant Pattern | Wardian Takeaway |
|---|---|---|
| [Orla](https://github.com/harvard-cns/orla) | Code-first stage execution with scheduling, routing, and shared inference state. | Keep workflow policy separate from provider/runtime mechanics so orchestration can evolve without tying the builder to one execution backend. |
| [Archon](https://github.com/coleam00/Archon) | Hybrid workflow construction with durable text definitions and visual execution surfaces. | Treat code or structured files as the construction surface, then project that model into a human-observable graph. |
| [Apache Burr](https://github.com/apache/burr) | State-machine workflows with a UI for tracking execution state and transitions. | Model workflow progress as explicit state transitions that can be logged, inspected, and resumed. |
| [LangGraph](https://github.com/langchain-ai/langgraph) | Stateful agent graphs with persistence, streaming, and human-in-the-loop patterns. | Agent workflows need first-class state, checkpoints, and interruption points rather than only linear task chains. |
| [Mastra](https://github.com/mastra-ai/mastra) | TypeScript-first workflow API with steps, branches, parallelism, and durable pauses. | A fluent or typed construction API can coexist with a graph view when both compile to the same workflow model. |
| [CrewAI](https://github.com/crewAIInc/crewAI) | Distinguishes autonomous agent crews from more deterministic flows. | Wardian should preserve the distinction between agent autonomy and deterministic workflow control. |
| [Dify](https://github.com/langgenius/dify) | Visual workflow and agent builder with node-level observability. | Node-first workflows are approachable for humans, but Wardian should still expose a text-native model for agent construction. |
| [n8n](https://github.com/n8n-io/n8n) | Mature visual automation graph with code escape hatches. | Visual authoring benefits from predictable node contracts, searchable blocks, and inspectable run history. |
| [ComfyUI](https://github.com/Comfy-Org/ComfyUI) | Node-first generative AI pipelines with visible dataflow. | Dense node graphs can stay understandable when ports, typed edges, previews, and execution feedback are clear. |
| [Dagger](https://github.com/dagger/dagger) | Code-defined DAGs with observable execution traces. | "Code as construction, graph as observability" is a strong fit for agent-authored Wardian workflows. |
| [Temporal](https://github.com/temporalio/temporal) | Durable workflow execution with history, retries, and a web UI. | Long-running workflows need execution history and replayable evidence, not only live status indicators. |
| [Pydantic AI](https://github.com/pydantic/pydantic-ai) | Typed agent and graph construction with structured outputs. | Typed schemas reduce ambiguity when agents create workflow definitions or pass outputs between nodes. |

## Reference Profiles

### Orla

**Source basis:** Public repo checked.

**What it includes:** Orla is a Go library with Python bindings for building and running LLM-based agentic systems. Its public architecture centers on workflows defined as stages, with runtime components for model/backend routing, stage scheduling and execution, and shared inference state.

**Distinctive components:**

- Stage Mapper for routing stages across heterogeneous models and backends.
- Workflow Orchestrator for scheduling and executing stages.
- Memory Manager for sharing KV-cache or inference state across stages.

**Wardian relevance:** Orla is a useful reference for separating workflow-level intent from provider execution mechanics. Wardian should preserve that separation: workflow definitions should describe agent, command, branch, wait, and memory semantics, while the Rust backend resolves provider, PTY, headless, and scheduling details.

### Archon

**Source basis:** Public repo checked.

**What it includes:** Archon is a workflow engine and harness builder for AI coding agents. It defines coding processes as YAML workflows under `.archon/workflows/`, supports deterministic and AI-driven nodes, runs jobs in isolated git worktrees, and exposes workflow activity through CLI, web UI, and platform adapters.

**Distinctive components:**

- YAML workflow definitions that encode planning, implementation, validation, review, approval, and PR steps.
- Visual workflow builder and workflow execution view.
- Isolation environments for parallel worktree-based runs.
- Platform adapters for CLI, web, Slack, Telegram, GitHub, and Discord.
- Built-in development workflows for issue fixing, idea-to-PR, review, validation, conflict resolution, and refactoring.

**Wardian relevance:** Archon is a relevant reference for "code as construction, nodes for observability." Wardian can use the same broad posture while staying local-first: durable workflow files should be agent-editable, and the UI should project those files into a tactile graph with run state, logs, approvals, and history.

### Apache Burr

**What it includes:** Burr is a Python library for applications that make decisions, including chatbots, agents, simulations, and other stateful systems. Applications are expressed as state machines or flowcharts composed from Python actions, with transitions, persisted state, and a telemetry UI.

**Distinctive components:**

- Low-abstraction action functions that read and write explicit state.
- State-machine graph model rather than only acyclic DAGs.
- UI for real-time monitoring, tracing, and debugging.
- Pluggable persisters for saving and loading application state.
- Framework-agnostic integration posture for LLMs, storage, telemetry, and other libraries.

**Wardian relevance:** Burr's state-machine framing fits Wardian's loops, waits, and pulse-based execution better than a pure DAG framing. Wardian should make state transitions visible: why a node became runnable, which dependency pulse was consumed, what state changed, and where execution can resume.

### LangGraph

**What it includes:** LangGraph is a low-level orchestration framework for building long-running, stateful agents. It emphasizes durable execution, human-in-the-loop interruptions, memory, deployment, and debugging through the broader LangChain/LangSmith ecosystem.

**Distinctive components:**

- Durable agent execution that can resume after failures.
- Human-in-the-loop mechanisms for inspecting and modifying state.
- Short-term and long-term memory patterns.
- Execution-path and state-transition visibility through LangSmith.
- Support for branching, subgraphs, streaming, persistence, and deployment patterns.

**Wardian relevance:** LangGraph highlights that agent workflows need durable checkpoints and human intervention points as first-class concepts. Wardian's workflow builder should avoid reducing agent orchestration to static boxes connected by arrows; each node needs state, interruption behavior, and resumability semantics.

### Mastra

**What it includes:** Mastra is a TypeScript framework for AI applications and agents. It includes model routing, agents, graph-based workflows, human-in-the-loop suspension, storage-backed execution state, context management, MCP servers, evals, and observability.

**Distinctive components:**

- TypeScript-first workflow syntax with `.then()`, `.branch()`, and `.parallel()`.
- Explicit split between autonomous agents and controlled workflows.
- Human-in-the-loop suspension that can pause indefinitely and resume from stored state.
- Model routing across many providers.
- Production tooling for evals and observability.

**Wardian relevance:** Mastra is a useful TypeScript-side reference for an eventual agent-authored construction API. Wardian can expose a builder-friendly structured model today and later layer a fluent or typed authoring surface on top, as long as both compile into the same normalized graph model.

### CrewAI

**What it includes:** CrewAI is a Python framework for multi-agent automation. It distinguishes between Crews, which emphasize autonomous role-based collaboration, and Flows, which emphasize event-driven execution control and structured state.

**Distinctive components:**

- Crews for role-based autonomous agent collaboration.
- Flows for precise event-driven orchestration.
- YAML configuration for agents and tasks in scaffolded projects.
- Human input support and control-plane observability in the broader CrewAI suite.
- Patterns for combining autonomous agent teams with deterministic flow control.

**Wardian relevance:** CrewAI's Crew versus Flow distinction is worth preserving conceptually. Wardian workflow nodes should make clear whether a step delegates autonomy to an agent/team or executes deterministic control logic owned by the workflow engine.

### Dify

**What it includes:** Dify is an open-source LLM application development platform. It combines AI workflows, RAG pipelines, agent capabilities, model management, observability integrations, and production deployment concerns behind an approachable visual interface.

**Distinctive components:**

- Visual AI workflow builder for LLM applications.
- RAG pipeline and dataset management surfaces.
- Agent, model, and prompt management in one product surface.
- Observability integrations such as Opik, Langfuse, and Arize Phoenix.
- Prototype-to-production orientation for LLM apps.

**Wardian relevance:** Dify shows how much of an AI system can be made observable through a visual builder. Wardian should borrow the clarity of node-level setup and execution feedback, but keep its source of truth local, text-inspectable, and friendly to agent edits.

### n8n

**What it includes:** n8n is a workflow automation platform with a visual editor, native AI capabilities, custom code support, self-hosting, and a large integration library.

**Distinctive components:**

- Visual node-based automation editor.
- JavaScript and Python escape hatches inside workflows.
- Native AI workflows based on LangChain and user-provided data/models.
- Large integration and template ecosystem.
- Self-hosting and enterprise deployment controls.

**Wardian relevance:** n8n is a mature example of visual workflows with code escape hatches. Wardian should apply that pattern in reverse for agents: text/code construction first, visual editing and observation always available, and predictable node contracts to keep both paths coherent.

### ComfyUI

**What it includes:** ComfyUI is a modular AI content creation engine with a graph/nodes interface. It supports image, video, audio, and 3D model workflows, local and cloud usage, API endpoints, workflow JSON, execution queues, and partial re-execution of changed graph regions.

**Distinctive components:**

- Dense node graph interface for complex generative pipelines.
- Workflow save/load as JSON, including workflows embedded in generated media.
- Asynchronous queue and history surfaces.
- Partial re-execution that only reruns changed workflow regions.
- App Mode for exposing sophisticated graphs through simpler user interfaces.

**Wardian relevance:** ComfyUI is not an agent workflow system, but it is a strong visual reference for complex graph ergonomics. Wardian should make graph density manageable with clear ports, labels, grouping, queue/history views, and execution feedback.

### Dagger

**What it includes:** Dagger is a programmable software delivery automation engine that runs locally, in CI, or in the cloud. It provides SDKs, reusable modules, containerized execution, caching, a CLI/TUI, and OpenTelemetry tracing.

**Distinctive components:**

- Code-defined automation instead of proprietary YAML.
- Multi-language SDKs and reusable modules.
- Local-first execution with consistent behavior across laptop, AI sandbox, CI, and cloud.
- Containerized, typed, repeatable operations with incremental caching.
- Built-in tracing with live CLI/TUI and OpenTelemetry export.

**Wardian relevance:** Dagger strongly validates the pattern "programmatic construction, observable execution." Wardian's equivalent should be workflow definitions and future APIs for construction, plus live telemetry, trace logs, and graph projection for observation.

### Temporal

**What it includes:** Temporal is a durable execution platform. Its server runs workflows and activities resiliently, handles intermittent failures and retries, and exposes CLI and web UI surfaces for interacting with workflow executions.

**Distinctive components:**

- Workflows and Activities as separate runtime concepts.
- Durable execution history for long-running application logic.
- Retry and failure handling as runtime behavior.
- Worker model with SDKs in supported languages.
- Web UI for inspecting executing workflows.

**Wardian relevance:** Temporal is the reference for durability discipline. Wardian does not need Temporal's distributed architecture to learn from its model: workflow runs should have durable identity, history, retry/failure semantics, and inspectable state.

### Pydantic AI

**What it includes:** Pydantic AI is a Python framework for production-grade agent applications. It emphasizes type safety, model-provider flexibility, observability, evals, composable capabilities, MCP/A2A/UI event-stream standards, tool approval, durable execution, streamed structured outputs, and graph support.

**Distinctive components:**

- Type-safe agent definitions and structured outputs.
- YAML/JSON agent definitions in addition to code.
- Human-in-the-loop tool approval.
- Durable agents for long-running and restart-tolerant workflows.
- Graph support backed by Python type hints.
- Observability through OpenTelemetry-compatible tooling.

**Wardian relevance:** Pydantic AI is useful for schema discipline. Wardian workflow definitions should keep node inputs, outputs, config, role assignments, and structured agent outputs typed enough that both humans and agents can safely compose them.

## Wardian Positioning

Wardian's direction is hybrid: agents and developers should be able to construct workflows through durable, reviewable definitions, while humans should be able to observe and edit those workflows as tactile node graphs.

The core pattern is:

```text
workflow definition -> normalized graph model -> Rust execution -> telemetry and run log -> observable node graph
```

This keeps the builder agent-friendly without making humans read raw workflow definitions during execution. It also keeps the visual graph honest: nodes are an observable projection of the workflow model, not a separate source of truth.
