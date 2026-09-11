    use super::*;
    use std::fs;
    use std::path::Path;
    use std::time::Duration;
    use tokio::time::{sleep, timeout};
    use wardian_core::models::AgentConfig;

    fn agent_config(agent_id: &str, root: &Path) -> AgentConfig {
        AgentConfig {
            session_id: agent_id.to_string(),
            folder: root.to_string_lossy().into_owned(),
            ..AgentConfig::default()
        }
    }

    fn test_runtime() -> FileResourceRuntime {
        FileResourceRuntime::with_timing(Duration::from_millis(150), Duration::from_secs(60))
    }

    #[tokio::test]
    async fn file_recovery_checkpoint_create_update_enforces_cas_revision() {
        let temp = tempfile::tempdir().expect("temp root");
        let workspace = temp.path().join("workspace");
        fs::create_dir(&workspace).expect("workspace");
        let path = workspace.join("draft.txt");
        fs::write(&path, "base text").expect("fixture");
        let config = agent_config("agent-a", &workspace);
        let runtime = FileResourceRuntime::with_recovery_root(
            Duration::from_millis(150),
            Duration::from_secs(60),
            temp.path().join("recovery"),
        );
        let opened = runtime
            .open_agent_file("agent-a", &config, &path, None)
            .await
            .expect("open");

        let created = runtime
            .checkpoint_recovery(
                None,
                None,
                &opened.resource_id,
                &opened.subscription_id,
                &opened.descriptor.content_hash,
                "base text",
                &opened.resource_id,
                "main",
                "first edit",
            )
            .await
            .expect("create recovery");
        assert_eq!(created.recovery_revision, 1);

        let wrong_scope = runtime
            .checkpoint_recovery(
                Some(&created.recovery_id),
                Some(created.recovery_revision),
                &opened.resource_id,
                &opened.subscription_id,
                &opened.descriptor.content_hash,
                "base text",
                &opened.resource_id,
                "other",
                "cross-scope edit",
            )
            .await
            .expect_err("another webview scope must not update recovery");
        assert_eq!(wrong_scope.code(), "unauthorized_recovery");

        let updated = runtime
            .checkpoint_recovery(
                Some(&created.recovery_id),
                Some(created.recovery_revision),
                &opened.resource_id,
                &opened.subscription_id,
                &opened.descriptor.content_hash,
                "base text",
                &opened.resource_id,
                "main",
                "second edit",
            )
            .await
            .expect("update recovery");
        assert_eq!(updated.recovery_revision, 2);

        let conflict = runtime
            .checkpoint_recovery(
                Some(&created.recovery_id),
                Some(created.recovery_revision),
                &opened.resource_id,
                &opened.subscription_id,
                &opened.descriptor.content_hash,
                "base text",
                &opened.resource_id,
                "main",
                "stale edit",
            )
            .await
            .expect_err("stale recovery CAS must fail");
        assert_eq!(conflict.code(), "recovery_conflict");
    }

    #[tokio::test]
    async fn file_recovery_cas_update_can_advance_base_after_guarded_save() {
        let temp = tempfile::tempdir().expect("temp root");
        let recovery_root = temp.path().join("recovery");
        let workspace = temp.path().join("workspace");
        fs::create_dir(&workspace).expect("workspace");
        let path = workspace.join("draft.txt");
        fs::write(&path, "base").expect("fixture");
        let config = agent_config("agent-a", &workspace);
        let runtime = FileResourceRuntime::with_recovery_root(
            Duration::from_millis(150),
            Duration::from_secs(60),
            recovery_root.clone(),
        );
        let opened = runtime
            .open_agent_file("agent-a", &config, &path, None)
            .await
            .expect("open");
        let first = runtime
            .checkpoint_recovery(
                None,
                None,
                &opened.resource_id,
                &opened.subscription_id,
                &opened.descriptor.content_hash,
                "base",
                &opened.resource_id,
                "main",
                "saved base",
            )
            .await
            .expect("first recovery");

        let (saved_revision, saved_hash) = match runtime
            .save_text(
                &opened.resource_id,
                &opened.subscription_id,
                opened.revision,
                &opened.descriptor.content_hash,
                "saved base",
            )
            .await
            .expect("guarded save")
        {
            FileResourceSaveResultV1::Saved {
                revision,
                content_hash,
            } => (revision, content_hash),
            other => panic!("expected saved result, got {other:?}"),
        };
        assert!(saved_revision > opened.revision);
        let updated = runtime
            .checkpoint_recovery(
                Some(&first.recovery_id),
                Some(first.recovery_revision),
                &opened.resource_id,
                &opened.subscription_id,
                &saved_hash,
                "saved base",
                &opened.resource_id,
                "main",
                "saved base\nnext edit",
            )
            .await
            .expect("next edit advances the existing recovery base");
        assert_eq!(updated.recovery_revision, first.recovery_revision + 1);
        assert_eq!(updated.base_content_hash, saved_hash);
        assert_ne!(updated.base_opaque_revision, first.base_opaque_revision);

        drop(runtime);
        let restarted = FileResourceRuntime::with_recovery_root(
            Duration::from_millis(150),
            Duration::from_secs(60),
            recovery_root,
        );
        let restored = restarted
            .get_recovery(&updated.recovery_id, &opened.resource_id, "main")
            .await
            .expect("restart reads one complete advanced generation");
        assert_eq!(restored.base_content_hash, saved_hash);
        assert_eq!(restored.base, "saved base");
        assert_eq!(restored.buffer, "saved base\nnext edit");
        assert_eq!(fs::read_to_string(path).expect("disk bytes"), "saved base");
    }

    #[tokio::test]
    async fn file_recovery_checkpoint_rejects_unverified_or_oversized_submitted_bases() {
        let temp = tempfile::tempdir().expect("temp root");
        let workspace = temp.path().join("workspace");
        fs::create_dir(&workspace).expect("workspace");
        let path = workspace.join("draft.txt");
        fs::write(&path, "base").expect("fixture");
        let config = agent_config("agent-a", &workspace);
        let runtime = FileResourceRuntime::with_recovery_root(
            Duration::from_millis(150),
            Duration::from_secs(60),
            temp.path().join("recovery"),
        );
        let opened = runtime
            .open_agent_file("agent-a", &config, &path, None)
            .await
            .expect("open");

        let mismatch = runtime
            .checkpoint_recovery(
                None,
                None,
                &opened.resource_id,
                &opened.subscription_id,
                &opened.descriptor.content_hash,
                "forged base",
                &opened.resource_id,
                "main",
                "dirty buffer",
            )
            .await
            .expect_err("mismatched submitted base and hash must fail closed");
        assert_eq!(mismatch.code(), "invalid_request");

        let other_path = workspace.join("other.txt");
        fs::write(&other_path, "other base").expect("other fixture");
        let other = runtime
            .open_agent_file("agent-a", &config, &other_path, None)
            .await
            .expect("open other resource");
        let wrong_subscription = runtime
            .checkpoint_recovery(
                None,
                None,
                &opened.resource_id,
                &other.subscription_id,
                &opened.descriptor.content_hash,
                "base",
                &opened.resource_id,
                "main",
                "dirty buffer",
            )
            .await
            .expect_err("another resource subscription must not checkpoint this resource");
        assert_eq!(wrong_subscription.code(), "unauthorized_resource");
        let wrong_resource_key = runtime
            .checkpoint_recovery(
                None,
                None,
                &opened.resource_id,
                &opened.subscription_id,
                &opened.descriptor.content_hash,
                "base",
                &other.resource_id,
                "main",
                "dirty buffer",
            )
            .await
            .expect_err("another resource key must not receive this recovery");
        assert_eq!(wrong_resource_key.code(), "unauthorized_resource");

        let oversized = "x".repeat(
            usize::try_from(FileResourceLimits::default().monaco_max_size_bytes)
                .expect("limit fits usize")
                + 1,
        );
        let oversized_hash = format!("sha256:{:x}", Sha256::digest(oversized.as_bytes()));
        let too_large = runtime
            .checkpoint_recovery(
                None,
                None,
                &opened.resource_id,
                &opened.subscription_id,
                &oversized_hash,
                &oversized,
                &opened.resource_id,
                "main",
                "dirty buffer",
            )
            .await
            .expect_err("oversized submitted base must fail closed");
        assert_eq!(too_large.code(), "file_too_large");
        assert!(runtime
            .list_recoveries(&opened.resource_id, "main")
            .await
            .expect("failed requests leave no recovery")
            .is_empty());
    }

    #[tokio::test]
    async fn file_recovery_first_checkpoint_survives_an_advanced_disk_head() {
        let temp = tempfile::tempdir().expect("temp root");
        let recovery_root = temp.path().join("recovery");
        let workspace = temp.path().join("workspace");
        fs::create_dir(&workspace).expect("workspace");
        let path = workspace.join("draft.txt");
        let original_base = "one\ntwo\nthree\n";
        let dirty_buffer = "ONE\ntwo\nthree\n";
        fs::write(&path, original_base).expect("fixture");
        let config = agent_config("agent-a", &workspace);
        let runtime = FileResourceRuntime::with_recovery_root(
            Duration::from_millis(150),
            Duration::from_secs(60),
            recovery_root.clone(),
        );
        let opened = runtime
            .open_agent_file("agent-a", &config, &path, None)
            .await
            .expect("open");

        fs::write(&path, "one\ntwo\nTHREE\n").expect("external disk update");
        let checkpoint = runtime
            .checkpoint_recovery(
                None,
                None,
                &opened.resource_id,
                &opened.subscription_id,
                &opened.descriptor.content_hash,
                original_base,
                &opened.resource_id,
                "main",
                dirty_buffer,
            )
            .await
            .expect("hash-verified editor base must remain checkpointable");

        drop(runtime);
        let restarted = FileResourceRuntime::with_recovery_root(
            Duration::from_millis(150),
            Duration::from_secs(60),
            recovery_root,
        );
        let discovered = restarted
            .list_recoveries(&opened.resource_id, "main")
            .await
            .expect("list after restart");
        assert_eq!(discovered.len(), 1);
        let restored = restarted
            .get_recovery(&checkpoint.recovery_id, &opened.resource_id, "main")
            .await
            .expect("get after restart");
        assert_eq!(restored.base, original_base);
        assert_eq!(restored.buffer, dirty_buffer);

        let reopened = restarted
            .open_agent_file("agent-a", &config, &path, None)
            .await
            .expect("restore live authorization");
        let merged = restarted
            .merge_recovery(
                &checkpoint.recovery_id,
                checkpoint.recovery_revision,
                &opened.resource_id,
                "main",
                &reopened.resource_id,
                &reopened.subscription_id,
            )
            .await
            .expect("merge against advanced disk head");
        match merged {
            FileRecoveryMergeResultV1::Clean {
                disk_changed,
                merged_text,
                ..
            } => {
                assert!(disk_changed);
                assert_eq!(merged_text, "ONE\ntwo\nTHREE\n");
            }
            other => panic!("expected clean stale merge, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn file_recovery_restart_read_is_scoped_and_discard_is_cas_guarded() {
        let temp = tempfile::tempdir().expect("temp root");
        let recovery_root = temp.path().join("recovery");
        let workspace = temp.path().join("workspace");
        fs::create_dir(&workspace).expect("workspace");
        let path = workspace.join("draft.txt");
        fs::write(&path, "stored base").expect("fixture");
        let config = agent_config("agent-a", &workspace);
        let runtime = FileResourceRuntime::with_recovery_root(
            Duration::from_millis(150),
            Duration::from_secs(60),
            recovery_root.clone(),
        );
        let opened = runtime
            .open_agent_file("agent-a", &config, &path, None)
            .await
            .expect("open");
        let checkpoint = runtime
            .checkpoint_recovery(
                None,
                None,
                &opened.resource_id,
                &opened.subscription_id,
                &opened.descriptor.content_hash,
                "stored base",
                &opened.resource_id,
                "main",
                "stored buffer",
            )
            .await
            .expect("checkpoint");
        drop(runtime);
        fs::write(&path, "current disk secret").expect("external write");

        let restarted = FileResourceRuntime::with_recovery_root(
            Duration::from_millis(150),
            Duration::from_secs(60),
            recovery_root,
        );
        let restored = restarted
            .get_recovery(&checkpoint.recovery_id, &opened.resource_id, "main")
            .await
            .expect("read-only restart restore");
        assert_eq!(restored.base, "stored base");
        assert_eq!(restored.buffer, "stored buffer");
        assert!(!restored.base.contains("current disk secret"));
        assert!(!restored.buffer.contains("current disk secret"));
        let current_read = restarted
            .read_text(
                &opened.resource_id,
                &opened.subscription_id,
                opened.revision,
                Some(&config),
            )
            .await
            .expect_err("recovery-only runtime must not revive a file subscription");
        assert_eq!(current_read.code(), "resource_not_found");

        let wrong_scope = restarted
            .get_recovery(&checkpoint.recovery_id, &opened.resource_id, "other")
            .await
            .expect_err("another webview must not read recovery");
        assert_eq!(wrong_scope.code(), "unauthorized_recovery");
        let wrong_resource = restarted
            .get_recovery(&checkpoint.recovery_id, "file:/another.txt", "main")
            .await
            .expect_err("another resource must not read recovery");
        assert_eq!(wrong_resource.code(), "unauthorized_recovery");

        let stale_discard = restarted
            .discard_recovery(
                &checkpoint.recovery_id,
                checkpoint.recovery_revision + 1,
                &opened.resource_id,
                "main",
            )
            .await
            .expect_err("discard must enforce recovery CAS");
        assert_eq!(stale_discard.code(), "recovery_conflict");
        restarted
            .discard_recovery(
                &checkpoint.recovery_id,
                checkpoint.recovery_revision,
                &opened.resource_id,
                "main",
            )
            .await
            .expect("discard");
        let discarded = restarted
            .get_recovery(&checkpoint.recovery_id, &opened.resource_id, "main")
            .await
            .expect_err("discarded recovery must stay gone");
        assert_eq!(discarded.code(), "recovery_not_found");
        assert_eq!(
            fs::read_to_string(path).expect("disk bytes"),
            "current disk secret"
        );
    }

    #[tokio::test]
    async fn file_recovery_restart_discovers_and_recheckpoints_without_private_base_revision() {
        let temp = tempfile::tempdir().expect("temp root");
        let recovery_root = temp.path().join("recovery");
        let workspace = temp.path().join("workspace");
        fs::create_dir(&workspace).expect("workspace");
        let path = workspace.join("draft.txt");
        fs::write(&path, "revision one").expect("fixture");
        let config = agent_config("agent-a", &workspace);
        let runtime = FileResourceRuntime::with_recovery_root(
            Duration::from_millis(150),
            Duration::from_secs(60),
            recovery_root.clone(),
        );
        let opened = runtime
            .open_agent_file("agent-a", &config, &path, None)
            .await
            .expect("open");
        let saved = runtime
            .save_text(
                &opened.resource_id,
                &opened.subscription_id,
                opened.revision,
                &opened.descriptor.content_hash,
                "revision two",
            )
            .await
            .expect("advance logical revision");
        let (base_revision, base_hash) = match saved {
            FileResourceSaveResultV1::Saved {
                revision,
                content_hash,
            } => (revision, content_hash),
            other => panic!("expected saved revision, got {other:?}"),
        };
        assert!(base_revision > 1);
        runtime
            .checkpoint_recovery(
                None,
                None,
                &opened.resource_id,
                &opened.subscription_id,
                &base_hash,
                "revision two",
                &opened.resource_id,
                "main",
                "first recovered edit",
            )
            .await
            .expect("checkpoint after later logical revision");
        let resource_key = opened.resource_id.clone();
        drop(runtime);

        let restarted = FileResourceRuntime::with_recovery_root(
            Duration::from_millis(150),
            Duration::from_secs(60),
            recovery_root,
        );
        let discovered = restarted
            .list_recoveries(&resource_key, "main")
            .await
            .expect("discover after process restart");
        assert_eq!(discovered.len(), 1);
        assert!(restarted
            .list_recoveries(&resource_key, "other")
            .await
            .expect("wrong scope discovery")
            .is_empty());
        assert!(restarted
            .list_recoveries("file:/another.txt", "main")
            .await
            .expect("wrong resource discovery")
            .is_empty());
        let restored = restarted
            .get_recovery(&discovered[0].recovery_id, &resource_key, "main")
            .await
            .expect("restore discovered recovery");
        assert_eq!(restored.buffer, "first recovered edit");

        let reopened = restarted
            .open_agent_file("agent-a", &config, &path, None)
            .await
            .expect("new live authorization");
        assert_eq!(reopened.revision, 1, "runtime revision is process-local");
        let updated = restarted
            .checkpoint_recovery(
                Some(&discovered[0].recovery_id),
                Some(discovered[0].recovery_revision),
                &reopened.resource_id,
                &reopened.subscription_id,
                &base_hash,
                "revision two",
                &resource_key,
                "main",
                "second recovered edit",
            )
            .await
            .expect("checkpoint recovered buffer with new runtime revision");
        assert_eq!(
            updated.recovery_revision,
            discovered[0].recovery_revision + 1
        );
        assert_eq!(
            restarted
                .get_recovery(&updated.recovery_id, &resource_key, "main")
                .await
                .expect("updated recovery")
                .buffer,
            "second recovered edit"
        );
    }

    #[tokio::test]
    async fn file_recovery_manifest_last_failure_never_mixes_blob_generations() {
        let temp = tempfile::tempdir().expect("temp root");
        let recovery_root = temp.path().join("recovery");
        let workspace = temp.path().join("workspace");
        fs::create_dir(&workspace).expect("workspace");
        let path = workspace.join("draft.txt");
        fs::write(&path, "durable base").expect("fixture");
        let config = agent_config("agent-a", &workspace);
        let runtime = FileResourceRuntime::with_recovery_root(
            Duration::from_millis(150),
            Duration::from_secs(60),
            recovery_root.clone(),
        );
        let opened = runtime
            .open_agent_file("agent-a", &config, &path, None)
            .await
            .expect("open");
        let first = runtime
            .checkpoint_recovery(
                None,
                None,
                &opened.resource_id,
                &opened.subscription_id,
                &opened.descriptor.content_hash,
                "durable base",
                &opened.resource_id,
                "main",
                "first generation",
            )
            .await
            .expect("first checkpoint");
        let first_manifest =
            load_recovery_manifest(&recovery_root, &first.recovery_id).expect("first manifest");
        assert!(first_manifest.base_blob.starts_with("sha256-"));
        assert!(first_manifest.buffer_blob.starts_with("sha256-"));

        let (rebased_revision, rebased_hash) = match runtime
            .save_text(
                &opened.resource_id,
                &opened.subscription_id,
                opened.revision,
                &opened.descriptor.content_hash,
                "rebased base",
            )
            .await
            .expect("advance editor base")
        {
            FileResourceSaveResultV1::Saved {
                revision,
                content_hash,
            } => (revision, content_hash),
            other => panic!("expected saved result, got {other:?}"),
        };
        assert!(rebased_revision > opened.revision);
        runtime.fail_next_recovery_before_manifest();
        let interrupted = runtime
            .checkpoint_recovery(
                Some(&first.recovery_id),
                Some(first.recovery_revision),
                &opened.resource_id,
                &opened.subscription_id,
                &rebased_hash,
                "rebased base",
                &opened.resource_id,
                "main",
                "uncommitted generation",
            )
            .await
            .expect_err("fault before manifest must fail checkpoint");
        assert_eq!(interrupted.code(), "recovery_unavailable");
        drop(runtime);

        let restarted = FileResourceRuntime::with_recovery_root(
            Duration::from_millis(150),
            Duration::from_secs(60),
            recovery_root,
        );
        let restored = restarted
            .get_recovery(&first.recovery_id, &opened.resource_id, "main")
            .await
            .expect("restore committed generation");
        assert_eq!(restored.recovery_revision, first.recovery_revision);
        assert_eq!(restored.base_content_hash, first.base_content_hash);
        assert_eq!(restored.base_opaque_revision, first.base_opaque_revision);
        assert_eq!(restored.base, "durable base");
        assert_eq!(restored.buffer, "first generation");
        assert_ne!(restored.buffer, "uncommitted generation");
        let manifest = load_recovery_manifest(
            &restarted.recovery_root().expect("recovery root"),
            &first.recovery_id,
        )
        .expect("committed manifest");
        let blob_count = fs::read_dir(
            restarted
                .recovery_root()
                .expect("recovery root")
                .join(&first.recovery_id)
                .join("blobs"),
        )
        .expect("blob directory")
        .count();
        assert!(
            blob_count > 2,
            "fresh unreachable blob is retained conservatively"
        );
        assert!(is_recovery_blob_name(&manifest.base_blob));
    }

    #[tokio::test]
    async fn file_recovery_store_sweeps_crash_debris_and_enforces_admission_budgets() {
        let temp = tempfile::tempdir().expect("temp root");
        let recovery_root = temp.path().join("recovery");
        let workspace = temp.path().join("workspace");
        fs::create_dir(&workspace).expect("workspace");
        let path = workspace.join("draft.txt");
        fs::write(&path, "base").expect("fixture");
        let config = agent_config("agent-a", &workspace);
        let runtime = FileResourceRuntime::with_recovery_root(
            Duration::from_millis(150),
            Duration::from_secs(60),
            recovery_root.clone(),
        );
        runtime.configure_recovery_store_for_test(2, 18, Duration::ZERO);
        let opened = runtime
            .open_agent_file("agent-a", &config, &path, None)
            .await
            .expect("open");

        runtime.fail_next_recovery_before_manifest();
        let interrupted = runtime
            .checkpoint_recovery(
                None,
                None,
                &opened.resource_id,
                &opened.subscription_id,
                &opened.descriptor.content_hash,
                "base",
                &opened.resource_id,
                "main",
                "lost",
            )
            .await
            .expect_err("initial manifest-last fault must fail");
        assert_eq!(interrupted.code(), "recovery_unavailable");
        assert_eq!(
            fs::read_dir(&recovery_root).expect("recovery root").count(),
            1
        );
        assert!(runtime
            .list_recoveries(&opened.resource_id, "main")
            .await
            .expect("sweep manifestless record")
            .is_empty());
        assert_eq!(
            fs::read_dir(&recovery_root).expect("recovery root").count(),
            0
        );

        runtime.configure_recovery_store_for_test(2, 18, RECOVERY_ORPHAN_GRACE_PERIOD);
        let first = runtime
            .checkpoint_recovery(
                None,
                None,
                &opened.resource_id,
                &opened.subscription_id,
                &opened.descriptor.content_hash,
                "base",
                &opened.resource_id,
                "main",
                "one",
            )
            .await
            .expect("first bounded record");
        let second = runtime
            .checkpoint_recovery(
                None,
                None,
                &opened.resource_id,
                &opened.subscription_id,
                &opened.descriptor.content_hash,
                "base",
                &opened.resource_id,
                "main",
                "two",
            )
            .await
            .expect("second bounded record");
        let record_limit = runtime
            .checkpoint_recovery(
                None,
                None,
                &opened.resource_id,
                &opened.subscription_id,
                &opened.descriptor.content_hash,
                "base",
                &opened.resource_id,
                "main",
                "new",
            )
            .await
            .expect_err("third recovery id must exceed record budget");
        assert_eq!(record_limit.code(), "recovery_capacity_exceeded");

        let byte_limit = runtime
            .checkpoint_recovery(
                Some(&first.recovery_id),
                Some(first.recovery_revision),
                &opened.resource_id,
                &opened.subscription_id,
                &opened.descriptor.content_hash,
                "base",
                &opened.resource_id,
                "main",
                "three",
            )
            .await
            .expect_err("fresh immutable generations must count toward byte budget");
        assert_eq!(byte_limit.code(), "recovery_capacity_exceeded");

        let second_manifest =
            load_recovery_manifest(&recovery_root, &second.recovery_id).expect("second manifest");
        let second_record = recovery_root.join(&second.recovery_id);
        let orphan_blob = write_recovery_blob(&second_record, "orphan").expect("orphan fixture");
        runtime.configure_recovery_store_for_test(2, 18, Duration::ZERO);
        let discovered = runtime
            .list_recoveries(&opened.resource_id, "main")
            .await
            .expect("store-wide sweep");
        assert_eq!(discovered.len(), 2);
        assert!(!second_record.join("blobs").join(orphan_blob).exists());
        assert!(second_record
            .join("blobs")
            .join(second_manifest.buffer_blob)
            .is_file());
        assert_eq!(
            runtime
                .get_recovery(&first.recovery_id, &opened.resource_id, "main")
                .await
                .expect("live recovery retained")
                .buffer,
            "one"
        );
    }

    #[test]
    fn file_recovery_merge_rejects_an_oversized_final_model() {
        let limits = FileResourceLimits {
            monaco_max_size_bytes: 8,
            ..FileResourceLimits::default()
        };
        let result = finalize_recovery_merge(
            Err("conflict!".to_string()),
            2,
            4,
            "sha256:current".to_string(),
            true,
            &limits,
        )
        .expect_err("final conflict-marker model must be bounded");
        assert_eq!(result.code(), "file_too_large");
    }

    #[tokio::test]
    async fn file_recovery_merge_reports_clean_and_conflicted_stale_outcomes() {
        let temp = tempfile::tempdir().expect("temp root");
        let recovery_root = temp.path().join("recovery");
        let workspace = temp.path().join("workspace");
        fs::create_dir(&workspace).expect("workspace");
        let path = workspace.join("draft.txt");
        fs::write(&path, "one\ntwo\nthree\n").expect("fixture");
        let config = agent_config("agent-a", &workspace);
        let runtime = FileResourceRuntime::with_recovery_root(
            Duration::from_millis(150),
            Duration::from_secs(60),
            recovery_root,
        );
        let opened = runtime
            .open_agent_file("agent-a", &config, &path, None)
            .await
            .expect("open");
        let clean_checkpoint = runtime
            .checkpoint_recovery(
                None,
                None,
                &opened.resource_id,
                &opened.subscription_id,
                &opened.descriptor.content_hash,
                "one\ntwo\nthree\n",
                &opened.resource_id,
                "main",
                "ONE\ntwo\nthree\n",
            )
            .await
            .expect("clean checkpoint");
        fs::write(&path, "one\ntwo\nTHREE\n").expect("external clean edit");

        let clean = runtime
            .merge_recovery(
                &clean_checkpoint.recovery_id,
                clean_checkpoint.recovery_revision,
                &opened.resource_id,
                "main",
                &opened.resource_id,
                &opened.subscription_id,
            )
            .await
            .expect("clean merge");
        match clean {
            FileRecoveryMergeResultV1::Clean {
                disk_changed,
                merged_text,
                ..
            } => {
                assert!(disk_changed);
                assert_eq!(merged_text, "ONE\ntwo\nTHREE\n");
            }
            other => panic!("expected clean merge, got {other:?}"),
        }
        assert_eq!(
            fs::read_to_string(&path).expect("disk after clean merge"),
            "one\ntwo\nTHREE\n"
        );

        let conflict_path = workspace.join("conflict.txt");
        fs::write(&conflict_path, "shared line\n").expect("conflict fixture");
        let conflict_opened = runtime
            .open_agent_file("agent-a", &config, &conflict_path, None)
            .await
            .expect("open conflict");
        let conflict_checkpoint = runtime
            .checkpoint_recovery(
                None,
                None,
                &conflict_opened.resource_id,
                &conflict_opened.subscription_id,
                &conflict_opened.descriptor.content_hash,
                "shared line\n",
                &conflict_opened.resource_id,
                "main",
                "buffer line\n",
            )
            .await
            .expect("conflict checkpoint");
        fs::write(&conflict_path, "disk line\n").expect("external conflict edit");
        let conflicted = runtime
            .merge_recovery(
                &conflict_checkpoint.recovery_id,
                conflict_checkpoint.recovery_revision,
                &conflict_opened.resource_id,
                "main",
                &conflict_opened.resource_id,
                &conflict_opened.subscription_id,
            )
            .await
            .expect("conflicted merge outcome");
        match conflicted {
            FileRecoveryMergeResultV1::Conflicted {
                disk_changed,
                merged_text,
                ..
            } => {
                assert!(disk_changed);
                assert!(merged_text.contains("<<<<<<<"));
                assert!(merged_text.contains("buffer line"));
                assert!(merged_text.contains("======="));
                assert!(merged_text.contains("disk line"));
                assert!(merged_text.contains(">>>>>>>"));
            }
            other => panic!("expected conflicted merge, got {other:?}"),
        }
        assert_eq!(
            fs::read_to_string(conflict_path).expect("disk after conflict merge"),
            "disk line\n"
        );
    }

    #[tokio::test]
    async fn file_recovery_merge_requires_new_live_authorization_after_restart() {
        let temp = tempfile::tempdir().expect("temp root");
        let workspace = temp.path().join("workspace");
        fs::create_dir(&workspace).expect("workspace");
        let path = workspace.join("draft.txt");
        fs::write(&path, "base").expect("fixture");
        let config = agent_config("agent-a", &workspace);
        let runtime = FileResourceRuntime::with_recovery_root(
            Duration::from_millis(150),
            Duration::from_secs(60),
            temp.path().join("recovery"),
        );
        let opened = runtime
            .open_agent_file("agent-a", &config, &path, None)
            .await
            .expect("open");
        let checkpoint = runtime
            .checkpoint_recovery(
                None,
                None,
                &opened.resource_id,
                &opened.subscription_id,
                &opened.descriptor.content_hash,
                "base",
                &opened.resource_id,
                "main",
                "buffer",
            )
            .await
            .expect("checkpoint");
        runtime.revoke_test_agent_config("agent-a");

        let restored = runtime
            .get_recovery(&checkpoint.recovery_id, &opened.resource_id, "main")
            .await
            .expect("stored bytes remain readable");
        assert_eq!(restored.buffer, "buffer");
        let updated = runtime
            .checkpoint_recovery(
                Some(&checkpoint.recovery_id),
                Some(checkpoint.recovery_revision),
                &opened.resource_id,
                &opened.subscription_id,
                &opened.descriptor.content_hash,
                "base",
                &opened.resource_id,
                "main",
                "newer buffer after revocation",
            )
            .await
            .expect("scoped recovery CAS update does not require current file authority");
        assert_eq!(
            updated
                .file_authorization_error
                .as_ref()
                .map(FileResourceErrorV1::code),
            Some("unauthorized_path")
        );
        assert_eq!(
            runtime
                .get_recovery(&checkpoint.recovery_id, &opened.resource_id, "main")
                .await
                .expect("updated recovery remains readable")
                .buffer,
            "newer buffer after revocation"
        );
        let create = runtime
            .checkpoint_recovery(
                None,
                None,
                &opened.resource_id,
                &opened.subscription_id,
                &opened.descriptor.content_hash,
                "base",
                &opened.resource_id,
                "main",
                "new recovery must still require live authority",
            )
            .await
            .expect_err("first recovery checkpoint must retain live authority requirement");
        assert_eq!(create.code(), "unauthorized_path");
        let merge = runtime
            .merge_recovery(
                &checkpoint.recovery_id,
                updated.recovery_revision,
                &opened.resource_id,
                "main",
                &opened.resource_id,
                &opened.subscription_id,
            )
            .await
            .expect_err("revoked subscription must not read disk for merge");
        assert_eq!(merge.code(), "unauthorized_path");
        let save = runtime
            .save_text(
                &opened.resource_id,
                &opened.subscription_id,
                opened.revision,
                &opened.descriptor.content_hash,
                "recovery must not authorize this write",
            )
            .await
            .expect_err("recovery must not revive revoked file authority");
        assert_eq!(save.code(), "unauthorized_path");
        assert_eq!(fs::read_to_string(path).expect("disk bytes"), "base");
    }

    #[tokio::test]
    async fn file_recovery_existing_cas_survives_last_subscription_close() {
        let temp = tempfile::tempdir().expect("temp root");
        let workspace = temp.path().join("workspace");
        fs::create_dir(&workspace).expect("workspace");
        let path = workspace.join("draft.txt");
        fs::write(&path, "base").expect("fixture");
        let config = agent_config("agent-a", &workspace);
        let runtime = FileResourceRuntime::with_recovery_root(
            Duration::from_millis(150),
            Duration::from_secs(60),
            temp.path().join("recovery"),
        );
        let opened = runtime
            .open_agent_file("agent-a", &config, &path, None)
            .await
            .expect("open");
        let checkpoint = runtime
            .checkpoint_recovery(
                None,
                None,
                &opened.resource_id,
                &opened.subscription_id,
                &opened.descriptor.content_hash,
                "base",
                &opened.resource_id,
                "main",
                "buffer",
            )
            .await
            .expect("checkpoint");
        runtime
            .close(&opened.subscription_id)
            .await
            .expect("close last subscription");
        assert_eq!(runtime.subscriber_count(&opened.resource_id).await, 0);

        let updated = runtime
            .checkpoint_recovery(
                Some(&checkpoint.recovery_id),
                Some(checkpoint.recovery_revision),
                "closed-resource-does-not-authorize-recovery",
                "closed-subscription-does-not-authorize-recovery",
                &opened.descriptor.content_hash,
                "base",
                &opened.resource_id,
                "main",
                "newer buffer after close",
            )
            .await
            .expect("scoped recovery CAS remains independent of a live subscription");
        assert_eq!(updated.recovery_revision, checkpoint.recovery_revision + 1);
        assert_eq!(
            updated
                .file_authorization_error
                .as_ref()
                .map(FileResourceErrorV1::code),
            Some("resource_not_found")
        );
        assert_eq!(
            runtime
                .get_recovery(&checkpoint.recovery_id, &opened.resource_id, "main")
                .await
                .expect("updated recovery")
                .buffer,
            "newer buffer after close"
        );

        let create = runtime
            .checkpoint_recovery(
                None,
                None,
                &opened.resource_id,
                &opened.subscription_id,
                &opened.descriptor.content_hash,
                "base",
                &opened.resource_id,
                "main",
                "new authority is forbidden",
            )
            .await
            .expect_err("new recovery still requires an open resource");
        assert_eq!(create.code(), "resource_not_found");
        assert_eq!(fs::read_to_string(path).expect("disk bytes"), "base");
    }

    #[tokio::test]
    async fn file_recovery_rejects_oversized_and_tampered_bodies_without_path_escape() {
        let temp = tempfile::tempdir().expect("temp root");
        let recovery_root = temp.path().join("recovery");
        let workspace = temp.path().join("workspace");
        fs::create_dir(&workspace).expect("workspace");
        let path = workspace.join("draft.txt");
        fs::write(&path, "base").expect("fixture");
        let config = agent_config("agent-a", &workspace);
        let runtime = FileResourceRuntime::with_recovery_root(
            Duration::from_millis(150),
            Duration::from_secs(60),
            recovery_root.clone(),
        );
        let opened = runtime
            .open_agent_file("agent-a", &config, &path, None)
            .await
            .expect("open");
        let oversized = "x".repeat(
            usize::try_from(FileResourceLimits::default().monaco_max_size_bytes)
                .expect("limit fits usize")
                + 1,
        );
        let too_large = runtime
            .checkpoint_recovery(
                None,
                None,
                &opened.resource_id,
                &opened.subscription_id,
                &opened.descriptor.content_hash,
                "base",
                &opened.resource_id,
                "main",
                &oversized,
            )
            .await
            .expect_err("oversized recovery buffer must fail");
        assert_eq!(too_large.code(), "file_too_large");

        let checkpoint = runtime
            .checkpoint_recovery(
                None,
                None,
                &opened.resource_id,
                &opened.subscription_id,
                &opened.descriptor.content_hash,
                "base",
                &opened.resource_id,
                "main",
                "buffer",
            )
            .await
            .expect("checkpoint");
        let mut manifest =
            load_recovery_manifest(&recovery_root, &checkpoint.recovery_id).expect("manifest");
        let buffer_path = recovery_root
            .join(&checkpoint.recovery_id)
            .join("blobs")
            .join(&manifest.buffer_blob);
        fs::write(&buffer_path, [0xff, 0xfe]).expect("corrupt blob");
        assert_eq!(
            runtime
                .list_recoveries(&opened.resource_id, "main")
                .await
                .expect("discovery validates metadata without reading every body")
                .len(),
            1
        );
        let invalid_utf8 = runtime
            .get_recovery(&checkpoint.recovery_id, &opened.resource_id, "main")
            .await
            .expect_err("invalid UTF-8 recovery blob must fail");
        assert_eq!(invalid_utf8.code(), "invalid_recovery");

        let secret = temp.path().join("secret.txt");
        fs::write(&secret, "must not be exposed").expect("secret fixture");
        manifest.buffer_blob = "../../secret.txt".to_string();
        wardian_core::conversations::write_json_atomic(
            &recovery_root
                .join(&checkpoint.recovery_id)
                .join("manifest.json"),
            &manifest,
        )
        .expect("tamper manifest");
        let escaped = runtime
            .get_recovery(&checkpoint.recovery_id, &opened.resource_id, "main")
            .await
            .expect_err("tampered blob path must not escape recovery record");
        assert_eq!(escaped.code(), "invalid_recovery");
        assert_eq!(
            fs::read_to_string(secret).expect("secret bytes"),
            "must not be exposed"
        );
    }

    #[test]
    fn file_recovery_rejects_non_file_preexisting_hash_blob() {
        let temp = tempfile::tempdir().expect("temp root");
        let record_dir = temp.path().join("recovery-id");
        let blobs_dir = record_dir.join("blobs");
        fs::create_dir_all(&blobs_dir).expect("blob directory");
        fs::create_dir(blobs_dir.join(recovery_blob_name("buffer")))
            .expect("non-file blob fixture");

        let failure = write_recovery_blob(&record_dir, "buffer")
            .expect_err("non-file hash blob must fail closed");
        assert_eq!(failure.code(), "invalid_recovery");
    }

    #[tokio::test]
    async fn file_recovery_is_cleaned_after_successful_guarded_save() {
        let temp = tempfile::tempdir().expect("temp root");
        let workspace = temp.path().join("workspace");
        fs::create_dir(&workspace).expect("workspace");
        let path = workspace.join("draft.txt");
        fs::write(&path, "base").expect("fixture");
        let config = agent_config("agent-a", &workspace);
        let runtime = FileResourceRuntime::with_recovery_root(
            Duration::from_millis(150),
            Duration::from_secs(60),
            temp.path().join("recovery"),
        );
        let opened = runtime
            .open_agent_file("agent-a", &config, &path, None)
            .await
            .expect("open");
        let checkpoint = runtime
            .checkpoint_recovery(
                None,
                None,
                &opened.resource_id,
                &opened.subscription_id,
                &opened.descriptor.content_hash,
                "base",
                &opened.resource_id,
                "main",
                "saved buffer",
            )
            .await
            .expect("checkpoint");

        let saved = runtime
            .save_text_with_recovery_cleanup(
                &opened.resource_id,
                &opened.subscription_id,
                opened.revision,
                &opened.descriptor.content_hash,
                "saved buffer",
                Some(&FileRecoveryCleanupV1 {
                    recovery_id: checkpoint.recovery_id.clone(),
                    expected_recovery_revision: checkpoint.recovery_revision,
                }),
                "main",
            )
            .await
            .expect("save");
        assert!(matches!(saved, FileResourceSaveResultV1::Saved { .. }));
        let recovery = runtime
            .get_recovery(&checkpoint.recovery_id, &opened.resource_id, "main")
            .await
            .expect_err("successful save must clean recovery");
        assert_eq!(recovery.code(), "recovery_not_found");
        assert_eq!(
            fs::read_to_string(path).expect("saved bytes"),
            "saved buffer"
        );
    }

    #[tokio::test]
    async fn file_resources_save_text_is_guarded_and_emits_one_logical_revision() {
        let temp = tempfile::tempdir().expect("temp root");
        let path = temp.path().join("draft.txt");
        fs::write(&path, "revision one").expect("fixture");
        let config = agent_config("agent-a", temp.path());
        let runtime = test_runtime();
        let opened = runtime
            .open_agent_file("agent-a", &config, &path, None)
            .await
            .expect("open");
        let base_hash = opened.descriptor.content_hash.clone();
        let mut events = runtime.subscribe_events();

        let saved = runtime
            .save_text(
                &opened.resource_id,
                &opened.subscription_id,
                opened.revision,
                &base_hash,
                "revision two",
            )
            .await
            .expect("save");
        let (saved_revision, saved_hash) = match saved {
            FileResourceSaveResultV1::Saved {
                revision,
                content_hash,
            } => (revision, content_hash),
            other => panic!("expected saved result, got {other:?}"),
        };
        assert_eq!(saved_revision, opened.revision + 1);
        assert_ne!(saved_hash, base_hash);
        let event = timeout(Duration::from_secs(1), events.recv())
            .await
            .expect("save event timeout")
            .expect("save event");
        assert_eq!(event.revision, saved_revision);
        assert_eq!(event.descriptor.content_hash, saved_hash);

        let stale = runtime
            .save_text(
                &opened.resource_id,
                &opened.subscription_id,
                opened.revision,
                &base_hash,
                "stale overwrite",
            )
            .await
            .expect("stale conflict result");
        assert_eq!(
            stale,
            FileResourceSaveResultV1::StaleConflict {
                revision: saved_revision,
                content_hash: saved_hash,
            }
        );
        assert_eq!(
            fs::read_to_string(&path).expect("saved bytes"),
            "revision two"
        );
        assert!(timeout(Duration::from_millis(400), events.recv())
            .await
            .is_err());
    }

    #[tokio::test]
    async fn file_resources_save_text_rebinds_every_live_subscription() {
        let temp = tempfile::tempdir().expect("temp root");
        let path = temp.path().join("shared-draft.txt");
        fs::write(&path, "revision one").expect("fixture");
        let config = agent_config("agent-a", temp.path());
        let runtime = test_runtime();
        let first = runtime
            .open_agent_file("agent-a", &config, &path, None)
            .await
            .expect("first open");
        let second = runtime
            .open_agent_file("agent-a", &config, &path, None)
            .await
            .expect("second open");

        let first_save = runtime
            .save_text(
                &first.resource_id,
                &first.subscription_id,
                first.revision,
                &first.descriptor.content_hash,
                "revision two",
            )
            .await
            .expect("first subscription save");
        let (second_revision, second_hash) = match first_save {
            FileResourceSaveResultV1::Saved {
                revision,
                content_hash,
            } => (revision, content_hash),
            unexpected => panic!("expected saved result, got {unexpected:?}"),
        };
        assert_eq!(
            runtime
                .read_text(
                    &second.resource_id,
                    &second.subscription_id,
                    second_revision,
                    Some(&config),
                )
                .await
                .expect("second subscription reads rebound identity")
                .text,
            "revision two"
        );

        let second_save = runtime
            .save_text(
                &second.resource_id,
                &second.subscription_id,
                second_revision,
                &second_hash,
                "revision three",
            )
            .await
            .expect("second subscription saves rebound identity");
        let third_revision = match second_save {
            FileResourceSaveResultV1::Saved { revision, .. } => revision,
            unexpected => panic!("expected saved result, got {unexpected:?}"),
        };
        assert_eq!(
            runtime
                .read_text(
                    &first.resource_id,
                    &first.subscription_id,
                    third_revision,
                    Some(&config),
                )
                .await
                .expect("first subscription reads second rebound identity")
                .text,
            "revision three"
        );
    }

    #[tokio::test]
    async fn file_resources_save_serializes_concurrent_subscription_admission() {
        let temp = tempfile::tempdir().expect("temp root");
        let path = temp.path().join("concurrent-draft.txt");
        fs::write(&path, "revision one").expect("fixture");
        let config = agent_config("agent-a", temp.path());
        let runtime = test_runtime();
        let first = runtime
            .open_agent_file("agent-a", &config, &path, None)
            .await
            .expect("first open");
        let hook = SaveAfterValidationHook {
            validation_reached: Arc::new(tokio::sync::Barrier::new(2)),
            resume_save: Arc::new(tokio::sync::Barrier::new(2)),
        };
        *runtime.inner.save_after_validation_hook.lock().await = Some(hook.clone());

        let save_runtime = runtime.clone();
        let save_resource_id = first.resource_id.clone();
        let save_subscription_id = first.subscription_id.clone();
        let save_hash = first.descriptor.content_hash.clone();
        let save = tokio::spawn(async move {
            save_runtime
                .save_text(
                    &save_resource_id,
                    &save_subscription_id,
                    first.revision,
                    &save_hash,
                    "revision two",
                )
                .await
        });
        hook.validation_reached.wait().await;

        let open_runtime = runtime.clone();
        let open_config = config.clone();
        let open_path = path.clone();
        let mut concurrent_open = tokio::spawn(async move {
            open_runtime
                .open_agent_file("agent-a", &open_config, &open_path, None)
                .await
        });
        assert!(
            timeout(Duration::from_millis(100), &mut concurrent_open)
                .await
                .is_err(),
            "existing-resource admission must wait for the save operation"
        );
        *runtime.inner.save_after_validation_hook.lock().await = None;
        hook.resume_save.wait().await;

        let saved = save.await.expect("save task").expect("save result");
        let (saved_revision, saved_hash) = match saved {
            FileResourceSaveResultV1::Saved {
                revision,
                content_hash,
            } => (revision, content_hash),
            unexpected => panic!("expected saved result, got {unexpected:?}"),
        };
        let second = concurrent_open
            .await
            .expect("open task")
            .expect("concurrent open");
        assert_eq!(second.revision, saved_revision);
        assert_eq!(
            runtime
                .read_text(
                    &second.resource_id,
                    &second.subscription_id,
                    saved_revision,
                    Some(&config),
                )
                .await
                .expect("concurrent subscription reads replacement")
                .text,
            "revision two"
        );

        let saved_again = runtime
            .save_text(
                &second.resource_id,
                &second.subscription_id,
                saved_revision,
                &saved_hash,
                "revision three",
            )
            .await
            .expect("concurrent subscription saves replacement");
        let final_revision = match saved_again {
            FileResourceSaveResultV1::Saved { revision, .. } => revision,
            unexpected => panic!("expected saved result, got {unexpected:?}"),
        };
        assert_eq!(
            runtime
                .read_text(
                    &first.resource_id,
                    &first.subscription_id,
                    final_revision,
                    Some(&config),
                )
                .await
                .expect("original subscription reads second replacement")
                .text,
            "revision three"
        );
    }

    #[tokio::test]
    async fn file_resources_save_as_consumes_one_exact_target_grant() {
        let temp = tempfile::tempdir().expect("temp root");
        let selected = temp.path().join("selected.txt");
        let sibling = temp.path().join("sibling.txt");
        let runtime = test_runtime();
        let grant = runtime
            .record_save_target(&selected)
            .await
            .expect("save target grant");

        let saved = runtime
            .save_file_resource_as_text(&grant.save_target_grant_id, "saved text")
            .await
            .expect("save as");

        assert_eq!(saved.canonical_path, grant.selected_path);
        assert_eq!(saved.resource_id, file_resource_id(&grant.selected_path));
        assert_eq!(
            fs::read_to_string(&selected).expect("selected bytes"),
            "saved text"
        );
        let opened = runtime
            .open_user_file(&saved.capability_id, &selected, None)
            .await
            .expect("published Save As capability opens its exact target");
        runtime
            .close(&opened.subscription_id)
            .await
            .expect("close saved target");
        assert!(
            !sibling.exists(),
            "exact grant must not create a sibling name"
        );
        assert_eq!(
            runtime
                .save_file_resource_as_text(&grant.save_target_grant_id, "second use")
                .await
                .expect_err("save target grant must be one-shot")
                .code(),
            "unauthorized_save_target"
        );
    }

    #[tokio::test]
    async fn file_resources_save_as_reserves_capacity_before_creating_missing_target() {
        let temp = tempfile::tempdir().expect("temp root");
        let occupied_path = temp.path().join("occupied.txt");
        let selected = temp.path().join("missing-target.txt");
        fs::write(&occupied_path, "occupied").expect("occupied fixture");
        let runtime = FileResourceRuntime::with_test_limits(
            Duration::from_millis(50),
            Duration::from_secs(60),
            1,
            MAX_TICKET_SNAPSHOT_BYTES,
        );
        let occupied_grant = runtime
            .record_user_file(&occupied_path)
            .await
            .expect("occupied capability");
        let occupied = runtime
            .open_user_file(&occupied_grant.capability_id, &occupied_path, None)
            .await
            .expect("active occupied capability");
        let save_target = runtime
            .record_save_target(&selected)
            .await
            .expect("missing save target");

        assert_eq!(
            runtime
                .save_file_resource_as_text(&save_target.save_target_grant_id, "submitted")
                .await
                .expect_err("active capacity must reject before create")
                .code(),
            "grant_limit_reached"
        );
        assert!(!selected.exists(), "capacity error must not create target");

        runtime
            .close(&occupied.subscription_id)
            .await
            .expect("release occupied capability");
        runtime
            .save_file_resource_as_text(&save_target.save_target_grant_id, "submitted")
            .await
            .expect("capacity rejection must not consume target grant");
        assert_eq!(
            fs::read_to_string(&selected).expect("saved target"),
            "submitted"
        );
    }

    #[tokio::test]
    async fn file_resources_save_as_reserves_capacity_before_replacing_existing_target() {
        let temp = tempfile::tempdir().expect("temp root");
        let occupied_path = temp.path().join("occupied.txt");
        let selected = temp.path().join("existing-target.txt");
        fs::write(&occupied_path, "occupied").expect("occupied fixture");
        fs::write(&selected, "original target bytes").expect("existing fixture");
        let runtime = FileResourceRuntime::with_test_limits(
            Duration::from_millis(50),
            Duration::from_secs(60),
            1,
            MAX_TICKET_SNAPSHOT_BYTES,
        );
        let occupied_grant = runtime
            .record_user_file(&occupied_path)
            .await
            .expect("occupied capability");
        let _occupied = runtime
            .open_user_file(&occupied_grant.capability_id, &occupied_path, None)
            .await
            .expect("active occupied capability");
        let save_target = runtime
            .record_save_target(&selected)
            .await
            .expect("existing save target");

        assert_eq!(
            runtime
                .save_file_resource_as_text(&save_target.save_target_grant_id, "submitted")
                .await
                .expect_err("active capacity must reject before replace")
                .code(),
            "grant_limit_reached"
        );
        assert_eq!(
            fs::read_to_string(&selected).expect("unchanged existing target"),
            "original target bytes"
        );
    }

    #[tokio::test]
    async fn file_resources_save_text_reports_unchanged_and_refreshes_external_stale_conflict() {
        let temp = tempfile::tempdir().expect("temp root");
        let path = temp.path().join("draft.txt");
        fs::write(&path, "revision one").expect("fixture");
        let config = agent_config("agent-a", temp.path());
        let runtime = test_runtime();
        let opened = runtime
            .open_agent_file("agent-a", &config, &path, None)
            .await
            .expect("open");
        let base_hash = opened.descriptor.content_hash.clone();
        let mut events = runtime.subscribe_events();

        assert_eq!(
            runtime
                .save_text(
                    &opened.resource_id,
                    &opened.subscription_id,
                    opened.revision,
                    &base_hash,
                    "revision one",
                )
                .await
                .expect("unchanged save"),
            FileResourceSaveResultV1::Unchanged {
                revision: opened.revision,
                content_hash: base_hash.clone(),
            }
        );
        assert!(timeout(Duration::from_millis(250), events.recv())
            .await
            .is_err());

        fs::write(&path, "external revision").expect("external mutation");
        let conflict = runtime
            .save_text(
                &opened.resource_id,
                &opened.subscription_id,
                opened.revision,
                &base_hash,
                "must not overwrite",
            )
            .await
            .expect("tagged stale conflict");
        let (revision, content_hash) = match conflict {
            FileResourceSaveResultV1::StaleConflict {
                revision,
                content_hash,
            } => (revision, content_hash),
            other => panic!("expected stale conflict, got {other:?}"),
        };
        assert_eq!(revision, opened.revision + 1);
        assert_ne!(content_hash, base_hash);
        assert_eq!(
            fs::read_to_string(&path).expect("external bytes"),
            "external revision"
        );
        let event = timeout(Duration::from_secs(1), events.recv())
            .await
            .expect("refresh event timeout")
            .expect("refresh event");
        assert_eq!(event.revision, revision);
        assert_eq!(event.descriptor.content_hash, content_hash);
        assert!(timeout(Duration::from_millis(400), events.recv())
            .await
            .is_err());
    }

    #[tokio::test]
    async fn file_resources_save_text_rejects_revoked_roots_and_changed_identity() {
        let temp = tempfile::tempdir().expect("temp root");
        let authorized_root = temp.path().join("authorized");
        let revoked_root = temp.path().join("revoked");
        fs::create_dir_all(&authorized_root).expect("authorized root");
        fs::create_dir_all(&revoked_root).expect("revoked root");
        let path = authorized_root.join("draft.txt");
        fs::write(&path, "revision one").expect("fixture");
        let config = agent_config("agent-a", &authorized_root);
        let runtime = test_runtime();
        let opened = runtime
            .open_agent_file("agent-a", &config, &path, None)
            .await
            .expect("open");

        let revoked = agent_config("agent-a", &revoked_root);
        runtime
            .current_agent_config_resolver()
            .observe_open("agent-a", &revoked);
        assert_eq!(
            runtime
                .save_text(
                    &opened.resource_id,
                    &opened.subscription_id,
                    opened.revision,
                    &opened.descriptor.content_hash,
                    "must not save",
                )
                .await
                .expect_err("revoked root must fail")
                .code(),
            "unauthorized_path"
        );
        assert_eq!(
            fs::read_to_string(&path).expect("original bytes"),
            "revision one"
        );

        runtime
            .current_agent_config_resolver()
            .observe_open("agent-a", &config);
        let replacement = authorized_root.join("replacement.txt");
        fs::write(&replacement, "replacement identity").expect("replacement fixture");
        replace_path_identity(&replacement, &path);
        assert_eq!(
            runtime
                .save_text(
                    &opened.resource_id,
                    &opened.subscription_id,
                    opened.revision,
                    &opened.descriptor.content_hash,
                    "must not overwrite replacement",
                )
                .await
                .expect_err("changed identity must fail")
                .code(),
            "unauthorized_path"
        );
        assert_eq!(
            fs::read_to_string(&path).expect("replacement bytes"),
            "replacement identity"
        );
    }

    #[tokio::test]
    async fn file_resources_save_revalidates_backend_claim_after_initial_validation() {
        let temp = tempfile::tempdir().expect("temp root");
        let path = temp.path().join("revoked-during-save.txt");
        fs::write(&path, "revision one").expect("fixture");
        let config = agent_config("agent-a", temp.path());
        let runtime = test_runtime();
        let opened = runtime
            .open_agent_file("agent-a", &config, &path, None)
            .await
            .expect("open");
        let mut events = runtime.subscribe_events();
        let hook = SaveAfterValidationHook {
            validation_reached: Arc::new(tokio::sync::Barrier::new(2)),
            resume_save: Arc::new(tokio::sync::Barrier::new(2)),
        };
        *runtime.inner.save_after_validation_hook.lock().await = Some(hook.clone());

        let save_runtime = runtime.clone();
        let resource_id = opened.resource_id.clone();
        let subscription_id = opened.subscription_id.clone();
        let content_hash = opened.descriptor.content_hash.clone();
        let save = tokio::spawn(async move {
            save_runtime
                .save_text(
                    &resource_id,
                    &subscription_id,
                    opened.revision,
                    &content_hash,
                    "must not commit",
                )
                .await
        });
        hook.validation_reached.wait().await;
        runtime.revoke_test_agent_config("agent-a");
        *runtime.inner.save_after_validation_hook.lock().await = None;
        hook.resume_save.wait().await;

        assert_eq!(
            save.await
                .expect("save task")
                .expect_err("commit-time revoked claim must fail")
                .code(),
            "unauthorized_path"
        );
        assert_eq!(
            fs::read_to_string(&path).expect("original bytes"),
            "revision one"
        );
        assert!(
            timeout(Duration::from_millis(400), events.recv())
                .await
                .is_err(),
            "rejected save must not emit a saved event"
        );
    }

    #[tokio::test]
    async fn file_resources_save_as_fails_closed_when_target_or_parent_binding_changes() {
        let temp = tempfile::tempdir().expect("temp root");
        let runtime = test_runtime();

        let missing = temp.path().join("missing.txt");
        let missing_grant = runtime
            .record_save_target(&missing)
            .await
            .expect("missing target grant");
        fs::write(&missing, "attacker bytes").expect("target race");
        assert_eq!(
            runtime
                .save_file_resource_as_text(&missing_grant.save_target_grant_id, "submitted")
                .await
                .expect_err("new target binding must fail")
                .code(),
            "unauthorized_save_target"
        );
        assert_eq!(
            fs::read_to_string(&missing).expect("attacker bytes"),
            "attacker bytes"
        );

        let existing = temp.path().join("existing.txt");
        fs::write(&existing, "original").expect("existing fixture");
        let existing_grant = runtime
            .record_save_target(&existing)
            .await
            .expect("existing target grant");
        let replacement = temp.path().join("existing-replacement.txt");
        fs::write(&replacement, "replacement identity").expect("replacement fixture");
        replace_path_identity(&replacement, &existing);
        assert_eq!(
            runtime
                .save_file_resource_as_text(&existing_grant.save_target_grant_id, "submitted")
                .await
                .expect_err("existing target identity change must fail")
                .code(),
            "unauthorized_path"
        );
        assert_eq!(
            fs::read_to_string(&existing).expect("replacement bytes"),
            "replacement identity"
        );

        let approved_parent = temp.path().join("approved-parent");
        let other_parent = temp.path().join("other-parent");
        fs::create_dir_all(&approved_parent).expect("approved parent");
        fs::create_dir_all(&other_parent).expect("other parent");
        let alias_parent = temp.path().join("selected-parent");
        create_directory_link(&approved_parent, &alias_parent);
        let alias_target = alias_parent.join("copy.txt");
        let parent_grant = runtime
            .record_save_target(&alias_target)
            .await
            .expect("parent-bound grant");
        remove_directory_link(&alias_parent);
        create_directory_link(&other_parent, &alias_parent);
        assert_eq!(
            runtime
                .save_file_resource_as_text(&parent_grant.save_target_grant_id, "submitted")
                .await
                .expect_err("retargeted parent must fail")
                .code(),
            "unauthorized_save_target"
        );
        assert!(!approved_parent.join("copy.txt").exists());
        assert!(!other_parent.join("copy.txt").exists());
    }

    #[tokio::test]
    async fn file_resources_save_as_never_retargets_the_open_source_resource() {
        let temp = tempfile::tempdir().expect("temp root");
        let source = temp.path().join("source.txt");
        let copy = temp.path().join("copy.txt");
        fs::write(&source, "source bytes").expect("source fixture");
        fs::write(&copy, "old copy bytes").expect("existing copy fixture");
        let config = agent_config("agent-a", temp.path());
        let runtime = test_runtime();
        let opened = runtime
            .open_agent_file("agent-a", &config, &source, None)
            .await
            .expect("open source");
        let grant = runtime.record_save_target(&copy).await.expect("copy grant");

        let saved = runtime
            .save_file_resource_as_text(&grant.save_target_grant_id, "copy bytes")
            .await
            .expect("save copy");

        assert_ne!(saved.resource_id, opened.resource_id);
        let source_snapshot = runtime
            .snapshot(&opened.resource_id)
            .await
            .expect("source remains open");
        assert_eq!(source_snapshot.resource_id, opened.resource_id);
        assert_eq!(source_snapshot.subscription_id, opened.subscription_id);
        assert_eq!(
            fs::read_to_string(&source).expect("source bytes"),
            "source bytes"
        );
        assert_eq!(fs::read_to_string(&copy).expect("copy bytes"), "copy bytes");
    }

    #[tokio::test]
    async fn subscribers_share_one_watcher_and_close_by_reference_count() {
        let temp = tempfile::tempdir().expect("temp root");
        let path = temp.path().join("shared.txt");
        fs::write(&path, "one\n").expect("fixture");
        let config = agent_config("agent-a", temp.path());
        let runtime = test_runtime();

        let first = runtime
            .open_agent_file("agent-a", &config, &path, None)
            .await
            .expect("first open");
        let second = runtime
            .open_agent_file("agent-a", &config, &path, None)
            .await
            .expect("second open");

        assert_eq!(first.resource_id, second.resource_id);
        assert_ne!(first.subscription_id, second.subscription_id);
        assert_eq!(runtime.watcher_count().await, 1);
        assert_eq!(runtime.subscriber_count(&first.resource_id).await, 2);

        runtime
            .close(&first.subscription_id)
            .await
            .expect("close first");
        assert_eq!(runtime.watcher_count().await, 1);
        assert_eq!(runtime.subscriber_count(&first.resource_id).await, 1);

        runtime
            .close(&second.subscription_id)
            .await
            .expect("close second");
        assert_eq!(runtime.watcher_count().await, 0);
    }

    #[tokio::test]
    async fn removed_alias_only_revokes_its_subscription_while_direct_text_refreshes() {
        let temp = tempfile::tempdir().expect("temp root");
        let canonical_dir = temp.path().join("z-approved");
        let alias_dir = temp.path().join("a-current");
        fs::create_dir(&canonical_dir).expect("canonical directory");
        create_directory_link(&canonical_dir, &alias_dir);
        let canonical_path = canonical_dir.join("shared.txt");
        let alias_path = alias_dir.join("shared.txt");
        fs::write(&canonical_path, "revision one\n").expect("fixture");
        let config = agent_config("agent-a", temp.path());
        let runtime = test_runtime();

        let alias = runtime
            .open_agent_file("agent-a", &config, &alias_path, None)
            .await
            .expect("open through alias");
        let direct = runtime
            .open_agent_file("agent-a", &config, &canonical_path, None)
            .await
            .expect("open directly");
        assert_eq!(alias.resource_id, direct.resource_id);
        assert_eq!(runtime.watcher_count().await, 1);
        assert_eq!(runtime.subscriber_count(&alias.resource_id).await, 2);

        remove_directory_link(&alias_dir);
        assert_eq!(
            runtime
                .read_text(
                    &alias.resource_id,
                    &alias.subscription_id,
                    alias.revision,
                    Some(&config),
                )
                .await
                .expect_err("removed alias must revoke only its subscription")
                .code(),
            "unauthorized_path"
        );
        assert_eq!(
            runtime
                .read_text(
                    &direct.resource_id,
                    &direct.subscription_id,
                    direct.revision,
                    Some(&config),
                )
                .await
                .expect("direct subscription remains readable")
                .text,
            "revision one\n"
        );

        let mut events = runtime.subscribe_events();
        fs::write(&canonical_path, "revision two\n").expect("updated fixture");
        runtime.schedule_refresh(direct.resource_id.clone());
        let event = timeout(Duration::from_secs(3), events.recv())
            .await
            .expect("direct refresh timeout")
            .expect("direct refresh event");
        assert_eq!(event.revision, direct.revision + 1);
        assert_eq!(event.descriptor.unavailable_reason, None);
        assert_eq!(
            runtime
                .read_text(
                    &direct.resource_id,
                    &direct.subscription_id,
                    event.revision,
                    Some(&config),
                )
                .await
                .expect("refreshed direct read")
                .text,
            "revision two\n"
        );
        assert_eq!(
            runtime
                .read_text(
                    &alias.resource_id,
                    &alias.subscription_id,
                    event.revision,
                    Some(&config),
                )
                .await
                .expect_err("removed alias stays revoked after shared refresh")
                .code(),
            "unauthorized_path"
        );

        runtime
            .close(&alias.subscription_id)
            .await
            .expect("close alias subscription");
        assert_eq!(runtime.watcher_count().await, 1);
        assert_eq!(runtime.subscriber_count(&direct.resource_id).await, 1);
        assert_eq!(
            runtime
                .read_text(
                    &direct.resource_id,
                    &direct.subscription_id,
                    event.revision,
                    Some(&config),
                )
                .await
                .expect("direct read survives alias close")
                .text,
            "revision two\n"
        );
        runtime
            .close(&direct.subscription_id)
            .await
            .expect("close direct subscription");
        assert_eq!(runtime.watcher_count().await, 0);
    }

    #[tokio::test]
    async fn valid_direct_join_recovers_alias_only_unavailable_without_file_event() {
        let temp = tempfile::tempdir().expect("temp root");
        let canonical_dir = temp.path().join("z-approved");
        let alias_dir = temp.path().join("a-current");
        fs::create_dir(&canonical_dir).expect("canonical directory");
        create_directory_link(&canonical_dir, &alias_dir);
        let canonical_path = canonical_dir.join("shared.txt");
        let alias_path = alias_dir.join("shared.txt");
        fs::write(&canonical_path, "stable\n").expect("fixture");
        let config = agent_config("agent-a", temp.path());
        let runtime = test_runtime();
        let mut events = runtime.subscribe_events();
        let alias = runtime
            .open_agent_file("agent-a", &config, &alias_path, None)
            .await
            .expect("open alias");

        remove_directory_link(&alias_dir);
        runtime.schedule_refresh(alias.resource_id.clone());
        let unavailable = timeout(Duration::from_secs(3), events.recv())
            .await
            .expect("alias unavailable timeout")
            .expect("alias unavailable event");
        assert_eq!(
            unavailable.descriptor.unavailable_reason.as_deref(),
            Some("unauthorized_path")
        );

        let direct = runtime
            .open_agent_file("agent-a", &config, &canonical_path, None)
            .await
            .expect("join valid direct subscription");
        assert_eq!(direct.revision, unavailable.revision);
        assert_eq!(
            direct.descriptor.unavailable_reason.as_deref(),
            Some("unauthorized_path")
        );
        let recovered = timeout(Duration::from_secs(3), events.recv())
            .await
            .expect("membership recovery timeout")
            .expect("membership recovery event");
        assert_eq!(recovered.revision, unavailable.revision + 1);
        assert_eq!(recovered.descriptor.unavailable_reason, None);
        assert_eq!(
            runtime
                .read_text(
                    &direct.resource_id,
                    &direct.subscription_id,
                    recovered.revision,
                    Some(&config),
                )
                .await
                .expect("direct subscription reads recovered revision")
                .text,
            "stable\n"
        );
        sleep(Duration::from_millis(500)).await;
        assert!(
            events.try_recv().is_err(),
            "membership recovery must not schedule an infinite refresh loop"
        );

        runtime
            .close(&alias.subscription_id)
            .await
            .expect("close alias");
        runtime
            .close(&direct.subscription_id)
            .await
            .expect("close direct");
    }

    #[tokio::test]
    async fn closing_last_valid_candidate_marks_invalid_only_resource_unavailable_once() {
        let temp = tempfile::tempdir().expect("temp root");
        let canonical_dir = temp.path().join("z-approved");
        let alias_dir = temp.path().join("a-current");
        fs::create_dir(&canonical_dir).expect("canonical directory");
        create_directory_link(&canonical_dir, &alias_dir);
        let canonical_path = canonical_dir.join("shared.txt");
        let alias_path = alias_dir.join("shared.txt");
        fs::write(&canonical_path, "stable\n").expect("fixture");
        let config = agent_config("agent-a", temp.path());
        let runtime = test_runtime();
        let alias = runtime
            .open_agent_file("agent-a", &config, &alias_path, None)
            .await
            .expect("open alias");
        let direct = runtime
            .open_agent_file("agent-a", &config, &canonical_path, None)
            .await
            .expect("open direct");
        let original_hash = direct.descriptor.content_hash.clone();
        let mut events = runtime.subscribe_events();

        remove_directory_link(&alias_dir);
        runtime
            .close(&direct.subscription_id)
            .await
            .expect("close last valid candidate");
        let unavailable = timeout(Duration::from_secs(3), events.recv())
            .await
            .expect("membership unavailable timeout")
            .expect("membership unavailable event");
        assert_eq!(unavailable.revision, direct.revision + 1);
        assert_eq!(unavailable.descriptor.content_hash, original_hash);
        assert_eq!(
            unavailable.descriptor.unavailable_reason.as_deref(),
            Some("unauthorized_path")
        );
        sleep(Duration::from_millis(500)).await;
        assert!(
            events.try_recv().is_err(),
            "invalid-only membership must settle after one unavailable revision"
        );

        runtime
            .close(&alias.subscription_id)
            .await
            .expect("close invalid alias");
        assert_eq!(runtime.watcher_count().await, 0);
    }

    #[tokio::test]
    async fn revoked_agent_candidate_is_skipped_before_valid_picker_refresh_scan() {
        let temp = tempfile::tempdir().expect("temp root");
        let canonical_dir = temp.path().join("z-approved");
        let alias_dir = temp.path().join("a-agent");
        fs::create_dir(&canonical_dir).expect("canonical directory");
        create_directory_link(&canonical_dir, &alias_dir);
        let canonical_path = canonical_dir.join("shared.txt");
        let alias_path = alias_dir.join("shared.txt");
        fs::write(&canonical_path, "revision one\n").expect("fixture");
        let config = agent_config("agent-a", temp.path());
        let runtime = test_runtime();
        let agent = runtime
            .open_agent_file("agent-a", &config, &alias_path, None)
            .await
            .expect("open agent alias");
        let grant = runtime
            .record_user_file(&canonical_path)
            .await
            .expect("picker grant");
        let picker = runtime
            .open_user_file(&grant.capability_id, &canonical_path, None)
            .await
            .expect("open picker direct");
        let initial_hash = picker.descriptor.content_hash.clone();
        runtime.revoke_test_agent_config("agent-a");
        let scans_before = runtime.inner.refresh_scan_count.load(Ordering::Acquire);
        let mut events = runtime.subscribe_events();

        fs::write(&canonical_path, "revision two\n").expect("updated fixture");
        runtime.schedule_refresh(picker.resource_id.clone());
        let event = timeout(Duration::from_secs(3), events.recv())
            .await
            .expect("picker fallback timeout")
            .expect("picker fallback event");
        assert_eq!(event.descriptor.unavailable_reason, None);
        assert_ne!(event.descriptor.content_hash, initial_hash);
        assert_eq!(
            runtime.inner.refresh_scan_count.load(Ordering::Acquire),
            scans_before + 1,
            "revoked agent candidate must be rejected before descriptor scanning"
        );
        assert_eq!(
            runtime
                .read_text(
                    &picker.resource_id,
                    &picker.subscription_id,
                    event.revision,
                    None,
                )
                .await
                .expect("picker reads refreshed revision")
                .text,
            "revision two\n"
        );

        runtime
            .close(&agent.subscription_id)
            .await
            .expect("close revoked agent");
        runtime
            .close(&picker.subscription_id)
            .await
            .expect("close picker");
    }

    #[tokio::test]
    async fn revoked_picker_candidate_is_skipped_before_valid_agent_refresh_scan() {
        let temp = tempfile::tempdir().expect("temp root");
        let canonical_dir = temp.path().join("z-approved");
        let alias_dir = temp.path().join("a-picker");
        fs::create_dir(&canonical_dir).expect("canonical directory");
        create_directory_link(&canonical_dir, &alias_dir);
        let canonical_path = canonical_dir.join("shared.txt");
        let alias_path = alias_dir.join("shared.txt");
        fs::write(&canonical_path, "revision one\n").expect("fixture");
        let config = agent_config("agent-a", temp.path());
        let runtime = test_runtime();
        let grant = runtime
            .record_user_file(&alias_path)
            .await
            .expect("picker grant");
        let picker = runtime
            .open_user_file(&grant.capability_id, &alias_path, None)
            .await
            .expect("open picker alias");
        let agent = runtime
            .open_agent_file("agent-a", &config, &canonical_path, None)
            .await
            .expect("open agent direct");
        let initial_hash = agent.descriptor.content_hash.clone();
        runtime
            .inner
            .user_file_grants
            .lock()
            .await
            .remove(&grant.capability_id);
        let scans_before = runtime.inner.refresh_scan_count.load(Ordering::Acquire);
        let mut events = runtime.subscribe_events();

        fs::write(&canonical_path, "revision two\n").expect("updated fixture");
        runtime.schedule_refresh(agent.resource_id.clone());
        let event = timeout(Duration::from_secs(3), events.recv())
            .await
            .expect("agent fallback timeout")
            .expect("agent fallback event");
        assert_eq!(event.descriptor.unavailable_reason, None);
        assert_ne!(event.descriptor.content_hash, initial_hash);
        assert_eq!(
            runtime.inner.refresh_scan_count.load(Ordering::Acquire),
            scans_before + 1,
            "revoked picker candidate must be rejected before descriptor scanning"
        );

        runtime
            .close(&picker.subscription_id)
            .await
            .expect("close revoked picker");
        runtime
            .close(&agent.subscription_id)
            .await
            .expect("close agent");
    }

    #[tokio::test]
    async fn invalid_only_live_claim_preserves_prior_hash_without_descriptor_scan() {
        let temp = tempfile::tempdir().expect("temp root");
        let path = temp.path().join("shared.txt");
        fs::write(&path, "revision one\n").expect("fixture");
        let config = agent_config("agent-a", temp.path());
        let runtime = test_runtime();
        let agent = runtime
            .open_agent_file("agent-a", &config, &path, None)
            .await
            .expect("open agent file");
        let initial_hash = agent.descriptor.content_hash.clone();
        runtime.revoke_test_agent_config("agent-a");
        let scans_before = runtime.inner.refresh_scan_count.load(Ordering::Acquire);
        let mut events = runtime.subscribe_events();

        fs::write(&path, "revision two\n").expect("updated fixture");
        runtime.schedule_refresh(agent.resource_id.clone());
        let event = timeout(Duration::from_secs(3), events.recv())
            .await
            .expect("invalid-only timeout")
            .expect("invalid-only event");
        assert_eq!(event.descriptor.content_hash, initial_hash);
        assert_eq!(
            event.descriptor.unavailable_reason.as_deref(),
            Some("unauthorized_path")
        );
        assert_eq!(
            runtime.inner.refresh_scan_count.load(Ordering::Acquire),
            scans_before,
            "invalid-only authority must not scan or publish the changed hash"
        );
        sleep(Duration::from_millis(500)).await;
        assert!(
            events.try_recv().is_err(),
            "invalid-only refresh must settle"
        );

        runtime
            .close(&agent.subscription_id)
            .await
            .expect("close revoked agent");
    }

    #[tokio::test]
    async fn retargeted_picker_alias_cannot_poison_direct_pdf_ticket_or_atomic_refresh() {
        let temp = tempfile::tempdir().expect("temp root");
        let canonical_dir = temp.path().join("z-approved");
        let other_dir = temp.path().join("y-other");
        let alias_dir = temp.path().join("a-current");
        fs::create_dir(&canonical_dir).expect("canonical directory");
        fs::create_dir(&other_dir).expect("other directory");
        let canonical_path = canonical_dir.join("shared.pdf");
        let other_path = other_dir.join("shared.pdf");
        fs::write(&canonical_path, b"%PDF-1.7 revision one").expect("fixture");
        fs::write(&other_path, b"%PDF-1.7 unrelated target").expect("other fixture");
        create_directory_link(&canonical_dir, &alias_dir);
        let alias_path = alias_dir.join("shared.pdf");
        let runtime = test_runtime();

        let alias_grant = runtime
            .record_user_file(&alias_path)
            .await
            .expect("alias picker grant");
        let alias = runtime
            .open_user_file(&alias_grant.capability_id, &alias_path, None)
            .await
            .expect("open picker alias");
        let direct_grant = runtime
            .record_user_file(&canonical_path)
            .await
            .expect("direct picker grant");
        assert_eq!(
            alias_grant.capability_id, direct_grant.capability_id,
            "exact grants deduplicate by canonical resource without widening a subscription"
        );
        let direct = runtime
            .open_user_file(&direct_grant.capability_id, &canonical_path, None)
            .await
            .expect("open picker path directly");
        let alias_after_dedup = runtime
            .open_user_file(&direct_grant.capability_id, &alias_path, None)
            .await
            .expect("deduplicated grant still retains this open's alias provenance");
        assert_eq!(alias.resource_id, direct.resource_id);
        assert_eq!(runtime.watcher_count().await, 1);
        assert_eq!(runtime.subscriber_count(&direct.resource_id).await, 3);

        remove_directory_link(&alias_dir);
        create_directory_link(&other_dir, &alias_dir);
        assert_eq!(
            runtime
                .issue_ticket(
                    &alias.resource_id,
                    &alias.subscription_id,
                    alias.revision,
                    None,
                    "alias-before-refresh",
                )
                .await
                .expect_err("retargeted alias must not mint a ticket")
                .code(),
            "unauthorized_path"
        );
        assert_eq!(
            runtime
                .issue_ticket(
                    &alias_after_dedup.resource_id,
                    &alias_after_dedup.subscription_id,
                    alias_after_dedup.revision,
                    None,
                    "deduplicated-alias-before-refresh",
                )
                .await
                .expect_err("deduplicated capability must not replace alias provenance")
                .code(),
            "unauthorized_path"
        );
        let direct_ticket = runtime
            .issue_ticket(
                &direct.resource_id,
                &direct.subscription_id,
                direct.revision,
                None,
                "direct-before-refresh",
            )
            .await
            .expect("direct ticket survives alias retarget");
        assert_eq!(
            runtime
                .read_ticket_range(&direct_ticket.ticket_id, None)
                .await
                .expect("read direct ticket")
                .bytes,
            b"%PDF-1.7 revision one"
        );

        let replacement = canonical_dir.join("shared.replacement");
        fs::write(&replacement, b"%PDF-1.7 revision two").expect("replacement fixture");
        replace_path_identity(&replacement, &canonical_path);
        let mut events = runtime.subscribe_events();
        runtime.schedule_refresh(direct.resource_id.clone());
        let event = timeout(Duration::from_secs(3), events.recv())
            .await
            .expect("atomic refresh timeout")
            .expect("atomic refresh event");
        assert_eq!(event.revision, direct.revision + 1);
        assert_eq!(event.descriptor.unavailable_reason, None);

        let refreshed_ticket = runtime
            .issue_ticket(
                &direct.resource_id,
                &direct.subscription_id,
                event.revision,
                None,
                "direct-after-refresh",
            )
            .await
            .expect("direct ticket survives atomic replacement");
        assert_eq!(
            runtime
                .read_ticket_range(&refreshed_ticket.ticket_id, None)
                .await
                .expect("read refreshed ticket")
                .bytes,
            b"%PDF-1.7 revision two"
        );
        assert_eq!(
            runtime
                .issue_ticket(
                    &alias.resource_id,
                    &alias.subscription_id,
                    event.revision,
                    None,
                    "alias-after-refresh",
                )
                .await
                .expect_err("retargeted alias remains revoked after direct refresh")
                .code(),
            "unauthorized_path"
        );

        runtime
            .close(&alias.subscription_id)
            .await
            .expect("close alias subscription");
        assert_eq!(runtime.watcher_count().await, 1);
        runtime
            .close(&alias_after_dedup.subscription_id)
            .await
            .expect("close deduplicated alias subscription");
        assert_eq!(runtime.watcher_count().await, 1);
        runtime
            .close(&direct.subscription_id)
            .await
            .expect("close direct subscription");
        assert_eq!(runtime.watcher_count().await, 0);
    }

    #[tokio::test]
    async fn concurrent_alias_and_direct_first_opens_retain_both_authorizations() {
        let temp = tempfile::tempdir().expect("temp root");
        let canonical_dir = temp.path().join("z-approved");
        let alias_dir = temp.path().join("a-current");
        fs::create_dir(&canonical_dir).expect("canonical directory");
        create_directory_link(&canonical_dir, &alias_dir);
        let canonical_path = canonical_dir.join("shared.txt");
        let alias_path = alias_dir.join("shared.txt");
        fs::write(&canonical_path, "concurrent\n").expect("fixture");
        let config = agent_config("agent-a", temp.path());
        let runtime = test_runtime();
        let first_open_barrier = Arc::new(tokio::sync::Barrier::new(2));
        *runtime.inner.open_after_entry_miss_hook.lock().await = Some(first_open_barrier.clone());

        let alias_task = {
            let runtime = runtime.clone();
            let config = config.clone();
            let alias_path = alias_path.clone();
            tokio::spawn(async move {
                runtime
                    .open_agent_file("agent-a", &config, &alias_path, None)
                    .await
            })
        };
        let direct_task = {
            let runtime = runtime.clone();
            let config = config.clone();
            let canonical_path = canonical_path.clone();
            tokio::spawn(async move {
                runtime
                    .open_agent_file("agent-a", &config, &canonical_path, None)
                    .await
            })
        };
        let (alias_result, direct_result) = timeout(Duration::from_secs(5), async {
            tokio::join!(alias_task, direct_task)
        })
        .await
        .expect("concurrent opens must not deadlock");
        let alias = alias_result
            .expect("alias task")
            .expect("concurrent alias open");
        let direct = direct_result
            .expect("direct task")
            .expect("concurrent direct open");
        *runtime.inner.open_after_entry_miss_hook.lock().await = None;

        assert_eq!(alias.resource_id, direct.resource_id);
        assert_ne!(alias.subscription_id, direct.subscription_id);
        assert_eq!(runtime.watcher_count().await, 1);
        assert_eq!(runtime.subscriber_count(&direct.resource_id).await, 2);

        remove_directory_link(&alias_dir);
        assert_eq!(
            runtime
                .read_text(
                    &alias.resource_id,
                    &alias.subscription_id,
                    alias.revision,
                    Some(&config),
                )
                .await
                .expect_err("concurrent alias subscription retained its own provenance")
                .code(),
            "unauthorized_path"
        );
        assert_eq!(
            runtime
                .read_text(
                    &direct.resource_id,
                    &direct.subscription_id,
                    direct.revision,
                    Some(&config),
                )
                .await
                .expect("concurrent direct subscription retained its own provenance")
                .text,
            "concurrent\n"
        );

        runtime
            .close(&alias.subscription_id)
            .await
            .expect("close alias subscription");
        runtime
            .close(&direct.subscription_id)
            .await
            .expect("close direct subscription");
        assert_eq!(runtime.watcher_count().await, 0);
    }

    #[tokio::test]
    async fn second_open_during_a_write_burst_returns_the_stored_stable_revision() {
        let temp = tempfile::tempdir().expect("temp root");
        let path = temp.path().join("second-open.txt");
        fs::write(&path, "stable\n").expect("fixture");
        let config = agent_config("agent-a", temp.path());
        let runtime = test_runtime();
        let mut events = runtime.subscribe_events();
        let first = runtime
            .open_agent_file("agent-a", &config, &path, None)
            .await
            .expect("first open");

        fs::write(&path, "intermediate\n").expect("intermediate write");
        runtime.schedule_refresh(first.resource_id.clone());
        sleep(Duration::from_millis(50)).await;
        let second = runtime
            .open_agent_file("agent-a", &config, &path, None)
            .await
            .expect("second open");

        assert_eq!(second.revision, first.revision);
        assert_eq!(
            second.descriptor.content_hash,
            first.descriptor.content_hash
        );
        assert!(
            events.try_recv().is_err(),
            "second open promoted an unstable revision"
        );

        fs::write(&path, "stable after burst\n").expect("final write");
        runtime.schedule_refresh(first.resource_id.clone());
        let event = timeout(Duration::from_secs(2), events.recv())
            .await
            .expect("stable event timeout")
            .expect("stable event");
        assert_eq!(event.revision, first.revision + 1);
    }

    #[tokio::test]
    async fn coalesces_write_bursts_into_one_stable_revision() {
        let temp = tempfile::tempdir().expect("temp root");
        let path = temp.path().join("burst.txt");
        fs::write(&path, "initial\n").expect("fixture");
        let config = agent_config("agent-a", temp.path());
        let runtime = test_runtime();
        let mut events = runtime.subscribe_events();
        let subscription = runtime
            .open_agent_file("agent-a", &config, &path, None)
            .await
            .expect("open");

        tokio::fs::write(&path, "first\n").await.expect("write one");
        tokio::fs::write(&path, "second\n")
            .await
            .expect("write two");
        tokio::fs::write(&path, "third\n")
            .await
            .expect("write three");

        let event = timeout(Duration::from_secs(3), events.recv())
            .await
            .expect("stable event timeout")
            .expect("stable event");
        assert_eq!(event.resource_id, subscription.resource_id);
        assert_eq!(event.revision, 2);
        sleep(Duration::from_millis(250)).await;
        assert!(events.try_recv().is_err(), "raw notify burst leaked");
    }

    #[tokio::test]
    async fn same_path_identity_replacement_advances_once_and_refreshes_picker_grant() {
        let temp = tempfile::tempdir().expect("temp root");
        let path = temp.path().join("atomic.txt");
        fs::write(&path, "revision one\n").expect("fixture");
        let runtime = test_runtime();
        let grant = runtime.record_user_file(&path).await.expect("picker grant");
        let subscription = runtime
            .open_user_file(&grant.capability_id, &path, None)
            .await
            .expect("open picker file");
        let mut events = runtime.subscribe_events();

        let replacement = temp.path().join("atomic.replacement");
        fs::write(&replacement, "revision two\n").expect("replacement fixture");
        replace_path_identity(&replacement, &path);
        runtime.schedule_refresh(subscription.resource_id.clone());

        let event = timeout(Duration::from_secs(3), events.recv())
            .await
            .expect("replacement event timeout")
            .expect("replacement event");
        assert_eq!(event.revision, subscription.revision + 1);
        assert_eq!(event.descriptor.unavailable_reason, None);
        assert_eq!(
            runtime
                .read_text(
                    &subscription.resource_id,
                    &subscription.subscription_id,
                    event.revision,
                    None,
                )
                .await
                .expect("replacement read")
                .text,
            "revision two\n"
        );
        sleep(Duration::from_millis(300)).await;
        assert!(events.try_recv().is_err(), "replacement emitted twice");

        let refreshed_grant = runtime
            .inner
            .user_file_grants
            .lock()
            .await
            .get(&grant.capability_id)
            .expect("live grant")
            .authorized
            .clone();
        verified_snapshot(refreshed_grant, runtime.inner.limits.clone())
            .await
            .expect("grant retains the replacement identity");
    }

    #[tokio::test]
    async fn persistent_refresh_failure_is_typed_once_and_recovers() {
        let temp = tempfile::tempdir().expect("temp root");
        let path = temp.path().join("unavailable.txt");
        let moved = temp.path().join("unavailable.moved");
        fs::write(&path, "stable\n").expect("fixture");
        let runtime = test_runtime();
        let grant = runtime.record_user_file(&path).await.expect("picker grant");
        let subscription = runtime
            .open_user_file(&grant.capability_id, &path, None)
            .await
            .expect("open");
        let mut events = runtime.subscribe_events();

        fs::rename(&path, &moved).expect("move file away");
        runtime.schedule_refresh(subscription.resource_id.clone());
        let unavailable = timeout(Duration::from_secs(3), events.recv())
            .await
            .expect("unavailable event timeout")
            .expect("unavailable event");
        assert_eq!(unavailable.revision, 2);
        assert_eq!(
            unavailable.descriptor.unavailable_reason.as_deref(),
            Some("unauthorized_path")
        );
        assert!(!unavailable.descriptor.capabilities.preview);
        assert_eq!(
            runtime
                .read_text(
                    &subscription.resource_id,
                    &subscription.subscription_id,
                    unavailable.revision,
                    None,
                )
                .await
                .expect_err("unavailable text revision must not read the old handle")
                .code(),
            "unauthorized_path"
        );
        assert_eq!(
            runtime
                .issue_ticket(
                    &subscription.resource_id,
                    &subscription.subscription_id,
                    unavailable.revision,
                    None,
                    "unavailable-lease",
                )
                .await
                .expect_err("unavailable revision must not mint a stream ticket")
                .code(),
            "unauthorized_path"
        );

        runtime.schedule_refresh(subscription.resource_id.clone());
        sleep(Duration::from_millis(300)).await;
        assert!(
            events.try_recv().is_err(),
            "identical failure state repeated"
        );

        fs::rename(&moved, &path).expect("restore original identity");
        runtime.schedule_refresh(subscription.resource_id.clone());
        let recovered = timeout(Duration::from_secs(3), events.recv())
            .await
            .expect("recovery event timeout")
            .expect("recovery event");
        assert_eq!(recovered.revision, 3);
        assert_eq!(recovered.descriptor.unavailable_reason, None);
        assert!(recovered.descriptor.capabilities.preview);
    }

    #[tokio::test]
    async fn persistent_unstable_scan_is_typed_once_and_recovers() {
        let temp = tempfile::tempdir().expect("temp root");
        let path = temp.path().join("unstable.txt");
        fs::write(&path, "stable\n").expect("fixture");
        let config = agent_config("agent-a", temp.path());
        let runtime = test_runtime();
        let subscription = runtime
            .open_agent_file("agent-a", &config, &path, None)
            .await
            .expect("open");
        let mut events = runtime.subscribe_events();

        *runtime.inner.forced_refresh_error.lock().await = Some(error(
            "unstable_file",
            "file changed during every descriptor scan attempt",
        ));
        runtime.schedule_refresh(subscription.resource_id.clone());
        let unavailable = timeout(Duration::from_secs(3), events.recv())
            .await
            .expect("unstable event timeout")
            .expect("unstable event");
        assert_eq!(unavailable.revision, 2);
        assert_eq!(
            unavailable.descriptor.unavailable_reason.as_deref(),
            Some("unstable_file")
        );
        assert_eq!(
            runtime
                .read_text(
                    &subscription.resource_id,
                    &subscription.subscription_id,
                    unavailable.revision,
                    Some(&config),
                )
                .await
                .expect_err("unstable text revision must reject reads")
                .code(),
            "unstable_file"
        );
        assert_eq!(
            runtime
                .issue_ticket(
                    &subscription.resource_id,
                    &subscription.subscription_id,
                    unavailable.revision,
                    Some(&config),
                    "unstable-lease",
                )
                .await
                .expect_err("unstable revision must reject stream tickets")
                .code(),
            "unstable_file"
        );

        *runtime.inner.forced_refresh_error.lock().await = Some(error(
            "unstable_file",
            "file changed during every descriptor scan attempt",
        ));
        runtime.schedule_refresh(subscription.resource_id.clone());
        sleep(Duration::from_millis(300)).await;
        assert!(
            events.try_recv().is_err(),
            "identical unstable state repeated"
        );

        runtime.schedule_refresh(subscription.resource_id.clone());
        let recovered = timeout(Duration::from_secs(3), events.recv())
            .await
            .expect("unstable recovery timeout")
            .expect("unstable recovery event");
        assert_eq!(recovered.revision, 3);
        assert_eq!(recovered.descriptor.unavailable_reason, None);
    }

    #[tokio::test]
    async fn debounce_waits_for_stability_after_the_last_separated_write() {
        use std::future::{poll_fn, Future};
        use std::pin::Pin;
        use std::task::Poll;
        use tokio::sync::broadcast::error::TryRecvError;
        use tokio::time::{advance, Instant as TokioInstant};

        async fn poll_once<F: Future>(mut future: Pin<&mut F>) -> Poll<F::Output> {
            // A depleted task budget must not masquerade as waiting on the timer.
            tokio::task::unconstrained(poll_fn(|cx| Poll::Ready(future.as_mut().poll(cx)))).await
        }

        let temp = tempfile::tempdir().expect("temp root");
        let path = temp.path().join("separated.txt");
        fs::write(&path, "initial\n").expect("fixture");
        let config = agent_config("agent-a", temp.path());
        let runtime = test_runtime();
        runtime
            .inner
            .watcher_refresh_enabled
            .store(false, Ordering::Release);
        *runtime.inner.debounce_wait_probe.lock().expect("probe") =
            Some(DebounceWaitProbe::default());
        let mut events = runtime.subscribe_events();
        let subscription = runtime
            .open_agent_file("agent-a", &config, &path, None)
            .await
            .expect("open");
        let incarnation_id =
            runtime.inner.entries.lock().await[&subscription.resource_id].incarnation_id;
        tokio::time::pause();
        let started = TokioInstant::now();
        let mut refreshes = Vec::new();

        for (index, content) in ["first\n", "second\n", "third\n"].into_iter().enumerate() {
            if index > 0 {
                advance(Duration::from_millis(75)).await;
            }
            let elapsed = Duration::from_millis(index as u64 * 75);
            assert_eq!(TokioInstant::now() - started, elapsed);
            fs::write(&path, content).expect("write");
            let mut refresh = Box::pin(
                runtime.refresh_after_stability(subscription.resource_id.clone(), incarnation_id),
            );
            assert!(poll_once(refresh.as_mut()).await.is_pending());
            {
                let probe = runtime.inner.debounce_wait_probe.lock().expect("probe");
                let probe = probe.as_ref().expect("enabled probe");
                assert_eq!(
                    probe.armed.len(),
                    index + 1,
                    "timer registration acknowledged"
                );
                assert_eq!(
                    probe.armed[index],
                    (
                        index as u64 + 1,
                        started + elapsed + Duration::from_millis(150)
                    ),
                    "each generation must arm the actual full stability timer"
                );
                assert!(probe.completed.is_empty(), "an armed timer completed early");
            }
            assert!(matches!(events.try_recv(), Err(TryRecvError::Empty)));
            refreshes.push(refresh);
        }

        // Finish each obsolete generation after its own deadline. One extra
        // millisecond permits timer granularity without crossing the latest deadline.
        advance(Duration::from_millis(1)).await;
        refreshes.remove(0).await;
        assert_eq!(runtime.inner.refresh_scan_count.load(Ordering::Acquire), 0);
        assert_eq!(TokioInstant::now() - started, Duration::from_millis(151));
        assert!(matches!(events.try_recv(), Err(TryRecvError::Empty)));
        advance(Duration::from_millis(75)).await;
        refreshes.remove(0).await;
        assert_eq!(runtime.inner.refresh_scan_count.load(Ordering::Acquire), 0);
        assert_eq!(TokioInstant::now() - started, Duration::from_millis(226));
        assert!(matches!(events.try_recv(), Err(TryRecvError::Empty)));

        // At 149 ms after the third write, explicitly poll the latest future.
        // The probe distinguishes the real timer wait from later snapshot I/O.
        advance(Duration::from_millis(73)).await;
        let mut latest = refreshes.pop().expect("latest generation");
        assert!(poll_once(latest.as_mut()).await.is_pending());
        assert_eq!(TokioInstant::now() - started, Duration::from_millis(299));
        {
            let probe = runtime.inner.debounce_wait_probe.lock().expect("probe");
            assert_eq!(probe.as_ref().expect("enabled probe").completed, vec![1, 2]);
        }
        assert_eq!(runtime.inner.refresh_scan_count.load(Ordering::Acquire), 0);
        assert_eq!(
            runtime.inner.entries.lock().await[&subscription.resource_id].revision,
            1
        );
        assert!(matches!(events.try_recv(), Err(TryRecvError::Empty)));
        assert_eq!(TokioInstant::now() - started, Duration::from_millis(299));

        // Await actual authorization, snapshot and publication, not just timer expiry.
        advance(Duration::from_millis(2)).await;
        latest.await;
        assert_eq!(TokioInstant::now() - started, Duration::from_millis(301));
        {
            let probe = runtime.inner.debounce_wait_probe.lock().expect("probe");
            assert_eq!(
                probe.as_ref().expect("enabled probe").completed,
                vec![1, 2, 3]
            );
        }
        let event = events.try_recv().expect("stable revision was published");
        assert_eq!(event.resource_id, subscription.resource_id);
        assert_eq!(event.revision, 2);
        assert_eq!(runtime.inner.refresh_scan_count.load(Ordering::Acquire), 1);
        assert_eq!(
            runtime
                .read_text(
                    &subscription.resource_id,
                    &subscription.subscription_id,
                    event.revision,
                    Some(&config),
                )
                .await
                .expect("latest stable text")
                .text,
            "third\n"
        );
        assert!(matches!(events.try_recv(), Err(TryRecvError::Empty)));
        runtime
            .close(&subscription.subscription_id)
            .await
            .expect("close");
        assert_eq!(runtime.watcher_count().await, 0);
    }

    #[tokio::test]
    async fn old_incarnation_cannot_refresh_a_closed_and_reopened_resource() {
        let temp = tempfile::tempdir().expect("temp root");
        let path = temp.path().join("aba.txt");
        fs::write(&path, "first incarnation\n").expect("fixture");
        let config = agent_config("agent-a", temp.path());
        let runtime = test_runtime();
        let first = runtime
            .open_agent_file("agent-a", &config, &path, None)
            .await
            .expect("first open");
        let old_incarnation = runtime
            .inner
            .entries
            .lock()
            .await
            .get(&first.resource_id)
            .expect("first entry")
            .incarnation_id;
        runtime
            .close(&first.subscription_id)
            .await
            .expect("close first");

        fs::write(&path, "second incarnation\n").expect("reopen content");
        let second = runtime
            .open_agent_file("agent-a", &config, &path, None)
            .await
            .expect("second open");
        fs::write(&path, "unstable replacement\n").expect("unstable content");

        runtime
            .refresh_if_stable(&second.resource_id, old_incarnation, 0)
            .await;
        let current = runtime
            .snapshot(&second.resource_id)
            .await
            .expect("current snapshot");
        assert_eq!(current.revision, 1);
        assert_eq!(
            current.descriptor.content_hash,
            second.descriptor.content_hash
        );
    }

    #[tokio::test]
    async fn unchanged_content_hash_does_not_advance_revision() {
        let temp = tempfile::tempdir().expect("temp root");
        let path = temp.path().join("same.txt");
        fs::write(&path, "same bytes\n").expect("fixture");
        let config = agent_config("agent-a", temp.path());
        let runtime = test_runtime();
        let mut events = runtime.subscribe_events();
        let subscription = runtime
            .open_agent_file("agent-a", &config, &path, None)
            .await
            .expect("open");

        tokio::fs::write(&path, "same bytes\n")
            .await
            .expect("rewrite");

        assert!(
            timeout(Duration::from_millis(500), events.recv())
                .await
                .is_err(),
            "unchanged content emitted a revision"
        );
        let current = runtime
            .snapshot(&subscription.resource_id)
            .await
            .expect("snapshot");
        assert_eq!(current.revision, 1);
    }

    #[tokio::test]
    async fn text_reads_are_revision_bound_and_revocation_is_rechecked() {
        let temp = tempfile::tempdir().expect("temp root");
        let allowed = temp.path().join("allowed");
        let revoked = temp.path().join("revoked");
        fs::create_dir_all(&allowed).expect("allowed root");
        fs::create_dir_all(&revoked).expect("revoked root");
        let path = allowed.join("report.txt");
        fs::write(&path, "revision one\n").expect("fixture");
        let initial_config = agent_config("agent-a", &allowed);
        let runtime = test_runtime();
        let mut events = runtime.subscribe_events();
        let subscription = runtime
            .open_agent_file("agent-a", &initial_config, &path, None)
            .await
            .expect("open");

        assert_eq!(
            runtime
                .read_text(
                    &subscription.resource_id,
                    &subscription.subscription_id,
                    1,
                    Some(&initial_config),
                )
                .await
                .expect("current read")
                .text,
            "revision one\n"
        );

        tokio::fs::write(&path, "revision two\n")
            .await
            .expect("rewrite");
        timeout(Duration::from_secs(3), events.recv())
            .await
            .expect("revision timeout")
            .expect("revision event");

        let stale = runtime
            .read_text(
                &subscription.resource_id,
                &subscription.subscription_id,
                1,
                Some(&initial_config),
            )
            .await
            .expect_err("stale revision must fail");
        assert_eq!(stale.code(), "stale_revision");

        let revoked_config = agent_config("agent-a", &revoked);
        let revoked = runtime
            .read_text(
                &subscription.resource_id,
                &subscription.subscription_id,
                2,
                Some(&revoked_config),
            )
            .await
            .expect_err("revoked root must fail");
        assert_eq!(revoked.code(), "unauthorized_path");
    }

    #[tokio::test]
    async fn oversized_resources_open_as_metadata_and_reject_reads_until_recovery() {
        let temp = tempfile::tempdir().expect("temp root");
        let limits = FileResourceLimits {
            monaco_max_size_bytes: 64,
            monaco_max_line_count: u64::MAX,
            diff_max_size_bytes_per_side: 64,
            diff_max_line_count: u64::MAX,
            image_max_size_bytes: 96,
            image_max_pixels: u64::MAX,
            pdf_max_size_bytes: 128,
        };
        let mut runtime = test_runtime();
        Arc::get_mut(&mut runtime.inner)
            .expect("unshared test runtime")
            .limits = limits.clone();
        let config = agent_config("agent-a", temp.path());
        let png = b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR\x00\x00\x00\x01\x00\x00\x00\x01";
        let mut image = png.to_vec();
        image.resize(limits.image_max_size_bytes as usize + 1, b'a');
        let mut pdf = b"%PDF-1.7\n".to_vec();
        pdf.resize(limits.pdf_max_size_bytes as usize + 1, b'a');
        let fixtures = [
            (
                "oversized.txt",
                vec![b'a'; limits.monaco_max_size_bytes as usize + 1],
                "monaco_size_limit_exceeded",
            ),
            ("oversized.png", image, "image_limit_exceeded"),
            ("oversized.pdf", pdf, "pdf_size_limit_exceeded"),
        ];

        for (name, bytes, reason) in fixtures {
            let path = temp.path().join(name);
            fs::write(&path, bytes).expect("oversized fixture");
            let snapshot = runtime
                .open_agent_file("agent-a", &config, &path, None)
                .await
                .expect("oversized resource opens as metadata");

            assert_eq!(snapshot.revision, 1, "{name}");
            assert_eq!(
                snapshot.descriptor.unavailable_reason.as_deref(),
                Some(reason),
                "{name}"
            );
            assert!(
                snapshot
                    .descriptor
                    .content_hash
                    .starts_with("bounded-sha256:"),
                "{name}"
            );
            assert!(!snapshot.descriptor.capabilities.preview, "{name}");
            assert!(!snapshot.descriptor.capabilities.changes, "{name}");
            assert!(!snapshot.descriptor.capabilities.draft, "{name}");
            assert!(!snapshot.descriptor.capabilities.stream, "{name}");
            assert_eq!(
                runtime
                    .read_text(
                        &snapshot.resource_id,
                        &snapshot.subscription_id,
                        snapshot.revision,
                        Some(&config),
                    )
                    .await
                    .expect_err("metadata-only revision must reject text reads")
                    .code(),
                reason,
                "{name}"
            );
            assert_eq!(
                runtime
                    .issue_ticket(
                        &snapshot.resource_id,
                        &snapshot.subscription_id,
                        snapshot.revision,
                        Some(&config),
                        &format!("oversized-{name}"),
                    )
                    .await
                    .expect_err("metadata-only revision must reject tickets")
                    .code(),
                reason,
                "{name}"
            );
        }

        let text_path = temp.path().join("oversized.txt");
        let text_resource = file_resource_id(
            std::fs::canonicalize(&text_path)
                .expect("canonical oversized text")
                .to_string_lossy()
                .as_ref(),
        );
        let mut events = runtime.subscribe_events();
        runtime.schedule_refresh(text_resource.clone());
        sleep(Duration::from_millis(300)).await;
        assert!(
            events.try_recv().is_err(),
            "unchanged bounded fingerprint emitted"
        );

        fs::write(
            &text_path,
            vec![b'b'; limits.monaco_max_size_bytes as usize + 1],
        )
        .expect("same-size oversized rewrite");
        runtime.schedule_refresh(text_resource.clone());
        let changed = timeout(Duration::from_secs(3), events.recv())
            .await
            .expect("oversized revision timeout")
            .expect("oversized revision event");
        assert_eq!(changed.revision, 2);
        assert_eq!(
            changed.descriptor.unavailable_reason.as_deref(),
            Some("monaco_size_limit_exceeded")
        );

        fs::write(&text_path, b"recovered\n").expect("recover within renderer limit");
        runtime.schedule_refresh(text_resource);
        let recovered = timeout(Duration::from_secs(3), events.recv())
            .await
            .expect("recovery timeout")
            .expect("recovery event");
        assert_eq!(recovered.revision, 3);
        assert_eq!(recovered.descriptor.unavailable_reason, None);
        assert!(recovered.descriptor.content_hash.starts_with("sha256:"));
        assert!(recovered.descriptor.capabilities.preview);
    }

    #[tokio::test]
    async fn ticket_is_exact_repeatable_range_scoped_and_expires() {
        let temp = tempfile::tempdir().expect("temp root");
        let first_path = temp.path().join("first.pdf");
        let second_path = temp.path().join("second.pdf");
        fs::write(&first_path, b"%PDF-1.7 first payload").expect("first fixture");
        fs::write(&second_path, b"%PDF-1.7 second payload").expect("second fixture");
        let config = agent_config("agent-a", temp.path());
        let runtime =
            FileResourceRuntime::with_timing(Duration::from_millis(50), Duration::from_millis(250));
        let first = runtime
            .open_agent_file("agent-a", &config, &first_path, None)
            .await
            .expect("first open");
        let second = runtime
            .open_agent_file("agent-a", &config, &second_path, None)
            .await
            .expect("second open");

        let mismatched = runtime
            .issue_ticket(
                &second.resource_id,
                &first.subscription_id,
                second.revision,
                Some(&config),
                "renderer-lease-a",
            )
            .await
            .expect_err("subscription cannot issue for another resource");
        assert_eq!(mismatched.code(), "unauthorized_resource");

        let ticket = runtime
            .issue_ticket_for_webview(
                &first.resource_id,
                &first.subscription_id,
                first.revision,
                Some(&config),
                "renderer-lease-a",
                Some("main"),
            )
            .await
            .expect("ticket");
        assert_eq!(ticket.renderer_lease_id, "renderer-lease-a");
        assert!(ticket.url.starts_with("wardian-resource://"));

        let reused_lease = runtime
            .issue_ticket_for_webview(
                &second.resource_id,
                &second.subscription_id,
                second.revision,
                Some(&config),
                "renderer-lease-a",
                Some("main"),
            )
            .await
            .expect_err("one renderer lease cannot cross subscriptions");
        assert_eq!(reused_lease.code(), "unauthorized_ticket");
        assert_eq!(runtime.ticket_count().await, 1);

        let wrong_webview = runtime
            .read_ticket_range_for_webview(&ticket.ticket_id, Some("bytes=0-3"), Some("secondary"))
            .await
            .expect_err("ticket must remain renderer-webview scoped");
        assert_eq!(wrong_webview.code(), "unauthorized_ticket");

        let first_range = runtime
            .read_ticket_range_for_webview(&ticket.ticket_id, Some("bytes=0-3"), Some("main"))
            .await
            .expect("first range");
        let repeated_range = runtime
            .read_ticket_range_for_webview(&ticket.ticket_id, Some("bytes=0-3"), Some("main"))
            .await
            .expect("repeated range");
        assert_eq!(first_range.bytes, b"%PDF");
        assert_eq!(repeated_range.bytes, first_range.bytes);
        assert_eq!(first_range.mime_type, "application/pdf");

        timeout(Duration::from_secs(2), async {
            loop {
                if runtime.ticket_count().await == 0
                    && runtime.renderer_lease_count().await == 0
                    && runtime.ticket_snapshot_bytes_in_use() == 0
                {
                    break;
                }
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("abandoned ticket state must be reclaimed at its deadline");
        let expired = runtime
            .read_ticket_range_for_webview(&ticket.ticket_id, Some("bytes=0-3"), Some("main"))
            .await
            .expect_err("expired ticket must fail");
        assert_eq!(expired.code(), "invalid_ticket");
    }

    #[tokio::test]
    async fn ticket_serves_its_immutable_revision_after_source_changes_and_is_revoked_on_close() {
        let temp = tempfile::tempdir().expect("temp root");
        let path = temp.path().join("lease.pdf");
        fs::write(&path, b"%PDF-1.7 lease payload").expect("fixture");
        let config = agent_config("agent-a", temp.path());
        let runtime = test_runtime();
        let subscription = runtime
            .open_agent_file("agent-a", &config, &path, None)
            .await
            .expect("open");
        let ticket = runtime
            .issue_ticket_for_webview(
                &subscription.resource_id,
                &subscription.subscription_id,
                subscription.revision,
                Some(&config),
                "lease-a",
                Some("main"),
            )
            .await
            .expect("ticket");

        fs::write(&path, b"%PDF-1.7 other payload").expect("mutate");
        assert_eq!(
            runtime
                .read_ticket_range_for_webview(&ticket.ticket_id, Some("bytes=9-13"), Some("main"))
                .await
                .expect("ticket retains the issued revision")
                .bytes,
            b"lease"
        );
        assert!(runtime
            .verify_ticket_range_for_webview(&ticket.ticket_id, Some("bytes=9-13"), Some("main"),)
            .await
            .expect("HEAD validates immutable snapshot")
            .bytes
            .is_empty());
        fs::remove_file(&path).expect("remove source after issuance");
        assert_eq!(
            runtime
                .read_ticket_range_for_webview(&ticket.ticket_id, Some("bytes=9-13"), Some("main"))
                .await
                .expect("range never rereads the removed source")
                .bytes,
            b"lease"
        );

        runtime
            .close(&subscription.subscription_id)
            .await
            .expect("close subscription");
        assert_eq!(
            runtime
                .read_ticket_range_for_webview(&ticket.ticket_id, Some("bytes=0-3"), Some("main"))
                .await
                .expect_err("closed subscription must revoke its lease")
                .code(),
            "invalid_ticket"
        );
    }

    #[tokio::test]
    async fn ticket_snapshot_disk_budget_is_bounded_and_released_with_the_lease() {
        let temp = tempfile::tempdir().expect("temp root");
        let first_path = temp.path().join("first.pdf");
        let second_path = temp.path().join("second.pdf");
        let first_bytes = b"%PDF-1.7 first payload";
        let second_bytes = b"%PDF-1.7 second payload";
        fs::write(&first_path, first_bytes).expect("first fixture");
        fs::write(&second_path, second_bytes).expect("second fixture");
        let config = agent_config("agent-a", temp.path());
        let runtime = FileResourceRuntime::with_test_limits(
            Duration::from_millis(50),
            Duration::from_secs(60),
            8,
            MIN_TICKET_SNAPSHOT_RESERVATION_BYTES,
        );
        let first = runtime
            .open_agent_file("agent-a", &config, &first_path, None)
            .await
            .expect("first open");
        let second = runtime
            .open_agent_file("agent-a", &config, &second_path, None)
            .await
            .expect("second open");
        runtime
            .issue_ticket(
                &first.resource_id,
                &first.subscription_id,
                first.revision,
                Some(&config),
                "first-lease",
            )
            .await
            .expect("first ticket");
        assert_eq!(
            runtime.ticket_snapshot_bytes_in_use(),
            MIN_TICKET_SNAPSHOT_RESERVATION_BYTES
        );

        assert_eq!(
            runtime
                .issue_ticket(
                    &second.resource_id,
                    &second.subscription_id,
                    second.revision,
                    Some(&config),
                    "second-lease",
                )
                .await
                .expect_err("snapshot budget must reject another file")
                .code(),
            "ticket_capacity_exceeded"
        );
        runtime
            .close_renderer_lease(
                &first.resource_id,
                &first.subscription_id,
                "first-lease",
                None,
            )
            .await
            .expect("release first lease");
        assert_eq!(runtime.ticket_snapshot_bytes_in_use(), 0);
        runtime
            .issue_ticket(
                &second.resource_id,
                &second.subscription_id,
                second.revision,
                Some(&config),
                "second-lease",
            )
            .await
            .expect("released budget admits another snapshot");
    }

    #[tokio::test]
    async fn picker_grants_survive_runtime_relaunch_without_persisting_capability_ids() {
        let temp = tempfile::tempdir().expect("temp root");
        let selected_path = temp.path().join("selected.txt");
        let sibling_path = temp.path().join("sibling.txt");
        let grant_store = temp.path().join("settings").join("file-grants.json");
        fs::write(&selected_path, "selected\n").expect("selected fixture");
        fs::write(&sibling_path, "sibling\n").expect("sibling fixture");

        let first_runtime = FileResourceRuntime::default();
        first_runtime.configure_user_file_grant_store_for_test(grant_store.clone());
        let grant = first_runtime
            .record_user_file(&selected_path)
            .await
            .expect("record durable picker grant");
        let persisted = fs::read_to_string(&grant_store).expect("read durable grant store");
        assert!(persisted.contains("selected.txt"));
        assert!(!persisted.contains(&grant.capability_id));
        first_runtime.close_all().await;

        let relaunched_runtime = FileResourceRuntime::default();
        relaunched_runtime.configure_user_file_grant_store_for_test(grant_store);
        let reopened = relaunched_runtime
            .open_matching_user_file(&selected_path, None)
            .await
            .expect("restore durable exact grant")
            .expect("selected path remains granted");
        assert!(relaunched_runtime
            .open_matching_user_file(&sibling_path, None)
            .await
            .expect("check sibling")
            .is_none());
        relaunched_runtime
            .close(&reopened.subscription_id)
            .await
            .expect("close restored grant");
    }

    #[cfg(windows)]
    #[test]
    fn windows_resource_ids_drop_extended_length_prefixes() {
        assert_eq!(
            file_resource_id(r"\\?\C:\work\Notes.md"),
            "file:C:/work/Notes.md"
        );
        assert_eq!(
            file_resource_id(r"\\?\UNC\server\share\Notes.md"),
            "file://server/share/Notes.md"
        );
    }

    #[tokio::test]
    async fn picker_grants_are_exact_path_deduplicated_and_lru_bounded() {
        let temp = tempfile::tempdir().expect("temp root");
        let first_path = temp.path().join("first.txt");
        let second_path = temp.path().join("second.txt");
        let third_path = temp.path().join("third.txt");
        for path in [&first_path, &second_path, &third_path] {
            fs::write(path, path.to_string_lossy().as_bytes()).expect("fixture");
        }
        let runtime = FileResourceRuntime::with_test_limits(
            Duration::from_millis(50),
            Duration::from_secs(60),
            2,
            MAX_TICKET_SNAPSHOT_BYTES,
        );
        let first = runtime
            .record_user_file(&first_path)
            .await
            .expect("first grant");
        let duplicate = runtime
            .record_user_file(&first_path)
            .await
            .expect("duplicate grant");
        assert_eq!(duplicate.capability_id, first.capability_id);
        assert_eq!(runtime.user_grant_count().await, 1);

        let second = runtime
            .record_user_file(&second_path)
            .await
            .expect("second grant");
        let active_first = runtime
            .open_user_file(&first.capability_id, &first_path, None)
            .await
            .expect("activate first grant");
        let third = runtime
            .record_user_file(&third_path)
            .await
            .expect("third grant evicts inactive LRU");
        assert_eq!(runtime.user_grant_count().await, 2);
        assert!(
            runtime
                .open_user_file(&second.capability_id, &second_path, None)
                .await
                .is_err(),
            "evicted grant must be revoked"
        );
        let active_third = runtime
            .open_user_file(&third.capability_id, &third_path, None)
            .await
            .expect("new grant remains available");
        runtime
            .snapshot(&active_first.resource_id)
            .await
            .expect("active grant is never evicted");
        assert_eq!(
            runtime
                .record_user_file(&second_path)
                .await
                .expect_err("all-active grant set must reject growth")
                .code(),
            "grant_limit_reached"
        );

        runtime
            .close(&active_first.subscription_id)
            .await
            .expect("close first grant");
        runtime
            .record_user_file(&second_path)
            .await
            .expect("closed LRU slot can be reused");
        assert_eq!(runtime.user_grant_count().await, 2);
        runtime
            .snapshot(&active_third.resource_id)
            .await
            .expect("remaining active grant is retained");
    }

    #[tokio::test]
    async fn active_picker_subscription_cannot_be_evicted_after_membership_interleaving() {
        let temp = tempfile::tempdir().expect("temp root");
        let first_path = temp.path().join("first.txt");
        let second_path = temp.path().join("second.txt");
        fs::write(&first_path, "first\n").expect("first fixture");
        fs::write(&second_path, "second\n").expect("second fixture");
        let runtime = FileResourceRuntime::with_test_limits(
            Duration::from_millis(50),
            Duration::from_secs(60),
            1,
            MAX_TICKET_SNAPSHOT_BYTES,
        );
        let first = runtime
            .record_user_file(&first_path)
            .await
            .expect("first grant");
        let hook = GrantEvictionBeforeLockHook {
            reached: Arc::new(tokio::sync::Barrier::new(2)),
            resume: Arc::new(tokio::sync::Barrier::new(2)),
        };
        *runtime.inner.grant_eviction_before_lock_hook.lock().await = Some(hook.clone());

        let competing_selection = {
            let runtime = runtime.clone();
            let second_path = second_path.clone();
            tokio::spawn(async move { runtime.record_user_file(&second_path).await })
        };
        hook.reached.wait().await;
        let active = runtime
            .open_user_file(&first.capability_id, &first_path, None)
            .await
            .expect("open first grant during eviction window");
        {
            let grants = runtime.inner.user_file_grants.lock().await;
            let grant = grants
                .get(&first.capability_id)
                .expect("active first capability");
            assert_eq!(grant.in_flight_uses, 0);
            assert_eq!(grant.active_subscriptions, 1);
        }
        hook.resume.wait().await;
        let selection_error = competing_selection
            .await
            .expect("competing selection task")
            .expect_err("live subscription must make the only grant ineligible for eviction");
        assert_eq!(selection_error.code(), "grant_limit_reached");
        assert!(
            runtime
                .inner
                .user_file_grants
                .lock()
                .await
                .contains_key(&first.capability_id),
            "authoritative activity must retain the first capability"
        );

        *runtime.inner.grant_eviction_before_lock_hook.lock().await = None;
        runtime
            .close(&active.subscription_id)
            .await
            .expect("close active subscription");
        runtime
            .record_user_file(&second_path)
            .await
            .expect("closed grant becomes evictable");
    }

    #[tokio::test]
    async fn renderer_lease_can_be_released_without_closing_shared_subscription() {
        let temp = tempfile::tempdir().expect("temp root");
        let path = temp.path().join("release.pdf");
        fs::write(&path, b"%PDF-1.7 release payload").expect("fixture");
        let config = agent_config("agent-a", temp.path());
        let runtime = test_runtime();
        let snapshot = runtime
            .open_agent_file("agent-a", &config, &path, None)
            .await
            .expect("open");
        runtime
            .issue_ticket(
                &snapshot.resource_id,
                &snapshot.subscription_id,
                snapshot.revision,
                Some(&config),
                "renderer-release",
            )
            .await
            .expect("ticket");
        runtime
            .close_renderer_lease(
                &snapshot.resource_id,
                &snapshot.subscription_id,
                "renderer-release",
                None,
            )
            .await
            .expect("release");
        runtime
            .close_renderer_lease(
                &snapshot.resource_id,
                &snapshot.subscription_id,
                "renderer-release",
                None,
            )
            .await
            .expect("idempotent release");

        assert_eq!(runtime.ticket_count().await, 0);
        assert_eq!(runtime.ticket_snapshot_bytes_in_use(), 0);
        assert!(runtime.inner.renderer_leases.lock().await.is_empty());
        assert_eq!(runtime.watcher_count().await, 1);
        assert_eq!(
            runtime
                .snapshot(&snapshot.resource_id)
                .await
                .expect("subscription remains open")
                .subscription_id,
            snapshot.subscription_id
        );
    }

    #[tokio::test]
    async fn reissuing_renderer_lease_purges_superseded_tickets() {
        let temp = tempfile::tempdir().expect("temp root");
        let path = temp.path().join("reissue.pdf");
        fs::write(&path, b"%PDF-1.7 reissue payload").expect("fixture");
        let config = agent_config("agent-a", temp.path());
        let runtime = test_runtime();
        let subscription = runtime
            .open_agent_file("agent-a", &config, &path, None)
            .await
            .expect("open");

        let first = runtime
            .issue_ticket(
                &subscription.resource_id,
                &subscription.subscription_id,
                subscription.revision,
                Some(&config),
                "renderer-reissue",
            )
            .await
            .expect("first ticket");
        let second = runtime
            .issue_ticket(
                &subscription.resource_id,
                &subscription.subscription_id,
                subscription.revision,
                Some(&config),
                "renderer-reissue",
            )
            .await
            .expect("replacement ticket");

        assert_eq!(runtime.ticket_count().await, 1);
        assert_eq!(
            runtime
                .read_ticket_range(&first.ticket_id, Some("bytes=0-3"))
                .await
                .expect_err("superseded ticket must be purged")
                .code(),
            "invalid_ticket"
        );
        assert_eq!(
            runtime
                .read_ticket_range(&second.ticket_id, Some("bytes=0-3"))
                .await
                .expect("replacement ticket remains active")
                .bytes,
            b"%PDF"
        );

        runtime
            .close_renderer_lease(
                &subscription.resource_id,
                &subscription.subscription_id,
                "renderer-reissue",
                None,
            )
            .await
            .expect("close replacement lease");
        runtime
            .close_renderer_lease(
                &subscription.resource_id,
                &subscription.subscription_id,
                "renderer-reissue",
                None,
            )
            .await
            .expect("idempotent repeated close");
        assert_eq!(runtime.ticket_count().await, 0);
    }

    #[tokio::test]
    async fn concurrent_same_lease_publication_keeps_the_newer_ticket() {
        let temp = tempfile::tempdir().expect("temp root");
        let path = temp.path().join("concurrent-reissue.pdf");
        fs::write(&path, b"%PDF-1.7 concurrent reissue payload").expect("fixture");
        let config = agent_config("agent-a", temp.path());
        let runtime = test_runtime();
        let subscription = runtime
            .open_agent_file("agent-a", &config, &path, None)
            .await
            .expect("open");
        let hook = TicketPublicationHook {
            pause_once: Arc::new(AtomicBool::new(true)),
            lease_published: Arc::new(tokio::sync::Barrier::new(2)),
            resume_publication: Arc::new(tokio::sync::Barrier::new(2)),
        };
        *runtime.inner.ticket_publication_hook.lock().await = Some(hook.clone());

        let first_runtime = runtime.clone();
        let first_config = config.clone();
        let first_resource_id = subscription.resource_id.clone();
        let first_subscription_id = subscription.subscription_id.clone();
        let revision = subscription.revision;
        let first_issue = tokio::spawn(async move {
            first_runtime
                .issue_ticket(
                    &first_resource_id,
                    &first_subscription_id,
                    revision,
                    Some(&first_config),
                    "renderer-concurrent-reissue",
                )
                .await
        });

        hook.lease_published.wait().await;
        let second_runtime = runtime.clone();
        let second_config = config.clone();
        let second_resource_id = subscription.resource_id.clone();
        let second_subscription_id = subscription.subscription_id.clone();
        let second_issue = tokio::spawn(async move {
            second_runtime
                .issue_ticket(
                    &second_resource_id,
                    &second_subscription_id,
                    revision,
                    Some(&second_config),
                    "renderer-concurrent-reissue",
                )
                .await
        });

        sleep(Duration::from_millis(75)).await;
        assert!(
            !second_issue.is_finished(),
            "same-lease publication must serialize behind the in-flight issue"
        );
        hook.resume_publication.wait().await;

        let first = first_issue
            .await
            .expect("first issuance task")
            .expect("first ticket");
        let second = second_issue
            .await
            .expect("second issuance task")
            .expect("replacement ticket");
        assert_eq!(runtime.ticket_count().await, 1);
        assert_eq!(runtime.renderer_lease_count().await, 1);
        assert_eq!(
            runtime
                .read_ticket_range(&first.ticket_id, Some("bytes=0-3"))
                .await
                .expect_err("older concurrent ticket must be purged")
                .code(),
            "invalid_ticket"
        );
        assert_eq!(
            runtime
                .read_ticket_range(&second.ticket_id, Some("bytes=0-3"))
                .await
                .expect("newer concurrent ticket remains active")
                .bytes,
            b"%PDF"
        );
        *runtime.inner.ticket_publication_hook.lock().await = None;
    }

    #[tokio::test]
    async fn reissuing_after_subscription_close_rolls_back_ticket_and_renderer_lease() {
        let temp = tempfile::tempdir().expect("temp root");
        let path = temp.path().join("publication-race.pdf");
        fs::write(&path, b"%PDF-1.7 publication race").expect("fixture");
        let config = agent_config("agent-a", temp.path());
        let runtime = test_runtime();
        let subscription = runtime
            .open_agent_file("agent-a", &config, &path, None)
            .await
            .expect("open");
        runtime
            .issue_ticket_for_webview(
                &subscription.resource_id,
                &subscription.subscription_id,
                subscription.revision,
                Some(&config),
                "lease-a",
                Some("main"),
            )
            .await
            .expect("initial ticket");
        assert_eq!(runtime.ticket_count().await, 1);
        let hook = IssueTicketAfterValidationHook {
            validation_reached: Arc::new(tokio::sync::Barrier::new(2)),
            resume_publication: Arc::new(tokio::sync::Barrier::new(2)),
        };
        *runtime
            .inner
            .issue_ticket_after_validation_hook
            .lock()
            .await = Some(hook.clone());

        let issuing_runtime = runtime.clone();
        let issuing_config = config.clone();
        let resource_id = subscription.resource_id.clone();
        let subscription_id = subscription.subscription_id.clone();
        let issuance = tokio::spawn(async move {
            issuing_runtime
                .issue_ticket_for_webview(
                    &resource_id,
                    &subscription_id,
                    subscription.revision,
                    Some(&issuing_config),
                    "lease-a",
                    Some("main"),
                )
                .await
        });

        hook.validation_reached.wait().await;
        runtime
            .close(&subscription.subscription_id)
            .await
            .expect("close completes while issuance is paused");
        hook.resume_publication.wait().await;

        let issue_error = issuance
            .await
            .expect("issuance task")
            .expect_err("closed subscription cannot publish a ticket");
        assert_eq!(issue_error.code(), "invalid_ticket");
        assert!(runtime.inner.read_tickets.lock().await.is_empty());
        assert!(runtime.inner.renderer_leases.lock().await.is_empty());

        *runtime
            .inner
            .issue_ticket_after_validation_hook
            .lock()
            .await = None;
        let reopened = runtime
            .open_agent_file("agent-a", &config, &path, None)
            .await
            .expect("reopen");
        runtime
            .issue_ticket_for_webview(
                &reopened.resource_id,
                &reopened.subscription_id,
                reopened.revision,
                Some(&config),
                "lease-a",
                Some("main"),
            )
            .await
            .expect("new subscription reuses renderer lease immediately");
    }

    #[tokio::test]
    async fn application_cleanup_closes_watchers_grants_and_tickets() {
        let temp = tempfile::tempdir().expect("temp root");
        let path = temp.path().join("cleanup.pdf");
        fs::write(&path, b"%PDF-1.7 cleanup payload").expect("fixture");
        let config = agent_config("agent-a", temp.path());
        let runtime = test_runtime();
        let subscription = runtime
            .open_agent_file("agent-a", &config, &path, None)
            .await
            .expect("open");
        runtime
            .issue_ticket(
                &subscription.resource_id,
                &subscription.subscription_id,
                subscription.revision,
                Some(&config),
                "renderer-lease-a",
            )
            .await
            .expect("ticket");
        let save_target = temp.path().join("cleanup-copy.txt");
        runtime
            .record_save_target(&save_target)
            .await
            .expect("save target grant");

        runtime.close_all().await;

        assert_eq!(runtime.watcher_count().await, 0);
        assert_eq!(runtime.ticket_count().await, 0);
        assert_eq!(runtime.user_grant_count().await, 0);
        assert!(runtime.inner.save_target_grants.lock().await.is_empty());
        assert!(runtime.inner.renderer_leases.lock().await.is_empty());
    }

    fn create_directory_link(target: &Path, link: &Path) {
        wardian_core::library::create_directory_link(target, link).expect("directory link");
    }

    fn remove_directory_link(link: &Path) {
        wardian_core::library::remove_existing_deployment(link).expect("remove directory link");
    }

    #[cfg(unix)]
    fn replace_path_identity(replacement: &Path, target: &Path) {
        fs::rename(replacement, target).expect("atomic replacement");
    }

    #[cfg(windows)]
    fn replace_path_identity(replacement: &Path, target: &Path) {
        let prior = target.with_extension("prior");
        fs::rename(target, &prior).expect("move prior identity aside");
        fs::rename(replacement, target).expect("move replacement into target");
        fs::remove_file(prior).expect("remove prior identity");
    }
