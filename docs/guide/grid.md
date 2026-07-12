# Grid

Grid is a display mode inside the [Agents](./agents-overview.md) Workbench surface. It is no longer a global page or a title-bar destination.

This compatibility guide preserves the familiar Grid workflows and points older instructions to their current location.

![Agents in Grid mode showing active agent cards](../assets/screenshots/grid/app-shell.png)

## Open Grid Mode

1. Press `Ctrl+P` / `Cmd+P`, or select a pane's **+** button.
2. Open **Agents**.
3. Select **Grid** in the surface header.

Use **Auto** when Wardian should switch between a multi-card grid and a focused single card based on available space. Use **Single** when one focused agent should fill the surface.

## What Grid Mode Keeps

- Live terminal cards side by side.
- Normalized Chat cards with per-agent prompt drafts.
- Drag-to-reorder card headers.
- Manual row and column resizing.
- Per-card **Terminal** / **Chat** switches.
- **Maximize** to focus one agent in Single mode.
- **Reset Grid Layout** in the background context menu.

The default card display comes from **Settings > Grid > Grid card display**. Per-agent overrides remain temporary to the Agents surface.

## What Changed

- To open another app area, open a Workbench surface instead of switching a global view.
- The right roster separates targeting from navigation. Selecting an agent targets auxiliary tools; **Open** or **Open to Side** opens its session.
- Closing the Agents tab closes only the presentation. It does not stop any agent.
- Workbench **Zoom pane** expands one pane. Card **Maximize** changes Agents to Single mode. Neither action maximizes the native application window.

## Terminal and Chat Behavior

Terminal mode uses the provider's real PTY, including raw keys, approvals, TUIs, scrollback, and clickable supported links. Click inside a terminal to explicitly activate that presentation before typing. A mirrored presentation stays read-only until activated.

Chat mode shows normalized user, assistant, status, tool, approval, and terminal-output events. Use `Enter` to send and `Shift+Enter` for a newline. Switch back to Terminal when you need provider-specific keys or a raw approval screen.

Wardian preserves terminal state across tab changes, pane moves, Agents layout changes, and remote handoff. Reclaimed renderers restore and fit automatically when their presentation becomes visible.

## Related Links

- [Agents](./agents-overview.md)
- [Workbench](./workbench.md)
- [Watchlists](./watchlists.md)
- [Command Panel](./command-panel.md)
