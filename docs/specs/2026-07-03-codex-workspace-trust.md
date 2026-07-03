# Codex Workspace Trust

Filename: `2026-07-03-codex-workspace-trust.md`

- **Status:** Implemented
- **Date:** 2026-07-03

## Context and Problem Statement

Codex can start in an untrusted-project state until the launch workspace is
trusted. In Wardian this is noisy because agents are commonly created for many
workspaces, worktrees, or cloned folders, so Codex may ask for trust each time a
new agent starts.

Wardian already manages provider-specific runtime defaults. The Codex trust
behavior should therefore be a Codex-specific runtime option, disabled by
default, so users can opt in without changing general sandbox or approval
policy.

## Proposed Decision

Add `trust_workspaces` to the global Codex runtime policy stored in
`<WARDIAN_HOME>/settings/shell.json`. The default is `false`, and sparse
settings files omit it unless the user enables it.

When enabled, Wardian passes a Codex config override during launch:

```bash
codex -c 'projects."<absolute-agent-workspace-path>".trust_level="trusted"'
```

PowerShell:

```powershell
codex -c 'projects."<absolute-agent-workspace-path>".trust_level="trusted"'
```

Wardian generates the override for the agent's configured workspace folder and
escapes the path as a TOML quoted key. The handler does not edit the user's
global Codex config file and does not imply Codex full-auto mode.

## Consequences

- **Positive**: Users can avoid repeated Codex directory trust prompts across
  Wardian-managed agents.
- **Positive**: The setting is safer than enabling full-auto because sandbox and
  approval policy remain independently controlled.
- **Positive**: The implementation follows Codex's documented project trust
  table instead of relying on terminal input automation.
- **Negative**: The trust applies per launched workspace path, not to every
  possible folder on the machine.
- **Negative**: Already-running Codex agents keep their existing provider
  process configuration until relaunched.
