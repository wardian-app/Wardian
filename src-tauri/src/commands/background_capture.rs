//! Production background dispatch shared by telemetry, restore, and status.

use crate::state::background_capture::{CaptureClaim, CaptureRequest, CaptureStop};
use crate::state::AppState;

/// Admit before spawning. Both the app and state-only hosts use this dispatch
/// core, so ordinary telemetry tests exercise the same ownership decision.
pub(crate) async fn dispatch_background_capture(
    state: &AppState,
    request: CaptureRequest,
    force: bool,
    spawn: impl FnOnce(CaptureClaim),
) {
    if let Some(claim) = admit_background_capture(state, request, force).await {
        spawn(claim);
    }
}

/// Validate the live roster immediately before synchronous admission. Source
/// observation happens before the roster/coordinator locks, never under them.
pub(crate) async fn admit_background_capture(
    state: &AppState,
    mut request: CaptureRequest,
    force: bool,
) -> Option<CaptureClaim> {
    if let Some(path) = request.source_path.as_deref() {
        match super::provider_log_acquisition::observe_provider_log_source(path) {
            Ok(actual) => {
                if request.source.as_ref().is_some_and(|expected| {
                    expected.path != actual.path
                        || expected.native_identity != actual.native_identity
                }) {
                    return None;
                }
                request.source = Some(actual);
            }
            Err(_) => request.source = None,
        }
    }
    let agents = state.agents.lock().await;
    let agent = agents.get(&request.session_id)?;
    if !std::sync::Arc::ptr_eq(&agent.current_status, &request.incarnation) {
        return None;
    }
    let config = agent.config.lock().ok()?;
    if config.provider != request.provider
        || config
            .resume_session
            .as_ref()
            .or(config.fresh_provider_session_id.as_ref())
            != request.conversation.as_ref()
        || *agent.log_path.lock().ok()? != request.source_path
    {
        return None;
    }
    // The live roster cannot replace this witness between validation and
    // admission. The coordinator never acquires the roster in reverse order.
    let force = force
        || state
            .conversation_archive
            .has_deferred_chat_summaries(&request.session_id);
    state.background_capture.admit(request, force)
}

pub(crate) async fn run_background_capture(state: &AppState, mut claim: CaptureClaim) {
    loop {
        match super::chat::archive_background_capture_pass(state, &claim.request).await {
            Ok(CaptureStop::More) => tokio::task::yield_now().await,
            Ok(stop) => {
                let session_id = claim.request.session_id.clone();
                let suspend = stop == CaptureStop::Disabled;
                claim.finish(stop);
                if suspend {
                    return;
                }
                match state.background_capture.take_pending(&session_id) {
                    Some(next) => claim = next,
                    None => return,
                }
            }
            Err(error) => {
                crate::manager::log_debug(&format!(
                    "[WARDIAN] background conversation capture failed for {}: {error}",
                    claim.request.session_id,
                ));
                // Drop releases only this nonce and retains retry intent. The
                // next ordinary observation retries even if the parser mtime
                // was consumed already; this worker never immediately retries.
                return;
            }
        }
    }
}

/// A detached restore/status caller joins the same coordinator as telemetry.
pub(crate) async fn capture_background_for_state(
    state: &AppState,
    session_id: &str,
) -> Result<(), String> {
    let Some(request) = super::chat::background_capture_request(state, session_id).await? else {
        return Ok(());
    };
    if let Some(claim) = admit_background_capture(state, request, true).await {
        run_background_capture(state, claim).await;
    }
    Ok(())
}
