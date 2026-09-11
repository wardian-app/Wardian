use super::*;

/// Build a database shaped like the provider's thread index.
fn thread_database(path: &Path, threads: i64, projects: i64) {
    let connection = rusqlite::Connection::open(path).expect("open fixture database");
    connection
        .execute_batch(
            "CREATE TABLE threads (id TEXT PRIMARY KEY, cwd TEXT);
             CREATE TABLE projects (id TEXT PRIMARY KEY, root TEXT);
             CREATE TABLE project_roots (id TEXT PRIMARY KEY);
             CREATE TABLE remote_control_enrollments (id TEXT PRIMARY KEY);",
        )
        .expect("create fixture schema");
    for index in 0..threads {
        connection
            .execute(
                "INSERT INTO threads (id, cwd) VALUES (?1, ?2)",
                [format!("thread-{index}"), "workspace".to_owned()],
            )
            .expect("insert thread");
    }
    for index in 0..projects {
        connection
            .execute(
                "INSERT INTO projects (id, root) VALUES (?1, ?2)",
                [format!("project-{index}"), "private".to_owned()],
            )
            .expect("insert project");
        connection
            .execute(
                "INSERT INTO remote_control_enrollments (id) VALUES (?1)",
                [format!("enrolment-{index}")],
            )
            .expect("insert enrolment");
    }
}

fn count(path: &Path, table: &str) -> i64 {
    let connection = rusqlite::Connection::open(path).expect("open database");
    connection
        .query_row(&format!("SELECT COUNT(*) FROM \"{table}\""), [], |row| {
            row.get(0)
        })
        .expect("count rows")
}

fn home(root: &Path, name: &str) -> PathBuf {
    let path = root.join(name);
    std::fs::create_dir_all(&path).expect("create home");
    path
}

#[test]
fn a_warm_home_publishes_a_snapshot_that_seeds_a_new_one() {
    let temp = tempfile::tempdir().expect("temp");
    let wardian_home = home(temp.path(), "wardian");
    let warm = home(temp.path(), "warm");
    thread_database(&warm.join("state_5.sqlite"), 12, 0);

    assert!(refresh(&wardian_home, &warm).expect("publish snapshot"));

    let fresh = home(temp.path(), "fresh");
    assert!(seed(&wardian_home, &fresh).expect("seed fresh home"));
    let seeded = fresh.join("state_5.sqlite");
    assert!(seeded.is_file(), "seed wrote the provider's own filename");
    assert_eq!(count(&seeded, "threads"), 12);
}

#[test]
fn per_agent_rows_never_travel_between_homes() {
    let temp = tempfile::tempdir().expect("temp");
    let wardian_home = home(temp.path(), "wardian");
    let warm = home(temp.path(), "warm");
    thread_database(&warm.join("state_5.sqlite"), 4, 3);

    refresh(&wardian_home, &warm).expect("publish snapshot");
    let fresh = home(temp.path(), "fresh");
    seed(&wardian_home, &fresh).expect("seed fresh home");

    let seeded = fresh.join("state_5.sqlite");
    assert_eq!(count(&seeded, "threads"), 4, "shared history is retained");
    assert_eq!(count(&seeded, "projects"), 0);
    assert_eq!(count(&seeded, "remote_control_enrollments"), 0);
    // The warm home keeps its own rows; only the published copy is stripped.
    assert_eq!(count(&warm.join("state_5.sqlite"), "projects"), 3);
}

#[test]
fn a_home_that_already_has_a_thread_database_is_left_alone() {
    let temp = tempfile::tempdir().expect("temp");
    let wardian_home = home(temp.path(), "wardian");
    let warm = home(temp.path(), "warm");
    thread_database(&warm.join("state_5.sqlite"), 9, 0);
    refresh(&wardian_home, &warm).expect("publish snapshot");

    let existing = home(temp.path(), "existing");
    thread_database(&existing.join("state_5.sqlite"), 1, 0);
    assert!(!seed(&wardian_home, &existing).expect("seed is skipped"));
    assert_eq!(
        count(&existing.join("state_5.sqlite"), "threads"),
        1,
        "the provider's own database is never overwritten"
    );
}

#[test]
fn seeding_without_a_published_snapshot_is_not_an_error() {
    let temp = tempfile::tempdir().expect("temp");
    let wardian_home = home(temp.path(), "wardian");
    let fresh = home(temp.path(), "fresh");
    assert!(!seed(&wardian_home, &fresh).expect("no snapshot yet"));
    assert!(state_database_name(&fresh).is_none());
}

#[test]
fn a_home_without_a_thread_database_publishes_nothing() {
    let temp = tempfile::tempdir().expect("temp");
    let wardian_home = home(temp.path(), "wardian");
    let cold = home(temp.path(), "cold");
    assert!(!refresh(&wardian_home, &cold).expect("nothing to publish"));
}

#[test]
fn a_recent_snapshot_is_not_republished() {
    let temp = tempfile::tempdir().expect("temp");
    let wardian_home = home(temp.path(), "wardian");
    let warm = home(temp.path(), "warm");
    thread_database(&warm.join("state_5.sqlite"), 2, 0);
    assert!(refresh(&wardian_home, &warm).expect("first publication"));

    // Change the source; a snapshot inside its interval must not follow it.
    let connection =
        rusqlite::Connection::open(warm.join("state_5.sqlite")).expect("reopen database");
    connection
        .execute("DELETE FROM threads", [])
        .expect("empty the source");
    drop(connection);
    assert!(!refresh(&wardian_home, &warm).expect("second call"));
    let cache = cache_directory(&wardian_home);
    assert_eq!(read_meta(&cache).expect("meta").threads, 2);
}

#[test]
fn a_stale_snapshot_is_replaced() {
    let temp = tempfile::tempdir().expect("temp");
    let wardian_home = home(temp.path(), "wardian");
    let warm = home(temp.path(), "warm");
    thread_database(&warm.join("state_5.sqlite"), 2, 0);
    refresh(&wardian_home, &warm).expect("first publication");

    let cache = cache_directory(&wardian_home);
    let mut meta = read_meta(&cache).expect("meta");
    meta.captured_at = (chrono::Utc::now()
        - chrono::Duration::seconds(REFRESH_INTERVAL_SECONDS + 60))
    .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    std::fs::write(
        cache.join(SNAPSHOT_META_FILE),
        serde_json::to_vec(&meta).expect("encode"),
    )
    .expect("write aged metadata");

    std::fs::remove_file(warm.join("state_5.sqlite")).expect("replace database");
    thread_database(&warm.join("state_5.sqlite"), 7, 0);
    assert!(refresh(&wardian_home, &warm).expect("republication"));
    assert_eq!(read_meta(&cache).expect("meta").threads, 7);
}

#[test]
fn a_snapshot_captured_from_a_live_writer_is_complete() {
    let temp = tempfile::tempdir().expect("temp");
    let wardian_home = home(temp.path(), "wardian");
    let warm = home(temp.path(), "warm");
    let database = warm.join("state_5.sqlite");
    thread_database(&database, 5, 0);

    // Hold the database open in write-ahead mode, as a resident daemon does.
    let live = rusqlite::Connection::open(&database).expect("open live writer");
    live.pragma_update(None, "journal_mode", "WAL")
        .expect("enable write-ahead log");
    live.execute(
        "INSERT INTO threads (id, cwd) VALUES ('live', 'workspace')",
        [],
    )
    .expect("live insert");

    assert!(refresh(&wardian_home, &warm).expect("publish while open"));
    let fresh = home(temp.path(), "fresh");
    seed(&wardian_home, &fresh).expect("seed");
    assert_eq!(count(&fresh.join("state_5.sqlite"), "threads"), 6);
}

#[test]
fn a_cached_name_that_escapes_its_home_is_refused() {
    let temp = tempfile::tempdir().expect("temp");
    let wardian_home = home(temp.path(), "wardian");
    let warm = home(temp.path(), "warm");
    thread_database(&warm.join("state_5.sqlite"), 1, 0);
    refresh(&wardian_home, &warm).expect("publish");

    let cache = cache_directory(&wardian_home);
    let mut meta = read_meta(&cache).expect("meta");
    meta.database = "../escaped.sqlite".to_owned();
    std::fs::write(
        cache.join(SNAPSHOT_META_FILE),
        serde_json::to_vec(&meta).expect("encode"),
    )
    .expect("write tampered metadata");

    let fresh = home(temp.path(), "fresh");
    assert!(seed(&wardian_home, &fresh).is_err());
    assert!(!temp.path().join("escaped.sqlite").exists());
}

#[test]
fn only_a_generation_suffixed_state_database_is_recognised() {
    let temp = tempfile::tempdir().expect("temp");
    let codex_home = home(temp.path(), "home");
    for name in [
        "state.sqlite",
        "state_.sqlite",
        "state_x.sqlite",
        "logs_2.sqlite",
    ] {
        std::fs::write(codex_home.join(name), b"").expect("write file");
    }
    assert!(state_database_name(&codex_home).is_none());

    std::fs::write(codex_home.join("state_12.sqlite"), b"").expect("write file");
    assert_eq!(
        state_database_name(&codex_home).as_deref(),
        Some("state_12.sqlite")
    );
}
