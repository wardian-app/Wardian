use crate::manager::codex::codex_session_file_path;
use crate::utils::fs::{
    create_directory_link, ensure_codex_home_projection, sync_codex_home_indexes,
};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

const SESSION_ID: &str = "6aa31bfb-52de-4ef8-a103-2b2a244a64af";

struct Fixture {
    _temp: tempfile::TempDir,
    original: PathBuf,
    selected: PathBuf,
    habitat: PathBuf,
    projected: PathBuf,
    previous_env: Vec<(&'static str, Option<OsString>)>,
    previous_native_home: Option<PathBuf>,
    _guard: tokio::sync::MutexGuard<'static, ()>,
}

impl Fixture {
    fn new() -> Self {
        let guard = crate::utils::wardian_test_env_lock();
        let temp = tempfile::tempdir().unwrap();
        let original = temp.path().join("original");
        let selected = temp.path().join("selected");
        let wardian = temp.path().join("wardian");
        let habitat = wardian.join("agents/agent/habitat");
        let projected = habitat.join(".codex");
        for path in [
            &original,
            &selected,
            &projected,
            &habitat.join(".agents/skills"),
        ] {
            std::fs::create_dir_all(path).unwrap();
        }
        let previous_env = ["WARDIAN_HOME", "CODEX_HOME"]
            .into_iter()
            .map(|key| (key, std::env::var_os(key)))
            .collect();
        unsafe {
            std::env::set_var("WARDIAN_HOME", &wardian);
            std::env::set_var("CODEX_HOME", &selected);
        }
        // Reuse the existing unit-test surrogate. No native-build switch or
        // provider environment is changed by this fixture.
        let previous_native_home = crate::utils::codex_messaging::TEST_NATIVE_HOME
            .with(|home| home.replace(Some(original.clone())));
        Self {
            _temp: temp,
            original,
            selected,
            habitat,
            projected,
            previous_env,
            previous_native_home,
            _guard: guard,
        }
    }

    fn prepare(&self) -> Result<(), String> {
        ensure_codex_home_projection(&self.habitat, self._temp.path(), "agent")
    }

    fn rollout(&self, home: &Path, namespace: &str) -> PathBuf {
        let directory = home.join(namespace);
        std::fs::create_dir_all(&directory).unwrap();
        let file = directory.join(format!("rollout-2026-10-04T00-00-00-{SESSION_ID}.jsonl"));
        let header = serde_json::json!({"type":"session_meta","payload":{
            "id": SESSION_ID, "source":"cli", "model_provider":"openai"
        }});
        std::fs::write(&file, format!("{header}\n")).unwrap();
        file
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        for (key, value) in &self.previous_env {
            unsafe {
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
        }
        crate::utils::codex_messaging::TEST_NATIVE_HOME
            .with(|home| home.replace(self.previous_native_home.take()));
    }
}

fn snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(root: &Path, directory: &Path, result: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in std::fs::read_dir(directory).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let relative = path.strip_prefix(root).unwrap().to_path_buf();
            if entry.file_type().unwrap().is_dir() {
                result.insert(relative, Vec::new());
                visit(root, &path, result);
            } else {
                result.insert(relative, std::fs::read(path).unwrap());
            }
        }
    }
    let mut result = BTreeMap::new();
    visit(root, root, &mut result);
    result
}

#[test]
fn issue1386_explicit_home_binds_preparation_and_periodic_index_writes() {
    let fixture = Fixture::new();
    fixture.rollout(&fixture.original, "sessions/2026/10/04");
    let active = fixture.rollout(&fixture.selected, "sessions/2026/10/04");
    let archive = fixture.rollout(&fixture.selected, "archived_sessions");
    std::fs::write(fixture.selected.join("auth.json"), "fixture-only-auth").unwrap();
    let original_before = snapshot(&fixture.original);

    fixture.prepare().unwrap();
    for name in ["sessions", "archived_sessions"] {
        assert_eq!(
            std::fs::canonicalize(fixture.projected.join(name)).unwrap(),
            std::fs::canonicalize(fixture.selected.join(name)).unwrap()
        );
    }
    let agent_dir = fixture.habitat.parent().unwrap().to_str().unwrap();
    // Filename and header remain unchanged; only the selected source differs.
    let path = codex_session_file_path(SESSION_ID, Some(agent_dir)).unwrap();
    assert_eq!(
        std::fs::canonicalize(path).unwrap(),
        std::fs::canonicalize(&active).unwrap()
    );
    std::fs::remove_file(active).unwrap();
    let path = codex_session_file_path(SESSION_ID, Some(agent_dir)).unwrap();
    assert_eq!(
        std::fs::canonicalize(path).unwrap(),
        std::fs::canonicalize(archive).unwrap()
    );

    std::fs::write(
        fixture.projected.join("session_index.jsonl"),
        "{\"id\":\"new\"}\n",
    )
    .unwrap();
    std::fs::write(
        fixture.projected.join("history.jsonl"),
        "{\"session_id\":\"new\",\"text\":\"hello\"}\n",
    )
    .unwrap();
    sync_codex_home_indexes(&fixture.projected).unwrap();
    assert!(fixture.selected.join(".wardian-codex-index.lock").is_file());
    assert_eq!(
        std::fs::read(fixture.selected.join("session_index.jsonl")).unwrap(),
        b"{\"id\":\"new\"}\n"
    );
    assert_eq!(
        std::fs::read(fixture.selected.join("history.jsonl")).unwrap(),
        b"{\"session_id\":\"new\",\"text\":\"hello\"}\n"
    );
    assert_eq!(
        std::fs::read(fixture.selected.join("auth.json")).unwrap(),
        b"fixture-only-auth"
    );
    assert_eq!(snapshot(&fixture.original), original_before);
}

#[test]
fn issue1386_foreign_archive_link_rejects_before_session_migration() {
    let fixture = Fixture::new();
    fixture.rollout(&fixture.original, "archived_sessions");
    fixture.rollout(&fixture.projected, "sessions/2026/10/04");
    create_directory_link(
        &fixture.original.join("archived_sessions"),
        &fixture.projected.join("archived_sessions"),
    )
    .unwrap();
    let original_before = snapshot(&fixture.original);
    let selected_before = snapshot(&fixture.selected);
    assert!(fixture.prepare().is_err());
    assert_eq!(snapshot(&fixture.original), original_before);
    assert_eq!(snapshot(&fixture.selected), selected_before);
    assert!(fixture.projected.join("sessions/2026/10/04").is_dir());
    assert!(sync_codex_home_indexes(&fixture.projected).is_err());
}

#[test]
fn issue1386_invalid_explicit_home_rejects_preparation_and_empty_index_sync() {
    let fixture = Fixture::new();
    let original_before = snapshot(&fixture.original);
    for invalid in [
        fixture._temp.path().join("missing"),
        fixture._temp.path().join("file"),
    ] {
        if invalid.file_name().unwrap() == "file" {
            std::fs::write(&invalid, "not a directory").unwrap();
        }
        unsafe { std::env::set_var("CODEX_HOME", &invalid) };
        assert!(fixture.prepare().is_err());
        // There are no local index files, so the unfixed wrapper can be
        // reproduced without writing any OS-profile index or lock.
        assert!(sync_codex_home_indexes(&fixture.projected).is_err());
    }
    assert_eq!(snapshot(&fixture.original), original_before);
}

#[test]
fn issue1386_invalid_explicit_home_cannot_use_a_foreign_projected_rollout() {
    let fixture = Fixture::new();
    fixture.rollout(&fixture.original, "sessions/2026/10/04");
    create_directory_link(
        &fixture.original.join("sessions"),
        &fixture.projected.join("sessions"),
    )
    .unwrap();
    unsafe { std::env::set_var("CODEX_HOME", fixture._temp.path().join("missing")) };
    let agent_dir = fixture.habitat.parent().unwrap().to_str().unwrap();
    assert!(codex_session_file_path(SESSION_ID, Some(agent_dir)).is_none());
}

#[test]
fn issue1386_default_home_remains_shared_when_override_is_unset_or_empty() {
    for empty in [false, true] {
        let fixture = Fixture::new();
        let rollout = fixture.rollout(&fixture.original, "sessions/2026/10/04");
        unsafe {
            if empty {
                std::env::set_var("CODEX_HOME", "");
            } else {
                std::env::remove_var("CODEX_HOME");
            }
        }
        let upstream = super::resolve_upstream_home().unwrap();
        assert!(!upstream.explicit);
        assert_eq!(upstream.path, fixture.original);
        fixture.prepare().unwrap();
        for name in ["sessions", "archived_sessions"] {
            assert_eq!(
                std::fs::canonicalize(fixture.projected.join(name)).unwrap(),
                std::fs::canonicalize(fixture.original.join(name)).unwrap()
            );
        }
        assert_eq!(codex_session_file_path(SESSION_ID, None), Some(rollout));
        std::fs::write(
            fixture.projected.join("session_index.jsonl"),
            "{\"id\":\"default\"}\n",
        )
        .unwrap();
        sync_codex_home_indexes(&fixture.projected).unwrap();
        assert_eq!(
            std::fs::read(fixture.original.join("session_index.jsonl")).unwrap(),
            b"{\"id\":\"default\"}\n"
        );
        assert!(fixture.original.join(".wardian-codex-index.lock").is_file());
        assert!(snapshot(&fixture.selected).is_empty());
    }
}

#[test]
fn issue1386_explicit_lookup_never_falls_back_when_the_selected_rollout_is_missing() {
    let fixture = Fixture::new();
    fixture.rollout(&fixture.original, "sessions/2026/10/04");
    unsafe { std::env::set_var("CODEX_HOME", fixture.selected.join(".")) };
    assert_eq!(
        super::resolve_upstream_home().unwrap().path,
        std::fs::canonicalize(&fixture.selected).unwrap()
    );
    assert!(codex_session_file_path(SESSION_ID, None).is_none());
    let selected = fixture.rollout(&fixture.selected, "sessions/2026/10/04");
    assert_eq!(
        std::fs::canonicalize(codex_session_file_path(SESSION_ID, None).unwrap()).unwrap(),
        std::fs::canonicalize(&selected).unwrap()
    );
    // Matching the filename alone cannot substitute a different native identity.
    std::fs::write(
        selected,
        "{\"type\":\"session_meta\",\"payload\":{\"id\":\"different\"}}\n",
    )
    .unwrap();
    assert!(codex_session_file_path(SESSION_ID, None).is_none());
}

#[test]
fn issue1386_periodic_writer_rejects_a_later_foreign_archive_link_before_locking() {
    let fixture = Fixture::new();
    fixture.prepare().unwrap();
    let archive = fixture.projected.join("archived_sessions");
    std::fs::remove_dir(&archive)
        .or_else(|_| std::fs::remove_file(&archive))
        .unwrap();
    fixture.rollout(&fixture.original, "archived_sessions");
    create_directory_link(&fixture.original.join("archived_sessions"), &archive).unwrap();
    std::fs::write(
        fixture.projected.join("session_index.jsonl"),
        "{\"id\":\"unpublished\"}\n",
    )
    .unwrap();
    let original_before = snapshot(&fixture.original);
    let selected_before = snapshot(&fixture.selected);
    assert!(sync_codex_home_indexes(&fixture.projected).is_err());
    assert_eq!(snapshot(&fixture.original), original_before);
    assert_eq!(snapshot(&fixture.selected), selected_before);
    assert!(!fixture.selected.join(".wardian-codex-index.lock").exists());
}
