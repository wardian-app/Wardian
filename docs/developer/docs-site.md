# Public Docs Site

Wardian publishes the Markdown files in `docs/` as a VitePress site. The Markdown files remain the source of truth; VitePress is only the public rendering and navigation layer.

## Local Development

Install dependencies first:

```bash
npm install
```

Start the local docs server:

```bash
npm run docs:dev
```

Build the static site:

```bash
npm run docs:build
```

Preview the built site:

```bash
npm run docs:preview
```

PowerShell uses the same commands.

## Publishing

The `Wardian Docs` GitHub Actions workflow builds the site on pushes to `main` that touch docs, package metadata, or the docs workflow. It deploys `docs/.vitepress/dist` to GitHub Pages.

Pull requests that touch those same paths run the docs build without deploying. Maintainers can also run the workflow manually with `workflow_dispatch` to rebuild the current `main` site.

Wardian does not publish docs from release tags today. Release documentation is expected to merge to `main` before or with the release; versioned or release-frozen docs are explicitly deferred until the release cadence needs them.

The workflow sets `DOCS_BASE=/` because the Wardian repository uses the custom GitHub Pages domain `docs.wardian.org`. If the site falls back to the default project URL, update the workflow environment to use `DOCS_BASE=/Wardian/`.

## Content Rules

- Keep docs source in `docs/`; do not move user guides into a separate website folder.
- Prefer plain Markdown and relative links so pages remain readable on GitHub.
- Use placeholders such as `<absolute-workspace-path>` instead of local machine paths.
- Put user-facing screenshots under `docs/assets/screenshots/` and follow [Screenshot Documentation](./screenshot-documentation.md).
- Follow [Documentation Maintenance](./docs-maintenance.md) when a feature PR changes user-facing behavior, release notes, screenshots, or public docs links.
- Avoid VitePress-only Vue components unless the page genuinely needs an interactive docs feature.
