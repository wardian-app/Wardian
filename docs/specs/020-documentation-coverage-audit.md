# Spec 020: Documentation Coverage Audit and Navigation Hardening

## Problem

Documentation coverage had several discoverability and completeness gaps:

- `docs/index.md` was empty, so there was no central docs entry point.
- `docs/features.md` was empty, so feature-to-guide mapping was missing.
- User-facing docs did not cover important left-sidebar workflows:
  - Command panel
  - Source control panel
  - Settings panel
- Developer docs lacked a dedicated command reference for the active Tauri surface.

This made onboarding slower for both users and contributors and increased dependency on source-code spelunking for common tasks.

## Decision

Implement a documentation coverage expansion focused on:

1. **Discoverability**
   - add a complete docs landing page (`docs/index.md`)
   - add user and developer indexes (`docs/guide/index.md`, `docs/developer/index.md`)
   - add top-level README links to docs entry points

2. **User-facing operational coverage**
   - add dedicated guides for command panel, source control, and settings
   - cross-link these guides from existing onboarding docs

3. **Developer-facing API surface clarity**
   - add a Tauri command reference with grouped command/event surfaces
   - include contract and maintenance guidance for command changes

## Rationale

- Improves first-run orientation and self-service troubleshooting.
- Reduces repeated support questions around hidden-but-critical features.
- Makes backend/frontend contract changes safer by documenting command/event surfaces in one place.
- Preserves Wardian's markdown-as-truth principle by keeping operational behavior inspectable in-repo.

## Scope

In scope:

- Documentation structure, indexing, and content expansion.
- Cross-linking between README, guide pages, workflow docs, and developer docs.

Out of scope:

- Functional code behavior changes.
- UI redesign or command API changes.

## Success Criteria

- New contributor can find setup, user workflows, and developer architecture from `README.md` + `docs/index.md`.
- User can discover and operate command panel, source control, and settings without reading source code.
- Maintainer can quickly enumerate active Tauri commands/events via one reference document.
