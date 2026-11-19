use super::*;
use std::sync::{Arc, Mutex};

#[test]
fn attachment_actions_reject_caller_paths_and_decode_text_only_compatibly() {
    let text_only: RemoteAgentActionRequest = serde_json::from_value(serde_json::json!({
        "action": "send_prompt", "target": "agent-1", "prompt": "hello"
    }))
    .unwrap();
    assert!(text_only.attachment_ids.is_empty());
    let invalid = serde_json::json!({
        "action": "send_prompt", "target": "agent-1", "prompt": "hello",
        "attachment_paths": ["<host-path>"]
    });
    assert!(serde_json::from_value::<RemoteAgentActionRequest>(invalid).is_err());
}

fn agent(workspace: &std::path::Path) -> crate::state::ActiveAgent {
    crate::state::ActiveAgent {
        config: Arc::new(Mutex::new(wardian_core::models::AgentConfig {
            session_id: "agent-1".to_string(),
            folder: workspace.to_string_lossy().to_string(),
            ..Default::default()
        })),
        child_process: None,
        background_processes: Vec::new(),
        memory_capability: None,
        runtime_generation: None,
        process_id: None,
        query_count: Arc::new(Mutex::new(0)),
        init_timestamp: Arc::new(Mutex::new(None)),
        last_query_timestamp: Arc::new(Mutex::new(None)),
        current_status: Arc::new(Mutex::new("Idle".to_string())),
        last_status_at: Arc::new(Mutex::new(None)),
        watch_state: Arc::new(Mutex::new(crate::state::AgentWatchState::new(
            "agent-1".to_string(),
            32,
            4096,
        ))),
        terminal_title: Arc::new(Mutex::new(String::new())),
        last_output_at: Arc::new(Mutex::new(None)),
        log_path: Arc::new(Mutex::new(None)),
        log_last_modified: Arc::new(Mutex::new(None)),
        #[cfg(windows)]
        job_object: None,
    }
}

fn request(method: Method, uri: &str, session: &RemoteSessionRecord, body: Body) -> Request<Body> {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .body(body)
        .unwrap();
    *request.headers_mut() = action_headers(session);
    request.headers_mut().insert(
        "x-wardian-filename",
        HeaderValue::from_static("evidence.txt"),
    );
    request
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static("text/plain"));
    request
}

#[tokio::test]
async fn attachment_http_boundary_authenticates_before_body_and_enforces_ownership() {
    let temp = tempfile::tempdir().unwrap();
    let _home = WardianHomeEnvGuard::set_async(temp.path()).await;
    crate::remote::storage::save_remote_config_at(temp.path(), &config()).unwrap();
    seed_active_remote_devices(temp.path(), &["device-1", "device-2"]);
    let app = tauri::test::mock_app();
    app.manage(AppState::new());
    let state = app.state::<AppState>();
    state
        .agents
        .lock()
        .await
        .insert("agent-1".to_string(), agent(temp.path()));
    let (session, other) = {
        let mut runtime = state.remote_runtime.lock().await;
        let now = chrono::Utc::now().timestamp_millis();
        (
            crate::remote::auth::create_session(&mut runtime, "device-1", now),
            crate::remote::auth::create_session(&mut runtime, "device-2", now),
        )
    };
    let ctx = RemoteGatewayContext {
        app: app.handle().clone(),
        config: config(),
    };
    let router = Router::new()
        .route(
            "/remote/api/agents/{session_id}/attachments/{upload_id}",
            post(attachments::upload)
                .get(attachments::status)
                .delete(attachments::remove)
                .layer(DefaultBodyLimit::disable()),
        )
        .with_state(ctx);
    let id = uuid::Uuid::new_v4();
    let uri = format!("/remote/api/agents/agent-1/attachments/{id}");
    // A body which panics if polled proves ordering through Axum's actual extractor.
    for invalid in ["origin", "csrf", "cookie", "host"] {
        let stream = futures_util::stream::poll_fn(
            |_| -> std::task::Poll<Option<Result<Bytes, std::io::Error>>> {
                panic!("unauthorized body must not be consumed")
            },
        );
        let mut req = request(Method::POST, &uri, &session, Body::from_stream(stream));
        match invalid {
            "origin" => {
                req.headers_mut().insert(
                    header::ORIGIN,
                    HeaderValue::from_static("https://evil.example"),
                );
            }
            "csrf" => {
                req.headers_mut().remove(REMOTE_CSRF_HEADER_NAME);
            }
            "cookie" => {
                req.headers_mut().remove(header::COOKIE);
            }
            _ => {
                req.headers_mut()
                    .insert(header::HOST, HeaderValue::from_static("evil.example"));
            }
        }
        let response = router.clone().oneshot(req).await.unwrap();
        assert!(matches!(
            response.status(),
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
        ));
    }
    let upload = router
        .clone()
        .oneshot(request(
            Method::POST,
            &uri,
            &session,
            Body::from("marker-1753"),
        ))
        .await
        .unwrap();
    assert_eq!(upload.status(), StatusCode::OK);
    assert_eq!(upload.headers()[header::CACHE_CONTROL], "no-store");
    let bytes = to_bytes(upload.into_body(), 4096).await.unwrap();
    let metadata: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(metadata["upload_id"], id.to_string());
    assert_eq!(metadata["size_bytes"], 11);
    assert!(metadata["expires_at"].as_str().unwrap().contains('T'));
    assert!(!String::from_utf8_lossy(&bytes).contains(&temp.path().to_string_lossy().to_string()));
    let stored = temp
        .path()
        .join(format!(".wardian/attachments/remote/{id}/{id}.txt"));
    assert_eq!(std::fs::read(&stored).unwrap(), b"marker-1753");
    let mut browser_get = request(Method::GET, &uri, &session, Body::empty());
    browser_get.headers_mut().remove(header::ORIGIN);
    assert_eq!(
        router.clone().oneshot(browser_get).await.unwrap().status(),
        StatusCode::OK
    );
    for (header_name, value) in [
        (header::ORIGIN, "https://evil.example"),
        (
            header::HeaderName::from_static("sec-fetch-site"),
            "cross-site",
        ),
    ] {
        let mut forbidden = request(Method::GET, &uri, &session, Body::empty());
        forbidden
            .headers_mut()
            .insert(header_name, HeaderValue::from_static(value));
        assert_eq!(
            router.clone().oneshot(forbidden).await.unwrap().status(),
            StatusCode::FORBIDDEN
        );
    }
    for method in [Method::GET, Method::DELETE] {
        let expected = if method == Method::DELETE {
            StatusCode::OK
        } else {
            StatusCode::NOT_FOUND
        };
        let response = router
            .clone()
            .oneshot(request(method, &uri, &other, Body::empty()))
            .await
            .unwrap();
        assert_eq!(response.status(), expected);
        assert_eq!(std::fs::read(&stored).unwrap(), b"marker-1753");
    }
    let foreign_target = uri.replace("agent-1", "agent-2");
    assert_eq!(
        router
            .clone()
            .oneshot(request(
                Method::GET,
                &foreign_target,
                &session,
                Body::empty()
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    let retry = router
        .clone()
        .oneshot(request(
            Method::POST,
            &uri,
            &session,
            Body::from("marker-1753"),
        ))
        .await
        .unwrap();
    assert_eq!(retry.status(), StatusCode::OK);
    assert_eq!(
        router
            .clone()
            .oneshot(request(Method::POST, &uri, &session, Body::from("changed")))
            .await
            .unwrap()
            .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        router
            .clone()
            .oneshot(request(Method::DELETE, &uri, &session, Body::empty()))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert!(!stored.exists());
    assert_eq!(
        router
            .clone()
            .oneshot(request(Method::GET, &uri, &session, Body::empty()))
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    let revoked_id = uuid::Uuid::new_v4();
    let revoked_uri = format!("/remote/api/agents/agent-1/attachments/{revoked_id}");
    let revoke_home = temp.path().to_path_buf();
    let stream = futures_util::stream::once(async move {
        crate::remote::storage::update_device_store_at(&revoke_home, |store| {
            store
                .devices
                .iter_mut()
                .find(|device| device.device_id == "device-1")
                .unwrap()
                .revoked_at = Some(chrono::Utc::now().to_rfc3339());
            Ok(())
        })
        .unwrap();
        Ok::<_, std::io::Error>(Bytes::from_static(b"not published"))
    });
    let response = router
        .oneshot(request(
            Method::POST,
            &revoked_uri,
            &session,
            Body::from_stream(stream),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(!temp
        .path()
        .join(format!(".wardian/attachments/remote/{revoked_id}"))
        .exists());
}
