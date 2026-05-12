# Screenshot Documentation

Use this guide when adding reader-facing screenshots to Wardian documentation.

## Directory Layout

Committed screenshots live under:

```text
docs/assets/screenshots/<feature-or-window>/<state>.png
```

Use kebab-case for folders and filenames. Match the folder to the feature guide that owns the screenshot:

- `docs/assets/screenshots/grid/`
- `docs/assets/screenshots/dashboard/`
- `docs/assets/screenshots/explorer/`
- `docs/assets/screenshots/command-panel/`
- `docs/assets/screenshots/watchlists/`
- `docs/assets/screenshots/library/`
- `docs/assets/screenshots/workflows/`
- `docs/assets/screenshots/source-control/`
- `docs/assets/screenshots/settings/`
- `docs/assets/screenshots/user-terminal/`

Do not place committed documentation images under `e2e/screenshots/`. That directory is ignored and reserved for local PR evidence, rendering audits, and temporary Playwright captures.

## Embedding

Embed screenshots from the guide or reference page that explains the feature:

```md
![Source Control diff modal showing staged and unstaged changes](../assets/screenshots/source-control/diff-modal.png)
```

Use alt text that describes the visible state. Avoid vague labels such as `Screenshot of Source Control`.

## Capture Rules

- Capture real Wardian UI with seeded or sanitized data.
- Keep screenshots feature-specific. Avoid generic empty-window captures.
- Hide or avoid local usernames, absolute paths, API keys, provider tokens, and private repository names.
- Capture the smallest useful app region or viewport that still shows the interaction clearly.
- Prefer PNG for UI screenshots.
- Compress large images before committing.
- Refresh screenshots in the same PR when a visual change makes existing documentation images stale.

## Recommended Workflow

Start the app with an isolated home so screenshots are reproducible.

```bash
WARDIAN_HOME="$(mktemp -d)" npm run dev
```

PowerShell:

```powershell
$env:WARDIAN_HOME = "$PWD\.tmp\wardian-docs-screenshots"
npm run dev
```

Capture with Playwright or a browser screenshot tool, then copy only curated images into `docs/assets/screenshots/<feature-or-window>/`.

The first-pass core feature screenshots can be refreshed with:

```bash
npm run docs:screenshots
```

For screenshots that require Tauri IPC, PTY behavior, or provider runtime behavior, use the native E2E harness instead of browser-only E2E.

## Review Checklist

- The image belongs under `docs/assets/screenshots/`, not `e2e/screenshots/`.
- The filename and directory are kebab-case.
- The image is embedded from the owning guide page.
- The surrounding text explains the state shown in the image.
- The alt text is descriptive.
- No local paths, secrets, or private data are visible.
- `git status` shows only intended docs assets and guide updates.

Use [Core Feature Screenshot Capture Plan](./screenshot-capture-plan.md) to decide which screenshots belong in the first documentation pass.

See [Spec 036: Core Feature Screenshot Documentation](../specs/036-core-feature-screenshot-documentation.md) for the architectural decision.
