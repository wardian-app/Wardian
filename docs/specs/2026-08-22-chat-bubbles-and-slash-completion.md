# Chat Bubble Alignment and Composer Command Completion

Status: Implemented

Related: supersedes the message-surface decisions in
`2026-08-16-chat-message-surfaces-and-contextual-actions.md` for user prompts
and copy-action placement; that spec's activity/work-log action rules remain in
force.

## Context

Operator feedback after the density pass: user messages read as faint tinted
strips rather than turns of a conversation, and the per-message copy actions —
absolutely positioned over the row — landed in different places depending on
row type and viewport width. Separately, providers accept interactive slash
commands, but the desktop composer neither completed them nor routed them
through a channel that guarantees an interactive surface.

## Decisions

### Message surfaces (revised)

- User prompts are right-aligned bubbles on every surface, mobile included:
  rounded ends, an accent-tinted fill with a matching border, capped at
  `min(92%, container)`. The previous full-width remote variant collapsed both
  roles into left-aligned strips distinguished only by a 2px edge.
- Assistant responses remain plain full-width prose capped at 76ch.
- System and tool roles keep their bordered status cards.
- The `layout` prop is retained only for call-site compatibility; geometry is
  role-driven, not layout-driven.

### Message actions (revised)

- Message rows render their actions **in flow below the content**,
  hover/focus-revealed, aligned to the author's edge (right for user bubbles,
  left for assistant text). An overlay can detach from its anchor when the
  anchor changes between row types; flow children cannot.
- Activity, work-log, terminal-fallback, and turn-change rows keep the
  corner-overlay pattern: they are bounded cards where a top-right control is
  conventional and stable.
- The touch-device `padding-right` reservation on message content is removed;
  card rows keep their explicit padding rules.

### Slash commands

- A curated per-provider catalog (`src/features/chat/slashCommands.ts`) lists
  only stable, documented provider CLI commands. Completion appears while the
  first token is being typed (`/`, `/mo`) and never after whitespace commits
  the command word.
- Arrow keys navigate, Enter/Tab complete, Escape dismisses until the draft
  changes.
- Any "/"-prefixed prompt is delivered as `input_mode: "command"` so provider
  slash commands always reach an interactive surface (or the mailbox when the
  agent is off), never headless execution. This matches the remote PWA's
  existing inference and fixes the composer's own `/model` live-apply, which
  previously traveled the message channel.
- The completion payload omits `input_mode` for ordinary messages, keeping the
  IPC shape unchanged for existing callers.

## Consequences

- `AgentsOverviewView` chat-mode cards and the remote PWA inherit the bubble
  treatment through the shared renderer without further changes.
- Provider catalogs are hand-curated; new provider commands require updating
  `slashCommands.ts`. Wrong entries are low-risk (they merely complete text)
  but should still be pruned.
