# Library Redesign: Unified Detailed-List Library

- **Status:** Implemented
- **Date:** 2026-07-05

## Sources

- [Malleable Software](https://www.inkandswitch.com/essay/malleable-software/)
  informs the Library's role as an in-place authoring surface with many small
  steps from use to creation, rather than a cliff from configuration to plugin
  development.
- [Entity-Oriented Agent Semantics](./2026-07-14-entity-oriented-agent-semantics.md)
  records how Library assets contribute to a durable entity's reusable context,
  relationships, and operational protocols without becoming its sole definition.

## Context and Problem Statement

Wardian's library is one of its most distinctive features: skills and prompts can be
scoped to agents, classes, or the user via filesystem links. But the surface has not
kept up with the concept:

- The UI is a card grid with breadcrumb drill-down and three modals (item editor,
  assign-skill, assign-prompt). Specialized skill managers such as
  [aghub](https://github.com/AkaraChen/aghub) and
  [chops](https://github.com/Shpigford/chops) have converged on a cleaner pattern:
  a searchable detailed list plus a persistent detail pane with an inline editor.
- Library-adjacent content is scattered. Workflow blueprints already live under
  `<wardian-home>/library/workflows/` but are managed in a separate view; classes
  live under `<wardian-home>/classes/` and are managed from a left-sidebar panel
  (`ClassManagerPanel`).
- Skill deployment is unusually slow on Windows. Every junction is created by
  spawning `cmd.exe /C mklink /J` (`commands/library.rs`, `utils/fs.rs`), each
  deploy triggers an Antigravity projection refresh (recursive directory copies),
  and per-skill deployment lookups re-scan every agent/class directory with
  `canonicalize()` calls.
- The tree API embeds full file content for every item, making every refresh cost
  proportional to total library size, and item descriptions are taken from the
  first line of `SKILL.md` (usually `---`).

## Library as a Tailoring Surface

The Library is not only a repository of files. It is Wardian's in-place path for
capturing a useful local adaptation and promoting it at the smallest appropriate
level of reuse:

| Need discovered in practice | Smallest Library or runtime move |
| --- | --- |
| Repeat an instruction. | Save or run a prompt. |
| Reuse a procedure or contextual capability. | Create a skill. |
| Apply that procedure to a chosen scope. | Deploy the skill to an agent, class, or user scope. |
| Reuse an agent blueprint. | Edit or create a class with its instructions, defaults, and skills. |
| Reuse a multi-entity process. | Author a workflow blueprint. |
| Preserve a result worth reusing. | Promote evidence into a reviewed memory, prompt, skill, or workflow input. |

The Library must keep these moves inspectable and reviewable. Prompt, skill,
class, and workflow assets remain distinct: a prompt is a reusable invocation,
a skill is a reusable procedure or context, a class is a reusable blueprint,
and a workflow is a process with typed runtime semantics. They should not be
collapsed into a single free-form persona document.

This redesign does not add first-class asset variants, lightweight branching, or
revision comparison. Those are a future part of the tailoring slope. When they
are introduced, Library assets and workflow runs should preserve lineage and
the exact asset revisions used, so a local experiment can be shared or adopted
without silently changing a live deployment.

## Decisions Already Made

Settled during design review with the maintainer:

1. The library becomes the **single home for classes**; `ClassManagerPanel` is
   removed. Spawn-with-class stays in the agent spawn flow.
2. **Workflow blueprints only** move into the library. Run observation, monitoring,
   and history stay in `WorkflowsView`.
3. The three modals are replaced by a **full detail pane** with an inline editor.
4. The view is **self-contained**: it does not mutate the global left sidebar.
   Section switching uses a slim in-view rail (icon + label, ~52px).
5. Sections: **skills, prompts, classes, workflows, MCPs**. MCPs ship as a
   designed-for but stubbed section.
6. Backend approach: a **unified library index command** (Approach A), not
   frontend composition over existing per-domain commands.
7. Delivery: **one PR** on `feat/library-redesign`.

## Proposed Decision

### 1. Data model: `get_library_index`

One Tauri command returns a metadata-only index:

```text
LibraryIndex {
  sections: {
    skills:    LibrarySection { tree: Folder -> [Folder | Entry] },
    prompts:   LibrarySection,
    workflows: LibrarySection,
    classes:   LibrarySection,      // flat; classes have no folder nesting
    mcps:      LibrarySection,      // stubbed: true, always empty for now
  },
  deployments: Map<entry_ref, DeploymentTarget[]>,  // computed in one pass
  orphans:     DeploymentTarget[],                  // deployments with no resolvable source
}
```

- **Entries carry metadata only** (`kind`, `path`, `name`, `description`, `tags`,
  `is_starred`, `deployment_count`, `linked`/`copied` state, `error` flag). File
  content is loaded lazily by `read_library_item(section, path)` when the detail
  pane selects an entry. Watcher-driven refreshes therefore stay cheap.
- **Descriptions parse YAML frontmatter** (`description:`) for skills and
  workflows, falling back to the first non-frontmatter content line. Malformed
  frontmatter never fails the index.
- **Entry refs are section-qualified** (`skills/dev/planner`). `library.json`
  metadata keys migrate to this form once, on first read; stale keys are dropped.
- **Deployment map is built in a single pass** over `common/`, `classes/*/`, and
  `agents/*/` skill directories, using the existing resolution order: source
  marker file (`.wardian-skill-source`), canonical path match, then unique-name
  inference. Unresolvable deployments are reported as `orphans` instead of being
  silently ignored.
- **Classes adapt into the same shape**: name from the class directory, description
  from `AGENTS.md` frontmatter or first heading, deployed skills from the
  target-side view of the same deployment map. No second scan.
- **MCPs**: the section constant and rail entry exist; the backend returns an empty
  section flagged `stubbed`. No `library/mcps` directory is created until the real
  feature lands (its own future spec).

### 2. Backend layering: library engine lives in `wardian-core`

A follow-up phase will expose library manipulation through `wardian-cli` so
agents can manage the library themselves. To make that a thin adapter rather
than a rewrite, all library logic introduced by this redesign lands in a new
`library` module in `crates/wardian-core`, not in the Tauri app crate:

- **In `wardian-core`:** index building, frontmatter parsing, metadata
  read/migrate/write, the one-pass deployment map, junction/symlink creation
  with copy fallback, `set_skill_deployments` diffing, and all CRUD operations.
  Everything operates on `WARDIAN_HOME`-derived paths (via `wardian_core::paths`)
  with no Tauri types.
- **In the Tauri layer:** thin `#[command]` wrappers, event emission
  (`library-changed`), the filesystem watcher, and the Antigravity projection
  refresh (it needs live agent state, which only the app has).
- **CLI-readiness consequence:** a future `wardian-cli library ...` command set
  calls the same core functions. Because the app's watcher observes the
  filesystem rather than in-process mutations, CLI- or agent-driven changes
  appear in the UI automatically — Markdown-as-Truth doing the synchronization.
  Existing `commands/library.rs` logic migrates into core as part of this PR
  rather than being duplicated later.

### 3. Backend commands and deployment performance

- **Junction creation goes native.** `link_skill_dir` and `create_directory_link`
  replace `cmd.exe /C mklink /J` with the `junction` crate
  (`FSCTL_SET_REPARSE_POINT` ioctl): no process spawn, no console host, no AV scan
  of `cmd.exe`. Unix keeps `std::os::unix::fs::symlink`. The copy fallback and
  marker-file logic are unchanged; existing behavioral tests verify the swap.
- **Set-based deployment.** New command `set_skill_deployments(source_path,
  targets[])` diffs desired vs. current targets, creates/removes links
  accordingly, and runs the Antigravity projection refresh once at the end, with
  an early exit when no live Antigravity agents exist. `deploy_skill` and
  `remove_deployed_skill` remain as thin wrappers over the same core
  single-target operations for existing callers.
- **CRUD completed.** New commands: `create_library_folder`,
  `rename_library_entry` (rename and move are the same operation),
  `delete_library_entry`. Deleting or renaming a deployed skill first
  cleans up / re-creates its deployments in the same backend operation so links
  never dangle. All mutations validate that the resolved path stays inside its
  section root and reject reserved names (`SKILL.md`, `.wardian-skill-source`).
- **Class workbench.** `read_library_item` / `save_library_item` resolve
  `classes/<Name>` to the classes root and handle `AGENTS.md` like any other
  entry. Class create/delete and provider defaults reuse the existing
  `commands/class.rs` surface. Deploying skills to a class from the class detail
  pane calls the same `set_skill_deployments`, target-first.
- **Watcher generalizes.** `library_watch` extends from skills-only to one
  registration watching the `library/` root plus the `classes/` root, emitting
  `library-changed { section }`. The debounced handler rebuilds the (cheap,
  metadata-only) index.

### 4. Frontend architecture

All components live under `src/features/library/`; the shell stays
`src/views/LibraryView.tsx`.

```text
LibraryView
├── SectionRail          five sections, icon + label, per-section count badge
├── LibraryList
│   ├── ListToolbar      search, starred toggle, tag filter, new item / new folder
│   └── FolderGroup(s)   collapsible headers; rows: name, description, tags,
│                        star, deployment badge (n deployed / drift warning)
└── DetailPane           kind-switched:
    ├── SkillDetail      frontmatter header, editor, deploy-targets control
    ├── PromptDetail     editor + "Run on selected agents" (current behavior kept)
    ├── WorkflowDetail   editor + Launch Run + link to Workflows view
    ├── ClassDetail      AGENTS.md editor, deployed skills, provider defaults
    └── McpStubDetail    empty state describing the upcoming feature
```

- **Retired in the same PR:** `LibraryGrid`, `LibraryCard`, `ItemEditorModal`,
  `AssignSkillModal`, `AssignPromptModal`, `ManageSkills`, `ClassManagerPanel`
  (and its `SidebarContentPane` slot). Entry points that opened the class panel
  deep-link to `library -> classes -> <name>`.
- **Store.** `useLibraryStore` is rewritten around the index: `index`,
  `selection {section, path}`, `expandedFolders`, `searchQuery`, `filters`, and a
  small `contentCache`. One `library-changed` subscription refreshes the index.
  If the selected entry changes on disk while the editor is clean, content
  reloads silently; if dirty, a "file changed on disk" conflict bar offers
  reload / keep-mine.
- **Editor.** Inline monospace editor with dirty indicator, `Ctrl+S` save, and an
  unsaved-changes prompt on selection change. No autosave: skills are live-linked
  into agent sessions, so half-typed saves would propagate instantly.
- **Search vs. hierarchy.** Browsing shows collapsible folder groups. Searching
  flattens the section into ranked matches (name > description > tags > content)
  with the folder path as a row subtitle.
- **Tactile interactions.** Rows drag onto folder group headers to move items
  (`rename_library_entry`); skill rows drag onto the deploy-targets control.
  No drag-to-roster in this PR.
- **Theming.** Semantic theme variables throughout. Deployment badges use the
  standard status colors: emerald for deployed and healthy, amber for
  drift/orphan/copied states.

### 5. Error handling and edge cases

- Link failure falls back to copy + marker (unchanged) but reports
  `linked: false` so the UI shows "copied — edits won't sync".
- Orphaned deployments get an amber drift badge and one-click removal.
- Renaming a deployed skill re-creates its junctions and rewrites copy markers in
  the same backend operation.
- Deleting a class removes its directory; junctions inside are reparse points, so
  `remove_dir_all` deletes links without touching library sources (pinned by a
  test on both OS families).
- Unreadable files become entries with an `error` flag, not missing rows. Missing
  section directories are created on demand. DTO paths normalize to `/`.
- Mutation errors surface inline where the action happened (deploy control,
  editor save bar); new paths do not swallow errors with `let _ =`.

### 6. Testing and verification

- **Rust unit tests:** index building (sections, frontmatter parsing and
  fallback, class adaptation, metadata migration); one-pass deployment map
  (marker / canonical / unique-name resolution, orphans, duplicate names);
  `set_skill_deployments` diffing; rename re-linking; delete cleanup;
  path-traversal rejection; junction-in-deleted-dir safety. Existing live-link
  behavioral tests verify the junction crate swap unchanged.
- **Frontend unit tests:** store behaviors (load, selection, dirty guard,
  conflict bar), list rendering (folder groups, search flattening, filters),
  each detail pane kind, section rail badges, and updates to `App` /
  `SidebarContentPane` tests for the panel removal.
- **Browser E2E** (seeded `WARDIAN_HOME`): navigate all sections, search, edit
  and save a prompt, starred filter, class workbench rendering, deployment rows
  from seeded state.
- **Native E2E:** deploy through the real backend and assert a reparse point
  exists and source edits propagate; rename follows; delete leaves no dangling
  link. Browser specs that require this layer are `test.skip` +
  `// @native-only`.
- **PR evidence:** timed before/after script for 20 sequential deploys on
  Windows (expected: seconds to milliseconds), plus feature screenshots of the
  rail / list / detail pane embedded in the PR description.

## Consequences

- **Positive:** One inspectable index and one store replace four data paths; the
  library becomes the single surface for everything deployable or assignable.
- **Positive:** Deployment latency drops from process-spawn scale to ioctl scale,
  and deployment badges become viable because lookups no longer scan per skill.
- **Positive:** Orphaned and copied deployments become visible drift instead of
  silent state.
- **Positive:** With the engine in `wardian-core`, the planned
  `wardian-cli library` phase becomes thin argument-parsing over existing,
  tested functions, and CLI/agent-driven edits surface in the UI via the
  filesystem watcher with no extra plumbing.
- **Negative:** Large single PR: view rewrite, sidebar panel removal, backend
  index, and perf fix land together; revert is coarse.
- **Negative:** Existing commands (`get_library_tree`, per-skill deployment
  lookups) become legacy surface kept only for the CLI until a follow-up
  consolidates them.
- **Negative:** The MCP section ships as a visible stub, which advertises a
  feature before it exists.
