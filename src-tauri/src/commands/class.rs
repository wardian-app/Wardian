use tauri::AppHandle;
use crate::models::AgentClassDefinition;
use crate::manager;

#[tauri::command]
pub async fn list_agent_classes(app: AppHandle) -> Result<Vec<AgentClassDefinition>, String> {
    manager::log_debug("[WARDIAN] list_agent_classes called");
    Ok(manager::get_all_agent_classes(&app))
}

#[tauri::command]
pub async fn create_agent_class(
    name: String,
    description: String,
    gemini_md: Option<String>,
    app: AppHandle,
) -> Result<Vec<AgentClassDefinition>, String> {
    manager::log_debug(&format!("[WARDIAN] create_agent_class called: {}", name));
    let trimmed_name = name.trim().to_string();
    if trimmed_name.is_empty() {
        return Err("Class name cannot be empty".to_string());
    }

    let mut all = manager::get_all_agent_classes(&app);
    if all
        .iter()
        .any(|c| c.name.to_lowercase() == trimmed_name.to_lowercase())
    {
        return Err(format!("A class named '{}' already exists", trimmed_name));
    }

    let new_class = AgentClassDefinition {
        name: trimmed_name.clone(),
        description: description.trim().to_string(),
        is_default: false,
        gemini_md: None,
        assigned_skills: None,
    };

    all.push(new_class.clone());
    manager::save_classes(&app, &all)?;

    if let Some(app_dir) = crate::utils::fs::get_wardian_home() {
        let role_dir = app_dir.join("classes").join(&trimmed_name);
        let _ = std::fs::create_dir_all(&role_dir);
        let gemini_md_path = role_dir.join("GEMINI.md");
        if !gemini_md_path.exists() {
            let content = match gemini_md {
                Some(ref md) if !md.trim().is_empty() => md.clone(),
                _ => format!("# {} Agent

{}
", trimmed_name, new_class.description),
            };
            let _ = std::fs::write(gemini_md_path, content);
        }
    }

    manager::init_agent_classes(&app);
    Ok(manager::get_all_agent_classes(&app))
}

#[tauri::command]
pub async fn delete_agent_class(
    name: String,
    app: AppHandle,
) -> Result<Vec<AgentClassDefinition>, String> {
    manager::log_debug(&format!("[WARDIAN] delete_agent_class called: {}", name));

    let mut all = manager::get_all_agent_classes(&app);
    if let Some(found) = all.iter().find(|c| c.name == name) {
        if found.is_default {
            return Err("Cannot delete a default class".to_string());
        }
    } else {
        return Err(format!("Class '{}' not found", name));
    }

    all.retain(|c| c.name != name);
    manager::save_classes(&app, &all)?;

    if let Some(app_dir) = crate::utils::fs::get_wardian_home() {
        let role_dir = app_dir.join("classes").join(&name);
        if role_dir.exists() {
            let _ = std::fs::remove_dir_all(&role_dir);
        }
    }

    Ok(manager::get_all_agent_classes(&app))
}
