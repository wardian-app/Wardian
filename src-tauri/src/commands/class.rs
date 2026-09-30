use crate::manager;
use tauri::AppHandle;
use wardian_core::models::AgentClassDefinition;

#[tauri::command]
pub async fn list_agent_classes(app: AppHandle) -> Result<Vec<AgentClassDefinition>, String> {
    manager::log_debug("[WARDIAN] list_agent_classes called");
    Ok(manager::get_all_agent_classes(&app))
}

#[tauri::command]
pub async fn create_agent_class(
    name: String,
    description: String,
    instruction_content: Option<String>,
    app: AppHandle,
) -> Result<Vec<AgentClassDefinition>, String> {
    manager::log_debug(&format!("[WARDIAN] create_agent_class called: {}", name));
    let trimmed_name = name.trim().to_string();
    if trimmed_name.is_empty() {
        return Err("Class name cannot be empty".to_string());
    }

    let app_dir = crate::utils::fs::get_wardian_home()
        .ok_or_else(|| "Could not locate Wardian home directory".to_string())?;
    wardian_core::classes::create_class(
        &app_dir,
        &trimmed_name,
        description.trim(),
        instruction_content.as_deref(),
    )?;

    manager::init_agent_classes(&app);
    Ok(manager::get_all_agent_classes(&app))
}

#[tauri::command]
pub async fn delete_agent_class(
    name: String,
    app: AppHandle,
) -> Result<Vec<AgentClassDefinition>, String> {
    manager::log_debug(&format!("[WARDIAN] delete_agent_class called: {}", name));

    let app_dir = crate::utils::fs::get_wardian_home()
        .ok_or_else(|| "Could not locate Wardian home directory".to_string())?;
    wardian_core::classes::delete_class(&app_dir, &name)?;

    Ok(manager::get_all_agent_classes(&app))
}

#[tauri::command]
pub async fn reset_class_to_default(name: String, _app: AppHandle) -> Result<(), String> {
    manager::log_debug(&format!(
        "[WARDIAN] reset_class_to_default called: {}",
        name
    ));

    let app_dir = crate::utils::fs::get_wardian_home()
        .ok_or_else(|| "Could not locate Wardian home directory".to_string())?;
    wardian_core::classes::restore_default_instruction(&app_dir, &name)
}
