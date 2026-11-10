//! Archive finalization shared by lifecycle boundaries.

use super::{
    AppState, ConversationBoundaryReason, ConversationLoggingSetting, PendingConversationBoundary,
};

/// Retire live Chat before publishing the durable lifecycle boundary.
pub(super) fn finalize_conversation_boundary(
    state: &AppState,
    session_id: &str,
    boundary_reason: ConversationBoundaryReason,
    pending: &PendingConversationBoundary,
) -> std::io::Result<Option<wardian_core::conversations::ConversationIndexEntry>> {
    if boundary_reason == ConversationBoundaryReason::Clear {
        state
            .conversation_archive
            .retire_live_conversation(session_id)?;
    }
    if pending.effective_logging == ConversationLoggingSetting::Enabled {
        if !state
            .conversation_archive
            .active_ends_with_lifecycle_boundary(session_id, boundary_reason)
            .map_err(|error| {
                std::io::Error::new(
                    error.kind(),
                    format!("could not inspect the active archive boundary: {error}"),
                )
            })?
        {
            state
                .conversation_archive
                .append_lifecycle_boundary_with_context(
                    pending.capture.context.clone(),
                    boundary_reason,
                )
                .map_err(|error| {
                    std::io::Error::new(
                        error.kind(),
                        format!("could not append the archive lifecycle boundary: {error}"),
                    )
                })?;
        }
        let active_conversation_id = state
            .conversation_archive
            .active_conversation_id(session_id)
            .map_err(|error| {
                std::io::Error::new(
                    error.kind(),
                    format!("could not resolve the active archive: {error}"),
                )
            })?
            .ok_or_else(|| std::io::Error::other("conversation boundary had no active archive"))?;
        let closed_conversation = state
            .conversation_archive
            .list(Some(session_id), false)
            .map_err(|error| {
                std::io::Error::new(
                    error.kind(),
                    format!("could not read the active archive index: {error}"),
                )
            })?
            .into_iter()
            .find(|entry| entry.conversation_id == active_conversation_id)
            .ok_or_else(|| {
                std::io::Error::other(format!(
                    "conversation {active_conversation_id} is missing from the archive index"
                ))
            })?;
        let closed_conversation_id = state
            .conversation_archive
            .rollover_agent(session_id, boundary_reason)
            .map_err(|error| {
                std::io::Error::new(
                    error.kind(),
                    format!("could not roll over the active archive: {error}"),
                )
            })?
            .ok_or_else(|| std::io::Error::other("conversation boundary had no active archive"))?;
        debug_assert_eq!(closed_conversation_id, active_conversation_id);
        return Ok(Some(closed_conversation));
    }

    state
        .conversation_archive
        .discard_agent_with_context(pending.capture.context.clone(), &pending.capture.events)?;
    Ok(None)
}
