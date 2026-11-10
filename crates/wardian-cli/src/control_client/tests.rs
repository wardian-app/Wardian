use super::*;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use wardian_core::control::MessageOrigin;

fn env_lock() -> std::sync::MutexGuard<'static, ()> {
    crate::test_support::test_env_lock()
}

#[test]
fn current_message_origin_uses_wardian_session_id() {
    let _guard = env_lock();
    std::env::set_var("WARDIAN_SESSION_ID", "source-1");

    assert_eq!(
        current_message_origin(),
        Some(MessageOrigin::WardianAgent {
            session_id: "source-1".to_string()
        })
    );

    std::env::remove_var("WARDIAN_SESSION_ID");
}

#[test]
fn current_message_origin_ignores_blank_session_id() {
    let _guard = env_lock();
    std::env::set_var("WARDIAN_SESSION_ID", "   ");

    assert_eq!(current_message_origin(), None);

    std::env::remove_var("WARDIAN_SESSION_ID");
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
