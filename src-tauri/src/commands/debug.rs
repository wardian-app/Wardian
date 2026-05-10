use crate::state::AppState;
use tauri::State;

#[tauri::command]
pub async fn debug_remove_agent_input_sender(
    session_id: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    if !cfg!(debug_assertions) {
        return Err("debug commands are disabled in production builds".to_string());
    }

    let mut senders = state
        .input_senders
        .write()
        .map_err(|_| "input_senders lock poisoned".to_string())?;
    senders.remove(&session_id);
    Ok(())
}
