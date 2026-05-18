# Key Features

This page maps Wardian's major capabilities to their detailed guides.

## Multi-Agent Command Center

- High-density Grid and Dashboard views for active session monitoring
- Left control rail for spawning, commands, workflows, explorer, and settings
- Right roster with status lights, thought snippets, and watchlists
- Queue tab for unread agent completions and workflow outcomes

Related docs:

- [UI Overview](./guide/ui-overview.md)
- [Getting Started](./guide/getting-started.md)
- [Grid](./guide/grid.md)
- [Dashboard](./guide/dashboard.md)
- [Watchlists](./guide/watchlists.md)
- [Queue](./guide/queue.md)

## Agent Classes and Reusable Library

- Class blueprints with instruction files and assigned skills
- Prompt and skill management from a filesystem-backed library
- Starred prompts for one-click execution from the Command panel

Related docs:

- [Class Management](./guide/class-management.md)
- [Library](./guide/library.md)
- [Command Panel](./guide/command-panel.md)

## Broadcast and Prompt Injection

- Broadcast text commands to selected agents or all active agents
- Inject starred prompts from the library after flattening into terminal input

Related docs:

- [Command Panel](./guide/command-panel.md)

## Agent-Facing Wardian CLI

- Give agents a textual control surface for inspecting live or persisted Wardian state
- Let agents spawn, clone, pause, resume, kill, wait on, and watch peers through the same backend control layer used by the desktop app
- Send prompts from inline text, standard input, or files; use `wardian ask` for bounded peer handoffs with response evidence
- List workflows, show workflow definitions, and start or stop workflow runs when the desktop app is available
- Inspect Wardian-managed agent worktrees, teams, and watchlists for automation-friendly coordination without treating the CLI as the primary human UI

Related docs:

- [Wardian CLI](./guide/cli.md)
- [Native E2E Harness](./developer/native-e2e.md)

## Source Control Per Agent

- Git status, staging/unstaging, discard, commit, pull/push
- Inline diff viewing and commit history
- Optional worktree mode for branch isolation

Related docs:

- [Source Control](./guide/source-control.md)
- [2026-04-17 Source Control Panel](./specs/2026-04-17-source-control-panel.md)

## Workflow Builder and Runtime

- Visual node-based builder with variable assistant
- Manual, scheduled, and listener-style trigger behaviors
- Scheduled run management and run-time role assignment
- Deterministic Rust workflow engine with pulse-driven candidate-node execution, branch/loop/wait control, agent execution modes, shared storage, and live telemetry

Related docs:

- [Workflow Overview](./workflows/index.md)
- [Building Workflows](./workflows/building-workflows.md)
- [Node Reference](./workflows/node-reference.md)
- [Triggers](./workflows/triggers.md)
- [Workflow Engine Architecture](./developer/workflow-engine.md)

## Queue and Completion Triage

- Captures agent completions when active terminal output settles back to Idle
- Records completed and failed workflow outcomes for later review
- Persists queue items under the Wardian home so unread work survives app restarts
- Supports unread badges, mark-read, clear-read, dismiss, and expandable long summaries

Related docs:

- [Queue](./guide/queue.md)
- [Workflow Overview](./workflows/index.md)

## Runtime Shell and Session Policy Controls

- Default shell selection (`auto`, discovered shell, or custom executable)
- Per-installation regular-agent session persistence policy (`resume` vs `fresh`)
- Provider-specific runtime utilities, including Gemini skill patching, Codex runtime policy, and OpenCode theme sync

Related docs:

- [Settings](./guide/settings.md)
- [2026-03-30 Runtime Shell Selection](./specs/2026-03-30-runtime-shell-selection.md)
- [2026-04-17 Session Persistence Policy](./specs/2026-04-17-session-persistence-policy.md)

## Provider-Aware Orchestration

- Unified orchestration over Gemini, Claude, Codex, and OpenCode
- Provider-specific lifecycle handling for resume/session identity and skill discovery

Related docs:

- [Provider Runtimes](./providers.md)
- [Developer Provider Runtime Notes](./developer/provider-runtimes.md)
