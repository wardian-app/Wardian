//! Typed v2 exchange. No legacy-send fallback, provider launch, or replay.
use super::{build_runtime, require_current_message_origin, send_request};
use std::{io, time::Duration};
use wardian_core::{agent_messaging::AgentMessagingRequest, control::ControlRequest};

/// Send exactly one authenticated messaging request with a hard transport bound.
pub(crate) fn request(request: AgentMessagingRequest) -> io::Result<serde_json::Value> {
    let origin = require_current_message_origin()?;
    let runtime = build_runtime()?;
    runtime.block_on(async {
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
    })
}
