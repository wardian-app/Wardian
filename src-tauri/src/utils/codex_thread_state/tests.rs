use super::*;

const REAL_SESSIONS_PARENT: &str = "real-codex";

/// Build a database shaped like the provider's thread index.
fn thread_database(path: &Path, threads: i64, projects: i64) {
    let connection = rusqlite::Connection::open(path).expect("open fixture database");
    connection
        .execute_batch(
            "CREATE TABLE threads (id TEXT PRIMARY KEY, rollout_path TEXT NOT NULL, cwd TEXT);
             CREATE TABLE thread_sections (id TEXT PRIMARY KEY);
             CREATE TABLE backfill_state (id INTEGER PRIMARY KEY);
             CREATE TABLE projects (id TEXT PRIMARY KEY, root TEXT);",
        )
        .expect("create fixture schema");
    for index in 0..threads {
        connection
            .execute(
                "INSERT INTO threads (id, rollout_path, cwd) VALUES (?1, ?2, ?3)",
                [
                    format!("thread-{index}"),
                    format!("C:\\agents\\publisher\\habitat\\.codex\\sessions\\2026\\rollout-{index}.jsonl"),
                    "workspace".to_owned(),
                ],
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

fn rollout_paths(path: &Path) -> Vec<String> {
    let connection = rusqlite::Connection::open(path).expect("open database");
    let mut statement = connection
        .prepare("SELECT rollout_path FROM threads ORDER BY id")
        .expect("prepare");
    let rows = statement
        .query_map([], |row| row.get::<_, String>(0))
        .expect("query")
        .collect::<Result<Vec<_>, _>>()
        .expect("collect");
    rows
}

fn home(root: &Path, name: &str) -> PathBuf {
    let path = root.join(name);
    std::fs::create_dir_all(&path).expect("create home");
    path
}

fn real_codex(root: &Path) -> PathBuf {
    let path = root.join(REAL_SESSIONS_PARENT);
    std::fs::create_dir_all(path.join("sessions")).expect("create central sessions");
    path
}

#[test]
fn a_warm_home_publishes_a_snapshot_that_seeds_a_new_one() {
    let temp = tempfile::tempdir().expect("temp");
    let wardian_home = home(temp.path(), "wardian");
    let central = real_codex(temp.path());
    let warm = home(temp.path(), "warm");
    thread_database(&warm.join("state_5.sqlite"), 12, 0);

    assert!(refresh(&wardian_home, &warm, &central).expect("publish snapshot"));

    let fresh = home(temp.path(), "fresh");
    assert!(seed(&wardian_home, &fresh).expect("seed fresh home"));
    let seeded = fresh.join("state_5.sqlite");
    assert!(seeded.is_file(), "seed wrote the provider's own filename");
    assert_eq!(count(&seeded, "threads"), 12);
}

#[test]
fn a_published_snapshot_names_no_agent_home() {
    let temp = tempfile::tempdir().expect("temp");
    let wardian_home = home(temp.path(), "wardian");
    let central = real_codex(temp.path());
    let warm = home(temp.path(), "warm");
    thread_database(&warm.join("state_5.sqlite"), 3, 0);

    refresh(&wardian_home, &warm, &central).expect("publish snapshot");
    let fresh = home(temp.path(), "fresh");
    seed(&wardian_home, &fresh).expect("seed");

    // Left alone, every row would point into the publishing agent's own home,
    // which dangles as soon as that agent is removed.
    for path in rollout_paths(&fresh.join("state_5.sqlite")) {
        assert!(
            !path.contains("publisher"),
            "seeded row still names the publishing agent: {path}"
        );
        assert!(
            path.starts_with(&central.join("sessions").to_string_lossy().into_owned()),
            "seeded row does not name the central tree: {path}"
        );
        assert!(
            path.ends_with(".jsonl"),
            "rollout identity was lost: {path}"
        );
    }
}

#[test]
fn an_unshared_table_with_rows_refuses_publication() {
    let temp = tempfile::tempdir().expect("temp");
    let wardian_home = home(temp.path(), "wardian");
    let central = real_codex(temp.path());
    let warm = home(temp.path(), "warm");
    thread_database(&warm.join("state_5.sqlite"), 4, 3);

    // `projects` is not on the allow-list and holds rows, so publishing it
    // would carry one agent's private rows into every other home.
    let error = refresh(&wardian_home, &warm, &central).expect_err("must refuse");
    assert!(error.contains("projects"), "{error}");

    let fresh = home(temp.path(), "fresh");
    assert!(!seed(&wardian_home, &fresh).expect("nothing published"));
}

#[test]
fn an_unshared_but_empty_table_is_cleared_rather_than_refused() {
    let temp = tempfile::tempdir().expect("temp");
    let wardian_home = home(temp.path(), "wardian");
    let central = real_codex(temp.path());
    let warm = home(temp.path(), "warm");
    thread_database(&warm.join("state_5.sqlite"), 2, 0);

    assert!(refresh(&wardian_home, &warm, &central).expect("publish"));
    let fresh = home(temp.path(), "fresh");
    seed(&wardian_home, &fresh).expect("seed");
    assert_eq!(count(&fresh.join("state_5.sqlite"), "projects"), 0);
    assert_eq!(count(&fresh.join("state_5.sqlite"), "threads"), 2);
}

#[test]
fn a_generation_change_republishes_and_seeds_the_new_name() {
    let temp = tempfile::tempdir().expect("temp");
    let wardian_home = home(temp.path(), "wardian");
    let central = real_codex(temp.path());

    let old = home(temp.path(), "old");
    thread_database(&old.join("state_5.sqlite"), 2, 0);
    assert!(refresh(&wardian_home, &old, &central).expect("publish generation 5"));

    // A provider upgrade moves to a new generation well inside the refresh
    // interval; the cache must follow rather than serve an unusable file.
    let upgraded = home(temp.path(), "upgraded");
    thread_database(&upgraded.join("state_6.sqlite"), 9, 0);
    assert!(refresh(&wardian_home, &upgraded, &central).expect("republish generation 6"));

    let fresh = home(temp.path(), "fresh");
    assert!(seed(&wardian_home, &fresh).expect("seed"));
    assert!(fresh.join("state_6.sqlite").is_file());
    assert!(!fresh.join("state_5.sqlite").exists());
    assert_eq!(count(&fresh.join("state_6.sqlite"), "threads"), 9);
}

#[test]
fn the_live_generation_is_selected_when_an_old_one_remains() {
    let temp = tempfile::tempdir().expect("temp");
    let codex_home = home(temp.path(), "home");
    thread_database(&codex_home.join("state_5.sqlite"), 1, 0);
    thread_database(&codex_home.join("state_12.sqlite"), 1, 0);
    assert_eq!(
        state_database_name(&codex_home).as_deref(),
        Some("state_12.sqlite"),
        "highest generation wins regardless of directory order"
    );
}

#[test]
fn a_home_that_already_has_a_thread_database_is_left_alone() {
    let temp = tempfile::tempdir().expect("temp");
    let wardian_home = home(temp.path(), "wardian");
    let central = real_codex(temp.path());
    let warm = home(temp.path(), "warm");
    thread_database(&warm.join("state_5.sqlite"), 9, 0);
    refresh(&wardian_home, &warm, &central).expect("publish snapshot");

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
fn a_database_created_after_the_guard_is_never_clobbered() {
    let temp = tempfile::tempdir().expect("temp");
    let wardian_home = home(temp.path(), "wardian");
    let central = real_codex(temp.path());
    let warm = home(temp.path(), "warm");
    thread_database(&warm.join("state_5.sqlite"), 6, 0);
    refresh(&wardian_home, &warm, &central).expect("publish");

    // Stand in for the provider winning the race between the emptiness check
    // and the copy: the write must refuse rather than replace.
    let racing = home(temp.path(), "racing");
    thread_database(&racing.join("state_5.sqlite"), 1, 0);
    let cache = cache_directory(&wardian_home);
    let snapshot = snapshot_path(&cache, "state_5.sqlite");
    assert!(!write_seed(&snapshot, &racing.join("state_5.sqlite")).expect("refuses"));
    assert_eq!(count(&racing.join("state_5.sqlite"), "threads"), 1);
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
    let central = real_codex(temp.path());
    let cold = home(temp.path(), "cold");
    assert!(!refresh(&wardian_home, &cold, &central).expect("nothing to publish"));
}

#[test]
fn a_recent_snapshot_of_the_same_generation_is_not_republished() {
    let temp = tempfile::tempdir().expect("temp");
    let wardian_home = home(temp.path(), "wardian");
    let central = real_codex(temp.path());
    let warm = home(temp.path(), "warm");
    thread_database(&warm.join("state_5.sqlite"), 2, 0);
    assert!(refresh(&wardian_home, &warm, &central).expect("first publication"));

    let connection =
        rusqlite::Connection::open(warm.join("state_5.sqlite")).expect("reopen database");
    connection
        .execute("DELETE FROM threads", [])
        .expect("empty the source");
    drop(connection);
    assert!(!refresh(&wardian_home, &warm, &central).expect("second call"));
    assert_eq!(
        read_meta(&cache_directory(&wardian_home))
            .expect("meta")
            .threads,
        2
    );
}

#[test]
fn a_stale_snapshot_is_replaced() {
    let temp = tempfile::tempdir().expect("temp");
    let wardian_home = home(temp.path(), "wardian");
    let central = real_codex(temp.path());
    let warm = home(temp.path(), "warm");
    thread_database(&warm.join("state_5.sqlite"), 2, 0);
    refresh(&wardian_home, &warm, &central).expect("first publication");

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
    assert!(refresh(&wardian_home, &warm, &central).expect("republication"));
    assert_eq!(read_meta(&cache).expect("meta").threads, 7);
}

#[test]
fn a_snapshot_captured_from_a_live_writer_is_complete() {
    let temp = tempfile::tempdir().expect("temp");
    let wardian_home = home(temp.path(), "wardian");
    let central = real_codex(temp.path());
    let warm = home(temp.path(), "warm");
    let database = warm.join("state_5.sqlite");
    thread_database(&database, 5, 0);

    // Hold the database open in write-ahead mode, as a resident daemon does.
    let live = rusqlite::Connection::open(&database).expect("open live writer");
    live.pragma_update(None, "journal_mode", "WAL")
        .expect("enable write-ahead log");
    live.execute(
        "INSERT INTO threads (id, rollout_path, cwd) VALUES ('live', 'C:\\a\\sessions\\x.jsonl', 'w')",
        [],
    )
    .expect("live insert");

    assert!(refresh(&wardian_home, &warm, &central).expect("publish while open"));
    let fresh = home(temp.path(), "fresh");
    seed(&wardian_home, &fresh).expect("seed");
    assert_eq!(count(&fresh.join("state_5.sqlite"), "threads"), 6);
}

#[test]
fn an_abandoned_staging_file_is_reclaimed() {
    let temp = tempfile::tempdir().expect("temp");
    let wardian_home = home(temp.path(), "wardian");
    let central = real_codex(temp.path());
    let warm = home(temp.path(), "warm");
    thread_database(&warm.join("state_5.sqlite"), 3, 0);

    // A publication interrupted mid-capture leaves a full-size file behind.
    let cache = private_cache(&wardian_home).expect("cache");
    let abandoned = cache.join(format!(
        "{SNAPSHOT_PREFIX}state_5.sqlite.9999{STAGING_SUFFIX}"
    ));
    std::fs::write(&abandoned, vec![0u8; 4096]).expect("write abandoned staging");

    refresh(&wardian_home, &warm, &central).expect("publish");
    assert!(!abandoned.exists(), "staging file was not reclaimed");
}

#[test]
fn a_cached_name_that_escapes_its_home_is_refused() {
    let temp = tempfile::tempdir().expect("temp");
    let wardian_home = home(temp.path(), "wardian");
    let central = real_codex(temp.path());
    let warm = home(temp.path(), "warm");
    thread_database(&warm.join("state_5.sqlite"), 1, 0);
    refresh(&wardian_home, &warm, &central).expect("publish");

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

#[test]
fn a_rollout_path_is_split_into_platform_neutral_components() {
    // The provider writes whichever separator its own platform uses, so both
    // spellings must yield the same components on either host.
    assert_eq!(
        sessions_tail(r"C:\agents\a\habitat\.codex\sessions\2026\09\r.jsonl"),
        Some(vec![
            "2026".to_owned(),
            "09".to_owned(),
            "r.jsonl".to_owned()
        ])
    );
    assert_eq!(
        sessions_tail("/home/u/.codex/sessions/2026/09/r.jsonl"),
        Some(vec![
            "2026".to_owned(),
            "09".to_owned(),
            "r.jsonl".to_owned()
        ])
    );
    assert_eq!(
        sessions_tail(r"\?\C:\Users\u\.codex\sessions\2026\r.jsonl"),
        Some(vec!["2026".to_owned(), "r.jsonl".to_owned()])
    );
    // An earlier `sessions` component is used when the rightmost one is part of
    // a longer name, so the row is rewritten rather than silently skipped.
    assert_eq!(
        sessions_tail(r"C:\a\sessions\archived-sessions-2025\r.jsonl"),
        Some(vec![
            "archived-sessions-2025".to_owned(),
            "r.jsonl".to_owned()
        ])
    );
    // No sessions component, nothing below it, or a traversal: left alone.
    assert!(sessions_tail(r"C:\somewhere\rollout.jsonl").is_none());
    assert!(sessions_tail(r"C:\a\sessions").is_none());
    assert!(sessions_tail(r"C:\a\my-sessions-archive\r.jsonl").is_none());
    assert!(sessions_tail(r"C:\a\sessions\..\..\escape.jsonl").is_none());
}

#[test]
fn a_rewritten_path_uses_this_platform_s_separator() {
    let temp = tempfile::tempdir().expect("temp");
    let wardian_home = home(temp.path(), "wardian");
    let central = real_codex(temp.path());
    let warm = home(temp.path(), "warm");
    thread_database(&warm.join("state_5.sqlite"), 2, 0);
    refresh(&wardian_home, &warm, &central).expect("publish");
    let fresh = home(temp.path(), "fresh");
    seed(&wardian_home, &fresh).expect("seed");

    // A string join with the wrong separator yields one filename rather than a
    // nested path, which reads as valid but names nothing on disk.
    for path in rollout_paths(&fresh.join("state_5.sqlite")) {
        let relative = PathBuf::from(&path)
            .strip_prefix(central.join("sessions"))
            .expect("row is under the central tree")
            .to_owned();
        assert_eq!(
            relative.components().count(),
            2,
            "expected year and filename components, got {relative:?}"
        );
        let filename = relative
            .file_name()
            .and_then(|name| name.to_str())
            .expect("rewritten row keeps a filename");
        assert!(
            filename.starts_with("rollout-") && filename.ends_with(".jsonl"),
            "filename survived the rewrite intact: {path}"
        );
    }
}

#[test]
fn a_row_that_cannot_be_rewritten_is_dropped_rather_than_published() {
    let temp = tempfile::tempdir().expect("temp");
    let wardian_home = home(temp.path(), "wardian");
    let central = real_codex(temp.path());
    let warm = home(temp.path(), "warm");
    let database = warm.join("state_5.sqlite");
    thread_database(&database, 2, 0);
    let connection = rusqlite::Connection::open(&database).expect("open");
    connection
        .execute(
            "INSERT INTO threads (id, rollout_path, cwd) VALUES ('odd', ?1, 'w')",
            [r"C:\agents\publisher\elsewhere\rollout.jsonl"],
        )
        .expect("insert unrewritable row");
    drop(connection);

    refresh(&wardian_home, &warm, &central).expect("publish");
    let fresh = home(temp.path(), "fresh");
    seed(&wardian_home, &fresh).expect("seed");

    let seeded = fresh.join("state_5.sqlite");
    assert_eq!(
        count(&seeded, "threads"),
        2,
        "the odd row was not published"
    );
    for path in rollout_paths(&seeded) {
        assert!(
            !path.contains("publisher"),
            "an agent home survived: {path}"
        );
    }
}

#[test]
fn engine_bookkeeping_tables_do_not_stop_publication() {
    let temp = tempfile::tempdir().expect("temp");
    let wardian_home = home(temp.path(), "wardian");
    let central = real_codex(temp.path());
    let warm = home(temp.path(), "warm");
    let database = warm.join("state_5.sqlite");
    thread_database(&database, 3, 0);
    // ANALYZE materialises sqlite_stat1, which holds rows and is not the
    // provider's data; refusing on it would disable the cache machine-wide.
    let connection = rusqlite::Connection::open(&database).expect("open");
    connection.execute("ANALYZE", []).expect("analyze");
    drop(connection);

    assert!(refresh(&wardian_home, &warm, &central).expect("publish despite sqlite_stat1"));
}

#[test]
fn a_superseded_snapshot_is_reclaimed() {
    let temp = tempfile::tempdir().expect("temp");
    let wardian_home = home(temp.path(), "wardian");
    let central = real_codex(temp.path());

    let old = home(temp.path(), "old");
    thread_database(&old.join("state_5.sqlite"), 2, 0);
    refresh(&wardian_home, &old, &central).expect("publish generation 5");
    let upgraded = home(temp.path(), "upgraded");
    thread_database(&upgraded.join("state_6.sqlite"), 4, 0);
    refresh(&wardian_home, &upgraded, &central).expect("publish generation 6");

    let cache = cache_directory(&wardian_home);
    let snapshots: Vec<String> = std::fs::read_dir(&cache)
        .expect("read cache")
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with(SNAPSHOT_PREFIX))
        .collect();
    assert_eq!(
        snapshots,
        vec!["snapshot-state_6.sqlite".to_owned()],
        "a superseded generation was left behind"
    );
}

#[test]
fn a_home_that_does_not_project_the_central_tree_is_detected() {
    let temp = tempfile::tempdir().expect("temp");
    let central = real_codex(temp.path());
    let projecting = home(temp.path(), "projecting");
    let isolated = home(temp.path(), "isolated");
    std::fs::create_dir_all(isolated.join("sessions")).expect("local fallback sessions");

    assert!(
        !projects_central_sessions(&isolated, &central),
        "a private local sessions directory must not pass"
    );
    // With no sessions entry at all there is nothing to contradict.
    assert!(projects_central_sessions(&projecting, &central));
}

#[test]
fn a_discarded_seed_leaves_the_home_without_a_database() {
    let temp = tempfile::tempdir().expect("temp");
    let wardian_home = home(temp.path(), "wardian");
    let central = real_codex(temp.path());
    let warm = home(temp.path(), "warm");
    thread_database(&warm.join("state_5.sqlite"), 3, 0);
    refresh(&wardian_home, &warm, &central).expect("publish");

    let fresh = home(temp.path(), "fresh");
    assert!(seed(&wardian_home, &fresh).expect("seed"));
    discard_seed(&wardian_home, &fresh);
    assert!(
        !fresh.join("state_5.sqlite").exists(),
        "the provider must rebuild rather than trust an unusable seed"
    );
}
