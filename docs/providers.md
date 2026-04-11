# Provider Runtimes & Nuances

Wardian provides a unified orchestration layer over multiple, disparate AI agent CLIs. Each provider has unique requirements for session management, skill discovery, and status reporting.

## Overview

| Provider        | Status  | Primary Communication                      | Skill Mechanism                          |
| :-------------- | :------ | :----------------------------------------- | :--------------------------------------- |
| **Gemini CLI**  | Stable  | JSON Events over PTY                       | `--include-directories`                  |
| **Claude Code** | Stable  | Permission Hooks + PTY                     | `.claude/skills` symlinks                |
| **Codex**       | Beta    | Thread Logs + PTY                          | Habitat Injection                        |
| **OpenCode**    | Beta    | JSON Events over PTY / headless run output | Injected `instructions` + `skills.paths` |
| **OpenClaw**    | Planned | To be determined                           | To be determined                         |

---

## Gemini CLI (`@google/gemini-cli`)

Gemini is integrated as a high-performance, stream-oriented provider.

### Implementation Nuance: Turn Completion

Unlike primitive terminal wrappers, Wardian doesn't wait for a "Done" string. It parses the underlying provider stream for formal `AgentEvent::TurnCompleted` signals to ensure the workflow engine moves to the next node immediately.

### Skill Discovery

Wardian patches the Gemini CLI's discovery logic to respect multiple `--include-directories`. This allows skills to be injected from the global common library, the agent's specific class, or the individual session's local folder.

---

## Claude Code (`@anthropic-ai/claude-code`)

Claude is integrated with a focus on governance and human-in-the-loop (HITL) precision.

### Implementation Nuance: Permission Hooks

Claude often requests user approval for filesystem or network actions. Wardian implements a custom hook that writes these requests to a JSONL file under `.wardian/agents/<session_id>/claude/`. The UI monitors this file to transition the agent state to `Action Needed` and surface the specific request.

### Workspace Isolation

Wardian enables `CLAUDE_CODE_ADDITIONAL_DIRECTORIES_CLAUDE_MD=1`, allowing Claude to read instruction files (`CLAUDE.md`) from assigned skill roots without them being physically present in the root of your project.

---

## Codex

Codex uses a "Habitat" model for maximum isolation.

### Implementation Nuance: Bootstrap Migration

Codex session IDs are discovered after launch. Wardian starts Codex in a temporary `.wardian/provider-bootstrap` directory to discover its ID, then migrates the entire session state into a permanent final habitat.

### Trust Binding

Wardian ensures that even though state is isolated, the "trust" and execution root remain bound to the real project workspace via the `--cd` flag, preventing agents from getting lost in a virtualized path.

---

## OpenCode

OpenCode runs directly in the real target workspace and consumes `AGENTS.md` natively.

### Implementation Nuance: Runtime Config Injection

Instead of projecting a fake workspace, Wardian injects extra instruction files and skill roots through `OPENCODE_CONFIG_CONTENT`. This lets OpenCode see:

- `AGENTS.md` files from Wardian common/class/agent roots
- `.agents/skills` directories from those same roots

without forcing those files into the repository itself.

### Session Model

OpenCode session IDs are discovered from JSON events emitted by `opencode run --format json`. Wardian captures the first `sessionID` from the provider output, then reuses it with `--session` for later resumes and headless follow-up runs.

## Planned: OpenClaw

Wardian is expanding its orchestration capabilities by integrating two emerging industry standards:

- **OpenClaw**: A self-hosted orchestration gateway. Wardian will use OpenClaw nodes for persistent, 24/7 background agents that handle long-running monitoring tasks, scheduled workflows, and cross-platform notification routing.
