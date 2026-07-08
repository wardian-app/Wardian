# Library CLI Agent Capabilities

- **Status:** Proposed
- **Date:** 2026-07-08

## Context and Problem Statement

The unified Library now stores Wardian's reusable agent capabilities in one
place: skills, prompts, classes, workflow blueprints, and the stubbed MCP
section. The desktop UI can inspect and edit these sections, but agents still
lack a first-class textual surface for the same work.

Agents need to turn useful work into durable Library assets without driving the
desktop UI. The CLI should let them contribute prompts and skills, edit class
instructions, deploy skills to the right scopes, and inspect workflow blueprint
files. It should do that without creating a second workflow runner, duplicating
Library business logic, or adding several aliases for the same behavior.

The prior Library redesign intentionally moved the core engine into
`wardian-core::library`. This slice uses that engine from `wardian-cli` instead
of reimplementing filesystem rules in the CLI.

## Decisions

### 1. One Library Namespace

All Library commands live under one namespace:

```bash
wardian library ...
```

Entries are addressed by section-qualified refs:

```text
skills/review/planner
prompts/triage.md
classes/Reviewer
workflows/audit.md
```

The supported sections are `skills`, `prompts`, `classes`, `workflows`, and
`mcps`. The MCP section remains read-only and stubbed until the MCP feature has
its own implementation.

### 2. Lean Command Surface

The first CLI slice intentionally avoids convenience aliases. Each command maps
to one durable behavior.

```bash
wardian library list [section] [--flat]
wardian library show <entry-ref> [--content]
wardian library read <entry-ref>

wardian library create <entry-ref> --stdin|--file <path>
wardian library write <entry-ref> --stdin|--file <path>
wardian library move <from-ref> <to-ref>
wardian library delete <entry-ref>

wardian library star <entry-ref>
wardian library unstar <entry-ref>
wardian library tags <entry-ref> --set <tag>...

wardian library deployments <skills/path>
wardian library deploy <skills/path> --targets <target-list>
wardian library orphans
wardian library orphan delete --target <target> --skill <name>

wardian library restore-default <classes/name>
```

`<target-list>` is comma-separated and uses explicit target refs:

```text
user:global,class:Reviewer,agent:<agent-id>
```

An empty target list means the skill should have no deployments:

```bash
wardian library deploy skills/review/planner --targets ""
```

PowerShell:

```powershell
wardian library deploy skills/review/planner --targets ""
```

### 3. Read, Show, Create, and Write Semantics

`list` returns Library index data. Without `--flat`, it returns tree-shaped JSON
for the requested section or all sections. With `--flat`, it returns agent-
friendly rows with `entry_ref`, `section`, `kind`, `name`, `description`,
`tags`, `is_starred`, `deployment_count`, and any entry error.

`show <entry-ref>` returns JSON metadata and resolved paths. It does not emit raw
content unless `--content` is set. For workflow entries, the response includes an
absolute `workflow_path` that can be passed to `wardian workflow ...`.

`read <entry-ref>` emits raw markdown content only. It has no JSON wrapper so it
can be piped directly into tools or another command.

`create <entry-ref>` creates a new entry and fails with `already_exists` if the
entry exists. Parent directories are created as needed for nested skills,
prompts, and workflows.

`write <entry-ref>` replaces content for an existing entry and fails with
`not_found` if the entry does not exist. There is no separate `save` command.

### 4. Section-Specific Rules

Skills are directories containing `SKILL.md`. `create skills/...` writes the
provided content to that file through the core Library content mapping.

Prompts and workflows are markdown files. The CLI writes the file content
directly at the section-relative path.

Classes are flat Library entries backed by `<wardian-home>/classes/<Name>/`.
`create classes/<Name>` creates the class directory, writes `AGENTS.md`, and
creates provider compatibility stubs in the same shape the desktop app uses.
`move classes/...` is not supported in this slice because class identity is
referenced by agents and runtime configuration. `delete classes/<Name>` must be
class-aware and reject default classes. `restore-default classes/<Name>` only
restores the default `AGENTS.md` for a built-in class; it does not change tags,
deployments, schedules, or agent assignments.

MCP entries are read-only. Mutations under `mcps` return `not_supported`.

### 5. Workflow Boundary

Workflow blueprints are stored in the Library, but workflow behavior remains in
the existing workflow CLI surface.

`wardian library` can:

- list, show, and read workflow blueprint files as `workflows/<path>.md`;
- create, write, move, and delete workflow blueprint files;
- return Library metadata and a `workflow_path` for handoff.

`wardian library` does not validate, parse, normalize, execute, replay,
schedule, inspect runs, or resolve workflow blueprints by declared id. Those
belong to:

```bash
wardian workflow validate <path-to-workflow.md>
wardian workflow parse <path-to-workflow.md>
wardian workflow normalize <path-to-workflow.md> --write
wardian workflow exec <path-to-workflow.md>
wardian workflow schedule ...
wardian workflow runs
```

This keeps Library authoring separate from workflow execution and monitoring.

### 6. Deployment Model

Skill deployment is set-based. The command:

```bash
wardian library deploy skills/review/planner --targets user:global,class:Reviewer
```

means "make these the complete desired targets for this skill." The CLI calls
`wardian_core::library::set_skill_deployments`, so it adds missing targets and
removes targets not present in the supplied list.

`wardian library deployments <skills/path>` reports the current target list for
one skill. `wardian library orphans` reports unresolved deployed skill
directories. `wardian library orphan delete` removes one unresolved deployment
directory and requires both the target and deployed skill directory name.

The CLI does not add separate `deployments add`, `deployments remove`, or
`undeploy` verbs in this slice.

### 7. Metadata Model

The first slice exposes only common metadata edits:

- `star`
- `unstar`
- `tags --set <tag>...`

`show` and `list` expose metadata for reading. Raw metadata JSON get/set is
deferred until there is a concrete use case that cannot be expressed through
these stable verbs.

### 8. Output and Errors

All non-`read` commands emit JSON on stdout with `schema: 1`. Mutation responses
include `ok`, the affected `entry_ref` where applicable, and operation-specific
fields such as deployment reconciliation counts.

Errors use the existing CLI error envelope on stderr. The Library CLI adds or
reuses these stable codes:

- `invalid_ref`
- `unknown_section`
- `not_supported`
- `not_found`
- `already_exists`
- `invalid_target`
- `request_failed`

Most Library commands operate directly on disk and do not require the desktop
app. A live app will pick up CLI-driven filesystem changes through the existing
Library watcher.

### 9. Implementation Boundary

`crates/wardian-cli` adds a `library` module that adapts arguments to the
existing core engine:

- `build_library_index`
- `read_item`
- `save_item`
- `rename_entry`
- `delete_entry`
- `update_metadata`
- `set_skill_deployments`
- `remove_orphan_deployment`

The CLI resolves `WARDIAN_HOME` with `wardian_core::paths::wardian_home()` and
passes explicit `home: &Path` values into `wardian-core`. No Tauri types or app
state enter the CLI path.

Class create, class delete, and restore-default need reusable class-management
helpers in `wardian-core` or another non-Tauri module before the CLI wires them.
The CLI should not copy logic from `src-tauri/src/commands/class.rs` directly.

### 10. Testing and Verification

Rust unit tests cover:

- ref parsing and section validation;
- create/write existence rules;
- class mutation constraints;
- workflow boundary fields, especially `workflow_path`;
- flat vs tree `list` output;
- raw `read` output;
- tag/star metadata mutation;
- set-based deployment parsing and reconciliation;
- orphan listing and deletion;
- JSON error envelopes.

CLI integration tests seed an isolated `WARDIAN_HOME`, run the built
`wardian-cli` binary, and assert disk results plus stdout/stderr contracts.

Native E2E coverage is required only for deployment claims that depend on real
junction or symlink behavior. Browser E2E cannot prove those filesystem details.

## Consequences

- **Positive:** Agents gain the same Library authoring powers as users without
  driving the desktop UI.
- **Positive:** A single section-qualified grammar covers skills, prompts,
  classes, and workflow files while preserving each section's real constraints.
- **Positive:** Workflow execution remains centralized in `wardian workflow`, so
  the Library CLI does not become a second workflow runtime.
- **Positive:** Set-based deployment matches the core engine and avoids a
  proliferation of add/remove/undeploy aliases.
- **Negative:** `deploy --targets ""` is a little awkward for undeploy-all, but
  it preserves one canonical deployment behavior.
- **Negative:** Raw metadata JSON mutation is deferred, so unusual metadata
  edits may still require direct file editing until a real use case justifies a
  broader command.
- **Negative:** Class create/delete requires moving or extracting some
  app-adjacent class logic before the CLI can remain thin and testable.
