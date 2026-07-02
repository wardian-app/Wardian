# Agent Protocol and UI References

This document maps public agent interoperability and agent-generated UI systems to design patterns relevant to Wardian's workflow builder, local agent Habitat, agent-facing APIs, and human-observable control surfaces.

This is not an endorsement, affiliation claim, product evaluation, or competitive teardown. The notes below describe public architecture and design pressure only.

Last reviewed: 2026-05-12.

Source basis: entries were selected through public research and checked against public project pages, repositories, or specifications where available.

## Design Axes

- **Agent-to-tool protocol**: how agents invoke tools, read resources, or use external context.
- **Agent-to-agent protocol**: how independent agents discover each other, delegate, negotiate tasks, and exchange artifacts.
- **Agent-to-UI protocol**: how agents communicate progress, ask for input, update shared state, or generate safe UI surfaces.
- **Declarative construction**: whether agent systems can be described as portable text or structured specs.
- **Trust boundary**: whether the protocol treats remote agents, generated UI, and tool outputs as untrusted data.
- **Human observability**: whether protocol events can be projected into visible UI state, logs, approvals, and workflow nodes.

## Summary Map

| System | Primary Pattern | Wardian Takeaway |
|---|---|---|
| [Model Context Protocol](https://modelcontextprotocol.io/) | Tool/resource protocol for model applications. | Wardian should expose skills, workflows, memory, and app actions through structured protocols where practical. |
| [Agent2Agent / A2A](https://github.com/google-a2a/A2A) | Agent-to-agent discovery and task communication. | Useful reference for cross-agent delegation that does not depend on shared internals. |
| [OpenClaw ACPX](https://github.com/openclaw/acpx) | Headless client for stateful Agent Client Protocol sessions. | Relevant as a structured alternative to PTY scraping for coding agents. |
| [AG-UI](https://github.com/ag-ui-protocol/ag-ui) | Agent-user interaction protocol for frontend applications. | Strong reference for turning backend agent events into user-facing shared state and controls. |
| [A2UI](https://github.com/google/A2UI) | Declarative, safe agent-generated UI format. | Directly relevant to "agents construct, humans observe" without letting agents emit executable UI code. |
| [Open Agent Spec](https://github.com/oracle/agent-spec) | Declarative language for agents, workflows, and multi-agent systems. | Useful reference for portable agent/workflow definitions. |
| [agent.json](https://agent-json.com/) | Web discovery document for agent-facing services. | Useful later if Wardian exposes local or shared capabilities to external agents. |
| [LSP](https://microsoft.github.io/language-server-protocol/) and [DAP](https://microsoft.github.io/debug-adapter-protocol/) | Mature editor/tool protocols. | Good governance references for stable schemas, capabilities, versioning, and client/server separation. |

## Reference Profiles

### Model Context Protocol

**Source basis:** Public protocol site checked.

**What it includes:** MCP defines a structured way for AI applications to connect models to tools, resources, prompts, and external context. It is becoming a baseline integration protocol for agent tooling.

**Distinctive components:**

- Client/server protocol for tools and resources.
- Typed capabilities and tool schemas.
- Stdio and remote transport patterns.
- Growing ecosystem of MCP servers and clients.

**Wardian relevance:** Wardian should treat MCP as a first-class interoperability boundary. Skills, workspace files, workflow operations, agent roster state, and Habitat actions should be exposed through stable structured interfaces where useful, not only through UI clicks or terminal text.

### Agent2Agent / A2A

**Source basis:** Public spec and repo checked.

**What it includes:** A2A is an open protocol for communication and interoperability between independent agentic applications. Its goals include agent discovery, capability negotiation, collaborative task management, and structured exchange without requiring access to another agent's internal memory or tools.

**Distinctive components:**

- Agent cards and capability discovery.
- Task-oriented communication.
- Modality-agnostic exchange of text, files, structured data, and UI references.
- Peer-to-peer agent collaboration model.

**Wardian relevance:** Wardian's internal agent roster is local-first, but its long-term roadmap will need external agent interoperability. A2A is useful as a reference for describing what an agent can do without exposing all Wardian internals.

### OpenClaw ACPX

**Source basis:** Public repo and OpenClaw docs checked.

**What it includes:** ACPX is a headless CLI client for stateful Agent Client Protocol sessions. OpenClaw uses ACP concepts to run external coding harnesses through a structured backend plugin rather than treating every agent as an opaque terminal.

**Distinctive components:**

- Structured sessions over coding-agent harnesses.
- External adapters for different agent runtimes.
- Separation between agent runtime control and terminal text.
- OpenClaw integration with MCP and ACP-style session management.

**Wardian relevance:** Wardian currently values visible PTYs, but PTYs should not be the only control plane. ACP-style adapters point toward a future where Wardian can run agents visibly while also receiving structured status, messages, tool calls, and artifacts.

### AG-UI

**Source basis:** Public repo and protocol site checked.

**What it includes:** AG-UI is an Agent-User Interaction protocol for connecting user-facing applications with agentic backends. It emphasizes shared state, tool-based generative UI, agentic chat, human-in-the-loop interactions, and frontend/backend event flow.

**Distinctive components:**

- Bidirectional agent-to-application runtime.
- Client support across frontend platforms and community SDKs.
- Shared state and human-in-the-loop primitives.
- Examples for focused protocol building blocks.
- Terminal-and-agent client support.

**Wardian relevance:** AG-UI is relevant because Wardian is a visible local agent Habitat, not just an agent launcher. Workflow nodes, action-needed prompts, approvals, agent messages, tool calls, and generated controls could all be represented as structured UI events instead of bespoke ad hoc IPC messages.

### A2UI

**Source basis:** Public repo checked.

**What it includes:** A2UI is an early public-preview format for updatable agent-generated user interfaces. Agents send declarative JSON describing UI intent; the client maps those abstract components to a trusted component catalog and renders them natively.

**Distinctive components:**

- Declarative data format rather than executable generated code.
- Trusted host component catalog.
- Incrementally updatable component graph.
- LLM-friendly flat structure with ID references.
- Compatible with transports such as A2A and AG-UI.
- Renderers for web and Flutter-oriented clients.

**Wardian relevance:** A2UI is highly relevant to Wardian's workflow builder. Agents could propose workflow nodes, forms, inspectors, or approval panels as data, while Wardian decides how to render them using safe local components. This keeps construction agent-friendly without letting agents own the UI runtime.

### Open Agent Spec

**Source basis:** Public repo checked.

**What it includes:** Open Agent Spec is a framework-agnostic declarative language for defining standalone agents, structured agentic workflows, and multi-agent compositions.

**Distinctive components:**

- YAML/spec-oriented agent definitions.
- Reusable building blocks for agents and workflows.
- Multi-agent composition concepts.
- Framework-neutral positioning.

**Wardian relevance:** Wardian should track this class of declarative agent specs. Even if Wardian has its own workflow format, import/export adapters will matter if agent definitions become portable across tools.

### agent.json

**Source basis:** Public proposal site checked.

**What it includes:** `agent.json` proposes a `.well-known` discovery document that lets websites describe agent-facing capabilities, messaging, responses, and callbacks.

**Distinctive components:**

- Web discovery endpoint.
- Capability metadata.
- Agent-facing service description.
- Callback and message-oriented interaction framing.

**Wardian relevance:** This is a later-stage reference. If Wardian exposes local or network-visible capabilities, a discovery document pattern could let other agents identify available Wardian actions without custom setup.

### LSP and DAP

**Source basis:** Public protocol sites checked at a high level.

**What they include:** LSP and DAP are mature protocols that separate editor UIs from language servers and debugger implementations. They are not AI-agent protocols, but their capability negotiation, schema stability, and client/server boundaries are useful references.

**Distinctive components:**

- Capability negotiation.
- Stable JSON-RPC style contracts.
- Clear client/server ownership.
- Broad multi-editor ecosystem.
- Versioned protocol evolution.

**Wardian relevance:** Wardian should learn governance discipline from LSP and DAP. Agent protocols will churn; Wardian's internal IPC and CLI contracts should remain explicit, versioned, and capability-based.

## Wardian Positioning

Wardian should separate three surfaces:

```text
agent runtime protocol -> normalized Wardian event/state model -> human UI projection
```

PTYs remain important because humans need to see real agent sessions. But as protocols mature, Wardian should prefer structured events for status, tool calls, approvals, artifacts, and generated controls.

Near-term implications:

- Add protocol-adapter boundaries instead of hardcoding provider behavior into UI components.
- Treat generated UI as declarative data mapped through trusted Wardian components.
- Keep workflow and agent definitions import/export-friendly.
- Version internal event schemas like public protocols.
- Use PTY text as evidence, but not the only source of truth when structured events exist.
