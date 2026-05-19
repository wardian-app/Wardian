# Documentation Maintenance

Wardian documentation is part of the product surface. Feature PRs should update the public docs in the same branch when the change affects what users install, see, configure, run, troubleshoot, or automate.

## When a PR Must Update User Docs

Update user-facing docs when a change affects any of these surfaces:

- first-run setup, provider installation, app launch, or troubleshooting
- visible UI behavior, labels, navigation, status indicators, settings, or empty states
- CLI commands, arguments, output fields, exit behavior, or live-control requirements
- workflow authoring, node behavior, scheduling, Queue behavior, or agent assignment
- provider runtime behavior, shell behavior, permissions, sandboxing, session persistence, or filesystem expectations
- release, installation, platform support, or upgrade behavior
- error messages or recovery paths that users are expected to act on

Internal-only changes do not need user docs when they leave the public workflow unchanged. In that case, state that docs are not applicable in the PR checklist or test plan.

## Where to Update

Choose the smallest docs surface that matches the change:

- `docs/guide/` for user workflows and task-oriented feature guides
- `docs/workflows/` for workflow concepts, node behavior, and execution semantics
- `docs/providers.md` and `docs/developer/provider-runtimes.md` for provider runtime behavior
- `docs/developer/` for implementation, testing, publishing, and contributor workflows
- `docs/specs/` for strategic decisions or behavior that needs design history
- `README.md` only when the public overview, installation path, or top-level links change

Keep cross-platform examples POSIX-first with a labeled PowerShell variant when commands differ. Use placeholders such as `<absolute-workspace-path>` instead of personal paths.

## Screenshot Refresh Rules

Refresh screenshots when a PR changes a user-visible state that an existing screenshot documents, or when a new guide needs visual evidence to be understandable.

Use the existing screenshot rules:

- Committed reader-facing screenshots live under `docs/assets/screenshots/<feature-or-window>/`.
- Follow [Screenshot Documentation](./screenshot-documentation.md) for naming, placement, alt text, and capture hygiene.
- Use `npm run docs:screenshots` for browser-capturable seeded UI states.
- Use native E2E evidence for screenshots that require real Tauri IPC, PTY behavior, provider spawning, or filesystem effects.
- Do not commit `e2e/screenshots/`; that path is for temporary PR evidence and local audit captures.

Frontend PRs that change behavior or visuals must also embed representative screenshot evidence in the PR body using an HTTPS image URL. A local path alone is not enough.

## Release Notes and Public Docs Checklist

Before merging user-facing work, check:

- The owning guide or reference page explains the new behavior.
- Any affected screenshots were refreshed or intentionally left unchanged.
- The PR body links the issue and lists verification evidence.
- Release-impacting changes use a Conventional Commit type and scope that Release Please can place in `CHANGELOG.md`.
- Public overview links in `README.md` and `docs/index.md` still point to the right guide pages.
- `npm run docs:build` passes after docs changes.

## Docs Publication Policy

Wardian currently publishes docs from `main`, not from release tags.

- Pull requests that touch docs, package metadata, or the docs workflow run the `Wardian Docs` build for validation.
- Merges to `main` that touch those paths build and deploy `docs/.vitepress/dist` to GitHub Pages.
- Maintainers can also run the workflow manually with `workflow_dispatch` to rebuild the current `main` docs site.
- Release tags do not trigger a separate docs publication. This is an explicit deferral until Wardian needs versioned or release-frozen documentation.

See [Public Docs Site](./docs-site.md) for the publishing workflow and base-path details.
