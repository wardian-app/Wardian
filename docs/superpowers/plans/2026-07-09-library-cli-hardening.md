# Library CLI Hardening Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make every accepted Library CLI mutation remain indexable, make destructive deployment cleanup state-checked, complete set-based deployment semantics, and make standalone class and flat-list behavior reliable for agents.

**Architecture:** Put filesystem and persistence invariants in `wardian-core`, where both Tauri and CLI callers share them. Keep `wardian-cli` responsible for Clap contracts, class initialization before CLI-only access, stable JSON/error envelopes, and agent-sized output. Reuse one cross-platform atomic JSON writer for class definitions, Library metadata, and existing conversation persistence.

**Tech Stack:** Rust 2021, Clap, Serde/serde_json, Wardian core Library APIs, Rust unit tests, CLI process integration tests.

## Global Constraints

- Do not add cross-process locking or merge semantics in this pass.
- Workflow validation, parsing, normalization, execution, scheduling, and run inspection remain under `wardian workflow`.
- Keep existing tree-shaped `library list` output backward compatible when `--flat` is absent.
- Keep malformed historical entries deletable so users and agents can repair old state.
- Preserve desktop-only provider discovery links in the Tauri layer.
- No frontend behavior changes; browser E2E and screenshot evidence are not required.

---

### Task 1: Atomic JSON Persistence and Standalone Class Initialization

**Files:**
- Create: `crates/wardian-core/src/atomic_file.rs`
- Modify: `crates/wardian-core/src/lib.rs`
- Modify: `crates/wardian-core/src/conversations.rs`
- Modify: `crates/wardian-core/src/library/metadata.rs`
- Modify: `crates/wardian-core/src/classes.rs`
- Modify: `src-tauri/src/manager/classes.rs`
- Test: `crates/wardian-core/src/atomic_file.rs`
- Test: `crates/wardian-core/src/classes.rs`
- Test: `crates/wardian-core/src/library/metadata.rs`

**Interfaces:**
- Produces: `atomic_file::write_json_atomic<T: Serialize>(path: &Path, value: &T) -> io::Result<()>`.
- Produces: `classes::initialize_classes(home: &Path) -> Result<Vec<AgentClassDefinition>, String>`.
- Preserves: `conversations::write_json_atomic` as a public compatibility wrapper.
- Changes: `classes::delete_class` also removes `classes/<Name>` metadata.

- [ ] **Step 1: Write failing core tests**

Add tests proving atomic JSON replacement overwrites valid JSON without leaving the temporary file, `initialize_classes` seeds default definitions plus `AGENTS.md`/provider stubs in a fresh home, and deleting/recreating a custom class does not retain metadata:

```rust
#[test]
fn write_json_atomic_replaces_existing_json_and_removes_temp_file() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("state.json");
    std::fs::write(&path, r#"{"old":true}"#).unwrap();

    write_json_atomic(&path, &serde_json::json!({"new": true})).unwrap();

    let value: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(value, serde_json::json!({"new": true}));
    assert!(!temp.path().join(".state.json.tmp").exists());
}

#[test]
fn initialize_classes_materializes_defaults_in_a_fresh_home() {
    let temp = tempfile::tempdir().unwrap();
    let classes = super::initialize_classes(temp.path()).unwrap();

    assert!(classes.iter().any(|class| class.name == "Reviewer"));
    let root = temp.path().join("classes/Reviewer");
    assert!(root.join("AGENTS.md").is_file());
    assert_eq!(std::fs::read_to_string(root.join("GEMINI.md")).unwrap(), "@AGENTS.md\n");
    assert_eq!(std::fs::read_to_string(root.join("CLAUDE.md")).unwrap(), "@AGENTS.md\n");
}
```

In the class deletion test, set metadata for `classes/Custom`, delete the class, recreate it, and assert `MetadataStore::load(home).get("classes/Custom").is_none()`.

- [ ] **Step 2: Run the focused tests and verify RED**

Run:

```powershell
cargo test -p wardian-core atomic_file
cargo test -p wardian-core initialize_classes_materializes_defaults_in_a_fresh_home
cargo test -p wardian-core delete_class_removes_metadata
```

Expected: compilation/test failures because `atomic_file`, `initialize_classes`, and metadata cleanup do not exist.

- [ ] **Step 3: Implement the atomic writer and class lifecycle changes**

Move the temp-path and cross-platform replacement implementation from `conversations.rs` into `atomic_file.rs`. The writer must create the parent, serialize pretty JSON plus a trailing newline, `sync_all`, and replace the destination. Keep this wrapper:

```rust
pub fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    crate::atomic_file::write_json_atomic(path, value)
}
```

Use the shared writer in `save_class_definitions` and `MetadataStore::save`. Implement initialization as:

```rust
pub fn initialize_classes(home: &Path) -> Result<Vec<AgentClassDefinition>, String> {
    let classes = if home.join("classes.json").exists() {
        load_class_definitions(home)?
    } else {
        let defaults = default_class_definitions();
        save_class_definitions(home, &defaults)?;
        defaults
    };
    for class in &classes {
        ensure_class_directory(home, class, None)?;
    }
    Ok(classes)
}
```

After removing a custom class definition, remove `classes/<Name>` from `MetadataStore` and save it. Refactor `init_agent_classes` to call `initialize_classes` after its legacy custom-class migration, then retain its provider-link loop.

- [ ] **Step 4: Run focused and dependent tests and verify GREEN**

Run:

```powershell
cargo test -p wardian-core atomic_file
cargo test -p wardian-core classes
cargo test -p wardian-core library::metadata
Set-Location src-tauri; cargo test manager::classes; Set-Location ..
```

Expected: all selected tests pass.

- [ ] **Step 5: Commit the task**

```powershell
git add crates/wardian-core/src/atomic_file.rs crates/wardian-core/src/lib.rs crates/wardian-core/src/conversations.rs crates/wardian-core/src/library/metadata.rs crates/wardian-core/src/classes.rs src-tauri/src/manager/classes.rs
git commit -m "fix(core): harden library class persistence"
```

---

### Task 2: Shared Library Entry-Shape Invariants

**Files:**
- Modify: `crates/wardian-core/src/library/mutations.rs`
- Modify: `crates/wardian-core/src/library/mod.rs`
- Modify: `crates/wardian-cli/src/library.rs`
- Test: `crates/wardian-core/src/library/mutations.rs`
- Test: `crates/wardian-cli/tests/library_cli.rs`

**Interfaces:**
- Produces: `library::validate_entry_destination(home: &Path, section: LibrarySectionId, rel: &str) -> Result<(), String>`.
- Changes: `save_item` and `rename_entry` enforce the same destination invariant internally.
- Consumes: CLI maps destination-validation failures to `CliError::invalid_ref` before mutation.

- [ ] **Step 1: Write failing core and CLI regression tests**

Add core tests for these exact cases:

```rust
assert!(save_item(home, LibrarySectionId::Prompts, "audit", "body").is_err());
assert!(save_item(home, LibrarySectionId::Workflows, "audit.txt", "body").is_err());

save_item(home, LibrarySectionId::Skills, "parent", "parent").unwrap();
assert!(save_item(home, LibrarySectionId::Skills, "parent/child", "child").is_err());

save_item(home, LibrarySectionId::Skills, "group/child", "child").unwrap();
assert!(save_item(home, LibrarySectionId::Skills, "group", "parent").is_err());
assert!(rename_entry(home, LibrarySectionId::Skills, "group/child", "parent/child").is_err());
```

Add CLI integration tests asserting extensionless prompt/workflow creation and both nested-skill conflict directions return `invalid_ref`, leave no content file behind, and preserve the previously indexed skill.

- [ ] **Step 2: Run the focused tests and verify RED**

Run:

```powershell
cargo test -p wardian-core library::mutations::tests::save_rejects_unindexable_entry_shapes
cargo test -p wardian-cli --test library_cli rejects_unindexable_entry_shapes
```

Expected: assertions fail because current saves accept all four malformed destinations.

- [ ] **Step 3: Implement destination validation in core and CLI**

`validate_entry_destination` must:

- require a case-insensitive `.md` extension for prompts and workflows;
- for skills, reject any ancestor directory containing `SKILL.md`;
- for skills, reject converting a directory without its own `SKILL.md` when a recursive descendant contains `SKILL.md`;
- allow writes to an existing valid skill itself;
- perform no additional restrictions for classes;
- rely on `resolve_entry_path` for traversal and MCP rejection.

Call it at the start of `save_item` and on `to_rel` before `rename_entry` mutates anything. In CLI create/write/move handlers, call it before reading stdin or changing the filesystem and map failures through `CliError::invalid_ref`.

- [ ] **Step 4: Run focused tests and verify GREEN**

Run:

```powershell
cargo test -p wardian-core library::mutations
cargo test -p wardian-cli --test library_cli
```

Expected: all mutation and Library CLI integration tests pass.

- [ ] **Step 5: Commit the task**

```powershell
git add crates/wardian-core/src/library/mutations.rs crates/wardian-core/src/library/mod.rs crates/wardian-cli/src/library.rs crates/wardian-cli/tests/library_cli.rs
git commit -m "fix(library): reject unindexable entries"
```

---

### Task 3: Safe and Complete Deployment Reconciliation

**Files:**
- Modify: `crates/wardian-core/src/library/deployments.rs`
- Modify: `crates/wardian-core/src/library/mutations.rs`
- Modify: `crates/wardian-cli/src/args.rs`
- Modify: `crates/wardian-cli/src/library.rs`
- Test: `crates/wardian-core/src/library/deployments.rs`
- Test: `crates/wardian-core/src/library/mutations.rs`
- Test: `crates/wardian-cli/src/args.rs`
- Test: `crates/wardian-cli/tests/library_cli.rs`

**Interfaces:**
- Changes: `remove_orphan_deployment(...) -> Result<bool, String>`, returning `false` without mutation when the exact tuple is not currently orphaned.
- Changes: `LibraryCommand::Deploy { skill_ref, targets: Option<String>, clear: bool }`.
- Changes: target parsing returns unique targets in first-seen order.

- [ ] **Step 1: Write failing deployment tests**

Add tests proving:

- `remove_orphan_deployment` returns `false` and preserves a healthy deployment;
- it returns `true` and removes a scanned orphan;
- duplicate desired targets produce `outcome.added == 1` and one scanned target;
- Clap accepts exactly one of `--targets` and `--clear`;
- CLI `deploy --clear` removes the final deployment;
- CLI duplicate targets return one target and `added: 1`;
- CLI healthy `orphan delete` returns `not_found` and preserves deployment state.

- [ ] **Step 2: Run the focused tests and verify RED**

Run:

```powershell
cargo test -p wardian-core deployment
cargo test -p wardian-cli parses_library_deploy_clear
cargo test -p wardian-cli --test library_cli deploy
cargo test -p wardian-cli --test library_cli orphan
```

Expected: failures show duplicate accounting, missing `--clear`, and unchecked orphan removal.

- [ ] **Step 3: Implement deployment safety and explicit clear**

Normalize desired targets with a `HashSet<(String, String)>` while retaining a `Vec<SkillDeployment>` in first-seen order. Use the normalized vector for both core reconciliation and CLI response JSON.

Before orphan removal, collect sources, scan deployments, and match all three fields. Return `Ok(false)` without touching the path when absent. CLI maps `false` to `CliError::library_not_found` using a synthetic ref `deployment/<target>/<skill>`.

Define Clap arguments with mutual exclusion and one-required semantics:

```rust
Deploy {
    skill_ref: String,
    #[arg(long, required_unless_present = "clear", conflicts_with = "clear")]
    targets: Option<String>,
    #[arg(long, required_unless_present = "targets", conflicts_with = "targets")]
    clear: bool,
}
```

`handle_deploy` passes an empty vector only for `clear == true`; an empty or malformed `--targets` string remains `invalid_target`.

- [ ] **Step 4: Run focused tests and verify GREEN**

Run:

```powershell
cargo test -p wardian-core library::deployments
cargo test -p wardian-core library::mutations
cargo test -p wardian-cli args::tests
cargo test -p wardian-cli --test library_cli
```

Expected: all selected tests pass.

- [ ] **Step 5: Commit the task**

```powershell
git add crates/wardian-core/src/library/deployments.rs crates/wardian-core/src/library/mutations.rs crates/wardian-cli/src/args.rs crates/wardian-cli/src/library.rs crates/wardian-cli/tests/library_cli.rs
git commit -m "fix(cli): make library deployments safe and complete"
```

---

### Task 4: Agent-Sized Flat Output, Class Access, and Help

**Files:**
- Modify: `crates/wardian-cli/src/args.rs`
- Modify: `crates/wardian-cli/src/library.rs`
- Test: `crates/wardian-cli/src/args.rs`
- Test: `crates/wardian-cli/tests/library_cli.rs`

**Interfaces:**
- Changes: scoped `--flat` omits `tree`; unscoped `--flat` returns combined deterministic entries and omits deployments/orphans.
- Changes: every flat row adds `section` without changing `LibraryEntry` storage models.
- Consumes: `classes::initialize_classes` before class listing/access and class deployment validation.

- [ ] **Step 1: Write failing flat-output, fresh-class, and help tests**

Extend `list_flat_outputs_section_entries` to assert `tree` is absent and each row has `section == "skills"`. Add an unscoped test that seeds one prompt and one skill, asserts both appear in `entries`, and asserts `sections`, `tree`, `deployments`, and `orphans` are absent.

Add a fresh-home test that runs class list/show/read/write and deployment to `class:Reviewer` without calling `seed_default_classes`, then checks `AGENTS.md`, `GEMINI.md`, and `CLAUDE.md` exist.

Add parser/help assertions that `wardian library --help` and `wardian library deploy --help` contain command descriptions, `--clear`, complete desired-set wording, and workflow-boundary wording.

- [ ] **Step 2: Run focused tests and verify RED**

Run:

```powershell
cargo test -p wardian-cli --test library_cli list_flat
cargo test -p wardian-cli --test library_cli fresh_home_default_classes
cargo test -p wardian-cli library_help_describes_agent_contracts
```

Expected: flat output still includes trees, unscoped flat lacks entries, fresh default classes are absent, and help descriptions are missing.

- [ ] **Step 3: Implement output shaping, initialization, and Clap docs**

For flat rows, serialize each `LibraryEntry` to a JSON object and insert its section name. Iterate `LibrarySectionId::ALL` for deterministic unscoped output rather than iterating the index `HashMap`.

Initialize classes only when needed:

- before unscoped lists and class-scoped lists;
- before show/read/create/write/delete/restore-default for a class ref;
- before validating class deployment targets.

Add `///` documentation to every `LibraryCommand`, `LibraryOrphanCommand`, and non-obvious field. Explain that workflow entries are authored here but operated through `wardian workflow`.

- [ ] **Step 4: Run focused tests and verify GREEN**

Run:

```powershell
cargo test -p wardian-cli args::tests
cargo test -p wardian-cli --test library_cli
```

Expected: all parser and Library CLI integration tests pass.

- [ ] **Step 5: Commit the task**

```powershell
git add crates/wardian-cli/src/args.rs crates/wardian-cli/src/library.rs crates/wardian-cli/tests/library_cli.rs
git commit -m "feat(cli): improve library agent ergonomics"
```

---

### Task 5: Documentation, Review, and Full Verification

**Files:**
- Modify: `docs/specs/2026-07-08-library-cli-agent-capabilities.md`
- Modify: `docs/guide/cli.md`
- Modify: `docs/guide/library.md`
- Verify: all files changed since `89a8e7b1`

**Interfaces:**
- Documents: `--clear`, unique targets, flat output shape, entry-shape restrictions, standalone class initialization, and the unchanged workflow boundary.

- [ ] **Step 1: Update user-facing documentation**

Document these exact examples:

```bash
wardian library list --flat
wardian library deploy skills/review/planner --clear
```

State that prompt/workflow refs end in `.md`, skills cannot contain other skills, deployment target lists are deduplicated, and class files initialize on first CLI access. Keep POSIX examples first and use no machine-specific paths.

- [ ] **Step 2: Run documentation and formatting checks**

Run:

```powershell
cargo fmt --all -- --check
git diff --check
git status --short
```

Expected: exit code 0 for formatting/diff checks and only intended files in status.

- [ ] **Step 3: Request independent Wardian review**

Ask `Wardian-Reviewer` to compare the implementation against `docs/specs/2026-07-09-library-cli-hardening.md`, reproduce the prior live failures, and return Critical/Important/Minor findings with file and line references. Fix all valid Critical and Important findings test-first.

- [ ] **Step 4: Run the complete repository verification checklist**

Run:

```powershell
cargo test -p wardian-cli
cargo test -p wardian-core
Set-Location src-tauri
cargo clippy
cargo test
cargo check
Set-Location ..
npm run lint
npm run test
npm run build
git diff --check
git status --short --branch
```

Expected: every command exits 0; frontend tests may retain the repository's existing jsdom canvas warnings and Vite chunk-size warning, with no new failures.

- [ ] **Step 5: Commit documentation and any review fixes**

```powershell
git add docs/specs/2026-07-08-library-cli-agent-capabilities.md docs/guide/cli.md docs/guide/library.md
git commit -m "docs(cli): document library hardening"
```

- [ ] **Step 6: Verify final branch state**

Run:

```powershell
git log --oneline --decorate -8
git status --short --branch
```

Expected: a clean `feat/library-cli-agent-capabilities` worktree with the hardening commits ahead of its base.
