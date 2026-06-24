use crate::state::AppState;
use tauri::State;
use wardian_core::control::{ConversationListResponse, ConversationShowResponse};

#[tauri::command]
pub async fn list_conversations(
    agent: Option<String>,
    scope_all: Option<bool>,
    state: State<'_, AppState>,
) -> Result<ConversationListResponse, String> {
    list_conversations_for_state(&state, agent.as_deref(), scope_all.unwrap_or(false))
}

pub fn list_conversations_for_state(
    state: &AppState,
    agent: Option<&str>,
    scope_all: bool,
) -> Result<ConversationListResponse, String> {
    state
        .conversation_archive
        .list(agent, scope_all)
        .map(ConversationListResponse::new)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn show_conversation(
    conversation_id: String,
    state: State<'_, AppState>,
) -> Result<ConversationShowResponse, String> {
    show_conversation_for_state(&state, &conversation_id)
}

pub fn show_conversation_for_state(
    state: &AppState,
    conversation_id: &str,
) -> Result<ConversationShowResponse, String> {
    state
        .conversation_archive
        .show(conversation_id)
        .map(|(manifest, conversation)| ConversationShowResponse::new(manifest, conversation))
        .map_err(|error| error.to_string())
}
