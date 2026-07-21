---
name: wardian-cli
description: "Use immediately when a request mentions Wardian, Wardian agents, other agents, peers, delegation, orchestration, workflows, agent identity, agent status, agent workspaces, live or persisted Wardian state, the Wardian CLI, or any interaction from inside a Wardian-managed terminal."
---

# Wardian CLI

Use `wardian` as the source of truth for Wardian state and peer coordination.
Do not infer agent state from UI, terminal titles, or files such as
`settings/state.json`.

## Start Safely

- In a Wardian-managed terminal, inspect yourself with `wardian agent`. Outside
  one, pass an explicit agent name or UUID.
- Default agent listings and `send` targeting are intentionally local: they use
  neighbors in a managed session and the workspace otherwise. Use `--scope all`
  only for real cross-community orchestration.
- Treat default JSON as the automation contract. Request `status_source` when
  it matters whether state is `live` (desktop app) or `persisted` (`state.db`).
- Require the running desktop app with the same `WARDIAN_HOME` for mutating
  commands. Never edit persisted state to replace `agent update` or worktree
  commands.

## Choose A Command Family

| Need | Start with | Read for details |
| --- | --- | --- |
| Inspect, coordinate, create, update, or move agents | `wardian agent` | [agents and worktrees](references/agents-and-worktrees.md) |
| Send work, request an accountable reply, or respond | `wardian send`, `wardian ask`, `wardian reply` | [messaging](references/messaging.md) |
| Inspect or deploy reusable agent assets | `wardian library` | [library](references/library.md) |
| Validate or run workflows; inspect teams or watchlists | `wardian workflow`, `wardian team`, `wardian watchlist` | [workflows](references/workflows.md) |
| Observe output, diagnose CLI errors, or run a dev/runtime check | `wardian agent watch` | [runtime debugging](references/runtime-debugging.md) |

## Non-Negotiable Defaults

- Use `agent update` rather than editing `settings/state.json`; restart an
  agent when its result reports `restart_required`.
- Use `agent worktree enable`, `join`, and `disable` for managed agents.
  `disable` clears only the assignment; it never deletes the physical worktree.
- Use `send` for a live message. Use `ask` when one named peer must return a
  structured `done`, `blocked`, or `failed` reply with delivery evidence.
  Use `reply` only to complete an ask request.
- Keep broadcasts and class sends neighbor-scoped unless `--scope all` is
  genuinely required. `ask` accepts one named peer or UUID, never a broadcast,
  class selector, or thread.
- Use `send --as-command` only for one explicit agent or UUID when a provider
  slash command must be the first input token; it intentionally omits sender
  attribution.
- Treat `library deploy --targets` as the complete desired target set. Pass a
  non-empty, explicit list, or use `--clear` to remove every deployment.
- Keep raw PTY inspection opt-in. Use the readable watch response unless
  terminal escape bytes or repaint evidence are specifically needed.

Read the linked reference before using a conditional command shape or relying
on command-specific behavior. Keep prompts bounded, verify delivery or replies,
and report provider/runtime failures plainly.
