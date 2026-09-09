//! Owned temporary filesystem fixtures only; no provider or production root use.
use super::*;

struct Fixture {
    _temp: tempfile::TempDir,
    home: PathBuf,
    source: PathBuf,
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let mut builder = tempfile::Builder::new();
        builder.prefix("ch");
        #[cfg(target_os = "macos")]
        let temp = builder.tempdir_in("/tmp").unwrap();
        #[cfg(not(target_os = "macos"))]
        let temp = builder.tempdir().unwrap();
        let base = canonical(temp.path()).unwrap();
        let home = base.join("wardian-home-deliberately-long-for-local-socket-migration");
        let source = home.join("agents/agent/habitat/.codex");
        std::fs::create_dir_all(source.join("unknown/nested")).unwrap();
        std::fs::write(source.join("config.toml"), b"model='baseline'\n").unwrap();
        std::fs::write(source.join("unknown/nested/state.bin"), [0, 255, 1, 0]).unwrap();
        let root = base.join("c");
        Self {
            _temp: temp,
            home,
            source,
            root,
        }
    }

    fn begin(&self) -> Intent {
        let target = reserve(&self.root).unwrap();
        let intent = Intent {
            version: 1,
            token: uuid::Uuid::new_v4().to_string(),
            agent_id: "agent".into(),
            wardian_home: self.home.clone(),
            source: self.source.clone(),
            target,
            source_identity: storage::directory_identity(&self.source).unwrap(),
            snapshot: tree::snapshot(&self.source).unwrap(),
        };
        storage::publish_new(&intent.slot().join(RECORD), &intent).unwrap();
        storage::publish_new(&self.record(), &intent).unwrap();
        intent
    }

    fn record(&self) -> PathBuf {
        self.home.join("agents/agent").join(RECORD)
    }
    fn prepare(&self) -> PathBuf {
        prepare_with_roots(&self.home, "agent", || Ok(vec![self.root.clone()])).unwrap()
    }
}

#[test]
fn absent_plain_home_resolution_is_read_only_and_short_home_is_unchanged() {
    let fixture = Fixture::new();
    let absent = fixture.home.join("agents/new/habitat/.codex");
    std::fs::create_dir_all(absent.parent().unwrap()).unwrap();
    assert_eq!(resolve_managed_home(&fixture.home, "new").unwrap(), absent);
    assert!(!absent.exists());
    assert_eq!(
        owner_preparation_home(&fixture.home, "new").unwrap(),
        absent
    );
    assert!(absent.is_dir());
    // Socket bound uses OS bytes, the real suffix and its terminating NUL.
    let capacity = if cfg!(target_os = "macos") { 104 } else { 108 };
    assert!(socket_fits(Path::new(&"x".repeat(capacity - 44))));
    assert!(!socket_fits(Path::new(&"x".repeat(capacity - 43))));
    assert!(!socket_fits(Path::new(&"é".repeat(capacity / 2))));
}

#[test]
fn compact_prepare_preserves_unknown_state_and_reuses_mutated_ready_home() {
    let fixture = Fixture::new();
    let _guard = acquire_preparation(&fixture.home, "agent").unwrap();
    let before = tree::snapshot(&fixture.source).unwrap();
    let home = fixture.prepare();
    assert!(socket_fits(&home));
    assert_eq!(tree::snapshot(&home).unwrap(), before);
    assert_eq!(
        std::fs::read(home.join("unknown/nested/state.bin")).unwrap(),
        [0, 255, 1, 0]
    );
    std::fs::write(home.join("later-session.jsonl"), b"new durable state").unwrap();
    assert_eq!(fixture.prepare(), home);
    assert_eq!(resolve_managed_home(&fixture.home, "agent").unwrap(), home);
}

#[test]
fn pending_overlay_must_recover_at_original_home_before_reserving_a_slot() {
    let fixture = Fixture::new();
    std::fs::write(
        fixture.source.join(OVERLAY),
        b"pending original-home journal",
    )
    .unwrap();
    assert_eq!(
        owner_preparation_home(&fixture.home, "agent").unwrap(),
        fixture.source
    );
    assert!(prepare_with_roots(&fixture.home, "agent", || panic!("must not allocate")).is_err());
    assert!(!fixture.record().exists());
    assert!(!fixture.root.exists());
}

#[test]
fn rename_crash_windows_resume_only_exact_recorded_identity() {
    for phase in 0..4 {
        let fixture = Fixture::new();
        let intent = fixture.begin();
        if phase >= 1 {
            std::fs::rename(&intent.source, &intent.target).unwrap();
        }
        if phase >= 2 {
            storage::publish_new(
                &intent.slot().join(READY),
                &Ready {
                    version: 1,
                    token: intent.token.clone(),
                    target_identity: intent.source_identity,
                    copied: false,
                },
            )
            .unwrap();
        }
        if phase >= 3 {
            crate::utils::fs::create_directory_link(&intent.target, &intent.source).unwrap();
        }
        if phase < 3 {
            assert!(resolve_managed_home(&fixture.home, "agent").is_err());
        }
        assert_eq!(
            owner_preparation_home(&fixture.home, "agent").unwrap(),
            intent.target
        );
        assert_eq!(tree::snapshot(&intent.target).unwrap(), intent.snapshot);
        assert!(!intent.backup().exists());
    }
}

#[test]
fn copied_crash_windows_keep_verified_backup_and_do_not_follow_global_links() {
    for phase in 0..4 {
        let fixture = Fixture::new();
        let global = fixture.home.join("external-sessions");
        std::fs::create_dir(&global).unwrap();
        std::fs::write(global.join("sentinel"), b"before").unwrap();
        crate::utils::fs::create_directory_link(&global, &fixture.source.join("sessions")).unwrap();
        let intent = fixture.begin();
        migration::copy_ready(&intent).unwrap(); // Force the cross-volume branch with local fixtures.
        if phase >= 1 {
            std::fs::rename(intent.staging(), &intent.target).unwrap();
        }
        if phase >= 2 {
            std::fs::rename(&intent.source, intent.backup()).unwrap();
        }
        if phase >= 3 {
            crate::utils::fs::create_directory_link(&intent.target, &intent.source).unwrap();
        }
        std::fs::write(
            global.join("sentinel"),
            b"external change is not copied state",
        )
        .unwrap();
        assert_eq!(
            owner_preparation_home(&fixture.home, "agent").unwrap(),
            intent.target
        );
        assert_eq!(
            storage::directory_identity(&intent.backup()).unwrap(),
            intent.source_identity
        );
        assert_eq!(tree::snapshot(&intent.backup()).unwrap(), intent.snapshot);
        assert!(storage::is_link(&intent.target.join("sessions")).unwrap());
    }
}

#[test]
fn unverified_or_changed_copy_is_retained_without_moving_the_source() {
    for verified in [false, true] {
        let fixture = Fixture::new();
        let intent = fixture.begin();
        if verified {
            migration::copy_ready(&intent).unwrap();
        } else {
            std::fs::create_dir(intent.staging()).unwrap();
        }
        std::fs::write(
            intent.staging().join("config.toml"),
            b"partial or external edit",
        )
        .unwrap();
        assert!(owner_preparation_home(&fixture.home, "agent").is_err());
        assert_eq!(tree::snapshot(&intent.source).unwrap(), intent.snapshot);
        assert!(intent.staging().exists());
        assert!(!intent.target.exists());
    }
}

#[test]
fn foreign_links_records_and_hardlinked_records_are_rejected() {
    let fixture = Fixture::new();
    let foreign = fixture.home.join("foreign");
    std::fs::create_dir(&foreign).unwrap();
    crate::utils::fs::create_directory_link(&foreign, &fixture.home.join("agents/alien")).unwrap();
    assert!(acquire_preparation(&fixture.home, "alien").is_err());
    let intent = fixture.begin();
    let mut edited = intent.clone();
    edited.token = uuid::Uuid::new_v4().to_string();
    std::fs::write(
        intent.slot().join(RECORD),
        serde_json::to_vec(&edited).unwrap(),
    )
    .unwrap();
    assert!(resolve_managed_home(&fixture.home, "agent").is_err());
    assert!(owner_preparation_home(&fixture.home, "agent").is_err());
    std::fs::write(
        intent.slot().join(RECORD),
        serde_json::to_vec(&intent).unwrap(),
    )
    .unwrap();
    std::fs::hard_link(fixture.record(), fixture.home.join("record-alias")).unwrap();
    assert!(owner_preparation_home(&fixture.home, "agent").is_err());
    assert_eq!(tree::snapshot(&fixture.source).unwrap(), intent.snapshot);
}

#[test]
fn preparation_lock_is_nonblocking_stable_across_agent_removal_and_rejects_hardlinks() {
    let fixture = Fixture::new();
    let guard = acquire_preparation(&fixture.home, "agent").unwrap();
    assert!(acquire_preparation(&fixture.home, "agent").is_err());
    // Remove only this fixture's agent tree; the lock must remain authoritative.
    std::fs::remove_dir_all(fixture.home.join("agents/agent")).unwrap();
    assert!(acquire_preparation(&fixture.home, "agent").is_err());
    drop(guard);
    let guard = acquire_preparation(&fixture.home, "agent").unwrap();
    drop(guard);
    let file = std::fs::read_dir(fixture.home.join("locks"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    std::fs::hard_link(file, fixture.home.join("lock-alias")).unwrap();
    assert!(acquire_preparation(&fixture.home, "agent").is_err());
}

#[test]
fn cleanup_deletes_only_ready_owned_trees_and_never_global_link_targets() {
    let fixture = Fixture::new();
    let _guard = acquire_preparation(&fixture.home, "agent").unwrap();
    let global = fixture.home.join("global");
    std::fs::create_dir(&global).unwrap();
    std::fs::write(global.join("sentinel"), b"keep").unwrap();
    crate::utils::fs::create_directory_link(&global, &fixture.source.join("sessions")).unwrap();
    let intent = fixture.begin();
    assert!(cleanup_managed_home(&fixture.home, "agent").is_err());
    migration::copy_ready(&intent).unwrap();
    migration::resume(&intent).unwrap();
    cleanup_managed_home(&fixture.home, "agent").unwrap();
    cleanup_managed_home(&fixture.home, "agent").unwrap();
    assert!(!intent.slot().exists());
    assert!(!intent.backup().exists());
    assert!(fixture.record().exists()); // Parent deletes agent only after success.
    assert_eq!(std::fs::read(global.join("sentinel")).unwrap(), b"keep");
}

#[test]
fn roots_injection_is_thread_local_and_unfittable_roots_never_move_source() {
    let fixture = Fixture::new();
    let previous = TEST_ROOTS.with(|roots| roots.replace(Some(vec![fixture.root.clone()])));
    let prepared = prepare_compact_home(&fixture.home, "agent");
    TEST_ROOTS.with(|roots| roots.replace(previous));
    assert!(prepared.is_ok());
    let fixture = Fixture::new();
    let original = tree::snapshot(&fixture.source).unwrap();
    assert!(
        prepare_with_roots(&fixture.home, "agent", || Ok(vec![fixture
            .home
            .join("too-long")]))
        .is_err()
    );
    assert_eq!(tree::snapshot(&fixture.source).unwrap(), original);
    assert!(!fixture.record().exists());
}

fn relative_directory_link(destination: &Path, path: &Path) {
    #[cfg(unix)]
    std::os::unix::fs::symlink(destination, path).unwrap();
    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(destination, path).unwrap();
}

#[test]
#[cfg_attr(
    windows,
    ignore = "Windows relative symlink creation requires privilege"
)]
fn external_relative_link_is_rejected_before_intent_or_move() {
    let fixture = Fixture::new();
    relative_directory_link(Path::new("../outside"), &fixture.source.join("external"));
    let before = std::fs::read(fixture.source.join("config.toml")).unwrap();
    let result = prepare_with_roots(&fixture.home, "agent", || {
        panic!("must reject before allocation")
    });
    assert!(result
        .unwrap_err()
        .contains("convert it to an absolute link"));
    assert!(!fixture.record().exists());
    assert!(!fixture.root.exists());
    assert_eq!(
        std::fs::read(fixture.source.join("config.toml")).unwrap(),
        before
    );
    assert_eq!(
        std::fs::read_link(fixture.source.join("external")).unwrap(),
        Path::new("../outside")
    );
}

#[test]
#[cfg_attr(
    windows,
    ignore = "Windows relative symlink creation requires privilege"
)]
fn internal_relative_link_retains_its_target_after_relocation() {
    let fixture = Fixture::new();
    relative_directory_link(
        Path::new("unknown/nested"),
        &fixture.source.join("internal"),
    );
    let prepared = fixture.prepare();
    assert_eq!(
        std::fs::read_link(prepared.join("internal")).unwrap(),
        Path::new("unknown/nested")
    );
    assert_eq!(
        std::fs::read(prepared.join("internal/state.bin")).unwrap(),
        [0, 255, 1, 0]
    );
}
