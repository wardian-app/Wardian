# Agent Roles and Responsibilities

Wardian is built on a **Modular Multi-Agent Architecture** where each agent is assigned a specialized "Role" (defined by its prompt class). This separation allows for high-governance orchestration.

## 🏛️ Default Roles

### 1. Architect (The Designer)

- **Core Mission**: To design robust, scalable system blueprints.
- **Primary Tools**: `codebase_investigator` (System mapping), `glob` (Discovery), `read_file` (Deep analysis).
- **Common Tasks**: Mapping system dependencies, defining architectural patterns, and producing implementation plans.

### 2. Coder (The Implementer)

- **Core Mission**: To translate architectural plans into high-quality, idiomatic source code.
- **Primary Tools**: `replace` (Precise modification), `write_file` (Scaffolding), `run_shell_command` (Build/Lint/Format).
- **Common Tasks**: Implementing React components, writing Rust backend logic, and refactoring legacy functions.

### 3. Orchestrator (The Manager)

- **Core Mission**: To decompose high-level user objectives into specialized sub-tasks and manage the collective output of the agent swarm.
- **Common Tasks**: Breaking down a complex "Build an app" request into specific Architect, Coder, and QA tasks.

### 4. Evolver (The Tool-Maker)

- **Core Mission**: To extend Wardian's capabilities by creating new `.skill` files or optimizing existing ones.
- **Common Tasks**: Identifying recurring complex tasks and automating them via specialized agent skills.

### 5. Researcher / Reviewer (The Auditor)

- **Core Mission**: To perform deep investigative research and critical auditing of project plans, code, or external papers.
- **Common Tasks**: Competitive forensic analysis, methodology critique, and skeptical empirical auditing.

## 🛠️ Customizing Roles

Roles are defined in `src-tauri/agent_prompts/` as standard Markdown files. When a new **Agent Class** is created via the Wardian UI, it generates a corresponding directory in `~/.wardian/classes/<Role_Name>/` where the `GEMINI.md` (the primary instruction file) resides.

### Instruction Precedence:

1. **`~/.wardian/classes/<Role_Name>/GEMINI.md`**: The foundation for the agent's identity and core instructions.
2. **Project `GEMINI.md`**: Global project-wide mandates (architecture, style, conventions).
3. **Session Context**: The immediate task-specific instructions from the user.
