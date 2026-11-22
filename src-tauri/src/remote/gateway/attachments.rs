//! PWA-only binary boundary. Never expose stored paths over HTTP.
use super::*;
use crate::remote::attachments::{
    AttachmentError, AttachmentOwner, AttachmentStore, DispatchOutcome, SubmissionStatus,
};
use futures_util::StreamExt;
use std::path::PathBuf;
use std::sync::Arc;

const MAX_UPLOAD_BYTES: usize = 10 * 1024 * 1024;

pub(super) async fn store<R: Runtime>(
    ctx: &RemoteGatewayContext<R>,
) -> Result<Arc<AttachmentStore>, RemoteGatewayError> {
    let state = ctx.app.state::<crate::state::AppState>();
    let mut slot = state.remote_attachments.lock().await;
    if let Some(store) = slot.as_ref() {
        return Ok(store.clone());
    }
    let home = crate::utils::get_wardian_home()
        .ok_or_else(|| attachment_error(AttachmentError::DiskFailure))?;
    let opened = blocking(move || AttachmentStore::open(&home)).await?;
    let weak = Arc::downgrade(&opened);
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            let Some(store) = weak.upgrade() else { break };
            let _ = tokio::task::spawn_blocking(move || {
                if let Ok(devices) = crate::remote::storage::load_device_store() {
                    for device in devices
                        .devices
                        .iter()
                        .filter(|device| device.revoked_at.is_some())
                    {
                        let _ = store.revoke_device(&device.device_id);
                    }
                }
                store.cleanup()
            })
            .await;
        }
    });
    *slot = Some(opened.clone());
    Ok(opened)
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, AttachmentError> + Send + 'static,
) -> Result<T, RemoteGatewayError> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|_| attachment_error(AttachmentError::DiskFailure))?
        .map_err(attachment_error)
}

fn attachment_error(error: AttachmentError) -> RemoteGatewayError {
    let status = match error {
        AttachmentError::NotFound => StatusCode::NOT_FOUND,
        AttachmentError::Expired | AttachmentError::SubmissionExpired => StatusCode::GONE,
        AttachmentError::FileTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
        AttachmentError::QuotaExceeded => StatusCode::TOO_MANY_REQUESTS,
        AttachmentError::Conflict | AttachmentError::Busy | AttachmentError::DispatchFenced => {
            StatusCode::CONFLICT
        }
        AttachmentError::DiskFailure => StatusCode::SERVICE_UNAVAILABLE,
        _ => StatusCode::BAD_REQUEST,
    };
    RemoteGatewayError {
        status,
        code: error.code(),
        detail: None,
    }
}

fn owner(session: &RemoteSessionRecord, origin: &str, agent_id: &str) -> AttachmentOwner {
    AttachmentOwner {
        device_id: session.device_id.clone(),
        origin: origin.to_string(),
        agent_id: agent_id.to_string(),
    }
}

pub(super) async fn upload<R: Runtime>(
    State(ctx): State<RemoteGatewayContext<R>>,
    AxumPath(path): AxumPath<AttachmentPath>,
    headers: HeaderMap,
    body: Body,
) -> Result<Response, RemoteGatewayError> {
    let (origin, session) = authorize(&ctx, &headers, true).await?;
    let workspace = target_workspace(&ctx, &path.session_id).await?;
    let name = filename(&headers)?;
    let media_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .filter(|value| value.len() <= 128)
        .ok_or_else(|| RemoteGatewayError::bad_request("unsupported_format"))?
        .to_string();
    let store = store(&ctx).await?;
    let bound_owner = owner(&session, &origin, &path.session_id);
    let upload_id = path.upload_id.clone();
    let reservation = blocking(move || store.reserve(&bound_owner, &workspace, &upload_id)).await?;
    let bytes = read_upload(body, &headers).await?;
    // Revalidation is under the same gates used by revocation/settings changes,
    // so publishing bytes cannot race a completed authority transition.
    let state = ctx.app.state::<crate::state::AppState>();
    let _config = state.remote_gateway_config_lock.lock().await;
    let _device = state.remote_device_store_lock.lock().await;
    authorize(&ctx, &headers, false).await?;
    let metadata = blocking(move || reservation.complete(&name, &media_type, &bytes)).await?;
    audit_gateway_event(
        &session,
        &origin,
        GatewayAuditEvent::accepted("attachment", "upload").target("agent", &path.session_id),
    );
    Ok(no_store(serde_json::json!({
        "upload_id": metadata.upload_id, "name": metadata.name,
        "size_bytes": metadata.size_bytes, "media_type": metadata.media_type,
        "expires_at": millis_to_rfc3339((metadata.expires_at as i64) * 1000),
    })))
}

pub(super) async fn status<R: Runtime>(
    State(ctx): State<RemoteGatewayContext<R>>,
    AxumPath(path): AxumPath<AttachmentPath>,
    headers: HeaderMap,
) -> Result<Response, RemoteGatewayError> {
    let (origin, session) = authorize(&ctx, &headers, false).await?;
    let store = store(&ctx).await?;
    let owner = owner(&session, &origin, &path.session_id);
    let metadata = blocking(move || store.status(&owner, &path.upload_id)).await?;
    Ok(no_store(serde_json::json!({
        "upload_id": metadata.upload_id, "name": metadata.name,
        "size_bytes": metadata.size_bytes, "media_type": metadata.media_type,
        "expires_at": millis_to_rfc3339((metadata.expires_at as i64) * 1000),
    })))
}

pub(super) async fn remove<R: Runtime>(
    State(ctx): State<RemoteGatewayContext<R>>,
    AxumPath(path): AxumPath<AttachmentPath>,
    headers: HeaderMap,
) -> Result<Response, RemoteGatewayError> {
    let (origin, session) = authorize(&ctx, &headers, true).await?;
    let store = store(&ctx).await?;
    let owner = owner(&session, &origin, &path.session_id);
    blocking(move || store.delete(&owner, &path.upload_id)).await?;
    Ok(no_store(serde_json::json!({ "ok": true })))
}

fn submission_response(submission_id: &str, status: SubmissionStatus) -> serde_json::Value {
    let state = match status {
        SubmissionStatus::Prepared | SubmissionStatus::Dispatching => "dispatching",
        SubmissionStatus::Submitted => "accepted",
        SubmissionStatus::SubmittedUnconfirmed => "submitted_unconfirmed",
        SubmissionStatus::FailedBeforeDispatch => "failed",
    };
    serde_json::json!({ "ok": true, "submission_id": submission_id, "state": state })
}

pub(super) async fn submission_status<R: Runtime>(
    State(ctx): State<RemoteGatewayContext<R>>,
    AxumPath(path): AxumPath<SubmissionPath>,
    headers: HeaderMap,
) -> Result<Response, RemoteGatewayError> {
    let (origin, session) = authorize(&ctx, &headers, false).await?;
    let store = store(&ctx).await?;
    let owner = owner(&session, &origin, &path.session_id);
    let id = path.submission_id.clone();
    let status = blocking(move || store.submission_status(&owner, &id)).await?;
    Ok(no_store(submission_response(&path.submission_id, status)))
}

pub(super) async fn send(
    ctx: &RemoteGatewayContext,
    headers: &HeaderMap,
    origin: &str,
    session: &RemoteSessionRecord,
    request: RemoteAgentActionRequest,
) -> Result<serde_json::Value, RemoteGatewayError> {
    if request.action != "send_prompt"
        || request.attachment_ids.is_empty()
        || request.inbox_item_id.is_some()
        || request.input_mode.unwrap_or_default()
            != wardian_core::control::MessageInputMode::Message
    {
        return Err(RemoteGatewayError::bad_request(
            "invalid_attachment_submission",
        ));
    }
    let id = request
        .submission_id
        .ok_or_else(|| RemoteGatewayError::bad_request("submission_id_required"))?;
    let prompt = request.prompt.unwrap_or_default();
    if !prompt.trim().is_empty() && !crate::remote::client_messages::valid_message_text(&prompt) {
        return Err(RemoteGatewayError::bad_request("invalid_request"));
    }
    target_workspace(ctx, &request.target).await?;
    let store = store(ctx).await?;
    let bound_owner = owner(session, origin, &request.target);
    let prepare_store = store.clone();
    let prepare_owner = bound_owner.clone();
    let prepare_id = id.clone();
    let prepared = blocking(move || {
        prepare_store.prepare_submission(
            &prepare_owner,
            &prepare_id,
            &prompt,
            &request.attachment_ids,
        )
    })
    .await?;
    if prepared.status != SubmissionStatus::Prepared {
        return Ok(submission_response(&id, prepared.status));
    }
    let state = ctx.app.state::<crate::state::AppState>();
    let authority = crate::remote::client_messages::ClientSendAuthority::for_pwa(
        &state,
        session,
        origin.to_string(),
    );
    let fence_store = store.clone();
    let fence_owner = bound_owner.clone();
    let fence_id = id.clone();
    match blocking(move || fence_store.begin_dispatch(&fence_owner, &fence_id)).await {
        Ok(_) => (),
        Err(error) if error.code == "submission_dispatch_fenced" => {
            return Ok(submission_response(&id, SubmissionStatus::Dispatching))
        }
        Err(error) => return Err(error),
    }
    // Only the winner of the durable fence may release claims after a proven
    // preflight failure. A competing duplicate must never release an active send.
    let preflight = async {
        authorize(ctx, headers, false).await?;
        drop(
            authority
                .acquire()
                .await
                .map_err(RemoteGatewayError::forbidden)?,
        );
        Ok::<_, RemoteGatewayError>(())
    }
    .await;
    if let Err(error) = preflight {
        let failed_store = store.clone();
        let failed_owner = bound_owner.clone();
        let failed_id = id.clone();
        blocking(move || {
            failed_store.finish_submission(
                &failed_owner,
                &failed_id,
                DispatchOutcome::FailedBeforeDispatch,
            )
        })
        .await?;
        return Err(error);
    }
    // Dispatch is durable before entering provider code; response loss and task
    // cancellation never make a duplicate request submit this message again.
    let deadline = authority.deadline_at.clone();
    let result = crate::control::deliver_remote_client_message(
        &state,
        &bound_owner.agent_id,
        &prepared.prompt,
        &id,
        &deadline,
        authority,
        Some(&ctx.app),
    )
    .await;
    let outcome = match result {
        Ok(details)
            if details.iter().any(|detail| {
                detail.delivery_state == "submitted_unconfirmed"
                    || detail.delivery_phase.as_deref() == Some("submitted_unconfirmed")
            }) =>
        {
            DispatchOutcome::SubmittedUnconfirmed
        }
        Ok(details)
            if !details.is_empty()
                && details.iter().all(|detail| {
                    detail.error.is_none()
                        && matches!(
                            detail.delivery_state.as_str(),
                            "submitted"
                                | "provider_accepted"
                                | "provider_applied"
                                | "queued"
                                | "delivered"
                        )
                }) =>
        {
            DispatchOutcome::Submitted
        }
        Ok(_) => DispatchOutcome::SubmittedUnconfirmed,
        Err(error) => {
            match crate::remote::client_messages::classify_dispatch_error(error.details()) {
                crate::remote::client_messages::ClientMessageDispatchOutcome::FailedBeforeSubmit(_) => DispatchOutcome::FailedBeforeDispatch,
                _ => DispatchOutcome::SubmittedUnconfirmed,
            }
        }
    };
    let finish_id = id.clone();
    let status =
        blocking(move || store.finish_submission(&bound_owner, &finish_id, outcome)).await?;
    audit_gateway_event(
        session,
        origin,
        GatewayAuditEvent::accepted("attachment", "send"),
    );
    Ok(submission_response(&id, status))
}

#[derive(serde::Deserialize)]
pub(super) struct AttachmentPath {
    session_id: String,
    upload_id: String,
}

#[derive(serde::Deserialize)]
pub(super) struct SubmissionPath {
    session_id: String,
    submission_id: String,
}

async fn authorize<R: Runtime>(
    ctx: &RemoteGatewayContext<R>,
    headers: &HeaderMap,
    mutation: bool,
) -> Result<(String, RemoteSessionRecord), RemoteGatewayError> {
    // Same-origin browser GETs can omit Origin. Reads use the existing gateway
    // policy (canonical Host and rejection of supplied foreign Origin/fetch-site);
    // mutations continue requiring explicit Origin plus session-bound CSRF.
    let origin = require_audited_request_boundary(&ctx.config, headers, mutation, "attachment")?;
    let session =
        require_audited_remote_session(ctx, headers, &origin, "attachment", "access").await?;
    if mutation {
        require_csrf_header(&session, headers)?;
        require_mutation_rate_limit(ctx, &session, &origin, "attachment", "access").await?;
    }
    Ok((origin, session))
}

async fn target_workspace<R: Runtime>(
    ctx: &RemoteGatewayContext<R>,
    target: &str,
) -> Result<PathBuf, RemoteGatewayError> {
    let state = ctx.app.state::<crate::state::AppState>();
    let configuration = {
        let agents = state.agents.lock().await;
        agents
            .get(target)
            .map(|agent| agent.config.clone())
            .ok_or_else(|| RemoteGatewayError::not_found("agent_not_found"))?
    };
    let config = configuration
        .lock()
        .map_err(|_| RemoteGatewayError::service_unavailable("agent_unavailable"))?;
    Ok(PathBuf::from(&config.folder))
}

/// Count actual frames, including chunked requests. No allocation follows an
/// untrusted Content-Length, and authorization precedes every call.
async fn read_upload(body: Body, headers: &HeaderMap) -> Result<Vec<u8>, RemoteGatewayError> {
    if let Some(length) = headers.get(header::CONTENT_LENGTH) {
        let length = length
            .to_str()
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .ok_or_else(|| RemoteGatewayError::bad_request("invalid_content_length"))?;
        if length > MAX_UPLOAD_BYTES {
            return Err(upload_too_large());
        }
    }
    tokio::time::timeout(std::time::Duration::from_secs(60), async move {
        let mut stream = body.into_data_stream();
        let mut bytes = Vec::new();
        while let Some(frame) = stream.next().await {
            let frame = frame.map_err(|_| RemoteGatewayError::bad_request("upload_interrupted"))?;
            if frame.len() > MAX_UPLOAD_BYTES.saturating_sub(bytes.len()) {
                return Err(upload_too_large());
            }
            bytes.extend_from_slice(&frame);
        }
        Ok(bytes)
    })
    .await
    .map_err(|_| RemoteGatewayError::bad_request("upload_timeout"))?
}

fn upload_too_large() -> RemoteGatewayError {
    RemoteGatewayError {
        status: StatusCode::PAYLOAD_TOO_LARGE,
        code: "file_too_large",
        detail: None,
    }
}

fn filename(headers: &HeaderMap) -> Result<String, RemoteGatewayError> {
    let encoded = headers
        .get("x-wardian-filename")
        .and_then(|value| value.to_str().ok())
        .filter(|value| value.len() <= 384)
        .ok_or_else(|| RemoteGatewayError::bad_request("invalid_filename"))?;
    let mut bytes = Vec::new();
    let mut input = encoded.bytes();
    while let Some(byte) = input.next() {
        if byte == b'%' {
            let high = input.next().and_then(|value| (value as char).to_digit(16));
            let low = input.next().and_then(|value| (value as char).to_digit(16));
            match (high, low) {
                (Some(high), Some(low)) => bytes.push((high * 16 + low) as u8),
                _ => return Err(RemoteGatewayError::bad_request("invalid_filename")),
            }
        } else {
            bytes.push(byte);
        }
    }
    String::from_utf8(bytes).map_err(|_| RemoteGatewayError::bad_request("invalid_filename"))
}

pub(super) fn no_store(value: impl serde::Serialize) -> Response {
    let mut response = Json(value).into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn declared_and_chunked_body_limits_count_bytes() {
        let mut headers = HeaderMap::new();
        headers.insert(header::CONTENT_LENGTH, HeaderValue::from_static("10485761"));
        assert_eq!(
            read_upload(Body::empty(), &headers)
                .await
                .unwrap_err()
                .status,
            StatusCode::PAYLOAD_TOO_LARGE
        );
        let stream = futures_util::stream::iter([
            Ok::<_, std::io::Error>(Bytes::from(vec![0; MAX_UPLOAD_BYTES])),
            Ok(Bytes::from_static(b"x")),
        ]);
        assert_eq!(
            read_upload(Body::from_stream(stream), &HeaderMap::new())
                .await
                .unwrap_err()
                .code,
            "file_too_large"
        );
        assert_eq!(
            read_upload(Body::from("hello"), &HeaderMap::new())
                .await
                .unwrap(),
            b"hello"
        );
    }

    #[test]
    fn filename_header_decodes_utf8_without_form_url_semantics() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-wardian-filename",
            HeaderValue::from_static("a%20%2B%20%C3%A9.txt"),
        );
        assert_eq!(filename(&headers).unwrap(), "a + é.txt");
        for invalid in ["bad%", "bad%0G", "%ff"] {
            headers.insert("x-wardian-filename", HeaderValue::from_static(invalid));
            assert_eq!(filename(&headers).unwrap_err().code, "invalid_filename");
        }
    }
}
