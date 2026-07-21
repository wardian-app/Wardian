# Key Features

This page maps Wardian's major capabilities to their detailed guides.

## Malleable Agent Environment

- Local-first agent habitat where prompts, classes, skills, workflows, inbox evidence, and memory-ready context are inspectable artifacts rather than opaque app-only state
- Multiple lenses over shared Wardian state: desktop UI, CLI, Library, Workflows, Explorer, Inbox, Graph, and Garden operate against the same canonical records
- Gentle slope from use to creation: run an agent, save a prompt, tune a class, deploy a skill, automate a workflow, then promote durable evidence into memory or project context
- Explicit scopes for global, class, agent, team/project, workspace, and workflow-run context so reusable capabilities can be shared without losing provenance

Related docs:

- [Library](./guide/library.md)
- [Class Management](./guide/class-management.md)
- [Workflow Overview](./workflows/index.md)
- [Wardian CLI](./guide/cli.md)
- [Architecture](./developer/architecture.md)

## Live Agent Habitat

- High-density Grid and Dashboard views for live local agent sessions
- Left control rail for spawning, commands, workflows, Explorer, and settings
- Right roster with status lights, thought snippets, and agent working sets
- Inbox tab for unread agent completions, important updates, and workflow outcomes
- Roadmap direction: Sites, Cohorts, movable surfaces, richer Garden spatial organization, and Graph communication topology

Related docs:

- [UI Overview](./guide/ui-overview.md)
- [Getting Started](./guide/getting-started.md)
- [Grid](./guide/grid.md)
- [Dashboard](./guide/dashboard.md)
- [Watchlists](./guide/watchlists.md)
- [Inbox](./guide/inbox.md)

## Agent Classes and Reusable Library

- Class blueprints with instruction files and assigned skills
- Prompt, skill, class, and workflow-blueprint management from a filesystem-backed library
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

- Captures agent completions without treating arbitrary terminal output as a final summary
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
