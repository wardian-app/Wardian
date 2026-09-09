# Agents

Agents is the Workbench view for monitoring several agent sessions at once. It replaces the old assumption that Grid is Wardian's global page: Grid is now one display mode inside this view.

Use Agents when you want live terminals or normalized chat activity arranged together, then switch to a focused presentation when one agent needs more space.

For a visual map of an agent's session, workspace, class, and skills, see [Key Concepts](./key-concepts.md#agents-and-their-setup).

## Open Agents

Press `Ctrl+P` / `Cmd+P`, select a pane's **+** button, or use the empty-pane Home state, then choose **Agents**. Wardian keeps a single Agents surface and focuses it when you open it again.

The right roster controls which agents auxiliary tools target. It does not filter Agents automatically. Use **Filter agents** in the surface to narrow the visible cards by agent name, description, class, provider, or workspace.

### Remember an Agent's Purpose

Select one agent, open **Agent Configuration** from the left rail, and add an
optional **Description**. Use it as a short memo about that specific agent's
responsibility, such as `Owns frontend release follow-up`.

Wardian keeps the memo available in agent configuration and automation
assignment choices, while the compact roster shows only the agent's class.
Roster and assignment searches include the memo. Descriptions do not change the
agent's instructions, skills, permissions, or provider session, and saving one
does not restart the agent. New agents start without a description, so the spawn
automation has no additional required step.

After restarting Wardian, agents can appear in the roster while their sessions
are still restoring. Configuration saves and lifecycle actions for such an
agent wait for its restoration to finish. Once a save succeeds, later resume
actions use the saved settings, including the choice to start a fresh provider
conversation. This also applies to paused agents.

### Open an Agent from Another View

When **Graph**, **Garden**, or **Inbox** uses **Open Agent**, Wardian first
targets an existing Agents view anywhere in the Workbench. It can activate an
inactive Agents tab in the same pane or another pane, select the requested
card, and scroll it into view. If the target is hidden by a zoomed pane,
Wardian restores the split layout before moving focus there.

If the view's local agent search or status filter would hide the requested
card, Wardian clears only that blocking filter before revealing it. Filters
that already include the requested agent stay in place.

The roster's explicit **Open** and **Open to Side** controls remain terminal
actions: they open or focus Agent Session tabs instead of changing the
contextual Agents destination.

## Choose a Layout Mode

The surface header provides three modes:

- **Auto** always keeps a multi-agent roster visible. It uses multiple columns whenever the pane can fit at least two useful cards side by side, and stacks the full roster into one scrolling column when it cannot. With one visible agent, that card naturally fills the surface.
- **Grid** keeps multiple visible cards and enables direct grid sizing and ordering controls. The layout can scroll when the available area is smaller than the cards' usable floor.
- **Single** shows the focused agent as one full-surface card. Use a card's **Maximize** control to focus it. **Minimize** restores the last explicit multi-agent mode, Auto or Grid, and reveals the roster again.

Auto derives its layout from the current surface dimensions and targets comfortable working cards before adding rows or columns. Terminal cards prefer about 640 x 450px and Chat cards prefer about 480 x 450px; smaller hard floors are reserved for genuinely constrained panes. Extra rows scroll. Auto does not overwrite the manual Grid arrangement, so you can return to Grid without losing the layout you tuned.

## Terminal and Chat Cards

Each card can show either:

- **Terminal** for the provider's real PTY, approvals, raw keybindings, and interactive TUI.
- **Chat** for normalized messages, tool activity, approval choices, and a compact prompt composer.

Chat messages and activity rows share a centered, readable transcript column that
expands fluidly on narrow surfaces.

Use the **Terminal** / **Chat** button in the card header for a temporary per-agent override. The default comes from **Settings > Agents > Agent card display**. Unsent Chat text stays with that agent when you switch modes.

In Terminal mode, click inside the terminal before typing. When the same agent terminal is visible elsewhere, the clicked presentation explicitly requests ownership; merely tabbing through the UI does not steal it. A **Mirror** remains read-only until ownership transfers. Reclaimed renderers restore and fit automatically when visible.

## Arrange and Focus Agents

- Drag card headers to reorder agents in Grid mode.
- Drag row and column gutters to adjust manual Grid sizing.
- Use **Maximize** on a card to enter Single mode for that agent.
- Use **Minimize** to leave explicit focus and return to the previous Auto or Grid mode.
- Right-click the background and choose **Reset Grid Layout** to discard the manual Grid arrangement without resetting the whole Workbench.
- Use the card menu for agent lifecycle actions. These actions affect the runtime; closing the Agents tab does not.

Auto adds columns only when the pane can support their preferred working width and permits vertical overflow. Below that threshold it switches to a one-column roster without hiding agents. Grid always honors the user's chosen tracks and row sizing instead of silently changing the user's arrangement.

## Important Boundaries

- Closing Agents closes only that Workbench presentation. Agents keep running.
- Closing an individual agent-session tab also leaves the runtime alive. Use Pause, Start Session, Restart Session, New Session, or Delete Agent only when you intend to alter the agent lifecycle.
- Roster selection and Agents focus are separate. Roster selection targets tools; Agents focus chooses the agent shown in Single mode.
- Grid is not a global navigation destination. It is an Agents mode retained for multi-agent compatibility and control.

## Configure Provider Launch Options

Open an agent's configuration and expand **Advanced Settings** to override
options for that provider. Wardian shows only options consumed by the selected
provider. Switching providers replaces the provider-specific configuration so
Claude, Gemini, Codex, Antigravity, OpenCode, and Pi flags do not leak into one
another.

Options marked **Headless** apply to background deliveries and automation runs
that use the agent profile. They do not alter the provider's interactive
terminal. Wardian owns structured output flags for those runs so provider JSON
or stream formats cannot break delivery parsing.

Codex sandbox and approval values inherit the provider-wide defaults under
**Settings > Agent Runtime > Codex** unless the agent has an explicit override.
Gemini's sandbox option requires Docker or Podman on the machine running the
provider CLI. If neither is available, Gemini rejects the launch before it can
begin a session.
Pi's project approval options control project-local configuration, extensions,
and skills. They do not sandbox Pi's tools or extensions.
Use **Custom Arguments** for a current provider flag that Wardian does not model;
custom arguments are passed only to the selected provider.

## Related Guides

- [Workbench](./workbench.md)
- [Grid compatibility guide](./grid.md)
- [Watchlists](./watchlists.md)
- [Dashboard](./dashboard.md)
