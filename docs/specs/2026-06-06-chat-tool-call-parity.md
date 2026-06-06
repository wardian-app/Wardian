# Chat Tool Call Parity

## Context

Wardian chat mode already normalizes provider transcripts into `AgentChatEvent`
records, but the first renderer treated most non-message events as generic work
logs or plain preformatted output. That made command execution, file edits,
patches, permission prompts, todo updates, and function calls harder to inspect
than comparable agent chat UIs.

Reference interfaces such as T3 Code, OpenCode UI, and OpenChamber emphasize
tool visibility: sessions stay chat-shaped, but tool calls have typed rows,
diffs and patches are recognizable, permissions are explicit, and long threads
remain responsive.

## Decision

Improve `AgentChatView` as the first parity slice without changing the backend
DTO. The renderer classifies existing `AgentChatEvent` records from `kind`,
`title`, `command`, `path`, `source`, `language`, and metadata. It then renders
specialized activity cards for shell commands, file/diff work, search, todo
updates, permission prompts, and generic tools.

Small adjacent tool sequences stay expanded as individual cards so a normal
call/result pair remains inspectable. Larger bursts still collapse into a work
group to keep agent-heavy sessions scannable.

## Lazy Rendering

Chat mode initially renders the newest transcript rows and exposes a load-earlier
control for older rows. This is a frontend rendering guard: the backend can keep
returning the bounded normalized transcript, while the DOM avoids mounting the
entire history at once.

## Remote Chat

The mobile/PWA agent detail view uses the same normalized transcript content
model for messages and activity rows. It groups large adjacent work bursts and
summarizes tool calls by concrete command or result content instead of showing
status-only rows.

When the remote status stream reports that the active agent is processing,
running, or waiting on action, chat mode schedules a throttled background
refresh even if the status string has not changed. Terminal mode keeps using the
terminal stream and does not poll snapshots for those repeated status frames.

## Testing

Focused frontend tests cover:

- tool cards surfacing command, path, changed-file, diff, and todo details;
- large work batches remaining grouped;
- older transcript rows being hidden until requested;
- PWA chat rendering grouped tool-call details and refreshing during repeated
  active status frames;
- existing copy, approval, markdown, and refresh behavior.
