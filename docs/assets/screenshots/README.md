# Documentation Screenshots

This directory stores committed, reader-facing screenshots for Wardian documentation.

Use one kebab-case subdirectory per feature or window:

```text
docs/assets/screenshots/<feature-or-window>/<state>.png
```

Do not store temporary Playwright output here. Local PR evidence and audit captures belong under `e2e/screenshots/`, which is intentionally ignored by git.

See [`docs/developer/screenshot-documentation.md`](../../developer/screenshot-documentation.md) for capture and embedding rules.

