use tauri::AppHandle;

#[tauri::command]
pub async fn load_watchlists(_app: AppHandle) -> Result<Vec<serde_json::Value>, String> {
    if let Some(app_dir) = crate::utils::fs::get_wardian_home() {
        let path = app_dir.join("watchlists/index.json");
        if let Ok(data) = std::fs::read_to_string(&path) {
            let parsed: Vec<serde_json::Value> = serde_json::from_str(&data).unwrap_or_default();
            return Ok(parsed);
        }
    }
    Ok(Vec::new())
}

#[tauri::command]
pub async fn save_watchlists(
    watchlists: Vec<serde_json::Value>,
    _app: AppHandle,
) -> Result<(), String> {
    let app_dir = crate::utils::fs::get_wardian_home()
        .ok_or_else(|| "Could not find home directory".to_string())?;
    let _ = std::fs::create_dir_all(&app_dir);
    let _ = std::fs::create_dir_all(app_dir.join("watchlists"));
    let path = app_dir.join("watchlists/index.json");
    let json = serde_json::to_string_pretty(&watchlists).map_err(|e| e.to_string())?;
    std::fs::write(path, json).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn load_watchlist_prefs(_app: AppHandle) -> Result<serde_json::Value, String> {
    if let Some(home) = crate::utils::fs::get_wardian_home() {
        let path = home.join("watchlists/prefs.json");
        if let Ok(data) = std::fs::read_to_string(&path) {
            let parsed: serde_json::Value =
                serde_json::from_str(&data).unwrap_or(serde_json::Value::Null);
            return Ok(parsed);
        }
    }
    Ok(serde_json::Value::Null)
}

#[tauri::command]
pub async fn save_watchlist_prefs(
    prefs: serde_json::Value,
    _app: AppHandle,
) -> Result<(), String> {
    let home = crate::utils::fs::get_wardian_home()
        .ok_or_else(|| "Could not find home directory".to_string())?;
    let _ = std::fs::create_dir_all(home.join("watchlists"));
    let path = home.join("watchlists/prefs.json");
    let json = serde_json::to_string_pretty(&prefs).map_err(|e| e.to_string())?;
    std::fs::write(path, json).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn load_agent_interactions(_app: AppHandle) -> Result<serde_json::Value, String> {
    if let Some(home) = crate::utils::fs::get_wardian_home() {
        let path = home.join("watchlists/interactions.json");
        if let Ok(data) = std::fs::read_to_string(&path) {
            let parsed: serde_json::Value =
                serde_json::from_str(&data).unwrap_or(serde_json::json!({}));
            return Ok(parsed);
        }
    }
    Ok(serde_json::json!({}))
}

#[tauri::command]
pub async fn save_agent_interactions(
    interactions: serde_json::Value,
    _app: AppHandle,
) -> Result<(), String> {
    let home = crate::utils::fs::get_wardian_home()
        .ok_or_else(|| "Could not find home directory".to_string())?;
    let _ = std::fs::create_dir_all(home.join("watchlists"));
    let path = home.join("watchlists/interactions.json");
    let json = serde_json::to_string_pretty(&interactions).map_err(|e| e.to_string())?;
    std::fs::write(path, json).map_err(|e| e.to_string())?;
    Ok(())
}
