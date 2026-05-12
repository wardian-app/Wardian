# Agent Evaluation References

This document maps public agent evaluation, benchmark, observability, and security-testing systems to design patterns relevant to Wardian's verification strategy, workflow run evidence, provider comparison, and agent safety posture.

This is not an endorsement, affiliation claim, product evaluation, or competitive teardown. The notes below describe public architecture and design pressure only.

Last reviewed: 2026-05-12.

Source basis: entries were selected through public research and checked against public project pages, repositories, papers, or documentation where available.

## Design Axes

- **Task success**: whether an agent actually completed the requested work, not just produced plausible text.
- **Trajectory evidence**: whether steps, tool calls, file changes, terminal output, and decisions are captured for replay.
- **Verifier model**: whether scoring is deterministic, test-based, rubric-based, LLM-judged, or human-reviewed.
- **Security robustness**: whether prompt injection, data exfiltration, unsafe commands, and credential misuse are tested.
- **Regression loop**: whether production failures can become repeatable evals.
- **Provider comparability**: whether different agents/providers can be measured under the same task harness.
- **Workflow observability**: whether evaluation data can map onto Wardian workflow nodes and run history.

## Summary Map

| System | Primary Pattern | Wardian Takeaway |
|---|---|---|
| [Inspect](https://inspect.aisi.org.uk/) | General evaluation framework with agent and tool support. | Strong reference for Wardian's own provider/workflow evaluation harness. |
| [SWE-bench](https://github.com/swe-bench) | Software engineering issue-resolution benchmark. | Useful for coding-agent task realism and reproducible environment discipline. |
| [tau-bench](https://sierra.ai/uk/resources/research/tau-bench) | Tool-agent-user benchmark for dynamic real-world interactions. | Useful for evaluating multi-turn tool use and policy adherence. |
| [Claw Bench](https://github.com/claw-bench/claw-bench) | Real agent benchmark with pytest verifiers across many domains. | Directly relevant to agent-run scoring and skill-like task instructions. |
| [AgentDojo](https://arxiv.org/abs/2406.13352) | Prompt-injection attack and defense benchmark for tool-using agents. | Strong reference for treating tool outputs and web content as untrusted. |
| [Phoenix](https://github.com/Arize-ai/phoenix) | Open-source AI observability and eval platform. | Reference for trace-to-debug workflows and OpenTelemetry alignment. |
| [Helicone](https://github.com/Helicone/helicone) | LLM observability, cost, prompt, and request analytics. | Useful for model/provider cost and request visibility. |
| [OpenLLMetry](https://github.com/traceloop/openllmetry) | OpenTelemetry instrumentation for LLM providers and vector DBs. | Strong reference for emitting portable telemetry instead of bespoke logs. |
| [Future AGI](https://github.com/future-agi/future-agi) | End-to-end tracing, evals, simulation, gateway, and guardrails. | Useful as a broad platform reference for trace/eval/guardrail convergence. |
| [Braintrust](https://www.braintrust.dev/) | Trace-to-eval production workflow. | Relevant for turning Wardian failures into repeatable regression cases. |

## Reference Profiles

### Inspect

**Source basis:** Public docs checked.

**What it includes:** Inspect is an open-source framework for frontier AI evaluations from the UK AI Security Institute and Meridian Labs. It supports coding, agentic tasks, reasoning, knowledge, behavior, multimodal understanding, tool calling, built-in agents, multi-agent evaluation, external agents, and human baselines.

**Distinctive components:**

- Reusable evaluation interfaces.
- Built-in tools such as bash, Python, editing, browsing, and computer tools.
- Agent Bridge for third-party agent frameworks.
- Inspect View for monitoring and visualizing evaluations.
- Multi-agent and custom-agent evaluation patterns.

**Wardian relevance:** Inspect is a relevant reference for Wardian's future verification layer. Wardian can use similar concepts internally: task definitions, solvers/providers, tools, scorers, logs, and replayable evidence.

### SWE-bench

**Source basis:** Public organization/repo checked.

**What it includes:** SWE-bench evaluates AI systems on real GitHub issues. The broader SWE-* ecosystem includes SWE-agent, SWE-smith, experiments, leaderboards, environment artifacts, and supporting infrastructure.

**Distinctive components:**

- Real-world software engineering tasks.
- Reproducible environment setup.
- Patch/test-oriented scoring.
- Public trajectories and experiment data.
- Agent harnesses and small reference agents.

**Wardian relevance:** SWE-bench matters less as a benchmark Wardian must run and more as a quality bar. Wardian workflows that claim to validate coding work need environment setup, tests, artifacts, and failure evidence that are as explicit as benchmark harnesses.

### tau-bench

**Source basis:** Public research page checked.

**What it includes:** tau-bench evaluates conversational AI agents in realistic domains with dynamic user and tool interaction. It is aimed at measuring performance and reliability in real-world settings rather than static answer quality.

**Distinctive components:**

- Simulated user-agent-tool interactions.
- Domain-specific policies and APIs.
- Multi-turn task completion.
- Reliability focus.

**Wardian relevance:** tau-bench is relevant for Wardian workflows that involve humans, tools, and policies. Wardian should eventually test not only "did the agent edit code?" but also "did the agent follow policy, ask when needed, and use tools correctly over several turns?"

### Claw Bench

**Source basis:** Public repo checked.

**What it includes:** Claw Bench evaluates real AI agent products directly across hundreds of curated tasks and domains. Agents read task instructions, complete the work, and submit results. Scoring uses pytest verifiers with weighted checks and dimensions such as efficiency, security, skills, and UX.

**Distinctive components:**

- Real agent execution rather than adapter-only scoring.
- Task library across many domains.
- Pytest verifiers with weighted checks.
- Dimension scoring.
- Public leaderboard and anti-abuse validation.
- Agent-readable skill/task instructions.

**Wardian relevance:** Claw Bench is very relevant to Wardian because it resembles what Wardian should produce for itself: task files, run artifacts, deterministic verifiers, weighted checks, and a visible record of how a provider/class performed.

### AgentDojo

**Source basis:** Public paper checked.

**What it includes:** AgentDojo is an evaluation framework for prompt-injection attacks and defenses in tool-using agents. It includes realistic tasks, security test cases, and attack/defense paradigms around untrusted external tool data.

**Distinctive components:**

- Realistic tool-using tasks such as email, banking, and travel.
- Prompt-injection attack cases.
- Defense evaluation.
- Extensible environment for new attacks and tasks.
- Emphasis on tool outputs as untrusted data.

**Wardian relevance:** This should influence Wardian's runtime model. Any workflow node that reads web pages, GitHub issues, emails, docs, or tool output should preserve a trust boundary between user instructions, agent reasoning, and untrusted data.

### Phoenix

**Source basis:** Public repo/docs checked.

**What it includes:** Phoenix is an open-source AI observability and evaluation platform. Its public positioning emphasizes OpenTelemetry/OpenInference instrumentation, tracing, debugging, datasets, evals, and integration with common LLM frameworks.

**Distinctive components:**

- OpenTelemetry-compatible tracing.
- OpenInference instrumentation.
- LLM, tool, and retrieval traces.
- Evaluation and debugging workflows.
- Local and hosted usage patterns.

**Wardian relevance:** Phoenix is useful as an observability reference. Wardian's workflow run history should be exportable or mappable to trace concepts: spans, tool calls, attributes, errors, timing, cost, and artifacts.

### Helicone

**Source basis:** Public repo checked.

**What it includes:** Helicone is an open-source LLM observability platform covering monitoring, analytics, evaluation, prompt management, cost visibility, and provider integrations.

**Distinctive components:**

- Provider request monitoring.
- Cost and model pricing data.
- Prompt and experiment tooling.
- Integrations with LLM frameworks and providers.
- Data export and MCP-related data access.

**Wardian relevance:** Helicone is relevant to Wardian's provider/runtime accounting. Users supervising many agents need cost, token, latency, and provider error visibility alongside PTY status and workflow node state.

### OpenLLMetry

**Source basis:** Public repo checked.

**What it includes:** OpenLLMetry provides OpenTelemetry-based instrumentation for LLM providers and vector databases, plus SDK support for emitting standard telemetry to existing observability stacks.

**Distinctive components:**

- Standard OpenTelemetry output.
- Instrumentations for providers and vector databases.
- SDK-based adoption.
- Compatibility with existing observability tooling.

**Wardian relevance:** Wardian should avoid inventing an isolated telemetry format for everything. A local event model can be Wardian-native, but exports should align with OpenTelemetry concepts where possible.

### Future AGI

**Source basis:** Public repo checked.

**What it includes:** Future AGI is an open-source platform spanning simulation, evaluation, guardrails, OpenTelemetry-native tracing, gateway/routing, optimization, datasets, and dashboards.

**Distinctive components:**

- Multi-turn simulation.
- Evaluation metrics and guardrail scanners.
- OpenTelemetry-native tracing.
- OpenAI-compatible gateway.
- Provider routing, caching, and virtual keys.
- Trace-to-optimization loops.

**Wardian relevance:** Future AGI is useful as a convergence reference: observability, evaluation, gateway control, and guardrails are moving toward one platform layer. Wardian should keep these concerns separate internally but let them meet in run history and workflow evidence.

### Braintrust

**Source basis:** Public product/research material checked.

**What it includes:** Braintrust focuses on evaluations, traces, datasets, prompt iteration, and production feedback loops for AI systems.

**Distinctive components:**

- Trace-to-eval workflow.
- Dataset and experiment management.
- CI-style evaluation gates.
- Production failure capture.

**Wardian relevance:** Braintrust's important pattern is not vendor-specific: production failures should become reusable test cases. Wardian should make it easy to turn a bad agent run into a regression fixture for the relevant provider, class, skill, or workflow.

## Wardian Positioning

Wardian should make verification visible at the same level as execution:

```text
task/run -> observed trajectory -> artifacts -> verifier/evaluator -> score and regression case
```

Near-term implications:

- Store run evidence in a structured form, not only terminal transcripts.
- Attach verifiers to workflow nodes where possible.
- Treat "agent got stuck" as an evaluable failure mode.
- Track provider/class/skill performance over repeatable tasks.
- Preserve trust boundaries for untrusted tool outputs.
- Allow a failed run to become a replayable regression case.
