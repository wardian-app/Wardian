# Library Redesign Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rebuild Wardian's library as a unified rail/list/detail view over skills, prompts, classes, workflow blueprints, and a stubbed MCP section, backed by a `wardian-core` library engine with native (non-`cmd.exe`) junction deployment.

**Architecture:** All library logic (index, deployment map, junction linking, CRUD, deployment diffing) moves into a new `library` module in `crates/wardian-core` operating on explicit `home: &Path` arguments. The Tauri layer keeps thin `#[command]` wrappers, the filesystem watcher, event emission, and the Antigravity projection refresh. The frontend replaces the card grid + three modals with `SectionRail` / `LibraryList` / `DetailPane` fed by one metadata-only `get_library_index` command.

**Tech Stack:** Rust (Tauri 2, `junction` crate, `serde_norway` for YAML frontmatter, `notify`), React + TypeScript + Zustand, Vitest + React Testing Library, Playwright browser E2E, native WebDriver E2E.

**Spec:** `docs/specs/2026-07-05-library-redesign.md` — read it before starting.

## Global Constraints

- Branch: `feat/library-redesign`. One PR. Never commit to `main`.
- DTO properties are `snake_case` in both Rust and TypeScript (IPC serialization parity).
- Frontend types live in `src/types/index.ts`; never use `any`.
- UI colors use theme variables (`var(--color-wardian-*)`) or themed classes (`.text-muted`), never hardcoded Tailwind palette colors. Status colors: emerald = healthy/deployed, amber = drift/copied, red = error.
- Core library functions take `home: &Path` explicitly (testable, CLI-ready); only the Tauri layer resolves the home via `get_wardian_home()` / `wardian_core::paths`.
- Entry refs are section-qualified: `skills/dev/planner`, `prompts/greeting`, `workflows/triage`, `classes/Architect`. DTO paths always use `/` separators.
- Rust env-var tests take `crate::utils::wardian_test_env_lock()` (app crate) or `tests::env_lock()` (core crate) and run with `--test-threads=1` if flaky.
- Commit after every green test cycle with conventional-commit messages ending in the Claude co-author trailer.
- Verification before PR: `npm run lint`, `npm run test`, `npm run build`, and in `src-tauri`: `cargo clippy`, `cargo test`, `cargo check`.

---

### Task 1: Core section model and path safety

**Files:**
- Create: `crates/wardian-core/src/library/mod.rs`
- Create: `crates/wardian-core/src/library/section.rs`
- Modify: `crates/wardian-core/src/lib.rs` (add `pub mod library;`)
- Modify: `crates/wardian-core/src/paths.rs` (add `_for_home` helpers)

**Interfaces:**
- Produces: `LibrarySectionId` enum (`Skills | Prompts | Workflows | Classes | Mcps`) with `parse(&str) -> Option<Self>`, `as_str(&self) -> &'static str`, `root_for_home(&self, home: &Path) -> PathBuf`.
- Produces: `resolve_entry_path(home: &Path, section: LibrarySectionId, rel: &str) -> Result<PathBuf, String>` — rejects traversal, absolute paths, reserved names.
- Produces: `paths::library_dir_for_home`, `paths::library_metadata_path_for_home`, `paths::classes_dir_for_home`, `paths::common_dir_for_home`.

- [ ] **Step 1: Write failing tests** in `crates/wardian-core/src/library/section.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn section_roots_resolve_under_home() {
        let home = Path::new("/tmp/wh");
        assert_eq!(LibrarySectionId::Skills.root_for_home(home), home.join("library").join("skills"));
        assert_eq!(LibrarySectionId::Prompts.root_for_home(home), home.join("library").join("prompts"));
        assert_eq!(LibrarySectionId::Workflows.root_for_home(home), home.join("library").join("workflows"));
        assert_eq!(LibrarySectionId::Classes.root_for_home(home), home.join("classes"));
    }

    #[test]
    fn parse_round_trips() {
        for id in ["skills", "prompts", "workflows", "classes", "mcps"] {
            assert_eq!(LibrarySectionId::parse(id).unwrap().as_str(), id);
        }
        assert!(LibrarySectionId::parse("plugins").is_none());
    }

    #[test]
    fn resolve_entry_path_rejects_escapes() {
        let home = Path::new("/tmp/wh");
        assert!(resolve_entry_path(home, LibrarySectionId::Skills, "../evil").is_err());
        assert!(resolve_entry_path(home, LibrarySectionId::Skills, "a/../../evil").is_err());
        assert!(resolve_entry_path(home, LibrarySectionId::Skills, "/abs").is_err());
        assert!(resolve_entry_path(home, LibrarySectionId::Skills, "").is_err());
        assert!(resolve_entry_path(home, LibrarySectionId::Skills, "dev/planner").is_ok());
    }

    #[test]
    fn resolve_entry_path_rejects_reserved_names() {
        let home = Path::new("/tmp/wh");
        assert!(resolve_entry_path(home, LibrarySectionId::Skills, "dev/.wardian-skill-source").is_err());
        assert!(resolve_entry_path(home, LibrarySectionId::Mcps, "anything").is_err()); // stubbed section: no paths
    }
}
```

- [ ] **Step 2: Run** `cargo test -p wardian-core library::section` from repo root. Expected: compile FAIL (module missing).

- [ ] **Step 3: Implement** `section.rs`:

```rust
use std::path::{Component, Path, PathBuf};

pub const DEPLOYED_SKILL_SOURCE_FILE: &str = ".wardian-skill-source";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LibrarySectionId {
    Skills,
    Prompts,
    Workflows,
    Classes,
    Mcps,
}

impl LibrarySectionId {
    pub const ALL: [LibrarySectionId; 5] = [
        LibrarySectionId::Skills,
        LibrarySectionId::Prompts,
        LibrarySectionId::Workflows,
        LibrarySectionId::Classes,
        LibrarySectionId::Mcps,
    ];

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "skills" => Some(Self::Skills),
            "prompts" => Some(Self::Prompts),
            "workflows" => Some(Self::Workflows),
            "classes" => Some(Self::Classes),
            "mcps" => Some(Self::Mcps),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Skills => "skills",
            Self::Prompts => "prompts",
            Self::Workflows => "workflows",
            Self::Classes => "classes",
            Self::Mcps => "mcps",
        }
    }

    pub fn root_for_home(&self, home: &Path) -> PathBuf {
        match self {
            Self::Skills => home.join("library").join("skills"),
            Self::Prompts => home.join("library").join("prompts"),
            Self::Workflows => home.join("library").join("workflows"),
            Self::Classes => home.join("classes"),
            // Stubbed: no directory is created until the MCP feature lands.
            Self::Mcps => home.join("library").join("mcps"),
        }
    }
}

/// Resolve a section-relative entry path, rejecting traversal, absolute
/// paths, empty paths, reserved file names, and the stubbed MCP section.
pub fn resolve_entry_path(
    home: &Path,
    section: LibrarySectionId,
    rel: &str,
) -> Result<PathBuf, String> {
    if section == LibrarySectionId::Mcps {
        return Err("The MCP section is not yet writable".to_string());
    }
    let normalized = rel.replace('\\', "/");
    if normalized.trim().is_empty() {
        return Err("Entry path must not be empty".to_string());
    }
    let candidate = Path::new(&normalized);
    if candidate.is_absolute() {
        return Err(format!("Entry path must be relative: {rel}"));
    }
    for component in candidate.components() {
        match component {
            Component::Normal(part) => {
                let text = part.to_string_lossy();
                if text == DEPLOYED_SKILL_SOURCE_FILE {
                    return Err(format!("Reserved name in entry path: {text}"));
                }
            }
            _ => return Err(format!("Invalid entry path: {rel}")),
        }
    }
    Ok(section.root_for_home(home).join(candidate))
}
```

Create `mod.rs` with `pub mod section;` and re-export: `pub use section::{resolve_entry_path, LibrarySectionId, DEPLOYED_SKILL_SOURCE_FILE};`. Add `pub mod library;` to `lib.rs` (alphabetical order). Add to `paths.rs`:

```rust
/// `<wardian-home>/library`.
pub fn library_dir_for_home(home: &Path) -> PathBuf {
    home.join("library")
}

/// `<wardian-home>/library/library.json` — tags/stars metadata index.
pub fn library_metadata_path_for_home(home: &Path) -> PathBuf {
    home.join("library").join("library.json")
}

/// `<wardian-home>/classes`.
pub fn classes_dir_for_home(home: &Path) -> PathBuf {
    home.join("classes")
}

/// `<wardian-home>/common`.
pub fn common_dir_for_home(home: &Path) -> PathBuf {
    home.join("common")
}
```

- [ ] **Step 4: Run** `cargo test -p wardian-core library::section`. Expected: PASS.

- [ ] **Step 5: Commit** `feat(core): add library section model and path safety`

---

### Task 2: Frontmatter parsing and description extraction

**Files:**
- Create: `crates/wardian-core/src/library/frontmatter.rs`
- Modify: `crates/wardian-core/src/library/mod.rs`

**Interfaces:**
- Produces: `parse_frontmatter(content: &str) -> (Option<serde_norway::Value>, &str)` — returns parsed YAML (None if absent/malformed) and the body after the frontmatter block.
- Produces: `extract_description(content: &str) -> String` — frontmatter `description:` first, else first non-empty body line stripped of leading `#` markers, else empty string.

- [ ] **Step 1: Write failing tests** in `frontmatter.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn description_from_frontmatter() {
        let content = "---\nname: planner\ndescription: Plans work\n---\n# Planner\nBody";
        assert_eq!(extract_description(content), "Plans work");
    }

    #[test]
    fn description_falls_back_to_first_body_line() {
        assert_eq!(extract_description("# Planner skill\nBody"), "Planner skill");
        assert_eq!(extract_description("---\nname: x\n---\n\n## Heading\n"), "Heading");
    }

    #[test]
    fn malformed_frontmatter_never_panics() {
        let content = "---\n: : bad yaml [\n---\nFallback line";
        let (fm, body) = parse_frontmatter(content);
        assert!(fm.is_none());
        assert!(body.contains("Fallback line"));
        assert_eq!(extract_description(content), "Fallback line");
    }

    #[test]
    fn no_frontmatter_returns_whole_body() {
        let (fm, body) = parse_frontmatter("just text");
        assert!(fm.is_none());
        assert_eq!(body, "just text");
    }
}
```

- [ ] **Step 2: Run** `cargo test -p wardian-core library::frontmatter`. Expected: FAIL.

- [ ] **Step 3: Implement:**

```rust
/// Split a markdown document into (frontmatter, body). The frontmatter is
/// `None` when absent or unparseable; the body always excludes the block.
pub fn parse_frontmatter(content: &str) -> (Option<serde_norway::Value>, &str) {
    let rest = match content.strip_prefix("---") {
        Some(rest) => rest,
        None => return (None, content),
    };
    let rest = rest.strip_prefix('\n').or_else(|| rest.strip_prefix("\r\n"));
    let Some(rest) = rest else { return (None, content) };
    let Some(end) = rest.find("\n---") else { return (None, content) };
    let yaml_text = &rest[..end];
    let after = &rest[end + 4..];
    let body = after.trim_start_matches(['\r', '\n']);
    match serde_norway::from_str::<serde_norway::Value>(yaml_text) {
        Ok(value) => (Some(value), body),
        Err(_) => (None, body),
    }
}

/// Human description for list rows: frontmatter `description`, else the
/// first non-empty body line without markdown heading markers.
pub fn extract_description(content: &str) -> String {
    let (frontmatter, body) = parse_frontmatter(content);
    if let Some(description) = frontmatter
        .as_ref()
        .and_then(|value| value.get("description"))
        .and_then(|value| value.as_str())
    {
        let trimmed = description.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    body.lines()
        .map(|line| line.trim().trim_start_matches('#').trim())
        .find(|line| !line.is_empty())
        .unwrap_or_default()
        .to_string()
}
```

Add `pub mod frontmatter;` and re-export both functions from `library/mod.rs`.

- [ ] **Step 4: Run** `cargo test -p wardian-core library::frontmatter`. Expected: PASS.

- [ ] **Step 5: Commit** `feat(core): parse library frontmatter descriptions`

---

### Task 3: Native directory links (the junction perf fix)

**Files:**
- Create: `crates/wardian-core/src/library/links.rs`
- Modify: `crates/wardian-core/Cargo.toml`
- Modify: `crates/wardian-core/src/library/mod.rs`

**Interfaces:**
- Produces: `create_directory_link(target: &Path, link: &Path) -> std::io::Result<()>` — junction ioctl on Windows (via `junction` crate), `std::os::unix::fs::symlink` elsewhere; creates parent dirs.
- Produces: `remove_existing_deployment(path: &Path) -> std::io::Result<()>` — removes a link without following it, or a real dir/file recursively.
- Produces: `deploy_skill_dir(src_dir: &Path, dst_dir: &Path) -> std::io::Result<bool>` — link with copy fallback; returns `true` when copied. Writes no marker (caller's job — it knows the source path).
- Produces: `copy_dir_all(src: &Path, dst: &Path) -> std::io::Result<()>`.

- [ ] **Step 1: Add the dependency** to `crates/wardian-core/Cargo.toml`:

```toml
[target.'cfg(windows)'.dependencies]
junction = "1"
```

- [ ] **Step 2: Write failing tests** in `links.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn link_is_live_and_removal_preserves_target() {
        let temp = tempfile::tempdir().expect("temp dir");
        let target = temp.path().join("source-skill");
        let link = temp.path().join("deploys").join("planner");
        fs::create_dir_all(&target).expect("target dir");
        fs::write(target.join("SKILL.md"), "one").expect("skill file");

        create_directory_link(&target, &link).expect("create link");
        fs::write(target.join("SKILL.md"), "two").expect("update source");
        assert_eq!(fs::read_to_string(link.join("SKILL.md")).expect("read via link"), "two");

        remove_existing_deployment(&link).expect("remove link");
        assert!(!link.exists());
        assert!(target.join("SKILL.md").exists(), "removing link must not touch target");
    }

    #[test]
    fn removing_parent_dir_of_link_preserves_target() {
        let temp = tempfile::tempdir().expect("temp dir");
        let target = temp.path().join("source-skill");
        let parent = temp.path().join("class-skills");
        fs::create_dir_all(&target).expect("target dir");
        fs::write(target.join("SKILL.md"), "keep me").expect("skill file");
        create_directory_link(&target, &parent.join("planner")).expect("link");

        fs::remove_dir_all(&parent).expect("remove parent");
        assert_eq!(fs::read_to_string(target.join("SKILL.md")).expect("target intact"), "keep me");
    }

    #[test]
    fn deploy_skill_dir_falls_back_to_copy() {
        let temp = tempfile::tempdir().expect("temp dir");
        let src = temp.path().join("src");
        let dst = temp.path().join("dst");
        fs::create_dir_all(src.join("nested")).expect("src dirs");
        fs::write(src.join("SKILL.md"), "s").expect("skill");
        fs::write(src.join("nested").join("n.md"), "n").expect("nested");

        let copied = deploy_skill_dir_with_linker(&src, &dst, |_, _| {
            Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied"))
        })
        .expect("fallback");
        assert!(copied);
        assert_eq!(fs::read_to_string(dst.join("nested").join("n.md")).expect("copied"), "n");
    }

    #[test]
    fn deploy_skill_dir_replaces_existing_deployment() {
        let temp = tempfile::tempdir().expect("temp dir");
        let src = temp.path().join("src");
        let dst = temp.path().join("dst");
        fs::create_dir_all(&src).expect("src");
        fs::write(src.join("SKILL.md"), "new").expect("skill");
        fs::create_dir_all(&dst).expect("stale dst");
        fs::write(dst.join("SKILL.md"), "stale").expect("stale file");

        let copied = deploy_skill_dir(&src, &dst).expect("deploy");
        assert!(!copied);
        assert_eq!(fs::read_to_string(dst.join("SKILL.md")).expect("read"), "new");
    }
}
```

- [ ] **Step 3: Run** `cargo test -p wardian-core library::links`. Expected: FAIL.

- [ ] **Step 4: Implement** `links.rs`:

```rust
use std::fs;
use std::io;
use std::path::Path;

/// Create a directory link (junction on Windows, symlink on Unix) without
/// spawning any external process. Creates the link's parent directories.
pub fn create_directory_link(target: &Path, link: &Path) -> io::Result<()> {
    if let Some(parent) = link.parent() {
        fs::create_dir_all(parent)?;
    }

    #[cfg(windows)]
    {
        junction::create(target, link)
    }

    #[cfg(not(windows))]
    {
        std::os::unix::fs::symlink(target, link)
    }
}

/// Remove whatever occupies `path`: a reparse point / symlink is unlinked
/// without following it; a real directory or file is removed recursively.
pub fn remove_existing_deployment(path: &Path) -> io::Result<()> {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return Ok(());
    };

    if metadata.file_type().is_symlink() {
        #[cfg(windows)]
        {
            return fs::remove_dir(path).or_else(|_| fs::remove_file(path));
        }
        #[cfg(not(windows))]
        {
            return fs::remove_file(path);
        }
    }

    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return fs::remove_dir(path).or_else(|_| fs::remove_file(path));
        }
    }

    if metadata.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
}

pub fn copy_dir_all(src: &Path, dst: &Path) -> io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let source = entry.path();
        let destination = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_all(&source, &destination)?;
        } else {
            fs::copy(&source, &destination)?;
        }
    }
    Ok(())
}

/// Replace any existing deployment at `dst_dir` with a link to `src_dir`,
/// falling back to a copy when linking fails. Returns `true` when copied.
pub fn deploy_skill_dir(src_dir: &Path, dst_dir: &Path) -> io::Result<bool> {
    deploy_skill_dir_with_linker(src_dir, dst_dir, create_directory_link)
}

pub(crate) fn deploy_skill_dir_with_linker<F>(
    src_dir: &Path,
    dst_dir: &Path,
    linker: F,
) -> io::Result<bool>
where
    F: FnOnce(&Path, &Path) -> io::Result<()>,
{
    remove_existing_deployment(dst_dir)?;
    if let Some(parent) = dst_dir.parent() {
        fs::create_dir_all(parent)?;
    }
    match linker(src_dir, dst_dir) {
        Ok(()) => Ok(false),
        Err(_) => {
            copy_dir_all(src_dir, dst_dir)?;
            Ok(true)
        }
    }
}
```

Note the Windows junction subtlety: `fs::symlink_metadata` on a junction reports a directory with `FILE_ATTRIBUTE_REPARSE_POINT`, not `is_symlink() == true` — that's why the attribute check exists (mirrors `remove_projected_path` in `src-tauri/src/utils/fs.rs:618`). Re-export from `library/mod.rs`.

- [ ] **Step 5: Run** `cargo test -p wardian-core library::links`. Expected: PASS on your OS; note in the task report that Windows CI exercises the junction path.

- [ ] **Step 6: Commit** `feat(core): native junction/symlink deployment links`

---

### Task 4: Metadata store with section-qualified key migration

**Files:**
- Create: `crates/wardian-core/src/library/metadata.rs`
- Modify: `crates/wardian-core/src/library/mod.rs`

**Interfaces:**
- Consumes: `LibrarySectionId`, `paths::library_metadata_path_for_home`.
- Produces: `MetadataStore` with `load(home: &Path) -> MetadataStore`, `get(&self, entry_ref: &str) -> Option<&LibraryItemMetadata>`, `set(&mut self, entry_ref: String, metadata: LibraryItemMetadata)`, `rename(&mut self, old_ref: &str, new_ref: &str)`, `remove(&mut self, entry_ref: &str)`, `save(&self, home: &Path) -> Result<(), String>`.
- Migration: on load, any key without a `skills/` / `prompts/` / `workflows/` / `classes/` prefix is qualified by probing the filesystem (`library/skills/<key>` dir → `skills/<key>`; `library/prompts/<key>.md` file… wait — prompt keys are stored **with** the `.md`-less relative path, matching `get_library_tree`'s `file_rel_path` which **includes** `.md`; probe both the raw key and the key as stored). Unresolvable keys are dropped. A migrated store is written back immediately.

- [ ] **Step 1: Write failing tests:**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::LibraryItemMetadata;
    use std::fs;

    fn meta(id: &str) -> LibraryItemMetadata {
        LibraryItemMetadata { id: id.to_string(), tags: vec![], is_starred: true, last_used: None }
    }

    #[test]
    fn migrates_legacy_keys_by_probing_sections() {
        let temp = tempfile::tempdir().expect("temp");
        let home = temp.path();
        fs::create_dir_all(home.join("library").join("skills").join("dev").join("planner")).unwrap();
        fs::create_dir_all(home.join("library").join("prompts")).unwrap();
        fs::write(home.join("library").join("prompts").join("greet.md"), "hi").unwrap();
        let legacy = serde_json::json!({
            "dev/planner": {"id": "s1", "tags": [], "is_starred": true, "last_used": null},
            "greet.md": {"id": "p1", "tags": [], "is_starred": false, "last_used": null},
            "ghost.md": {"id": "g1", "tags": [], "is_starred": false, "last_used": null}
        });
        fs::write(home.join("library").join("library.json"), legacy.to_string()).unwrap();

        let store = MetadataStore::load(home);
        assert_eq!(store.get("skills/dev/planner").expect("skill migrated").id, "s1");
        assert_eq!(store.get("prompts/greet.md").expect("prompt migrated").id, "p1");
        assert!(store.get("ghost.md").is_none(), "unresolvable keys drop");

        // Migration writes back once: reloading needs no probing.
        let raw = fs::read_to_string(home.join("library").join("library.json")).unwrap();
        assert!(raw.contains("skills/dev/planner"));
    }

    #[test]
    fn rename_moves_metadata() {
        let temp = tempfile::tempdir().expect("temp");
        let mut store = MetadataStore::default();
        store.set("skills/old".into(), meta("m1"));
        store.rename("skills/old", "skills/new");
        assert!(store.get("skills/old").is_none());
        assert_eq!(store.get("skills/new").expect("moved").id, "m1");
        store.save(temp.path()).expect("save");
        assert!(MetadataStore::load(temp.path()).get("skills/new").is_some());
    }
}
```

- [ ] **Step 2: Run** `cargo test -p wardian-core library::metadata`. Expected: FAIL.

- [ ] **Step 3: Implement:**

```rust
use std::collections::HashMap;
use std::fs;
use std::path::Path;

use crate::models::LibraryItemMetadata;
use crate::paths::library_metadata_path_for_home;
use super::section::LibrarySectionId;

const SECTION_PREFIXES: [&str; 4] = ["skills/", "prompts/", "workflows/", "classes/"];

#[derive(Debug, Default)]
pub struct MetadataStore {
    items: HashMap<String, LibraryItemMetadata>,
}

impl MetadataStore {
    pub fn load(home: &Path) -> MetadataStore {
        let path = library_metadata_path_for_home(home);
        let raw: HashMap<String, LibraryItemMetadata> = fs::read_to_string(&path)
            .ok()
            .and_then(|data| serde_json::from_str(&data).ok())
            .unwrap_or_default();

        let mut items = HashMap::new();
        let mut migrated = false;
        for (key, value) in raw {
            if SECTION_PREFIXES.iter().any(|prefix| key.starts_with(prefix)) {
                items.insert(key, value);
            } else if let Some(qualified) = qualify_legacy_key(home, &key) {
                migrated = true;
                items.insert(qualified, value);
            } else {
                migrated = true; // dropped key still means the file must be rewritten
            }
        }

        let store = MetadataStore { items };
        if migrated {
            let _ = store.save(home);
        }
        store
    }

    pub fn get(&self, entry_ref: &str) -> Option<&LibraryItemMetadata> {
        self.items.get(entry_ref)
    }

    pub fn set(&mut self, entry_ref: String, metadata: LibraryItemMetadata) {
        self.items.insert(entry_ref, metadata);
    }

    pub fn rename(&mut self, old_ref: &str, new_ref: &str) {
        if let Some(metadata) = self.items.remove(old_ref) {
            self.items.insert(new_ref.to_string(), metadata);
        }
    }

    pub fn remove(&mut self, entry_ref: &str) {
        self.items.remove(entry_ref);
    }

    pub fn save(&self, home: &Path) -> Result<(), String> {
        let path = library_metadata_path_for_home(home);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let json = serde_json::to_string_pretty(&self.items).map_err(|e| e.to_string())?;
        fs::write(&path, json).map_err(|e| e.to_string())
    }
}

fn qualify_legacy_key(home: &Path, key: &str) -> Option<String> {
    let skills_probe = LibrarySectionId::Skills.root_for_home(home).join(key);
    if skills_probe.is_dir() {
        return Some(format!("skills/{key}"));
    }
    let prompts_probe = LibrarySectionId::Prompts.root_for_home(home).join(key);
    if prompts_probe.is_file() {
        return Some(format!("prompts/{key}"));
    }
    None
}
```

- [ ] **Step 4: Run** `cargo test -p wardian-core library::metadata`. Expected: PASS.

- [ ] **Step 5: Commit** `feat(core): library metadata store with section-qualified migration`

---

### Task 5: DTO models for the library index

**Files:**
- Modify: `crates/wardian-core/src/models/library.rs`
- Modify: `crates/wardian-core/src/models/mod.rs` (re-export new types the same way existing library types are re-exported)

**Interfaces:**
- Produces (all `snake_case` serde, all `Debug + Clone + Serialize + Deserialize`):

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LibraryEntry {
    pub kind: String,          // "skill" | "prompt" | "workflow" | "class"
    pub path: String,          // section-relative, '/'-separated
    pub entry_ref: String,     // section-qualified, e.g. "skills/dev/planner"
    pub name: String,
    pub description: String,
    pub tags: Vec<String>,
    pub is_starred: bool,
    pub deployment_count: u32,
    pub error: Option<String>, // unreadable file, never a missing row
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum LibraryIndexNode {
    Folder(LibraryIndexFolder),
    Entry(LibraryEntry),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LibraryIndexFolder {
    pub path: String,
    pub name: String,
    pub children: Vec<LibraryIndexNode>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LibrarySection {
    pub tree: LibraryIndexFolder,
    pub stubbed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DeploymentTarget {
    pub target_type: String, // "user" | "class" | "agent"
    pub target_id: String,   // "global" | class name | agent id
    pub linked: bool,        // false = copy fallback ("edits won't sync")
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OrphanDeployment {
    pub target_type: String,
    pub target_id: String,
    pub skill_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LibraryIndex {
    pub sections: std::collections::HashMap<String, LibrarySection>,
    pub deployments: std::collections::HashMap<String, Vec<DeploymentTarget>>,
    pub orphans: Vec<OrphanDeployment>,
}
```

**IMPORTANT — untagged enum ordering:** `LibraryIndexNode::Folder` must be declared before `Entry`, and `LibraryEntry` must have required fields a folder lacks (`kind`, `entry_ref`) so serde disambiguates. Keep `LibraryFolder` / `LibraryNode` / `LibrarySkill` / `LibraryPrompt` untouched — the CLI-facing legacy commands still use them.

- [ ] **Step 1: Write a serialization round-trip test** in `models/library.rs`'s existing `tests` module:

```rust
#[test]
fn library_index_round_trips_with_snake_case() {
    let entry = LibraryEntry {
        kind: "skill".into(),
        path: "dev/planner".into(),
        entry_ref: "skills/dev/planner".into(),
        name: "planner".into(),
        description: "Plans work".into(),
        tags: vec!["dev".into()],
        is_starred: false,
        deployment_count: 2,
        error: None,
    };
    let node = LibraryIndexNode::Entry(entry.clone());
    let json = serde_json::to_string(&node).unwrap();
    assert!(json.contains("\"entry_ref\""));
    match serde_json::from_str::<LibraryIndexNode>(&json).unwrap() {
        LibraryIndexNode::Entry(parsed) => assert_eq!(parsed, entry),
        LibraryIndexNode::Folder(_) => panic!("entry must not parse as folder"),
    }

    let folder = LibraryIndexFolder { path: "".into(), name: "Root".into(), children: vec![node] };
    let json = serde_json::to_string(&folder).unwrap();
    let parsed: LibraryIndexFolder = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.children.len(), 1);
}
```

- [ ] **Step 2: Run** `cargo test -p wardian-core models::library`. Expected: FAIL → implement the structs above → PASS.

- [ ] **Step 3: Commit** `feat(core): library index DTOs`

---

### Task 6: One-pass deployment scan

**Files:**
- Create: `crates/wardian-core/src/library/deployments.rs`
- Modify: `crates/wardian-core/src/library/mod.rs`

**Interfaces:**
- Consumes: `LibrarySectionId`, `DEPLOYED_SKILL_SOURCE_FILE`, models from Task 5.
- Produces:
  - `pub struct SkillSource { pub rel_path: String, pub name: String, pub canonical: PathBuf }`
  - `collect_skill_sources(home: &Path) -> Vec<SkillSource>` — recursive walk of `library/skills` finding `SKILL.md` dirs (port of `collect_library_skill_sources` from `src-tauri/src/commands/library.rs:571`).
  - `pub struct DeploymentScan { pub deployments: HashMap<String, Vec<DeploymentTarget>>, pub orphans: Vec<OrphanDeployment> }` (key = source rel_path, e.g. `dev/planner` — **not** section-qualified; the index builder qualifies).
  - `scan_deployments(home: &Path, sources: &[SkillSource]) -> DeploymentScan` — walks `common/.agents/skills`, `classes/*/.agents/skills`, `agents/*/.agents/skills` exactly once. Resolution order per deployed dir: marker file → canonical-path match → unique-name inference. `linked` = symlink_metadata is a link/reparse point. Unresolvable → orphan.
  - `get_target_skills_dir(home: &Path, target_type: &str, target_id: &str) -> Result<PathBuf, String>` (port of the same-named fn at `src-tauri/src/commands/library.rs:373`).

- [ ] **Step 1: Write failing tests:**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::links::{copy_dir_all, create_directory_link};
    use std::fs;

    fn seed_skill(home: &std::path::Path, rel: &str) -> std::path::PathBuf {
        let dir = home.join("library").join("skills").join(rel);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("SKILL.md"), rel).unwrap();
        dir
    }

    #[test]
    fn one_scan_resolves_links_copies_and_orphans() {
        let temp = tempfile::tempdir().expect("temp");
        let home = temp.path();
        let planner = seed_skill(home, "dev/planner");
        seed_skill(home, "reviewer");

        // Linked deploy to a class.
        let class_target = home.join("classes").join("Architect").join(".agents").join("skills").join("planner");
        create_directory_link(&planner, &class_target).unwrap();

        // Copied deploy (with marker) to an agent.
        let agent_target = home.join("agents").join("a1").join(".agents").join("skills").join("planner");
        copy_dir_all(&planner, &agent_target).unwrap();
        fs::write(agent_target.join(super::super::section::DEPLOYED_SKILL_SOURCE_FILE), "dev/planner").unwrap();

        // Orphan: deployed dir with no source anywhere.
        let orphan_target = home.join("common").join(".agents").join("skills").join("ghost");
        fs::create_dir_all(&orphan_target).unwrap();
        fs::write(orphan_target.join("SKILL.md"), "ghost").unwrap();

        let sources = collect_skill_sources(home);
        assert_eq!(sources.len(), 2);
        let scan = scan_deployments(home, &sources);

        let planner_targets = scan.deployments.get("dev/planner").expect("planner deployed");
        assert_eq!(planner_targets.len(), 2);
        let class_dep = planner_targets.iter().find(|t| t.target_type == "class").expect("class");
        assert!(class_dep.linked);
        let agent_dep = planner_targets.iter().find(|t| t.target_type == "agent").expect("agent");
        assert!(!agent_dep.linked);

        assert_eq!(scan.orphans.len(), 1);
        assert_eq!(scan.orphans[0].skill_name, "ghost");
        assert_eq!(scan.orphans[0].target_type, "user");
    }

    #[test]
    fn duplicate_names_resolve_only_via_marker_or_canonical() {
        let temp = tempfile::tempdir().expect("temp");
        let home = temp.path();
        seed_skill(home, "group-a/planner");
        let b = seed_skill(home, "group-b/planner");
        let target = home.join("agents").join("a1").join(".agents").join("skills").join("planner");
        copy_dir_all(&b, &target).unwrap(); // legacy copy, no marker

        let sources = collect_skill_sources(home);
        let scan = scan_deployments(home, &sources);
        assert!(scan.deployments.get("group-a/planner").is_none());
        assert!(scan.deployments.get("group-b/planner").is_none());
        assert_eq!(scan.orphans.len(), 1, "ambiguous copy stays orphaned");
    }
}
```

- [ ] **Step 2: Run** `cargo test -p wardian-core library::deployments`. Expected: FAIL.

- [ ] **Step 3: Implement.** Port the resolution helpers from `src-tauri/src/commands/library.rs` (`read_deployed_skill_source_marker`, `source_path_for_deployed_skill`, `collect_library_skill_sources`) into this module, adapted to `home: &Path` and the `SkillSource` struct. The scanner:

```rust
pub fn scan_deployments(home: &Path, sources: &[SkillSource]) -> DeploymentScan {
    let mut deployments: HashMap<String, Vec<DeploymentTarget>> = HashMap::new();
    let mut orphans = Vec::new();

    let mut scan_target_dir = |target_type: &str, target_id: &str, skills_dir: &Path| {
        let Ok(entries) = fs::read_dir(skills_dir) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            let linked = fs::symlink_metadata(&path)
                .map(|m| is_reparse_or_symlink(&m))
                .unwrap_or(false);
            match resolve_source(&path, &name, sources) {
                Some(rel_path) => deployments.entry(rel_path).or_default().push(DeploymentTarget {
                    target_type: target_type.to_string(),
                    target_id: target_id.to_string(),
                    linked,
                }),
                None => orphans.push(OrphanDeployment {
                    target_type: target_type.to_string(),
                    target_id: target_id.to_string(),
                    skill_name: name,
                }),
            }
        }
    };

    scan_target_dir("user", "global", &home.join("common").join(".agents").join("skills"));
    for root in ["classes", "agents"] {
        let type_name = if root == "classes" { "class" } else { "agent" };
        let Ok(entries) = fs::read_dir(home.join(root)) else { continue };
        for entry in entries.flatten() {
            if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                let id = entry.file_name().to_string_lossy().to_string();
                scan_target_dir(type_name, &id, &entry.path().join(".agents").join("skills"));
            }
        }
    }

    DeploymentScan { deployments, orphans }
}
```

`resolve_source` = marker → canonical → unique-name inference (same-name count == 1). `is_reparse_or_symlink` checks `file_type().is_symlink()` plus the Windows `FILE_ATTRIBUTE_REPARSE_POINT` attribute (same pattern as Task 3). Note the test expects the **ambiguous legacy copy to be an orphan** — that's a deliberate behavior sharpening vs. the old `source_path: None` row; the UI shows it as drift with a cleanup action.

- [ ] **Step 4: Run** `cargo test -p wardian-core library::deployments`. Expected: PASS.

- [ ] **Step 5: Commit** `feat(core): one-pass skill deployment scan with orphan detection`

---

### Task 7: Index builder

**Files:**
- Create: `crates/wardian-core/src/library/index.rs`
- Modify: `crates/wardian-core/src/library/mod.rs`

**Interfaces:**
- Consumes: everything from Tasks 1–6.
- Produces: `build_library_index(home: &Path) -> Result<LibraryIndex, String>`.
  - `skills`: tree of folders + `kind: "skill"` entries (dirs containing `SKILL.md`), description via `extract_description`, metadata from `MetadataStore` keyed `skills/<rel>`, `deployment_count` from the scan.
  - `prompts`: `.md` files → `kind: "prompt"`, ref `prompts/<rel-with-.md>` (matches legacy key shape), `name` = file stem.
  - `workflows`: `.md` files under `library/workflows` (folders allowed) → `kind: "workflow"`.
  - `classes`: flat — each dir under `classes/` → `kind: "class"`, `path` = dir name, description from `AGENTS.md` via `extract_description`, `deployment_count` = number of deployments whose `target_type == "class" && target_id == <name>`.
  - `mcps`: `stubbed: true`, empty tree, **no directory created**.
  - Unreadable content files → entry with `error: Some(msg)`, `description: ""`.
  - Missing section dirs (except mcps) are created.

- [ ] **Step 1: Write failing tests** (uses `WARDIAN_HOME`-free explicit paths — no env vars needed):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn index_covers_all_sections_with_metadata_and_deployments() {
        let temp = tempfile::tempdir().expect("temp");
        let home = temp.path();
        // skill with frontmatter + linked deployment to class
        let skill = home.join("library").join("skills").join("dev").join("planner");
        fs::create_dir_all(&skill).unwrap();
        fs::write(skill.join("SKILL.md"), "---\ndescription: Plans work\n---\nbody").unwrap();
        crate::library::links::create_directory_link(
            &skill,
            &home.join("classes").join("Architect").join(".agents").join("skills").join("planner"),
        ).unwrap();
        // prompt
        fs::create_dir_all(home.join("library").join("prompts")).unwrap();
        fs::write(home.join("library").join("prompts").join("greet.md"), "# Greeting\nHello").unwrap();
        // workflow
        fs::create_dir_all(home.join("library").join("workflows")).unwrap();
        fs::write(home.join("library").join("workflows").join("triage.md"), "---\ndescription: Triage\n---\n").unwrap();
        // class AGENTS.md
        fs::write(home.join("classes").join("Architect").join("AGENTS.md"), "# Role: Architect\nDesigns").unwrap();
        // starred metadata (already-qualified key)
        fs::write(
            home.join("library").join("library.json"),
            r#"{"skills/dev/planner": {"id":"m1","tags":["dev"],"is_starred":true,"last_used":null}}"#,
        ).unwrap();

        let index = build_library_index(home).expect("index");

        let skills = &index.sections["skills"];
        let dev = match &skills.tree.children[0] { LibraryIndexNode::Folder(f) => f, _ => panic!("dev folder") };
        let planner = match &dev.children[0] { LibraryIndexNode::Entry(e) => e, _ => panic!("planner entry") };
        assert_eq!(planner.entry_ref, "skills/dev/planner");
        assert_eq!(planner.description, "Plans work");
        assert!(planner.is_starred);
        assert_eq!(planner.tags, vec!["dev".to_string()]);
        assert_eq!(planner.deployment_count, 1);

        let prompts = &index.sections["prompts"];
        let greet = match &prompts.tree.children[0] { LibraryIndexNode::Entry(e) => e, _ => panic!("greet") };
        assert_eq!(greet.kind, "prompt");
        assert_eq!(greet.name, "greet");
        assert_eq!(greet.description, "Greeting");

        assert_eq!(index.sections["workflows"].tree.children.len(), 1);

        let classes = &index.sections["classes"];
        let architect = match &classes.tree.children[0] { LibraryIndexNode::Entry(e) => e, _ => panic!("class") };
        assert_eq!(architect.entry_ref, "classes/Architect");
        assert_eq!(architect.description, "Role: Architect");
        assert_eq!(architect.deployment_count, 1);

        assert!(index.sections["mcps"].stubbed);
        assert!(index.sections["mcps"].tree.children.is_empty());
        assert!(!home.join("library").join("mcps").exists(), "stub creates no dir");

        assert_eq!(index.deployments["skills/dev/planner"].len(), 1);
        assert!(index.orphans.is_empty());
    }

    #[test]
    fn unreadable_entries_carry_error_flag() {
        let temp = tempfile::tempdir().expect("temp");
        let home = temp.path();
        let skill = home.join("library").join("skills").join("broken");
        fs::create_dir_all(&skill).unwrap();
        fs::write(skill.join("SKILL.md"), [0xFF, 0xFE, 0x00]).unwrap(); // invalid UTF-8

        let index = build_library_index(home).expect("index survives");
        let broken = match &index.sections["skills"].tree.children[0] {
            LibraryIndexNode::Entry(e) => e,
            _ => panic!("entry expected"),
        };
        assert!(broken.error.is_some() || broken.description.is_empty());
    }
}
```

- [ ] **Step 2: Run** `cargo test -p wardian-core library::index`. Expected: FAIL.

- [ ] **Step 3: Implement.** One recursive tree builder shared by skills/prompts/workflows, parameterized by an entry detector:

```rust
enum SectionShape {
    SkillDirs,       // dir containing SKILL.md is an entry; other dirs recurse
    MarkdownFiles,   // *.md files are entries; dirs recurse
}
```

Deployment counts: build `collect_skill_sources` + `scan_deployments` first; skills look up by rel_path; classes count targets with matching `target_id`. Deployments map in the returned `LibraryIndex` is re-keyed to section-qualified refs (`skills/<rel>`). Children sort folders-first, then entries, both alphabetical (case-insensitive) — deterministic output for tests and UI. Read content per entry only to extract the description (`fs::read_to_string`; on error → `error: Some(e.to_string())`, empty description); content is **not** stored in the index.

- [ ] **Step 4: Run** `cargo test -p wardian-core library::index`. Expected: PASS.

- [ ] **Step 5: Commit** `feat(core): metadata-only library index builder`

---

### Task 8: Read/save/create-folder mutations

**Files:**
- Create: `crates/wardian-core/src/library/mutations.rs`
- Modify: `crates/wardian-core/src/library/mod.rs`

**Interfaces:**
- Produces:
  - `read_item(home: &Path, section: LibrarySectionId, rel: &str) -> Result<String, String>` — skills read `<dir>/SKILL.md`; classes read `classes/<name>/AGENTS.md`; prompts/workflows read the file itself.
  - `save_item(home: &Path, section: LibrarySectionId, rel: &str, content: &str) -> Result<(), String>` — same path mapping; creates parents.
  - `create_folder(home: &Path, section: LibrarySectionId, rel: &str) -> Result<(), String>` — skills/prompts/workflows only (classes are flat: `Err`).
  - `update_metadata(home: &Path, entry_ref: &str, metadata: LibraryItemMetadata) -> Result<(), String>` — loads store, sets, saves.

- [ ] **Step 1: Write failing tests:**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::section::LibrarySectionId;
    use std::fs;

    #[test]
    fn read_save_maps_content_files_per_section() {
        let temp = tempfile::tempdir().expect("temp");
        let home = temp.path();
        save_item(home, LibrarySectionId::Skills, "dev/planner", "skill body").unwrap();
        save_item(home, LibrarySectionId::Prompts, "greet.md", "prompt body").unwrap();
        save_item(home, LibrarySectionId::Classes, "Architect", "agents body").unwrap();

        assert!(home.join("library/skills/dev/planner/SKILL.md").exists());
        assert!(home.join("library/prompts/greet.md").exists());
        assert!(home.join("classes/Architect/AGENTS.md").exists());
        assert_eq!(read_item(home, LibrarySectionId::Skills, "dev/planner").unwrap(), "skill body");
        assert_eq!(read_item(home, LibrarySectionId::Classes, "Architect").unwrap(), "agents body");
    }

    #[test]
    fn create_folder_rejects_flat_sections() {
        let temp = tempfile::tempdir().expect("temp");
        create_folder(temp.path(), LibrarySectionId::Skills, "dev/tools").unwrap();
        assert!(temp.path().join("library/skills/dev/tools").is_dir());
        assert!(create_folder(temp.path(), LibrarySectionId::Classes, "sub").is_err());
    }

    #[test]
    fn mutations_reject_traversal() {
        let temp = tempfile::tempdir().expect("temp");
        assert!(save_item(temp.path(), LibrarySectionId::Prompts, "../evil.md", "x").is_err());
        assert!(read_item(temp.path(), LibrarySectionId::Prompts, "../../etc/passwd").is_err());
    }
}
```

- [ ] **Step 2: Run** `cargo test -p wardian-core library::mutations`. Expected: FAIL.

- [ ] **Step 3: Implement** (content-file mapping in one helper):

```rust
pub(crate) fn content_file_path(
    home: &Path,
    section: LibrarySectionId,
    rel: &str,
) -> Result<PathBuf, String> {
    let base = resolve_entry_path(home, section, rel)?;
    Ok(match section {
        LibrarySectionId::Skills => base.join("SKILL.md"),
        LibrarySectionId::Classes => base.join("AGENTS.md"),
        _ => base,
    })
}
```

`read_item` = `fs::read_to_string(content_file_path(..))`; `save_item` creates the parent dir then writes; `create_folder` errors for `Classes`/`Mcps`, else `fs::create_dir_all`. `update_metadata` = `MetadataStore::load` → `set` → `save`.

- [ ] **Step 4: Run** `cargo test -p wardian-core library::mutations`. Expected: PASS.

- [ ] **Step 5: Commit** `feat(core): library read/save/create-folder mutations`

---

### Task 9: Deployment-aware rename and delete

**Files:**
- Modify: `crates/wardian-core/src/library/mutations.rs`

**Interfaces:**
- Produces:
  - `rename_entry(home: &Path, section: LibrarySectionId, from_rel: &str, to_rel: &str) -> Result<(), String>` — `fs::rename`; for a deployed skill: re-create each linked deployment's junction at the new source path, rewrite each copied deployment's marker file; migrates metadata key. Renaming classes is rejected (class identity is referenced by agents; out of scope).
  - `delete_entry(home: &Path, section: LibrarySectionId, rel: &str) -> Result<(), String>` — for skills: first `remove_existing_deployment` on every deployment target, then delete the source; prompts/workflows delete the file; folders delete recursively; classes rejected here (class deletion goes through the existing `delete_agent_class` flow which handles agent references).
  - `remove_orphan_deployment(home: &Path, target_type: &str, target_id: &str, skill_name: &str) -> Result<(), String>`.

- [ ] **Step 1: Write failing tests:**

```rust
#[test]
fn rename_deployed_skill_relinks_and_remarks() {
    let temp = tempfile::tempdir().expect("temp");
    let home = temp.path();
    let src = home.join("library/skills/planner");
    fs::create_dir_all(&src).unwrap();
    fs::write(src.join("SKILL.md"), "v1").unwrap();
    // one linked, one copied deployment
    let linked = home.join("classes/Architect/.agents/skills/planner");
    crate::library::links::create_directory_link(&src, &linked).unwrap();
    let copied = home.join("agents/a1/.agents/skills/planner");
    crate::library::links::copy_dir_all(&src, &copied).unwrap();
    fs::write(copied.join(crate::library::section::DEPLOYED_SKILL_SOURCE_FILE), "planner").unwrap();
    // starred metadata
    fs::write(home.join("library/library.json"),
        r#"{"skills/planner": {"id":"m1","tags":[],"is_starred":true,"last_used":null}}"#).unwrap();
    fs::create_dir_all(home.join("library/prompts")).unwrap();

    rename_entry(home, LibrarySectionId::Skills, "planner", "dev/planner").unwrap();

    fs::write(home.join("library/skills/dev/planner/SKILL.md"), "v2").unwrap();
    assert_eq!(fs::read_to_string(linked.join("SKILL.md")).unwrap(), "v2", "junction re-created");
    assert_eq!(
        fs::read_to_string(copied.join(crate::library::section::DEPLOYED_SKILL_SOURCE_FILE)).unwrap().trim(),
        "dev/planner", "marker rewritten"
    );
    let store = crate::library::metadata::MetadataStore::load(home);
    assert!(store.get("skills/dev/planner").expect("metadata migrated").is_starred);
}

#[test]
fn delete_deployed_skill_cleans_all_targets() {
    let temp = tempfile::tempdir().expect("temp");
    let home = temp.path();
    let src = home.join("library/skills/planner");
    fs::create_dir_all(&src).unwrap();
    fs::write(src.join("SKILL.md"), "v1").unwrap();
    let linked = home.join("common/.agents/skills/planner");
    crate::library::links::create_directory_link(&src, &linked).unwrap();

    delete_entry(home, LibrarySectionId::Skills, "planner").unwrap();
    assert!(!src.exists());
    assert!(!linked.exists(), "no dangling deployment");
}
```

- [ ] **Step 2: Run.** Expected: FAIL.

- [ ] **Step 3: Implement.** Both operations start with `collect_skill_sources` + `scan_deployments` (from Task 6) to find the entry's targets via `get_target_skills_dir(home, target_type, target_id).join(skill_name)`. Rename order: (1) `fs::rename` source, (2) per linked target: `remove_existing_deployment` + `create_directory_link` to the new path, (3) per copied target: rewrite marker, (4) `MetadataStore` rename + save. Note the skill *directory name* can change in a rename (`planner` → `dev/planner` keeps it, `planner` → `strategist` changes it): the deployed dir name must become the new `file_name()`, so remove the old target path and create the new one. Delete order: (1) remove deployments, (2) `remove_existing_deployment` on the source path (handles skill dirs and plain files uniformly), (3) metadata remove + save.

- [ ] **Step 4: Run** `cargo test -p wardian-core library::mutations`. Expected: PASS.

- [ ] **Step 5: Commit** `feat(core): deployment-aware rename and delete`

---

### Task 10: `set_skill_deployments` diffing

**Files:**
- Modify: `crates/wardian-core/src/library/deployments.rs`

**Interfaces:**
- Consumes: `SkillDeployment { target_type, target_id }` (existing model), Task 6 scan.
- Produces: `set_skill_deployments(home: &Path, source_rel: &str, desired: &[SkillDeployment]) -> Result<SetDeploymentsOutcome, String>` where `pub struct SetDeploymentsOutcome { pub added: u32, pub removed: u32, pub copied_fallbacks: u32 }`. Also `deploy_skill(home, source_rel, target_type, target_id) -> Result<bool /*copied*/, String>` and `remove_deployed_skill(home, target_type, target_id, skill_name) -> Result<(), String>` as the single-target primitives (ported from `deploy_skill_from_library` at `src-tauri/src/commands/library.rs:469`, minus the Tauri parts; the copy fallback writes the `.wardian-skill-source` marker).

- [ ] **Step 1: Write failing tests:**

```rust
#[test]
fn set_deployments_diffs_adds_and_removes() {
    let temp = tempfile::tempdir().expect("temp");
    let home = temp.path();
    let src = home.join("library/skills/planner");
    fs::create_dir_all(&src).unwrap();
    fs::write(src.join("SKILL.md"), "v1").unwrap();

    let desired = vec![
        SkillDeployment { target_type: "class".into(), target_id: "Architect".into() },
        SkillDeployment { target_type: "user".into(), target_id: "global".into() },
    ];
    let outcome = set_skill_deployments(home, "planner", &desired).unwrap();
    assert_eq!(outcome.added, 2);
    assert!(home.join("classes/Architect/.agents/skills/planner").join("SKILL.md").exists());
    assert!(home.join("common/.agents/skills/planner").join("SKILL.md").exists());

    // Narrow to just the class: user deployment is removed, class untouched.
    let narrowed = vec![SkillDeployment { target_type: "class".into(), target_id: "Architect".into() }];
    let outcome = set_skill_deployments(home, "planner", &narrowed).unwrap();
    assert_eq!(outcome.added, 0);
    assert_eq!(outcome.removed, 1);
    assert!(!home.join("common/.agents/skills/planner").exists());
    assert!(home.join("classes/Architect/.agents/skills/planner").exists());

    // Idempotent.
    let outcome = set_skill_deployments(home, "planner", &narrowed).unwrap();
    assert_eq!((outcome.added, outcome.removed), (0, 0));
}
```

- [ ] **Step 2: Run.** Expected: FAIL.

- [ ] **Step 3: Implement.** Current targets = `scan_deployments` filtered to this source. Compare as `(target_type, target_id)` sets; deploy missing, remove extra, count copy fallbacks from `deploy_skill_dir`'s return.

- [ ] **Step 4: Run** `cargo test -p wardian-core library::deployments`. Expected: PASS.

- [ ] **Step 5: Commit** `feat(core): set-based skill deployment diffing`

---

### Task 11: Tauri layer — rewire commands onto the core engine

**Files:**
- Modify: `src-tauri/src/commands/library.rs` (major rewrite; keep the Antigravity + watcher parts)
- Modify: `src-tauri/src/lib.rs:658-664` region (command registration)
- Modify: `src-tauri/src/utils/fs.rs:875` (`create_directory_link` delegates to `wardian_core::library::create_directory_link`; delete the `mklink` body)
- Modify: `src-tauri/src/manager/classes.rs`, `src-tauri/src/manager/codex.rs`, `src-tauri/src/commands/agent.rs`, `src-tauri/src/commands/git.rs` only if they call the old fs helper signature (they shouldn't need changes if the signature stays `(&Path, &Path) -> Result<(), String>`; adapt the io::Error with `.map_err(|e| e.to_string())`).

**Interfaces:**
- Produces (new commands):
  - `get_library_index() -> Result<LibraryIndex, String>`
  - `read_library_item(section: String, path: String) -> Result<String, String>`
  - `save_library_item(section: String, path: String, content: String) -> Result<(), String>` (**breaking**: old signature had `library_type` + metadata; metadata now goes through `update_library_metadata`)
  - `update_library_metadata(entry_ref: String, metadata: LibraryItemMetadata) -> Result<(), String>` (**breaking**: key is now the qualified ref)
  - `create_library_folder(section: String, path: String) -> Result<(), String>`
  - `rename_library_entry(section: String, from_path: String, to_path: String) -> Result<(), String>`
  - `delete_library_entry(section: String, path: String) -> Result<(), String>`
  - `set_skill_deployments(source_path: String, targets: Vec<SkillDeployment>) -> Result<(), String>` (runs Antigravity refresh once after)
  - `remove_orphan_deployment(target_type: String, target_id: String, skill_name: String) -> Result<(), String>`
  - `open_library_folder(section: String, path: Option<String>)` (updated param name)
- Kept as thin wrappers over core (single-target ops used by CLI-adjacent flows & clone previews): `deploy_skill`, `remove_deployed_skill`, `list_deployed_skills`, `list_deployed_skill_refs`, `list_skill_deployments`. Internal callers `list_deployed_skill_refs_for_target(_strict)` and `deploy_skill_from_library` keep their signatures but delegate to core.
- Removed: `get_library_tree` (UI-only; delete command + registration).
- Watcher: `library_watch` / `library_unwatch` lose their `library_type` param semantics — they accept it but only `"library"` is valid; registration watches `library/` **and** `classes/` roots plus external linked-skill targets (existing `discover_skill_watch_targets` logic, now also run over `classes` deployments is unnecessary — keep it scoped to library skills). Emit payload `{ "library_type": "library" }`.
- Antigravity refresh: `refresh_live_antigravity_skill_projections` gains an early exit: snapshot configs, `if !configs.iter().any(|c| c.provider == "antigravity") { return; }` before any filesystem work.

- [ ] **Step 1: Rewrite `commands/library.rs`.** Delete the moved logic (`build_tree`, link/copy helpers, scan helpers — now in core) and implement each command as resolve-home → parse-section → delegate:

```rust
fn parse_section(section: &str) -> Result<LibrarySectionId, String> {
    LibrarySectionId::parse(section).ok_or_else(|| format!("Unknown library section: {section}"))
}

#[tauri::command]
pub async fn get_library_index(_app: AppHandle) -> Result<LibraryIndex, String> {
    let home = get_wardian_home().ok_or("Could not find Wardian home")?;
    wardian_core::library::build_library_index(&home)
}

#[tauri::command]
pub async fn set_skill_deployments(
    app: AppHandle,
    source_path: String,
    targets: Vec<SkillDeployment>,
) -> Result<(), String> {
    let home = get_wardian_home().ok_or("Could not find Wardian home")?;
    wardian_core::library::set_skill_deployments(&home, &source_path, &targets)?;
    refresh_live_antigravity_skill_projections(&app).await;
    Ok(())
}
```

(Same shape for the rest.) Keep the existing library-watch tests; port the deployment behavior tests that moved to core out of this file (they live in core now — delete duplicates here, keep `deploy_skill_uses_live_link_for_agent_targets` and the antigravity projection test since they exercise the app-crate wiring end to end).

- [ ] **Step 2: Update registrations** in `src-tauri/src/lib.rs` — remove `get_library_tree`, add the nine new commands.

- [ ] **Step 3: Run** `cd src-tauri && cargo test && cargo clippy`. Expected: PASS, no warnings. The pre-existing live-link test now proves the junction-crate path end to end.

- [ ] **Step 4: Grep check** `grep -rn "mklink" src-tauri/ crates/` → expected: no hits outside comments/docs.

- [ ] **Step 5: Commit** `feat(backend): library index commands over wardian-core engine`

---

### Task 12: Frontend types and store rewrite

**Files:**
- Modify: `src/types/index.ts` (add index types; keep `LibraryFolder`/`LibrarySkill`/`LibraryPrompt` until Task 16 removes their consumers)
- Rewrite: `src/store/useLibraryStore.ts`
- Test: `src/store/useLibraryStore.test.ts` (create; follow the mocking pattern in existing store tests — `vi.mock('@tauri-apps/api/core')`)

**Interfaces:**
- Produces (types — exact mirror of Task 5 DTOs):

```typescript
export type LibrarySectionId = 'skills' | 'prompts' | 'workflows' | 'classes' | 'mcps';
export type LibraryEntryKind = 'skill' | 'prompt' | 'workflow' | 'class';

export interface LibraryEntry {
    kind: LibraryEntryKind;
    path: string;
    entry_ref: string;
    name: string;
    description: string;
    tags: string[];
    is_starred: boolean;
    deployment_count: number;
    error?: string | null;
}

export interface LibraryIndexFolder {
    path: string;
    name: string;
    children: (LibraryIndexFolder | LibraryEntry)[];
}

export interface LibrarySection {
    tree: LibraryIndexFolder;
    stubbed: boolean;
}

export interface DeploymentTarget {
    target_type: 'user' | 'class' | 'agent';
    target_id: string;
    linked: boolean;
}

export interface OrphanDeployment {
    target_type: string;
    target_id: string;
    skill_name: string;
}

export interface LibraryIndex {
    sections: Record<LibrarySectionId, LibrarySection>;
    deployments: Record<string, DeploymentTarget[]>;
    orphans: OrphanDeployment[];
}

export function isLibraryEntry(node: LibraryIndexFolder | LibraryEntry): node is LibraryEntry {
    return 'entry_ref' in node;
}
```

- Produces (store shape):

```typescript
interface LibraryState {
    index: LibraryIndex | null;
    isLoading: boolean;
    error: string | null;
    activeSection: LibrarySectionId;
    selection: { section: LibrarySectionId; entryRef: string } | null;
    expandedFolders: Set<string>;             // keyed `${section}:${folderPath}`
    searchQuery: string;
    showStarredOnly: boolean;
    selectedContent: string | null;           // lazy-loaded
    contentStale: boolean;                    // external change while editor dirty
    setActiveSection: (s: LibrarySectionId) => void;
    select: (entryRef: string | null, opts?: { editorDirty?: boolean }) => Promise<void>;
    toggleFolder: (key: string) => void;
    setSearchQuery: (q: string) => void;
    setShowStarredOnly: (v: boolean) => void;
    fetchIndex: () => Promise<void>;
    subscribeToLibraryChanges: () => () => void;   // single 'library-changed' listener + library_watch
    reloadSelectedContent: () => Promise<void>;
    saveItem: (section: LibrarySectionId, path: string, content: string) => Promise<void>;
    updateMetadata: (entryRef: string, metadata: LibraryItemMetadata) => Promise<void>;
    createFolder: (section: LibrarySectionId, path: string) => Promise<void>;
    renameEntry: (section: LibrarySectionId, fromPath: string, toPath: string) => Promise<void>;
    deleteEntry: (section: LibrarySectionId, path: string) => Promise<void>;
    setSkillDeployments: (sourcePath: string, targets: { target_type: string; target_id: string }[]) => Promise<void>;
    removeOrphan: (o: OrphanDeployment) => Promise<void>;
    openLibraryFolder: (section: LibrarySectionId, path?: string) => Promise<void>;
}
```

Behavior contract: `select(ref)` invokes `read_library_item` and puts the result in `selectedContent`; on `library-changed`, `fetchIndex()` always runs, and `reloadSelectedContent()` runs only when the caller-supplied dirty flag from the last `select`/`markEditorDirty` is false — otherwise `contentStale: true`. Keep the existing subscription refCount pattern (reuse the current file's `librarySubscriptions` machinery, adapted to the single `'library'` watch type).

- [ ] **Step 1: Write failing store tests** covering: `fetchIndex` populates `index`; `select` loads content via `read_library_item`; mutation methods invoke the right command names with snake_case args and refresh the index; external change with dirty editor sets `contentStale` instead of clobbering `selectedContent`.

```typescript
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { invoke } from '@tauri-apps/api/core';
import { useLibraryStore } from './useLibraryStore';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn().mockResolvedValue(() => {}) }));

const emptyIndex = {
    sections: {
        skills: { tree: { path: '', name: 'Root', children: [] }, stubbed: false },
        prompts: { tree: { path: '', name: 'Root', children: [] }, stubbed: false },
        workflows: { tree: { path: '', name: 'Root', children: [] }, stubbed: false },
        classes: { tree: { path: '', name: 'Root', children: [] }, stubbed: false },
        mcps: { tree: { path: '', name: 'Root', children: [] }, stubbed: true },
    },
    deployments: {},
    orphans: [],
};

describe('useLibraryStore', () => {
    beforeEach(() => {
        vi.mocked(invoke).mockReset();
        useLibraryStore.setState({ index: null, selection: null, selectedContent: null, contentStale: false });
    });

    it('fetchIndex loads the unified index', async () => {
        vi.mocked(invoke).mockResolvedValueOnce(emptyIndex);
        await useLibraryStore.getState().fetchIndex();
        expect(invoke).toHaveBeenCalledWith('get_library_index');
        expect(useLibraryStore.getState().index?.sections.mcps.stubbed).toBe(true);
    });

    it('select lazy-loads content', async () => {
        vi.mocked(invoke).mockResolvedValueOnce('# body');
        await useLibraryStore.getState().select('skills/dev/planner');
        expect(invoke).toHaveBeenCalledWith('read_library_item', { section: 'skills', path: 'dev/planner' });
        expect(useLibraryStore.getState().selectedContent).toBe('# body');
    });

    it('setSkillDeployments invokes command and refreshes index', async () => {
        vi.mocked(invoke).mockResolvedValue(emptyIndex);
        await useLibraryStore.getState().setSkillDeployments('dev/planner', [{ target_type: 'class', target_id: 'Architect' }]);
        expect(invoke).toHaveBeenCalledWith('set_skill_deployments', {
            sourcePath: 'dev/planner',
            targets: [{ target_type: 'class', target_id: 'Architect' }],
        });
    });
});
```

- [ ] **Step 2: Run** `npm run test -- useLibraryStore`. Expected: FAIL.

- [ ] **Step 3: Implement the store** per the interface block. `select` parses the section from the ref prefix (`ref.split('/')[0]` as section, remainder as path).

- [ ] **Step 4: Run** `npm run test -- useLibraryStore`. Expected: PASS.

- [ ] **Step 5: Commit** `feat(library): unified index types and store`

---

### Task 13: View shell — SectionRail + layout skeleton

**Files:**
- Create: `src/features/library/SectionRail.tsx`
- Rewrite: `src/views/LibraryView.tsx`
- Test: `src/features/library/SectionRail.test.tsx`, update `src/views/LibraryView.test.tsx`

**Interfaces:**
- Produces: `SectionRail({ activeSection, sections, onSelect }: { activeSection: LibrarySectionId; sections: LibraryIndex['sections'] | null; onSelect: (s: LibrarySectionId) => void })` — vertical strip, one button per section (`data-testid="library-section-<id>"`), label + count badge (entries only, recursive count), active state via themed classes.
- Produces: `LibraryView({ selectedAgentIds }: { selectedAgentIds: Set<string> })` — flex row: `SectionRail` (~w-14, border-right) | `LibraryList` (flex-1 min-w-0) | `DetailPane` (~w-[380px], resizable later, border-left). `LibraryList`/`DetailPane` are stubbed as placeholder divs in this task (real ones in Tasks 14–15) so the shell lands green.
- Section metadata lives in one exported constant:

```typescript
export const LIBRARY_SECTIONS: { id: LibrarySectionId; label: string; kindLabel: string }[] = [
    { id: 'skills', label: 'Skills', kindLabel: 'skill' },
    { id: 'prompts', label: 'Prompts', kindLabel: 'prompt' },
    { id: 'classes', label: 'Classes', kindLabel: 'class' },
    { id: 'workflows', label: 'Workflows', kindLabel: 'workflow' },
    { id: 'mcps', label: 'MCPs', kindLabel: 'MCP server' },
];
```

- [ ] **Step 1: Write failing tests:** rail renders 5 sections with counts; clicking fires `onSelect`; `LibraryView` renders rail + list + detail regions (`data-testid="library-list"`, `data-testid="library-detail"`); switching sections updates the store's `activeSection`.

- [ ] **Step 2: Run** `npm run test -- SectionRail LibraryView`. Expected: FAIL.

- [ ] **Step 3: Implement.** Rail button skeleton:

```tsx
<button
    key={section.id}
    data-testid={`library-section-${section.id}`}
    onClick={() => onSelect(section.id)}
    className={`flex flex-col items-center gap-1 py-3 w-full border-l-2 transition-colors ${
        activeSection === section.id
            ? 'border-[var(--color-wardian-accent)] text-primary bg-wardian-sidebar-primary'
            : 'border-transparent text-muted hover:text-primary'
    }`}
>
    <span className="label-small">{section.label}</span>
    {count > 0 && <span className="text-[10px] text-muted-neutral">{count}</span>}
</button>
```

`LibraryView` subscribes once (`useEffect(() => subscribeToLibraryChanges(), [])`) and keeps the existing `selectedAgentIds` prop for prompt-run wiring in Task 14.

- [ ] **Step 4: Run tests.** Expected: PASS.

- [ ] **Step 5: Commit** `feat(library): section rail and view shell`

---

### Task 14: LibraryList — toolbar, folder groups, rows, search

**Files:**
- Create: `src/features/library/LibraryList.tsx`, `src/features/library/ListToolbar.tsx`, `src/features/library/libraryListUtils.ts`
- Test: `src/features/library/libraryListUtils.test.ts`, `src/features/library/LibraryList.test.tsx`

**Interfaces:**
- Produces (`libraryListUtils.ts` — pure, unit-test heavy):

```typescript
export interface ListRow {
    type: 'folder-header' | 'entry';
    depth: number;
    folderPath?: string;      // for folder-header
    entry?: LibraryEntry;     // for entry
    pathSubtitle?: string;    // parent folder path, shown in search mode
}

/** Browse mode: hierarchical rows honoring expandedFolders. */
export function flattenTree(
    tree: LibraryIndexFolder,
    section: LibrarySectionId,
    expandedFolders: Set<string>,
): ListRow[];

/** Search mode: rank name > description > tags; flat entry rows with pathSubtitle. */
export function searchEntries(tree: LibraryIndexFolder, query: string): ListRow[];

export function filterStarred(rows: ListRow[]): ListRow[]; // keeps folder headers with starred descendants
```

- Produces: `LibraryList` renders rows from the store state; entry rows show name, description (truncated), tag chips, star toggle, deployment badge (`●{deployment_count}` in emerald when `> 0`; amber `⚠` when the entry's `error` is set or a related orphan exists); folder headers toggle `expandedFolders`; rows are draggable (`draggable`, `onDragStart` sets `text/wardian-entry-ref`), folder headers accept drops → `renameEntry(section, entryPath, `${folderPath}/${name}`)`. Toolbar: search input, starred toggle, "New" split-button (new item / new folder — inline name input, calls `saveItem` with template content or `createFolder`), Reveal in Explorer.
- MCP stub: when `activeSection === 'mcps'`, list renders the empty-state copy instead of rows: heading "MCP servers are coming to the library", body "Define once, deploy to agents and classes — the same scoping skills use today."

- [ ] **Step 1: Write failing util tests** (flatten respects expansion; search ranks name over content; starred filter keeps ancestor folders) and component tests (row rendering incl. badges, folder toggle, search flattening switches modes, drag payload set).

- [ ] **Step 2: Run.** Expected: FAIL.

- [ ] **Step 3: Implement** utils then components. Keep `LibraryList.tsx` under ~250 lines by pushing row rendering into a `ListRowItem` component in the same file.

- [ ] **Step 4: Run** `npm run test -- libraryList LibraryList`. Expected: PASS.

- [ ] **Step 5: Commit** `feat(library): detailed list with folder groups and search`

---

### Task 15: DetailPane — editor + per-kind panels

**Files:**
- Create: `src/features/library/DetailPane.tsx`, `src/features/library/MarkdownEditor.tsx`, `src/features/library/DeployTargetsControl.tsx`
- Create: `src/features/library/detail/SkillDetail.tsx`, `detail/PromptDetail.tsx`, `detail/WorkflowDetail.tsx`, `detail/ClassDetail.tsx`, `detail/McpStubDetail.tsx`
- Test: `src/features/library/DetailPane.test.tsx`, `src/features/library/MarkdownEditor.test.tsx`, `src/features/library/DeployTargetsControl.test.tsx`

**Interfaces:**
- `MarkdownEditor({ value, onChange, onSave, dirty, stale, onReloadExternal })` — `<textarea>` with monospace themed styling, `Ctrl+S`/`Cmd+S` calls `onSave`, dirty dot in the header, and when `stale` is true renders the conflict bar: "File changed on disk — [Reload] [Keep mine]".
- `DeployTargetsControl({ entry, deployments, onApply })` — checklist of targets: "User (global)", every class (from index `classes` section), every *persisted* agent (needs agent list — reuse the same source `AssignSkillModal` uses today; check its imports and reuse that store/command). Copied targets render the amber "copied — edits won't sync" note. Apply button calls `onApply(targets)` → store `setSkillDeployments`. Accepts drops of skill refs.
- `DetailPane` switches on `selection`: none → empty-state; entry kind → panel. All panels share a header (name, tag editor, star toggle → `updateMetadata`; rename control → inline input → `renameEntry`; delete with confirm → `deleteEntry`).
- `PromptDetail` includes the **Run** button preserving today's behavior (`LibraryView.tsx:103-120`): flatten via `flattenPromptForInjection`, `submitInputToAgents(selectedAgentIds, …)`; disabled with tooltip when no agents selected.
- `WorkflowDetail` = editor + "Launch Run" (reuse `RunLaunchDialog` from `src/features/workflows/RunLaunchDialog.tsx`) + "Open in Workflows view" link (calls the same view-switch used by `onOpenWorkflowsView` in `App.tsx:1061` — thread a callback prop through `LibraryView`).
- `ClassDetail` = AGENTS.md editor (via the same read/save path) + deployed-skills list (from index deployments filtered to this class, with per-skill remove) + provider defaults display (reuse `list_agent_classes` data) + "Reset to default" (existing `reset_class_to_default` command) + delete class (existing `delete_agent_class`).

- [ ] **Step 1: Write failing tests:** editor save shortcut fires `onSave`; stale bar renders and Reload calls `onReloadExternal`; deploy control checks/unchecks and applies the full desired set; DetailPane renders the right panel per kind; prompt run disabled without selected agents.

- [ ] **Step 2: Run.** Expected: FAIL.

- [ ] **Step 3: Implement.** Dirty state lives in `DetailPane` (local `draft` string vs `selectedContent`); pass `editorDirty` into `store.select` so external changes set `contentStale` instead of overwriting. Unsaved-changes guard: switching selection with a dirty draft opens a confirm ("Discard changes?").

- [ ] **Step 4: Run** `npm run test -- Detail MarkdownEditor DeployTargets`. Expected: PASS.

- [ ] **Step 5: Commit** `feat(library): detail pane with inline editor and deploy control`

---

### Task 16: Retire old components and rewire the app

**Files:**
- Delete: `src/features/library/LibraryGrid.tsx` (+test), `LibraryCard.tsx`, `ItemEditorModal.tsx` (+test), `AssignSkillModal.tsx` (+test), `AssignPromptModal.tsx` (+test), `ManageSkills.tsx` (+test), `src/features/agents/ClassManagerPanel.tsx` (+test)
- Modify: `src/layout/SidebarContentPane.tsx` (remove `classes` tab block at :100-105 and the import at :8), `src/layout/SidebarIconRail.tsx` (remove the classes rail entry), `src/views/App.tsx` (drop props that existed only for the class panel; wire `onOpenWorkflowsView` through `LibraryView`), plus every file that imported the deleted components (grep first)
- Modify: `src/types/index.ts` — delete `LibraryFolder`/`LibrarySkill`/`LibraryPrompt`/`LibraryNode`-era types **if** `grep -rn "LibraryFolder\|LibrarySkill\|LibraryPrompt" src/` shows no remaining consumers (clone-preview types keep `DeployedSkillRef`)
- Test: update `src/views/App.test.tsx`, `src/layout/SidebarContentPane.test.tsx`, `src/layout/SidebarIconRail.test.tsx`

**Interfaces:**
- Consumes: everything from Tasks 12–15.
- Produces: any surface that previously opened `ClassManagerPanel` or `AssignSkillModal` now navigates to the library view with a preselected section/entry: add `openLibraryAt(section: LibrarySectionId, entryRef?: string)` to `useLibraryStore` (sets `activeSection` + `selection`) and have `App.tsx` switch the main view to the library when it fires. Check `ConfigureAgentPanel.tsx` and `SpawnAgentPanel.tsx` for `ManageSkills`/assign usages — `SpawnAgentPanel`'s class *selection* dropdown stays (it uses `list_agent_classes`, untouched); only management affordances re-point.

- [ ] **Step 1:** `grep -rn "ClassManagerPanel\|AssignSkillModal\|AssignPromptModal\|ItemEditorModal\|LibraryGrid\|LibraryCard\|ManageSkills" src/` — list every consumer before deleting anything.

- [ ] **Step 2: Update tests first** (SidebarContentPane test drops the classes-tab assertion; App test drops class-panel props), run to see them fail against current code where appropriate.

- [ ] **Step 3: Delete + rewire.** Remove dead props end to end (e.g. `agentClasses`/`onClassesUpdated` chains that existed only for the panel — verify with grep before removing; `SpawnAgentPanel` likely still needs `agentClasses`).

- [ ] **Step 4: Run** `npm run test` (full suite) and `npm run build`. Expected: PASS, zero TS errors, zero unused-import warnings.

- [ ] **Step 5: Commit** `refactor(library): retire grid, modals, and class sidebar panel`

---

### Task 17: E2E coverage and perf evidence

**Files:**
- Create: `e2e/library-redesign.spec.ts`
- Create: `e2e-native/tests/library-deployment-native.test.mjs`
- Modify: `crates/wardian-core/src/library/deployments.rs` (add `#[ignore]` perf-evidence test)
- Modify: `e2e/fixtures/mockAgent.ts` only if it lacks a library-seeding helper (add `seedLibraryFixtures(home)` writing 2 skills in nested folders, 1 prompt, 1 workflow, 1 class)

**Interfaces:**
- Consumes: the running app, seeded `WARDIAN_HOME`.

- [ ] **Step 1: Browser E2E** (`e2e/library-redesign.spec.ts`): seed fixtures → open library → assert all five rail sections render with counts; navigate a nested skill folder; search flattens with path subtitle; edit + `Ctrl+S` a prompt and assert the toolbar shows saved state; open a class and assert the AGENTS.md editor renders; MCP section shows the stub copy. Add a `test.skip(true, 'junction behavior — see library-deployment-native.test.mjs') // @native-only` placeholder for deploy assertions. Run: `npm run test:e2e -- library-redesign`. Expected: PASS.

- [ ] **Step 2: Native E2E** (`library-deployment-native.test.mjs`, modeled on `e2e-native/tests/cli-shared-state-native.test.mjs`): through the app's invoke bridge, `set_skill_deployments` to a class → assert the target is a reparse point (`fs.lstatSync(target).isSymbolicLink()` is false for junctions on Windows — instead assert `fs.statSync` works AND editing the library source file is visible through the target path); `rename_library_entry` → junction follows; `delete_library_entry` → target gone. Run: `npm run test:e2e:native:fast -- e2e-native/tests/library-deployment-native.test.mjs`. Expected: PASS.

- [ ] **Step 3: Perf evidence** — an `#[ignore]` Rust test in `crates/wardian-core/src/library/deployments.rs` (a standalone script is not needed; the library CLI does not exist yet):

```rust
#[test]
#[ignore = "manual perf evidence: cargo test -p wardian-core --release deploy_perf -- --ignored --nocapture"]
fn deploy_perf_twenty_sequential() {
    let temp = tempfile::tempdir().expect("temp");
    let home = temp.path();
    let src = home.join("library/skills/planner");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("SKILL.md"), "perf").unwrap();
    let start = std::time::Instant::now();
    for i in 0..20 {
        deploy_skill(home, "planner", "agent", &format!("agent-{i}")).expect("deploy");
    }
    println!("20 deploys in {:?}", start.elapsed());
}
```

Run it on Windows, note the elapsed time for the PR description. For the "before" number, run the same loop once on `main` (the old `mklink` path) via a scratch checkout, or cite a manual timing of 20 deploys through the current UI.

- [ ] **Step 4: Commit** `test(library): browser and native e2e for redesigned library`

---

### Task 18: Docs, screenshots, and final verification

**Files:**
- Modify: `docs/guide/library.md` (the `DocsLink path="/guide/library"` target — rewrite sections describing tabs/grid/modals to describe rail/list/detail, classes-in-library, workflow blueprints, and the MCP stub; keep cross-OS command examples per repo doc rules)
- Modify: `docs/specs/2026-07-05-library-redesign.md` — flip **Status:** to `Implemented`
- Create: `e2e/screenshots/library-redesign/<timestamp>/` captures

**Steps:**

- [ ] **Step 1: Update the guide.** Describe: the five sections and what each holds; folder organization + drag-to-move; deploying skills from the detail pane (and what emerald/amber badges mean, including "copied — edits won't sync"); class workbench; where workflow *runs* still live; MCP stub disclaimer.

- [ ] **Step 2: Screenshots.** With `WARDIAN_HOME` pointed at a seeded temp dir, capture: (1) skills list with folder groups + deployment badges, (2) skill detail pane with deploy targets open, (3) class workbench. Save under `e2e/screenshots/library-redesign/<timestamp>/`, upload via the GitHub attachment flow, embed at least one HTTPS image in the PR body (`npm run check:frontend-screenshot` gates this).

- [ ] **Step 3: Full verification suite:**

```
npm run lint && npm run test && npm run build
cd src-tauri && cargo clippy && cargo test && cargo check
npm run test:e2e
npm run test:e2e:native   # if harness available
```

Expected: all green.

- [ ] **Step 4: Commit** `docs(library): guide refresh for unified library`, then open the PR: link the GitHub issue (create one first if none exists — required by repo standards), use the PR template, include the perf before/after numbers and the embedded screenshot.

---

## Self-Review Notes

- **Spec coverage:** §1 data model → Tasks 5–7; §2 core layering → Tasks 1–10 (all engine code in `wardian-core`); §3 commands/perf → Tasks 3, 10, 11; §4 frontend → Tasks 12–16; §5 errors/edges → Tasks 4 (migration), 6 (orphans), 9 (rename/delete integrity), 15 (conflict bar, copied warning); §6 testing → every task + Task 17; CLI-readiness → enforced by `home: &Path` signatures and zero Tauri types in `crates/wardian-core/src/library/`.
- **Known sharpening vs. old behavior:** ambiguous unmarked legacy copies become orphans (Task 6) instead of `source_path: None` rows — surfaced as drift with cleanup, matching the spec's "visible drift" consequence.
- **Type consistency:** `LibrarySectionId` (Rust enum / TS union), `LibraryEntry.entry_ref`, `DeploymentTarget.linked`, `set_skill_deployments(source_path, targets)` are used with identical names in Tasks 5, 10, 11, 12, 15, 17.
