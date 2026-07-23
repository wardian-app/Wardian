# Key Features

This page maps Wardian's major capabilities to their detailed guides.

## Work with Agents Your Way

- Your prompts, classes, skills, workflows, and results are saved as files you can inspect instead of being hidden only inside the app.
- Use the desktop app, CLI, Library, Workflows, Explorer, Inbox, and Graph with the same saved Wardian data.
- Start with an agent, then save a prompt, improve a class, share a skill, or automate a workflow when that helps.
- Share reusable setup with every agent, a class, one agent, a team, a workspace, or a workflow run as appropriate.

Related docs:

- [Library](./guide/library.md)
- [Class Management](./guide/class-management.md)
- [Workflow Overview](./workflows/index.md)
- [Wardian CLI](./guide/cli.md)
- [Architecture](./developer/architecture.md)

## Monitor Live Agents

- High-density Grid and Dashboard views for live local agent sessions
- Left control rail for spawning, commands, workflows, Explorer, and settings
- Right roster with status lights, thought snippets, and agent working sets
- Inbox tab for unread agent completions, important updates, and workflow outcomes
- Use [Key Concepts](./guide/key-concepts.md) to understand the app layout and how its parts connect.

Related docs:

- [UI Overview](./guide/ui-overview.md)
- [Getting Started](./guide/getting-started.md)
- [Grid](./guide/grid.md)
- [Dashboard](./guide/dashboard.md)
- [Watchlists](./guide/watchlists.md)
- [Inbox](./guide/inbox.md)

## Agent Classes and Reusable Library

- Reusable agent setups with instruction files and assigned skills
- Prompt, skill, class, and workflow management from a filesystem-backed library
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

## Inbox and Completion Triage

- Captures explicit provider turn completions for named agents with their canonical final response, without treating Idle transitions or arbitrary terminal output as a final summary
- Receives explicit agent updates and exceptional manual approval requests through `wardian notify`
- Records completed and failed workflow outcomes for later review
- Persists Inbox completion projections under the Wardian home so unread work survives app restarts
- Supports unread badges, mark-read, clear-read, dismiss, and expandable long summaries

Related docs:

- [Inbox](./guide/inbox.md)
- [Workflow Overview](./workflows/index.md)

## Runtime Shell and Session Policy Controls

- Default shell selection (`auto`, discovered shell, or custom executable)
- Per-installation regular-agent session persistence policy (`resume` vs `fresh`)
- Theme sync support and Gemini patch actions from settings

Related docs:

- [Settings](./guide/settings.md)

## Provider-Aware Runtime

- Unified runtime model over Antigravity, Claude, Codex, and OpenCode (Gemini is unmaintained — use Antigravity for Google-model access)
- Provider-specific lifecycle handling for resume/session identity and skill discovery

Related docs:

- [Provider Runtimes](./providers.md)
- [Developer Provider Runtime Notes](./developer/provider-runtimes.md)
