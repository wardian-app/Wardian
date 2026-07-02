# Chat-First Long Prompt Composition

- **Status:** Implemented
- **Date:** 2026-05-26

## Context

Wardian users can type directly into provider terminals, but long prompt editing
inside a provider TUI is not a normal text editor experience. Less
terminal-savvy desktop users may expect mouse selection, click-to-position
caret movement, native undo, and paragraph editing to work like a macOS text
field. Provider TUIs do not expose that contract uniformly. Some support shell
line-editor shortcuts, some implement their own composer, and some interpret
mouse input as terminal selection rather than caret movement.

The terminal grid is valuable because it preserves the real provider session:
approvals, provider-specific controls, raw output, and keyboard behavior remain
visible and debuggable. Adding a persistent external input panel to every
terminal card would consume scarce grid space and create another place where
Wardian might accidentally conflict with provider-specific entry fields.

Wardian already has two safer composition surfaces:

- **Chat mode** for a single agent, backed by Wardian's provider-aware prompt
  submission path.
- **Command panel** for selected-agent, broadcast, and quick-prompt workflows.

Plan A is to make long-form prompt drafting a Chat-first workflow while keeping
terminal mode a raw terminal.

## Goals

- Give desktop users a reliable place to draft and edit long prompts with
  native textarea behavior.
- Preserve raw terminal behavior and provider-specific TUI entry fields.
- Avoid adding persistent input panels to terminal grid cards.
- Route submitted prompts through Wardian's existing provider-aware delivery
  path instead of typing blindly into provider composers.
- Make the beginner-friendly path discoverable without turning terminal mode
  into a chat UI.
- Keep the Command panel focused on cross-agent and desktop-Habitat workflows.

## Non-Goals

- Implementing mouse click-to-caret inside arbitrary provider TUIs.
- Replacing terminal grid cards with a Warp-style split input/output model.
- Synchronizing text already typed into a provider TUI back into Wardian's Chat
  composer.
- Reimplementing provider line editors, shell keybindings, or TUI-specific
  editing semantics.
- Adding provider-specific prompt panels inside terminal mode.

## Decision

Use Chat mode as the first-class long prompt composer for single-agent desktop
workflows. Terminal mode remains raw and focused on TUI fidelity. The Command
panel remains the multi-agent and desktop-Habitat composition surface.

The first implementation slice should be conservative:

1. Add a minimal per-card mode switch instead of a one-way compose button. The
   control should be visible in both Terminal and Chat modes, show the current
   mode, and let the user switch to the other mode.
2. When switching from Terminal to Chat through this control, focus the Chat
   composer for that agent.
3. Preserve terminal mode as the default raw-control surface unless the user or
   future profile settings choose otherwise.
4. Submit Chat prompts through the existing `submit_prompt_to_agent` path.
5. If delivery is unsafe, keep the draft intact and show a clear failure state
   rather than injecting text into the terminal.

This keeps the product boundary clear: Chat is for composing, Terminal is for
terminal control, and Command is for orchestration.

## User Experience

### Terminal Mode

Terminal mode should not gain a permanent prompt editor. It should expose a
small card-level mode switch that is easy to ignore:

- Label or tooltip: `Terminal` / `Chat`, with the current mode visible.
- Action: switch modes for the same agent. Switching to Chat focuses the
  composer.
- Placement: card header or compact toolbar, not inside the terminal content.
- Visibility: available on desktop terminal cards; no large overlay or footer.

Terminal focus, selection, provider TUI input, and raw keyboard handling remain
unchanged.

### Chat Mode

Chat mode should behave like a normal desktop writing surface:

- Mouse click positions the caret inside the textarea.
- Drag and keyboard selection work with native platform behavior.
- macOS shortcuts such as `Cmd+A`, `Cmd+Left`, `Cmd+Right`, `Option+Left`,
  `Option+Right`, undo, and redo are handled by the browser textarea.
- `Enter` sends and `Shift+Enter` inserts a newline.
- The draft remains visible if submission fails.
- Switching away from Chat mode should not discard an unsent draft unless the
  user explicitly clears or sends it.

The placeholder can stay concise, for example:

```text
Write a prompt...
```

Shortcut hints should be discoverable through existing tooltip or help surfaces
rather than permanent instructional text in the grid.

### Command Panel

The Command panel stays optimized for:

- Prompting multiple selected agents.
- Broadcasting to a working set.
- Reusing quick prompts.
- Command-center operations where the user is not focused on one raw terminal.

It should not become the default single-agent long prompt editor unless the user
is already working from the command sidebar.

## Delivery Behavior

Chat submissions must use the same backend-owned provider delivery path as other
Wardian prompt sends. This keeps provider-specific timing, submit keys,
readiness checks, and failure states centralized.

Required behavior:

- Preserve user-entered text until the backend confirms the prompt was accepted
  for delivery or queued.
- Do not type partial text into the raw terminal from the frontend.
- If the target is busy, action-required, or input-unsafe, follow the current
  delivery policy and present the resulting state.
- Do not infer approval actions from freeform Chat text.

## Testing

Use the lowest test layer that proves each behavior:

- Frontend unit tests for the terminal-to-Chat affordance, mode switching, focus
  behavior, and draft preservation.
- Existing Chat composer tests for `Enter` send and `Shift+Enter` newline should
  remain in place.
- Browser E2E for the discoverable desktop flow: terminal card action switches
  to Chat, focuses the composer, edits a long prompt, and submits it.
- Backend delivery tests only if the submission contract changes.
- Native runtime E2E only if the implementation changes PTY ownership,
  terminal focus, or provider input injection.

## Accessibility

- The composition affordance must be keyboard reachable and have an accessible
  name.
- Focus movement from Terminal to Chat must be explicit and predictable.
- Chat composer should keep native textarea semantics rather than custom cursor
  or selection handling.
- Error states must be exposed as visible text and not only color.

## Risks

- Users may still try to edit long prompts inside provider TUIs. The Chat
  affordance needs to be discoverable enough to teach the safer path without
  cluttering terminal mode.
- Switching modes from a terminal card can surprise users if it steals focus
  while they are typing. The affordance must require an explicit click or
  keyboard command.
- Provider delivery reliability remains bounded by the existing delivery
  transport. This spec should not claim stronger delivery guarantees than the
  backend can verify.

## Open Questions

- Should the terminal-to-Chat affordance live in the card header, card action
  menu, or both?
- Should draft preservation be per agent, per card, or global to the selected
  agent session?
- Should Wardian offer a beginner profile that defaults grid cards to Chat mode?
- Are there provider-independent terminal improvements, informed by Warp's
  behavior, that can become Plan B without modifying provider TUI input fields?

## Plan B Input

Warp's current model points to a provider-independent follow-up, but not to
arbitrary terminal caret control:

- Warp's Terminal and Agent modes keep terminal command entry and agent
  conversations visually distinct.
- Warp's rich input editor gives supported CLI coding agents an IDE-style
  prompt editor with mouse caret placement, selection, undo, multiline input,
  context attachment, and explicit focus transfer from the CLI agent to Warp's
  editor.
- Warp's full-screen app behavior still treats mouse input as terminal mouse
  reporting. Clicks may be handled by Warp or forwarded to the running app as
  ANSI mouse events, so caret movement inside an arbitrary TUI remains the
  running app's responsibility.

Sources:

- [Warp Terminal and Agent modes](https://docs.warp.dev/agent-platform/local-agents/interacting-with-agents/terminal-and-agent-modes/)
- [Warp rich input editor](https://docs.warp.dev/agent-platform/cli-agents/rich-input/)
- [Warp full-screen apps](https://docs.warp.dev/terminal/more-features/full-screen-apps)

If Wardian pursues Plan B, it should be a Wardian-owned rich prompt drawer for
active agent sessions. It can offer textarea-grade editing, optional context
attachment, clear focus state, and submission through Wardian's provider-aware
delivery path. It should not try to reposition the caret inside provider TUI
prompt fields.
