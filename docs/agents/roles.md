# Agent Roles and Responsibilities

The system is built on a **Modular Multi-Agent Architecture** where each agent is assigned a specialized "Role" (defined by its prompt class). This separation allows for high-governance orchestration and deterministic task execution.

## 🏛️ Default Roles

### 1. Generalist (The Polymath)
- **Core Mission**: To provide versatile support across a wide range of tasks without specialized role constraints.
- **Common Tasks**: General inquiries, basic script writing, and cross-domain reasoning.

### 2. Coder (The Implementer)
- **Core Mission**: To translate specifications into high-quality, idiomatic source code.
- **Common Tasks**: Feature implementation, bug fixing, and refactoring.

### 3. Reviewer (The Auditor)
- **Core Mission**: To rigorously audit code and plans for security, performance, and best practices.
- **Common Tasks**: Security audits, code review, and forensic verification.

### 4. QA (The Tester)
- **Core Mission**: To ensure software reliability through comprehensive testing and validation.
- **Common Tasks**: Automated test writing, bug hunting, and manual verification plans.

### 5. Architect (The Designer)
- **Core Mission**: To design robust, scalable system blueprints and technical specifications.
- **Common Tasks**: System mapping, dependency analysis, and implementation planning.

### 6. Orchestrator (The Manager)
- **Core Mission**: To delegate tasks and manage multi-agent workflows.
- **Common Tasks**: Task decomposition, progress monitoring, and output synthesis.

### 7. Evolver (The Optimizer)
- **Core Mission**: To iteratively optimize systems, workflows, and agent instructions.
- **Common Tasks**: Performance tuning, recursive instruction optimization, and bottleneck analysis.

### 8. Researcher (The Investigator)
- **Core Mission**: To gather data and perform deep investigative synthesis on complex topics.
- **Common Tasks**: Market research, technical deep-dives, and comparative analyses.

### 9. Editor (The Polisher)
- **Core Mission**: To refine written content for clarity, tone, and structural flow.
- **Common Tasks**: Copy editing, documentation refinement, and content strategy.

### 10. Personal Assistant (The Support)
- **Core Mission**: To provide administrative and organizational support.
- **Common Tasks**: Scheduling, email drafting, and task management.

## 🛠️ Customizing Roles

Roles are defined as standard Markdown files. When a new **Agent Class** is created, it resides in a dedicated directory within the application's configuration path, primarily managed via `AGENTS.md` as the master instruction file.

Wardian adapts that role file to each supported provider. Codex and OpenCode consume `AGENTS.md` directly. Gemini and Claude receive provider-native instruction files that point back to the same Wardian role context. When a provider needs additional runtime structure, Wardian projects or injects the class and skill scope through the provider-specific path described in [Provider Runtimes](../providers.md).

### Instruction Precedence:

1. **`AGENTS.md`**: The foundation for the agent's identity and core instructions.
2. **Project Context**: Global project-wide mandates (architecture, style, conventions) often provided in a root-level documentation file.
3. **Session Context**: The immediate task-specific instructions from the user.
