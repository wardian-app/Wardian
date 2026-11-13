use super::*;

#[tokio::test]
async fn stopped_mock_binding_requires_one_unambiguous_provider_session() {
    let _lock = crate::utils::wardian_test_env_lock_async().await;
    let root = tempfile::tempdir().unwrap();
    let _home = TestHome::set(root.path());
    let fixture = Fixture::new(root.path(), "mock", false).await;
    let id = fixture.config.fresh_provider_session_id.clone().unwrap();
    let roots = source_roots(&fixture.config, None);
    for (fresh, resume, accepted) in [
        (Some(id.clone()), None, true),
        (None, Some(id.clone()), true),
        (Some(id.clone()), Some(id.clone()), true),
        (Some(id.clone()), Some(" ".into()), true),
        (Some(" ".into()), Some(id.clone()), true),
        (
            Some(id.clone()),
            Some(uuid::Uuid::new_v4().to_string()),
            false,
        ),
        (Some("invalid".into()), None, false),
        (None, None, false),
    ] {
        let mut config = fixture.config.clone();
        config.fresh_provider_session_id = fresh;
        config.resume_session = resume;
        assert_eq!(
            validated_source(&config, &fixture.captured, &roots).is_some(),
            accepted
        );
    }
    let mut active = fixture.config.clone();
    active.is_off = false;
    assert!(validated_source(&active, &fixture.captured, &roots).is_none());
    fixture.assert_capture_unchanged();
}

#[tokio::test]
async fn stopped_mock_binding_rejects_nested_and_wrong_named_files() {
    let _lock = crate::utils::wardian_test_env_lock_async().await;
    let root = tempfile::tempdir().unwrap();
    let _home = TestHome::set(root.path());
    for nested in [false, true] {
        let fixture = Fixture::new(root.path(), "mock", false).await;
        let parent = fixture.path.parent().unwrap();
        let target = if nested {
            let directory = parent.join("nested");
            std::fs::create_dir(&directory).unwrap();
            directory.join("mock-transcript.jsonl")
        } else {
            parent.join("other-transcript.jsonl")
        };
        std::fs::rename(&fixture.path, &target).unwrap();
        let mut captured = fixture.captured.clone();
        captured.path = target.to_string_lossy().into_owned();
        assert!(validated_source(
            &fixture.config,
            &captured,
            &source_roots(&fixture.config, None)
        )
        .is_none());
        fixture.assert_capture_unchanged();
    }
}

#[tokio::test]
async fn stopped_mock_binding_requires_complete_bounded_owned_init() {
    let _lock = crate::utils::wardian_test_env_lock_async().await;
    let root = tempfile::tempdir().unwrap();
    let _home = TestHome::set(root.path());
    for mode in [
        "foreign",
        "wrong-type",
        "incomplete",
        "oversized",
        "shorter",
    ] {
        let fixture = Fixture::new(root.path(), "mock", false).await;
        let id = fixture.config.fresh_provider_session_id.as_deref().unwrap();
        let mut header = serde_json::json!({"type":"init","session_id":id});
        if mode == "foreign" {
            header["session_id"] = serde_json::json!(uuid::Uuid::new_v4());
        } else if mode == "wrong-type" {
            header["type"] = serde_json::json!("result");
        } else if mode == "oversized" {
            header["padding"] = serde_json::json!("x".repeat(OWNERSHIP_HEADER_BYTES as usize));
        }
        let raw = format!("{header}{}", if mode == "incomplete" { "" } else { "\n" });
        std::fs::write(&fixture.path, raw).unwrap();
        let mut receipt = acquire_provider_log_batch(
            &fixture.config.session_id,
            "mock",
            &fixture.path,
            &fixture.captured.provider_source_key,
            None,
            true,
        )
        .unwrap()
        .next;
        if mode == "shorter" {
            receipt.committed_offset = std::fs::metadata(&fixture.path).unwrap().len() + 1;
        }
        assert!(
            validated_source(
                &fixture.config,
                &receipt,
                &source_roots(&fixture.config, None)
            )
            .is_none(),
            "{mode}"
        );
        fixture.assert_capture_unchanged();
    }
}

#[tokio::test]
async fn stopped_mock_binding_cannot_cache_after_replacement_during_validation() {
    let _lock = crate::utils::wardian_test_env_lock_async().await;
    let root = tempfile::tempdir().unwrap();
    let _home = TestHome::set(root.path());
    for mode in ["config", "path", "incarnation"] {
        let fixture = Fixture::new(root.path(), "mock", false).await;
        let (entered, observing) = tokio::sync::oneshot::channel();
        let (release, blocked) = std::sync::mpsc::channel();
        let successor_path = root.path().join("successor.jsonl");
        let binding = bind_for_background_using(
            &fixture.state,
            &fixture.config.session_id,
            None,
            None,
            move |config, captured, roots| {
                let candidate = validated_source(config, captured, roots);
                assert!(candidate.is_some());
                entered.send(()).unwrap();
                blocked.recv().unwrap();
                candidate
            },
        );
        let replace = async {
            observing.await.unwrap();
            let mut agents = fixture.state.agents.lock().await;
            let agent = agents.get_mut(&fixture.config.session_id).unwrap();
            match mode {
                "config" => {
                    agent.config.lock().unwrap().fresh_provider_session_id =
                        Some(uuid::Uuid::new_v4().to_string());
                }
                "path" => {
                    agent.log_path = Arc::new(Mutex::new(Some(successor_path.clone())));
                }
                _ => agent.current_status = Arc::new(Mutex::new("Off".into())),
            }
            drop(agents);
            release.send(()).unwrap();
        };
        let (result, ()) = tokio::join!(binding, replace);
        result.unwrap();
        assert_eq!(
            fixture.cached().await,
            (mode == "path").then_some(successor_path)
        );
        fixture.assert_capture_unchanged();
    }
}
