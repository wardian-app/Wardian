# Command Panel

The Command panel is the left-sidebar surface for fast message delivery and prompt execution across one or many agents.

## Two Modes in One Panel

![Wardian Command panel showing quick prompts and a broadcast message ready to send](../assets/screenshots/command-panel/broadcast-prompt.png)

1. **Quick Prompts**
2. **Broadcast**

Both modes operate on your current agent selection in the right roster.

## Quick Prompts

Quick Prompts are starred prompt files from the Library (`~/.wardian/library/prompts`).

What happens when you click a quick prompt:

- Wardian reads the prompt content
- flattens multiline content into terminal-safe input
- sends it to selected agents
- if no agents are selected, asks for confirmation before sending to all agents

Tips:

- Star prompts in the Library to make them appear here
- Use short prompt names so they are easy to scan under pressure
- Use this path for repeatable operational tasks (tests, diagnostics, common instructions)

## Broadcast

Broadcast sends freeform text from the textarea to:

- selected agents, or
- all agents (after confirmation if nothing is selected)

Behavior:

- the text is submitted as terminal input through the same backend path used by direct terminal interaction
- one submission fan-outs to all chosen agent sessions

## Selection Rules

- **Single selected agent**: command goes to that one agent
- **Multi-select**: command goes to all selected agents
- **No selection**: confirmation prompt appears before sending to all active agents

## Common Patterns

- Use **Quick Prompts** for curated, repeatable instructions.
- Use **Broadcast** for ad-hoc coordination messages.
- Combine with watchlists to target only the relevant squad of agents.

## Related References

- [Library](./library.md)
- [Watchlists](./watchlists.md)
- [UI Overview](./ui-overview.md)
