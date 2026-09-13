//! Typed v2 exchange. No legacy-send fallback, provider launch, or replay.
use super::{build_runtime, require_current_message_origin, send_request, ControlEndpointError};
use std::{io, time::Duration};
use wardian_core::{
    agent_messaging::{AgentMessagingRequest, AgentMessagingResponse},
    control::ControlRequest,
};

/// Send exactly one authenticated messaging request with a hard transport bound.
pub(crate) fn request(request: AgentMessagingRequest) -> io::Result<serde_json::Value> {
    let origin = require_current_message_origin()?;
    let expected = request.clone();
    let runtime = build_runtime()?;
    let value = runtime.block_on(async {
        tokio::time::timeout(
            Duration::from_secs(61), // Allows the bounded 60s receive to return its timeout receipt.
            send_request(ControlRequest::AgentMessaging { request, origin }),
        )
        .await
        .map_err(|_| {
            io::Error::new(
                io::ErrorKind::TimedOut,
                "Messaging control response timed out",
            )
        })?
    })?;
    validate_receipt(&expected, &value)?;
    Ok(value)
}

/// Preserve backend IDs exactly; an invalid response is uncertainty, not permission to resend.
fn validate_receipt(request: &AgentMessagingRequest, value: &serde_json::Value) -> io::Result<()> {
    let invalid = || {
        io::Error::other(ControlEndpointError::new(
            "invalid_receipt",
            "Invalid canonical messaging receipt; do not replay automatically",
        ))
    };
    let response: AgentMessagingResponse =
        serde_json::from_value(value.clone()).map_err(|_| invalid())?;
    let valid = match (request, response) {
        (
            AgentMessagingRequest::SendMessage { .. },
            AgentMessagingResponse::SendMessage { interaction_id, .. },
        ) => !interaction_id.trim().is_empty(),
        (
            AgentMessagingRequest::FollowupTask { .. },
            AgentMessagingResponse::FollowupTask { request_id, .. },
        ) => !request_id.trim().is_empty(),
        (
            AgentMessagingRequest::Reply {
                request_id: expected,
                ..
            },
            AgentMessagingResponse::Reply {
                request_id,
                interaction_id,
                ..
            },
        ) => *expected == request_id && !interaction_id.trim().is_empty(),
        (
            AgentMessagingRequest::ReceiveMessages { .. },
            AgentMessagingResponse::ReceiveMessages { .. },
        )
        | (AgentMessagingRequest::ListAgents, AgentMessagingResponse::ListAgents { .. })
        | (
            AgentMessagingRequest::InterruptAgent { .. },
            AgentMessagingResponse::InterruptAgent { .. },
        ) => true,
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(invalid())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reply_receipt_must_keep_exact_request_id() {
        let request = AgentMessagingRequest::Reply {
            request_id: "canonical-id".into(),
            status: wardian_core::control::ReplyStatus::Done,
            message: "done".into(),
        };
        for id in ["ask_canonical-id", "another-id", ""] {
            assert!(validate_receipt(&request, &json!({"operation":"reply","request_id":id,"interaction_id":"reply-id","delivery_state":"pending","duplicate":false})).is_err());
        }
        assert!(validate_receipt(&request, &json!({"operation":"reply","request_id":"canonical-id","interaction_id":"reply-id","delivery_state":"pending","duplicate":false})).is_ok());
    }

    #[test]
    fn legacy_and_wrong_operation_receipts_are_rejected() {
        let request = AgentMessagingRequest::FollowupTask {
            target: "Peer".into(),
            message: "task".into(),
            idempotency_key: None,
        };
        assert!(validate_receipt(&request, &json!({"schema":1,"ok":true,"delivery":[]})).is_err());
        assert!(validate_receipt(&request, &json!({"operation":"send_message","interaction_id":"id","delivery_state":"pending","duplicate":false})).is_err());
    }
}
