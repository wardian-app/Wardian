# Core Feature Screenshot Capture Plan

This plan defines the first committed screenshot pass for Wardian documentation. It complements [Screenshot Documentation](./screenshot-documentation.md) and follows the canonical asset layout:

```text
docs/assets/screenshots/<feature-or-window>/<state>.png
```

## Capture Priorities

1. **Reader orientation:** one clean screenshot for every top-level window or persistent panel readers need to recognize.
2. **Workflow proof:** one screenshot for important interactions that are hard to understand from text alone.
3. **Native/runtime evidence:** terminal and provider screenshots only when the native harness is needed to show the real state.

## Initial Screenshot Set

| Priority | Feature or window | Owning guide | Asset path | Capture layer | State to show |
| --- | --- | --- | --- | --- | --- |
| P0 | App shell and Grid | `docs/guide/ui-overview.md` | `docs/assets/screenshots/grid/app-shell.png` | Browser E2E | Main command center with left rail, grid, and roster visible |
| P0 | Dashboard | `docs/guide/ui-overview.md` | `docs/assets/screenshots/dashboard/system-summary.png` | Browser E2E | Dashboard cards and telemetry summary |
| P0 | Agent roster / Watchlists | `docs/guide/watchlists.md` | `docs/assets/screenshots/watchlists/agent-roster.png` | Browser E2E | Searchable right roster with agent rows and status indicators |
| P0 | Spawn Agent | `docs/guide/getting-started.md` | `docs/assets/screenshots/spawn-agent/spawn-form.png` | Browser E2E | Agent configuration form with name, class, workspace, and provider controls |
| P0 | Command Panel | `docs/guide/command-panel.md` | `docs/assets/screenshots/command-panel/broadcast-prompt.png` | Browser E2E | Broadcast textarea and target controls |
| P0 | Library | `docs/guide/library.md` | `docs/assets/screenshots/library/library-view.png` | Browser E2E | Prompt, skill, or class library browser |
| P0 | Workflows | `docs/guide/workflows.md` | `docs/assets/screenshots/workflows/builder-canvas.png` | Browser E2E | Workflow sidebar and builder canvas |
| P0 | Settings | `docs/guide/settings.md` | `docs/assets/screenshots/settings/runtime-settings.png` | Browser E2E | Theme, shell, and runtime settings controls |
| P0 | Explorer | `docs/guide/explorer.md` | `docs/assets/screenshots/explorer/workspace-tree.png` | Browser E2E | File tree rooted in a seeded workspace |
| P0 | Source Control | `docs/guide/source-control.md` | `docs/assets/screenshots/source-control/status-panel.png` | Browser E2E | Git branch bar, staged/unstaged groups, and commit box |
| P1 | Source Control diff | `docs/guide/source-control.md` | `docs/assets/screenshots/source-control/diff-modal.png` | Native E2E | Inline diff modal with hunk markers |
| P1 | User Terminal | future user guide or `docs/specs/2026-05-04-user-terminal-panel-design.md` | `docs/assets/screenshots/user-terminal/open-terminal.png` | Native E2E | User terminal panel with workspace context |
| P2 | Agent lifecycle | `docs/guide/ui-overview.md` or `docs/guide/watchlists.md` | `docs/assets/screenshots/grid/active-agent-card.png` | Native E2E | Running agent card with terminal output |
| P2 | Workflow run state | `docs/workflows/index.md` | `docs/assets/screenshots/workflows/run-state.png` | Native E2E | Workflow block status while running |

## First Pass Scope

The first pass should commit P0 browser-capturable screenshots and embed them in the owning guides. P1/P2 captures should follow in later commits because they require native IPC, real filesystem state, PTY behavior, or provider runtime behavior.

## Capture Data Rules

- Use seeded names such as `E2E Mock Agent`, `Docs Demo`, and `TestClass`.
- Use generic workspaces such as `<absolute-workspace-path>` in visible fields when possible.
- Avoid showing local user names, production `WARDIAN_HOME`, private repository names, provider tokens, or real provider conversations.
- Prefer light, crisp screenshots at `1440x960` for documentation unless the feature needs a narrower responsive state.

## Embedding Order

1. Add the screenshot under `docs/assets/screenshots/<feature-or-window>/`.
2. Embed it near the first paragraph that describes the visible state.
3. Keep surrounding text short; the image should support the guide rather than replace it.
4. Run markdown and build verification before finalizing.
