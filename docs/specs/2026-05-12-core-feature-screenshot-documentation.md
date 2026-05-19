# Core Feature Screenshot Documentation

- **Status:** Proposed
- **Date:** 2026-05-12
- **Decider:** Tan Gemicioglu

## Context and Problem Statement

Wardian has user guides for the core windows and feature areas, but most pages describe the interface only in text. Clean screenshots would make the documentation easier to scan, especially for readers learning the Grid, Dashboard, Library, Workflows, Explorer, Command Panel, Source Control, Settings, and Watchlist surfaces.

[Testing Coverage and Screenshot Documentation](./2026-04-26-testing-coverage-and-screenshot-docs.md) already defines local PR screenshot evidence under `e2e/screenshots/<feature>/<timestamp>/`. That path is intentionally ignored by git and should remain a temporary evidence location. Reader-facing documentation screenshots need a committed, stable home that does not mix with local test artifacts.

## Proposed Decision

Store canonical documentation screenshots under:

```text
docs/assets/screenshots/<feature-or-window>/<state>.png
```

Examples:

```text
docs/assets/screenshots/grid/active-agent-card.png
docs/assets/screenshots/dashboard/system-summary.png
docs/assets/screenshots/library/library-view.png
docs/assets/screenshots/workflows/builder-canvas.png
docs/assets/screenshots/source-control/status-panel.png
```

Embed screenshots from the guide or reference page that owns the workflow:

```md
![Source Control panel showing branch state, commit box, staged changes, unstaged changes, and history](../assets/screenshots/source-control/status-panel.png)
```

Keep `e2e/screenshots/` for ignored PR evidence and local audit runs. When an E2E or Playwright capture is clean enough to become long-lived documentation, copy or recapture the selected image into `docs/assets/screenshots/` with a stable kebab-case name.

## Screenshot Inventory

The first documentation pass should cover these surfaces:

| Feature or window | Owning guide | Screenshot directory | Initial states |
| --- | --- | --- | --- |
| Grid | `docs/guide/ui-overview.md` | `docs/assets/screenshots/grid/` | active card, stacked cards |
| Dashboard | `docs/guide/ui-overview.md` | `docs/assets/screenshots/dashboard/` | system summary |
| Explorer | `docs/guide/explorer.md` | `docs/assets/screenshots/explorer/` | workspace tree, selected agent workspace |
| Command Panel | `docs/guide/command-panel.md` | `docs/assets/screenshots/command-panel/` | broadcast prompt, starred prompt |
| Watchlists | `docs/guide/watchlists.md` | `docs/assets/screenshots/watchlists/` | roster states, filtered roster |
| Library | `docs/guide/library.md` | `docs/assets/screenshots/library/` | prompts, skills, agent classes |
| Workflows | `docs/guide/workflows.md` and `docs/workflows/index.md` | `docs/assets/screenshots/workflows/` | builder canvas, run state |
| Source Control | `docs/guide/source-control.md` | `docs/assets/screenshots/source-control/` | status panel, diff modal, commit flow |
| Settings | `docs/guide/settings.md` | `docs/assets/screenshots/settings/` | provider/runtime settings |
| User Terminal | `docs/specs/2026-05-04-user-terminal-panel-design.md` or a future guide | `docs/assets/screenshots/user-terminal/` | open terminal, resized terminal |

## Capture Standards

- Capture real app UI, not mockups or cropped marketing-style images.
- Use a deterministic `WARDIAN_HOME` with seeded data so screenshots do not expose personal projects, credentials, local paths, or production agent state.
- Prefer feature-specific captures over full-app tours. A screenshot should explain the state described by the nearby paragraph.
- Use kebab-case filenames and directories.
- Keep images reasonably compressed before committing. Prefer PNG for UI screenshots unless a large photographic image requires WebP.
- Add descriptive alt text that explains the visible state, not just the feature name.
- Avoid embedding screenshots that show machine-specific absolute paths. Use seeded fixture names and generic workspace labels where possible.
- Do not commit temporary Playwright reports, traces, videos, or local `e2e/screenshots/` captures.

## Capture Workflow

Use the lowest layer that can produce the required state:

1. Browser E2E or Playwright for static UI, navigation, forms, mock-provider flows, and seeded guide screenshots.
2. Native E2E for Tauri IPC, PTY, filesystem, provider runtime, or terminal screenshots.
3. Real provider E2E only when the screenshot must show provider-specific behavior.

Recommended shell flow:

```bash
WARDIAN_HOME="$(mktemp -d)" npm run dev
```

PowerShell equivalent:

```powershell
$env:WARDIAN_HOME = "$PWD\.tmp\wardian-docs-screenshots"
npm run dev
```

After capture, move only the useful reader-facing images into `docs/assets/screenshots/<feature-or-window>/` and embed them from the owning guide.

## Consequences

- **Positive:** User-facing documentation gets stable visual references without relying on ignored local artifacts.
- **Positive:** The structure keeps screenshots close to docs while preserving feature ownership.
- **Positive:** The `e2e/screenshots/` and `docs/assets/screenshots/` split keeps PR evidence separate from committed documentation assets.
- **Negative:** Screenshots require periodic refresh when the UI changes.
- **Negative:** The repository size will grow if screenshots are not curated and compressed.
