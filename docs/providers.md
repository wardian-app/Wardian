# Provider Runtimes & Nuances

Wardian provides a unified orchestration layer over multiple, disparate AI agent CLIs. Each provider has unique requirements for session management, skill discovery, and status reporting.

## Overview

| Provider | Status | Primary Communication | Skill Mechanism |
| :--- | :--- | :--- | :--- |
| **Gemini CLI** | Stable | JSON Events over PTY | `--include-directories` |
| **Claude Code** | Stable | Permission Hooks + PTY | `.claude/skills` symlinks |
| **Codex** | Beta | Thread Logs + PTY | Habitat Injection |
| **OpenCode** | Planned | To Be Determined. | To Be Determined. |
| **OpenClaw** | Planned | Markdown Soul / IPC | Autonomous Orchestration |

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
## Planned: OpenCode & OpenClaw

Wardian is expanding its orchestration capabilities by integrating two emerging industry standards:

- **OpenCode**: Integration details are To Be Determined.
- **OpenClaw**: A self-hosted orchestration gateway. Wardian will use OpenClaw nodes for persistent, 24/7 background agents that handle long-running monitoring tasks, scheduled workflows, and cross-platform notification routing.
