use super::*;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use wardian_core::control::MessageOrigin;

fn env_lock() -> std::sync::MutexGuard<'static, ()> {
    crate::test_support::test_env_lock()
}

struct RestoreOriginEnv(Vec<(&'static str, Option<std::ffi::OsString>)>);

impl RestoreOriginEnv {
    fn set(session_id: Option<&str>, thread_id: Option<&str>) -> Self {
        Self(
            [
                ("WARDIAN_SESSION_ID", session_id),
                ("CODEX_THREAD_ID", thread_id),
            ]
            .into_iter()
            .map(|(key, value)| {
                let previous = std::env::var_os(key);
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
                (key, previous)
            })
            .collect(),
        )
    }
}

impl Drop for RestoreOriginEnv {
    fn drop(&mut self) {
        for (key, value) in &self.0 {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
    }
}

#[test]
fn current_message_origin_uses_wardian_session_id() {
    let _guard = env_lock();
    let _restore = RestoreOriginEnv::set(Some("source-1"), None);

    assert_eq!(
        require_current_message_origin().unwrap(),
        MessageOrigin::WardianAgent {
            session_id: "source-1".to_string(),
            provider_thread_id: None,
        }
    );
    assert_eq!(
        serde_json::to_value(require_current_message_origin().unwrap()).unwrap(),
        serde_json::json!({"kind":"wardian_agent","session_id":"source-1"})
    );
}

#[test]
fn current_message_origin_ignores_blank_session_id() {
    let _guard = env_lock();
    let _restore = RestoreOriginEnv::set(Some("   "), Some("native-thread"));

    assert_eq!(
        require_current_message_origin().unwrap_err().kind(),
        io::ErrorKind::PermissionDenied
    );
}

#[test]
fn cli_message_origin_reads_each_invocations_native_thread() {
    let _guard = env_lock();
    for thread_id in [Some("root-thread"), Some("child-thread"), None, Some("   ")] {
        let _restore = RestoreOriginEnv::set(Some("shared-session"), thread_id);
        assert_eq!(
            require_current_message_origin().unwrap(),
            MessageOrigin::WardianAgent {
                session_id: "shared-session".into(),
                provider_thread_id: thread_id
                    .map(str::trim)
                    .filter(|id| !id.is_empty())
                    .map(str::to_owned),
            }
        );
    }
}

#[test]
fn explicit_message_origin_never_inherits_ambient_thread_id() {
    let _guard = env_lock();
    let _restore = RestoreOriginEnv::set(Some("shared-session"), Some("ambient-root-thread"));
    for thread_id in [None, Some("child-thread".to_owned())] {
        assert_eq!(
            require_message_origin(thread_id.clone()).unwrap(),
            MessageOrigin::WardianAgent {
                session_id: "shared-session".into(),
                provider_thread_id: thread_id,
            }
        );
    }
}

#[test]
fn exchange_json_preserves_backend_error_details() {
    let runtime = build_runtime().unwrap();
    runtime.block_on(async {
        let (mut client, mut server) = tokio::io::duplex(4096);
        tokio::spawn(async move {
            let mut line = String::new();
            let mut reader = BufReader::new(&mut server);
            reader.read_line(&mut line).await.unwrap();
            let stream = reader.get_mut();
            stream
                .write_all(
                    br#"{"schema":1,"error":{"code":"request_failed","message":"message delivery failed","details":{"delivery":[{"uuid":"agent-2","name":"CoderTwo","provider":"claude","runtime_state":"restored_without_sender","delivery_state":"failed","error":{"code":"no_input_channel","message":"missing sender"}}]}}}"#,
                )
                .await
                .unwrap();
            stream.write_all(b"\n").await.unwrap();
        });

        let error = exchange_json(&mut client, ControlRequest::AgentList)
            .await
            .unwrap_err();
        let endpoint_error = error
            .get_ref()
            .and_then(|inner| inner.downcast_ref::<ControlEndpointError>())
            .unwrap();

        assert_eq!(endpoint_error.code(), "request_failed");
        assert_eq!(
            endpoint_error.details().unwrap()["delivery"][0]["runtime_state"],
            "restored_without_sender"
        );
    });
}
