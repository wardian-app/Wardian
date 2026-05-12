# Skill Manager References

This document maps public skill-management systems to design patterns relevant to Wardian's skill library, agent habitats, and future workflow-builder integration.

This is not an endorsement, affiliation claim, product evaluation, or competitive teardown. The notes below describe public architecture and design pressure only.

Last reviewed: 2026-05-12.

Source basis: AGHub and Chops were specifically requested and were checked against their public repositories. Other entries were selected through public search plus repo inspection where available. The Assistant agent was also queried for additional candidates; its most relevant extra suggestion, Everything Claude Code, is listed as an adjacent harness pack rather than a skill manager.

## Design Axes

- **Discovery surface**: how humans or agents find skills by name, tags, source, provider, purpose, trust, or compatibility.
- **Install semantics**: whether installation means copy, symlink, junction, package install, plugin install, or registry subscription.
- **Scope model**: whether skills are global, project-local, tool-local, workspace-local, or synced across several targets.
- **Source of truth**: whether the canonical copy lives in a central repo, a registry, a tool dotfolder, a package cache, or a workspace folder.
- **Skill and agent duality**: whether the manager treats skills, subagents, commands, prompts, hooks, and plugins as one ecosystem or separate object types.
- **Human observability**: whether users can inspect the real files, metadata, activation state, provenance, and target paths.
- **Agent friendliness**: whether agents can install, update, query, and author skills through stable CLI/API/file contracts.
- **Trust and lifecycle**: whether the system supports versioning, pinning, review, moderation, update checks, security metadata, or provenance.

## Summary Map

| System | Primary Surface | Relevant Pattern | Wardian Takeaway |
|---|---|---|---|
| [AGHub / Agent Skills Hub](https://github.com/agent-skills-hub/agent-skills-hub) | Registry repo + NPX CLI | Large cross-agent catalog with targeted install flags and project-local override guidance. | Useful as a bulk import and compatibility reference, but Wardian should add stronger provenance, curation, and local observability. |
| [Chops](https://github.com/Shpigford/chops) | Native macOS app | Local scanner/editor for skills and agents across coding tools, with file watching, collections, search, and frontmatter parsing. | Useful reference for a human-observable local library: inspect real files, organize without moving source, and watch dotfolders continuously. |
| [Vercel `skills`](https://github.com/vercel-labs/skills) | CLI/package manager | Open agent-skills installer with many source formats, project/global scopes, symlink/copy modes, list/find/remove/update/init commands, and broad agent target matrix. | Strong reference for agent-friendly install contracts and normalized target resolution. |
| [Skills Hub Desktop](https://github.com/qufei1993/skills-hub) | Cross-platform Tauri desktop app | Central skill repository synced to many tool-specific global or project directories, preferring symlink/junction and falling back to copy. | Relevant implementation reference for Wardian's local-first central skill store and cross-tool activation state. |
| [Quiver](https://github.com/sam-blakeman/quiver) | Local web UI + CLI | Claude Code skill inventory over local skills and marketplace plugins, with add/remove/import/export, sync, registry browsing, and simple REST routes. | Useful pattern for a small, agent-operable local HTTP/CLI layer over disk skills. |
| [ClawHub](https://github.com/openclaw/clawhub) | Hosted registry + CLI/API | OpenClaw public skill registry with publish/version/search, vector search, pinning, install/update, comments, moderation, package catalog, and security metadata. | Useful trust-lifecycle reference: Wardian should separate local activation from registry provenance, pinning, and review state. |
| [Trail of Bits Skills](https://github.com/trailofbits/skills) | Domain marketplace / skill pack | Security-focused Claude plugin marketplace with Codex sidecar support and curated domain skills. | Useful for vertical curation: Wardian should support themed packs without treating every skill source as equally trusted. |
| [HOL Registry Broker Skills](https://github.com/hashgraph-online/registry-broker-skills) | Registry skill pack + SDK/API | Universal agentic registry skills with CLI, MCP, TypeScript SDK, verification/publish commands, trust scores, and GitHub Action publishing. | Adjacent reference for registry interoperability, skill publishing, and trust metadata around agents and skills. |
| [Everything Claude Code](https://github.com/affaan-m/everything-claude-code) | Harness pack + installer | Large cross-harness system of skills, agents, hooks, commands, rules, dashboards, install manifests, and security tooling. | Not a skill manager, but relevant as a stress test for managing bundles that mix skills, hooks, agents, rules, and runtime policy. |

## Reference Profiles

### AGHub / Agent Skills Hub

**Source basis:** Public repo checked.

**What it includes:** AGHub is a centralized open-source registry of Markdown `SKILL.md` folders distributed through an NPX-first CLI. The repo catalog advertises more than 790 skills and targets Claude Code, Gemini CLI, Cursor, Kiro, Codex, Antigravity, OpenCode, AdaL, OpenClaw, and custom paths.

**Distinctive components:**

- Bulk install into a global agent-skills location.
- Project-local clone guidance where local skills override global skills.
- Target flags for common agent environments.
- Specific skill install by name.
- Simple contribution model: add a folder containing `SKILL.md`.
- Roadmap items around versioning, dependency graphs, and marketplace integrations.

**Wardian relevance:** AGHub is useful as a catalog-scale reference, especially for import and search. It is less useful as a complete governance model because it does not appear to own a rich local inventory, trust workflow, or human-observable activation view. Wardian should be able to ingest from a catalog like this while preserving local source-of-truth records, source URL, install target, activation state, and review notes.

### Chops

**Source basis:** Public repo checked, including `ToolSource.swift`.

**What it includes:** Chops is a native macOS SwiftUI/SwiftData app for discovering, organizing, editing, and creating coding-agent skills and agents. It scans local tool directories, parses Markdown frontmatter and Cursor MDC files, watches the filesystem through FSEvents, and stores user collections as metadata rather than moving source files.

**Distinctive components:**

- Three-column native layout: tool filters, filtered skill/agent list, detail editor.
- Built-in monospaced editor with save behavior and metadata display.
- File watcher that rescans when skill files change on disk.
- Full-text search across name, description, and content.
- Collections that organize skills without modifying source files.
- Deduplication by resolved symlink path so one skill can appear under several tools.
- Support for both skills and agents across Claude Code, Cursor, Codex, Amp, OpenCode, OpenClaw, Hermes, Antigravity, Augment, Pi, and custom paths.

**Wardian relevance:** Chops is a relevant local-observability reference. Wardian should treat skills and agents as inspectable filesystem objects first, then layer organization, filters, collections, and activation badges on top. The important product pattern is "organize without hiding the file."

### Vercel `skills`

**Source basis:** Public repo checked.

**What it includes:** Vercel's `skills` package is a CLI for installing agent skills from GitHub shorthands, GitHub URLs, direct repo paths, GitLab URLs, arbitrary git URLs, and local paths. It supports project and global scopes, targeted agents, targeted skills, list/find/remove/update/init commands, symlink versus copy install modes, and a large supported-agent matrix.

**Distinctive components:**

- `add`, `list`, `find`, `remove`, `update`, and `init` command set.
- Project scope as the default, global scope by flag.
- Install all skills, specific skills, specific agents, or all agents.
- Symlink by default for canonical source-of-truth behavior, copy as fallback.
- Discovery of skills from common layouts such as root `SKILL.md`, `skills/`, `.agents/skills/`, `.claude/skills/`, `.codex/skills/`, and agent-specific directories.
- Plugin manifest discovery through Claude plugin manifest layouts.
- Compatibility posture around a shared Agent Skills specification.

**Wardian relevance:** This is a useful CLI contract reference. Wardian should expose similarly scriptable skill operations, but resolve them through Wardian's own library state so the UI can show where every skill came from, which agents can see it, whether the target is copied or linked, and when it was last updated.

### Skills Hub Desktop

**Source basis:** Public repo checked.

**What it includes:** Skills Hub is a Tauri + React desktop app that manages a central skill repository and syncs skills into global or project-level directories for many AI coding tools. It supports curated browsing, online search, one-click install and sync, tags, detail rendering, onboarding migration from existing tools, local folder import, Git URL import, update from source, and detection of newly installed tools.

**Distinctive components:**

- Central repo as canonical local storage.
- Global and project sync modes.
- Per-tool activation status.
- Symlink/junction preferred, copy fallback for restricted platforms or incompatible tools.
- Onboarding migration that scans existing tool skills and imports them into the central repo.
- Import workflows for local folders and multi-skill Git repositories.
- Scope controls and project-directory management.
- Broad tool matrix, including Claude Code, Cursor, Codex, OpenCode, Antigravity, Amp, Kimi, Augment, OpenClaw, Cline, Continue, Kiro, OpenHands, Pi, Qwen, Goose, Gemini, Copilot, Droid, Windsurf, Hermes, and others.

**Wardian relevance:** Skills Hub is a relevant structural reference if Wardian wants a central habitat/library that syncs into provider-specific directories. The key Wardian extension should be stronger observability: a skill should show canonical path, sync target, link/copy mode, owning collection, enabled agents, last validation, and provenance.

### Quiver

**Source basis:** Public repo checked.

**What it includes:** Quiver is a small local web UI and CLI for Claude Code skills. It scans local skills and Claude plugin marketplace layouts, serves a searchable interface, and exposes CLI commands for list, add, remove, import, export, registry browsing, sync, and local web-server startup.

**Distinctive components:**

- Local Express server and Preact UI.
- REST-style routes for listing, reading, saving, adding, deleting, importing, and exporting skills.
- CLI wrappers around the same inventory operations.
- Symlink add by default, copy add by flag.
- Zip-based import/export.
- Marketplace plugin browsing with source toggles and category/search filters.
- Remote sync commands for skill state across machines.

**Wardian relevance:** Quiver is useful because it keeps the management surface small and automatable. Wardian should consider a local API/CLI layer for skills that mirrors UI operations, so agents can ask "what skills are installed, where, and for whom?" without scraping the app.

### ClawHub

**Source basis:** Public repo checked.

**What it includes:** ClawHub is a hosted public skill registry for OpenClaw with a web app, Convex backend, shared API schema, CLI flows, vector search, publishing/versioning, comments, moderation, local install management, package catalog, pinning, and soft-delete/restore semantics. It also includes skill metadata for runtime requirements and security analysis.

**Distinctive components:**

- Publish new skill versions with changelogs, tags, and `latest`.
- Rename and merge owned skills while preserving old slugs.
- Vector search over skills.
- Star/comment and moderation/approval hooks.
- Local install commands: install, pin, unpin, uninstall, list, update.
- Skill and package catalogs under one registry surface.
- Runtime requirement metadata for environment variables, binaries, config, and install specs.
- Security analysis that compares declared requirements with behavior.

**Wardian relevance:** ClawHub is a relevant provenance and trust reference. Wardian does not need to become a hosted registry to learn from it: local skills should support version source, pinning, update policy, required capabilities, and review status. Registry metadata should remain separate from local activation so a user can install, inspect, pin, or quarantine without ambiguity.

### Trail of Bits Skills

**Source basis:** Public repo checked.

**What it includes:** Trail of Bits Skills is a curated Claude Code plugin marketplace for security analysis, testing, reverse engineering, mobile security, smart contracts, verification, infrastructure, and development workflows. It also includes a Codex-oriented sidecar under `.codex/skills/`.

**Distinctive components:**

- Domain-specific curated skill marketplace.
- Claude Code plugin marketplace install flow.
- Codex sidecar installation support.
- Large security-specialist taxonomy.
- Authoring guidance through repository instructions.

**Wardian relevance:** This is useful as a pack and curation reference. Wardian should support curated skill bundles and domain packs while exposing the specific files and provenance of each installed skill. A "security pack" should be inspectable as individual skills, not a black-box plugin.

### HOL Registry Broker Skills

**Source basis:** Public repo checked.

**What it includes:** HOL Registry Broker Skills provides skills and tooling for a Universal Agentic Registry. It supports search over live agent inventory, chat with agents, registration, publishing, verification, a CLI, MCP server, TypeScript SDK, direct API use, and GitHub Action-based publishing.

**Distinctive components:**

- CLI commands for config, init, lint, list, verify, publish, quote, and job status.
- MCP and SDK access to registry operations.
- Keyword, vector, and capability search.
- Agent details, trust scores, and similar-agent queries.
- Publish workflow that validates package files and records repo/commit metadata.

**Wardian relevance:** This is adjacent rather than a simple skill manager because it manages agent registry concerns too. Wardian should track it for interoperability: skills, agents, trust, and capability discovery are converging, and Wardian's local library should not assume skills are the only reusable object type.

### Everything Claude Code

**Source basis:** Assistant-suggested candidate; public repo checked.

**What it includes:** Everything Claude Code is a broad harness-performance system rather than a skill manager. It bundles skills, agents, hooks, rules, commands, install manifests, security scanning, memory/session patterns, dashboard tooling, and cross-harness support for Claude Code, Codex, Cursor, OpenCode, Gemini, and others.

**Distinctive components:**

- Selective installation profiles and component manifests.
- Skills mixed with agents, rules, hooks, commands, and security tooling.
- Cross-harness packaging and compatibility documentation.
- Session/state infrastructure and dashboard commands.
- Security and cost-control components alongside skill content.

**Wardian relevance:** This is a useful warning case. Real-world "skill libraries" quickly become bundles of runtime policy, hooks, commands, subagents, and safety tooling. Wardian should model these object types explicitly instead of flattening everything into "skill" or hiding non-skill side effects behind an install button.

## Implications for Wardian

Wardian should treat skill management as a local-library problem with registry hooks, not only as a package-install problem.

The likely model is:

```text
external source -> Wardian library record -> canonical local copy -> per-agent activation target -> observable status
```

That keeps the filesystem inspectable while still allowing catalogs, registries, and CLIs to feed the library.

Near-term design implications:

- Maintain a canonical Wardian skill library before syncing into provider-specific directories.
- Store provenance for every skill: source URL/path, source type, import date, last update check, pinned state, and trust/review notes.
- Distinguish `skill`, `agent`, `command`, `hook`, `rule`, `plugin`, and `bundle` as separate object types.
- Expose activation state per provider and per project, including whether the target is linked, junctioned, copied, disabled, missing, or stale.
- Keep human-facing organization metadata separate from source files, as Chops does with collections.
- Offer agent-friendly CLI/API operations for install, list, find, sync, pin, update, validate, and explain.
- Add compatibility adapters, but keep Wardian's own record as the source of truth.
- Treat registry trust metadata as advisory until the user or project policy accepts it.

The pattern that fits Wardian's current direction is not "download skills into dotfolders." It is "make skills visible habitat objects, then project them into the tool-specific places agents already know how to read."
