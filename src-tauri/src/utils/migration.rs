use std::path::Path;

const CURRENT_SCHEMA_VERSION: u32 = 1;
const MIGRATIONS_FILE: &str = "settings/migrations.json";

fn get_schema_version(home: &Path) -> u32 {
    let path = home.join(MIGRATIONS_FILE);
    if let Ok(data) = std::fs::read_to_string(&path) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&data) {
            return v["version"].as_u64().unwrap_or(0) as u32;
        }
    }
    0
}

fn set_schema_version(home: &Path, version: u32) -> std::io::Result<()> {
    let path = home.join(MIGRATIONS_FILE);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::json!({ "version": version }).to_string();
    std::fs::write(path, json)
}

/// Public entry point — resolves WARDIAN_HOME and delegates to run_migration.
pub fn migrate_home_layout() {
    let Some(home) = crate::utils::fs::get_wardian_home() else { return };
    run_migration(&home);
}

/// Testable inner function — accepts home dir directly.
fn run_migration(home: &Path) {
    let version = get_schema_version(home);
    if version >= CURRENT_SCHEMA_VERSION {
        return;
    }
    let success = version < 1 && run_migration_1(home);
    if success {
        if let Err(e) = set_schema_version(home, CURRENT_SCHEMA_VERSION) {
            eprintln!("[migration] Failed to write version marker: {e}");
        }
    } else if version < 1 {
        eprintln!("[migration] Migration 1 incomplete — will retry next launch");
    }
}

/// Copy src to dst then delete src. Returns true if succeeded or src was absent/already moved.
fn move_file(src: &std::path::Path, dst: &std::path::Path) -> bool {
    if !src.exists() { return true; }
    if dst.exists() { return true; }
    if let Some(parent) = dst.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            eprintln!("[migration] Could not create dir {:?}: {e}", parent);
            return false;
        }
    }
    match std::fs::copy(src, dst) {
        Ok(_) => {
            if let Err(e) = std::fs::remove_file(src) {
                eprintln!("[migration] Copied {:?} but could not delete original: {e}", src);
            } else {
                println!("[migration] Moved {:?} \u{2192} {:?}", src, dst);
            }
            true
        }
        Err(e) => { eprintln!("[migration] Failed to copy {:?}: {e}", src); false }
    }
}

/// Rename directory src → dst. Returns true if succeeded or src was absent/already moved.
fn move_dir(src: &std::path::Path, dst: &std::path::Path) -> bool {
    if !src.exists() { return true; }
    if dst.exists() { return true; }
    if let Some(parent) = dst.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match std::fs::rename(src, dst) {
        Ok(_) => { println!("[migration] Moved dir {:?} \u{2192} {:?}", src, dst); true }
        Err(e) => { eprintln!("[migration] Failed to move dir {:?}: {e}", src); false }
    }
}

fn run_migration_1(home: &Path) -> bool {
    [
        move_file(&home.join("watchlists.json"),     &home.join("watchlists/index.json")),
        move_file(&home.join("wardian_state.json"),  &home.join("settings/state.json")),
        move_file(&home.join("shell_settings.json"), &home.join("settings/shell.json")),
        move_file(&home.join("scheduled_runs.json"), &home.join("scheduled_workflows.json")),
        move_dir( &home.join("workflow_logs"),        &home.join("logs/workflows")),
    ].iter().all(|&ok| ok)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn temp_home(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("wardian-migration-test-{}", name));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn get_schema_version_returns_zero_when_missing() {
        let home = temp_home("no-file");
        assert_eq!(get_schema_version(&home), 0);
    }

    #[test]
    fn set_and_get_schema_version_round_trips() {
        let home = temp_home("round-trip");
        set_schema_version(&home, 1).unwrap();
        assert_eq!(get_schema_version(&home), 1);
    }

    #[test]
    fn migration_skipped_when_version_current() {
        let home = temp_home("skip");
        fs::write(home.join("watchlists.json"), "[]").unwrap();
        set_schema_version(&home, CURRENT_SCHEMA_VERSION).unwrap();
        run_migration(&home); // call inner directly — no env var needed
        // File should still be at old location — migration was skipped
        assert!(home.join("watchlists.json").exists());
        assert!(!home.join("watchlists/index.json").exists());
    }

    #[test]
    fn migration_1_moves_all_files_and_sets_version() {
        let home = temp_home("migration-1");
        fs::write(home.join("watchlists.json"), "[{\"id\":\"1\"}]").unwrap();
        fs::write(home.join("wardian_state.json"), "[]").unwrap();
        fs::write(home.join("shell_settings.json"), "{\"shell_id\":\"bash\"}").unwrap();
        fs::write(home.join("scheduled_runs.json"), "[]").unwrap();
        fs::create_dir_all(home.join("workflow_logs/wf-1")).unwrap();
        fs::write(home.join("workflow_logs/wf-1/run.log"), "log").unwrap();

        run_migration(&home);

        assert!(home.join("watchlists/index.json").exists(), "watchlists moved");
        assert!(!home.join("watchlists.json").exists(), "old watchlists removed");
        assert!(home.join("settings/state.json").exists(), "state moved");
        assert!(!home.join("wardian_state.json").exists(), "old state removed");
        assert!(home.join("settings/shell.json").exists(), "shell moved");
        assert!(!home.join("shell_settings.json").exists(), "old shell removed");
        assert!(home.join("scheduled_workflows.json").exists(), "scheduled renamed");
        assert!(!home.join("scheduled_runs.json").exists(), "old scheduled removed");
        assert!(home.join("logs/workflows/wf-1/run.log").exists(), "workflow logs moved");
        assert_eq!(get_schema_version(&home), CURRENT_SCHEMA_VERSION);
    }

    #[test]
    fn migration_is_idempotent() {
        let home = temp_home("idempotent");
        fs::write(home.join("watchlists.json"), "[]").unwrap();
        run_migration(&home);
        run_migration(&home); // second run should be a no-op
        assert!(home.join("watchlists/index.json").exists());
        assert_eq!(get_schema_version(&home), CURRENT_SCHEMA_VERSION);
    }
}
