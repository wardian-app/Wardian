# Grid Chat Mode

- **Status:** Implemented
- **Date:** 2026-05-21

## Context

Wardian's Grid is currently terminal-first: each visible agent card embeds a
PTY-backed terminal and preserves raw provider interaction. This remains the
right ground truth for interactive CLIs, approvals, and provider TUIs, but it is
not always the best reading surface. Users supervising several agents need a
more scannable view of prompts, assistant responses, tool activity, command
output, errors, approvals, and completion state.

External references point toward two complementary patterns:

- Agent terminal web UIs can normalize CLI sessions into chat-like transcripts
  while preserving a raw terminal fallback.
- Modern terminal apps such as Warp improve readability by grouping command
  and output into blocks, keeping command context visible, and highlighting
  structured text.

Wardian should adopt the structure, not the dependency model. The Rust backend
remains authoritative for agent session lifecycle, PTY state, telemetry, and
provider transcript discovery.

This spec includes the Antigravity provider added on 2026-05-20. Antigravity is
separate from Gemini, uses the `antigravity` provider id, and exposes useful
conversation output through provider state rather than reliable stdout.

## Goals

- Let Grid cards render an agent session as a structured conversation when the
  user selects chat display mode.
- Preserve raw terminal mode as an always-available fallback.
- Normalize provider-specific events into a shared transcript model before the
  frontend renders them.
- Render tool calls, command output, approvals, errors, and terminal fallback
  output as activity blocks instead of plain chat bubbles.
- Add syntax highlighting and copy affordances where they materially improve
  supervision.
- Keep the first control surface in Settings until the navigation/titlebar model
  is redesigned.
- Ship the first slice as read-only chat mode. Raw terminal mode remains the
  input, approval-response, and provider-TUI interaction path.

## Non-Goals

- Replacing the PTY terminal with chat mode.
- Supporting provider-specific full-fidelity UI controls in chat mode.
- Building a global cross-agent transcript search in the first slice.
- Copying another project's tmux/server architecture.
- Adding card-level display overrides in the first slice.
- Sending prompts or approval responses directly from chat mode in the first
  slice.

## User Experience

### Settings

The first implementation adds a Grid section to Settings:

- **Grid card display**
  - `Terminal` default
  - `Chat` experimental

The setting applies immediately to the Grid view. It is a global display
preference, not a provider runtime setting. Future navigation work may promote
this into a view-level control while preserving the Settings value as the
default.

### Grid Card Rendering

In terminal mode, the card body keeps the existing `AgentTerminal` behavior.

In chat mode, the card body renders:

- The existing card header with agent name, class, status orb, maximize, and
  context actions.
- A scrollable transcript surface.
- User prompts as compact user message blocks.
- Assistant prose as readable message blocks.
- Tool calls, terminal output, approvals, errors, and command results as
  activity blocks.
- A compact read-only footer that explains when raw terminal mode is needed for
  input or approval responses.
- Empty, loading, unsupported, and parse-failed states.

Chat mode must be dense enough for multi-agent supervision. It should not copy
consumer chat spacing. The visual primitive is an **Activity Block**, not a
large decorative card.

### Activity Blocks

Activity blocks summarize work before showing full output. They support:

- A label such as `Shell`, `Read file`, `Edit`, `Search`, `Approval`, or
  `Terminal output`.
- Status: `running`, `succeeded`, `failed`, `action_required`, `cancelled`, or
  `unknown`.
- Optional duration, exit code, file path, command, provider source, and turn id.
- Collapsed and expanded states.
- Copy actions for command, output, full block, file path, and error snippet.
- Syntax highlighting for commands, diffs, JSON, stack traces, and common code
  fences.

Long output should keep context visible: the block header remains visible while
the block is scrolled or the output is collapsed behind a summary.

## Data Model

The backend exposes provider-normalized chat events using snake_case DTO fields.

```text
AgentChatEvent
- id: string
- session_id: string
- provider: string
- kind: message | tool_call | tool_result | approval | status | terminal_output | error
- role: user | assistant | system | tool | null
- text: string | null
- title: string | null
- status: running | succeeded | failed | action_required | cancelled | idle | processing | unknown | null
- turn_id: string | null
- source: string | null
- command: string | null
- exit_code: number | null
- path: string | null
- language: string | null
- created_at: string | null
- sequence: number | null
- metadata: object
```

The initial contract can remain smaller in code, but the design should leave
room for these fields. Unknown provider-specific data belongs in `metadata`
instead of adding one-off frontend fields.

## Backend Architecture

Add a provider transcript layer that can produce normalized `AgentChatEvent`
records from structured provider sources.

Required surfaces:

- `load_agent_chat_transcript(session_id)` returns a bounded recent transcript.
- `agent-chat-event` emits live normalized events when structured events arrive.
- Terminal fallback events are generated only when no structured source can
  explain useful output.

Initial transcript implementation:

- `load_agent_chat_transcript(session_id)` reads the active agent's discovered
  provider log path when available, normalizes it through the provider transcript
  parser, and merges it with current watch status.
- Watch transcript and raw terminal output are used as fallback only when the
  provider log does not contain structured message, tool, approval, or error
  events.
- Raw terminal fallback renders as a compact expandable row in chat mode rather
  than as a full-height output block.

The existing assistant-text extraction in `providers/transcript.rs` is a useful
starting point, but it currently returns only assistant transcript messages. Chat
mode needs user prompts, tool calls, tool results, approvals, status transitions,
and terminal fallback blocks.

## Provider Mapping

### Codex

Primary source: Codex session JSONL under the provider's Codex home.

Map:

- `event_msg` / `response_item` assistant messages to assistant message events.
- User messages to user message events when present in the session log.
- Function calls, shell calls, custom tool calls, and command starts to
  `tool_call`.
- Function output, command output, and custom tool output to `tool_result`.
- Escalated execution requests and approval requests to `approval`.
- Turn lifecycle events to `status`.

### Claude

Primary source: Claude transcript JSONL.

Map:

- Real user prompt events to user message events.
- Assistant text content blocks to assistant message events.
- Assistant `tool_use` blocks to `tool_call`.
- Tool result user events to `tool_result`.
- Permission requests to `approval`.
- Result and turn-duration records to status transitions.

Claude local-command artifacts and provider control messages must be filtered so
they do not appear as user prompts.

### Gemini

Primary source: Gemini chat logs and structured stream records.

Map:

- User records to user message events.
- Completed model records to assistant message events.
- Tool-use records to `tool_call`.
- Tool results and result records to `tool_result` or status transitions.

Gemini partial model chunks must not duplicate final assistant messages. Commit
assistant messages only when completion markers such as token usage or finish
reasons are present, or represent active partial text as a streaming event with
stable identity.

### Antigravity

Primary source:

```text
<antigravity-state-root>/brain/<conversation-id>/.system_generated/logs/transcript.jsonl
```

Antigravity conversation identity is resolved from provider state and stored in
`resume_session`.

Map:

- `USER_INPUT` records from explicit user sources to user message events.
- `MODEL` `PLANNER_RESPONSE` records with `DONE` status to assistant message
  events.
- Planner responses without `DONE` status to processing status.
- System/history records to status or ignored metadata unless they are useful to
  users.

Antigravity should not inherit Gemini log assumptions. It is a separate provider
adapter.

### OpenCode

Primary source: OpenCode provider database for messages and parts.

Map:

- User message rows and text parts to user message events.
- Assistant message rows and text parts to assistant message events.
- Step/tool/finish parts to activity blocks where useful.
- OpenCode logs remain useful for status, session id discovery, and errors.

OpenCode TUI output should be fallback only because redraw behavior is not a
stable transcript source.

### Mock

The mock provider should produce deterministic chat events for tests:

- One user prompt.
- One assistant response.
- At least one tool call and tool result.
- One approval-needed scenario.
- One long-output terminal fallback scenario.

## Frontend Architecture

Add a focused chat renderer under `src/features/terminal/` or
`src/features/grid/`:

- `AgentChatView`
- `AgentChatMessage`
- `ActivityBlock`
- `ActivityBlockHeader`
- `TranscriptComposer`
- `useAgentChatTranscript`

`GridView` switches the card body between `AgentTerminal` and `AgentChatView`
based on the Settings value.

The renderer must preserve layout stability:

- Fixed card body layout with scrollable transcript and stable composer height.
- No layout jumps when blocks expand or streaming text updates.
- Text wraps within the card at narrow widths.
- Long paths, commands, and stack traces preserve readable monospace formatting
  without overflowing the card.

## Syntax Highlighting

Syntax highlighting is part of the quality bar, but should be introduced through
a bounded utility rather than ad hoc rendering in each block.

Initial support:

- shell commands
- JSON
- diffs
- stack traces
- TypeScript, JavaScript, Rust, Python, Markdown

If a full highlighter increases bundle size too much, the first slice can use a
lightweight classifier for command, diff, error, and JSON blocks while leaving
language highlighting as a follow-up.

## Input Behavior

The first chat-mode slice was read-only. The next slice adds a standard text
composer for provider prompts while keeping terminal mode as the authoritative
fallback for raw TUI control.

Rules:

- Chat mode renders a compact composer at the bottom of the card.
- Chat mode submits text through the same `submit_prompt_to_agent` command used
  by the command panel, so provider-specific submit sequences remain centralized.
- Successful sends render an optimistic user bubble until the provider
  transcript confirms the message.
- The composer is disabled while the agent is off, paused, headless, in an
  error state, or actively processing.
- The terminal remains the authoritative fallback for provider states that
  require raw keyboard interaction.
- Action-required states may be answered through the composer, but dedicated
  approve/deny controls are not part of this slice.

## Error Handling

Chat mode must show clear fallback states:

- **No transcript yet:** show booting/empty state and offer terminal fallback.
- **Unsupported provider event:** show sanitized terminal output block.
- **Parse failure:** show a compact parse warning with raw terminal fallback.
- **Provider action required:** show an approval block and route the user to raw
  terminal when chat response is unsupported.
- **Offline agent:** show last transcript if available and an offline status.

## Testing

Frontend:

- Unit tests for Settings `Grid card display`.
- Unit tests for `GridView` terminal/chat switching.
- Rendering tests for message, activity, approval, error, collapsed, expanded,
  empty, and parse-failed states.
- Text wrapping tests for long commands, paths, and stack traces.

Backend:

- Fixture tests for each provider parser: Codex, Claude, Gemini, Antigravity,
  OpenCode, and mock.
- Backfill tests for bounded transcript loading.
- Live-event tests for normalized event emission where provider streams already
  exist.

E2E:

- Browser E2E with mock provider for Settings toggle and chat rendering.
- Native E2E only for claims involving real PTY fallback, provider spawning, or
  raw terminal interaction.
- Real provider E2E remains opt-in for provider-specific transcript behavior.

Screenshots:

- Frontend PRs must capture feature-specific screenshots under
  `e2e/screenshots/grid-chat-mode/<timestamp>/`.
- At least one representative screenshot must be embedded in the PR description
  using a hosted image URL.

## Implementation Slices

1. **Spec and contract**
   - Finalize the `AgentChatEvent` DTO.
   - Decide bounded transcript size and event identity rules.

2. **Backend transcript API**
   - Add normalized transcript parser interface.
   - Add `load_agent_chat_transcript`.
   - Extend provider fixtures.

3. **Provider adapters**
   - Codex, Claude, Gemini, Antigravity, OpenCode, mock.
   - Keep terminal fallback separate.

4. **Frontend renderer**
   - Add chat components and activity blocks.
   - Add syntax and copy utilities.

5. **Settings and Grid integration**
   - Add `grid_card_display_mode`.
   - Add Settings control.
   - Switch Grid card body based on the setting.
   - Render a chat composer that reuses backend provider submit behavior.

6. **Polish and documentation**
   - Update `docs/guide/grid.md` and `docs/guide/settings.md`.
   - Add screenshots.
   - Run appropriate frontend/backend/native validation.

## Decisions

- Chat mode now accepts standard prompt text. Terminal mode still handles raw
  keyboard workflows and remains the fallback for provider-specific TUI
  interactions.
- The first implementation uses a Settings-level global display preference.
- Card-level overrides are a follow-up after the global mode proves useful.

## Open Questions

- Should expanded/collapsed activity block state persist per session?
- Should copy actions be available on hover only, or always visible in dense
  card headers?
- How much syntax highlighting is acceptable before bundle size becomes a
  concern?

## References

- `docs/specs/2026-05-20-antigravity-provider.md`
- `docs/specs/2026-05-18-agent-watch-readable-output.md`
- `docs/guide/grid.md`
- `docs/guide/settings.md`
- Agent terminal web reference assets:
  `https://github.com/antonlobanovskiy/agent-tmux-web/tree/main/docs/assets`
- Warp terminal block reference:
  `https://docs.warp.dev/terminal/blocks`
