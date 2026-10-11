use super::*;

fn fixture() -> (tempfile::TempDir, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("opencode.db");
    let db = Connection::open(&path).unwrap();
    db.execute_batch("CREATE TABLE session (id TEXT, directory TEXT); CREATE TABLE message (id TEXT, session_id TEXT, data TEXT); CREATE TABLE part (id TEXT, message_id TEXT, session_id TEXT, data TEXT); INSERT INTO session VALUES ('ses_owned','/historical-origin');").unwrap();
    drop(db);
    (temp, path)
}

#[test]
fn materialized_empty_session_and_historical_directory_are_valid() {
    let (_temp, path) = fixture();
    let before = std::fs::read(&path).unwrap();
    inspect_database(&path, "ses_owned").unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert!(!path.with_extension("db-shm").exists());
}

#[test]
fn missing_session_never_becomes_same_id_fresh() {
    let (_temp, path) = fixture();
    assert!(inspect_database(&path, "ses_other")
        .unwrap_err()
        .contains("materialized"));
    assert!(
        inspect_database(&path.with_extension("missing"), "ses_owned")
            .unwrap_err()
            .contains("missing")
    );
}

#[test]
fn valid_history_is_preserved_and_invalid_json_refused() {
    let (_temp, path) = fixture();
    let db = Connection::open(&path).unwrap();
    db.execute_batch("INSERT INTO message VALUES ('m','ses_owned','{\"role\":\"user\"}'); INSERT INTO part VALUES ('p','m','ses_owned','{\"type\":\"text\",\"text\":\"original\"}');").unwrap();
    inspect_database(&path, "ses_owned").unwrap();
    db.execute("UPDATE part SET data='invalid'", []).unwrap();
    assert!(inspect_database(&path, "ses_owned")
        .unwrap_err()
        .contains("malformed JSON"));
}

#[test]
fn duplicate_identity_or_orphan_history_refused() {
    let (_temp, path) = fixture();
    let db = Connection::open(&path).unwrap();
    db.execute_batch("INSERT INTO part VALUES ('p','absent','ses_owned','{}');")
        .unwrap();
    assert!(inspect_database(&path, "ses_owned")
        .unwrap_err()
        .contains("orphan"));
    db.execute_batch("DELETE FROM part; INSERT INTO session VALUES ('ses_owned','/other');")
        .unwrap();
    assert!(inspect_database(&path, "ses_owned").is_err());
}

#[test]
fn corrupt_database_and_recovery_journal_refused_without_mutation() {
    let (_temp, path) = fixture();
    let journal = path.with_extension("db-journal");
    std::fs::write(&journal, b"pending").unwrap();
    assert!(inspect_database(&path, "ses_owned")
        .unwrap_err()
        .contains("recovery"));
    assert_eq!(std::fs::read(&journal).unwrap(), b"pending");
    let corrupt = path.with_extension("corrupt");
    std::fs::write(&corrupt, b"not sqlite").unwrap();
    assert!(inspect_database(&corrupt, "ses_owned").is_err());
}

#[test]
fn active_wal_snapshot_retains_committed_identity_without_checkpointing_source() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("opencode.db");
    let db = Connection::open(&path).unwrap();
    db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0; CREATE TABLE session (id TEXT, directory TEXT); CREATE TABLE message (id TEXT, session_id TEXT, data TEXT); CREATE TABLE part (id TEXT, message_id TEXT, session_id TEXT, data TEXT); INSERT INTO session VALUES ('ses_owned','/origin');").unwrap();
    let before = std::fs::read(&path).unwrap();
    let wal = path.with_extension("db-wal");
    let wal_before = std::fs::read(&wal).unwrap();
    inspect_database(&path, "ses_owned").unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(std::fs::read(&wal).unwrap(), wal_before);
}

#[test]
fn unsupported_options_and_identity_or_destination_refused() {
    let temp = tempfile::tempdir().unwrap();
    let mut config = AgentConfig {
        provider: "opencode".into(),
        folder: temp.path().to_string_lossy().into(),
        ..Default::default()
    };
    validate_options(&config, "ses_owned").unwrap();
    for options in [
        "--fork",
        "--session ses_other",
        "--attach http://other",
        "--dir /other",
        "'unclosed",
    ] {
        config.custom_args = Some(options.into());
        assert!(validate_options(&config, "ses_owned").is_err());
    }
    config.custom_args = None;
    assert!(validate_options(&config, "../session").is_err());
    config.folder = "relative".into();
    assert!(validate_options(&config, "ses_owned").is_err());
}

#[test]
fn binding_allows_append_and_unfinished_completion_but_rejects_confirmed_rewrite() {
    let (_temp, path) = fixture();
    let db = Connection::open(&path).unwrap();
    db.execute_batch("INSERT INTO message VALUES ('m','ses_owned','{\"role\":\"user\"}'); INSERT INTO part VALUES ('p','m','ses_owned','{\"type\":\"text\",\"text\":\"original\"}'); INSERT INTO message VALUES ('a','ses_owned','{\"role\":\"assistant\",\"time\":{}}');").unwrap();
    let original = inspect_database(&path, "ses_owned").unwrap();
    assert_eq!(original.confirmed_records.len(), 2);
    db.execute_batch("UPDATE message SET data='{\"role\":\"assistant\",\"time\":{\"completed\":1}}' WHERE id='a'; INSERT INTO part VALUES ('ap','a','ses_owned','{\"type\":\"text\",\"text\":\"answer\"}');").unwrap();
    let completed = inspect_database(&path, "ses_owned").unwrap();
    validate_binding(&completed, &original).unwrap();
    assert_eq!(completed.confirmed_records.len(), 4);
    db.execute(
        "UPDATE part SET data='{\"type\":\"text\",\"text\":\"replacement\"}' WHERE id='p'",
        [],
    )
    .unwrap();
    assert!(
        validate_binding(&inspect_database(&path, "ses_owned").unwrap(), &original)
            .unwrap_err()
            .contains("confirmed history")
    );
}

#[test]
fn binding_rejects_physical_replacement_with_identical_database_bytes() {
    let (_temp, path) = fixture();
    let original = inspect_database(&path, "ses_owned").unwrap();
    let bytes = std::fs::read(&path).unwrap();
    let replacement = path.with_extension("replacement");
    std::fs::write(&replacement, bytes).unwrap();
    let mut current = inspect_database(&replacement, "ses_owned").unwrap();
    current.source_path = original.source_path.clone();
    assert!(validate_binding(&current, &original)
        .unwrap_err()
        .contains("source identity"));
}

#[test]
fn foreign_session_part_linked_to_owned_message_is_refused() {
    let (_temp, path) = fixture();
    let db = Connection::open(&path).unwrap();
    db.execute_batch("INSERT INTO message VALUES ('m','ses_owned','{\"role\":\"user\"}'); INSERT INTO part VALUES ('p','m','ses_other','{}');").unwrap();
    assert!(inspect_database(&path, "ses_owned")
        .unwrap_err()
        .contains("foreign-session"));
}

#[test]
fn provider_database_override_matches_upstream_resolution_and_memory_refuses() {
    let root = tempfile::tempdir().unwrap();
    let absolute = root.path().join("absolute.db");
    assert_eq!(
        database_path(root.path(), Some(absolute.as_os_str().into())).unwrap(),
        absolute
    );
    assert_eq!(
        database_path(root.path(), Some("named.db".into())).unwrap(),
        root.path().join("opencode/named.db")
    );
    assert!(database_path(root.path(), Some(":memory:".into())).is_err());
}

#[test]
fn alternate_v2_history_is_not_silently_ignored() {
    let (_temp, path) = fixture();
    let db = Connection::open(&path).unwrap();
    db.execute_batch("CREATE TABLE session_message (id TEXT, session_id TEXT); INSERT INTO session_message VALUES ('v2','ses_owned');").unwrap();
    assert!(inspect_database(&path, "ses_owned")
        .unwrap_err()
        .contains("alternate session history"));
}

#[cfg(windows)]
#[test]
fn drive_relative_database_override_does_not_select_an_unrelated_drive_cwd() {
    let root = tempfile::tempdir().unwrap();
    assert!(database_path(root.path(), Some("C:other.db".into())).is_err());
}
