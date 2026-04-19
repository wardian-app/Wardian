# Key Features

This page maps Wardian's major capabilities to their detailed guides.

## Multi-Agent Command Center

- High-density Grid and Dashboard views for active session monitoring
- Left control rail for spawning, commands, workflows, explorer, and settings
- Right roster with status lights, thought snippets, and watchlists

Related docs:

- [UI Overview](./guide/ui-overview.md)
- [Getting Started](./guide/getting-started.md)
- [Watchlists](./guide/watchlists.md)

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

## Source Control Per Agent

- Git status, staging/unstaging, discard, commit, pull/push
- Inline diff viewing and commit history
- Optional worktree mode for branch isolation

Related docs:

- [Source Control](./guide/source-control.md)
- [Spec 018: Source Control Panel](./specs/018-source-control-panel.md)

## Workflow Builder and Runtime

- Visual node-based builder with variable assistant
- Manual, scheduled, and listener-style trigger behaviors
- Scheduled run management and run-time role assignment

Related docs:

- [Workflow Overview](./workflows/index.md)
- [Building Workflows](./workflows/building-workflows.md)
- [Node Reference](./workflows/node-reference.md)
- [Triggers](./workflows/triggers.md)

## Runtime Shell and Session Policy Controls

- Default shell selection (`auto`, discovered shell, or custom executable)
- Per-installation regular-agent session persistence policy (`resume` vs `fresh`)
- Theme sync support and Gemini patch actions from settings

Related docs:

- [Settings](./guide/settings.md)
- [Spec 010: Runtime Shell Selection](./specs/010-runtime-shell-selection.md)
- [Spec 019: Session Persistence Policy](./specs/019-session-persistence-policy.md)

## Provider-Aware Orchestration

- Unified orchestration over Gemini, Claude, Codex, and OpenCode
- Provider-specific lifecycle handling for resume/session identity and skill discovery

Related docs:

- [Provider Runtimes](./providers.md)
- [Developer Provider Runtime Notes](./developer/provider-runtimes.md)
