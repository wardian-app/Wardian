use tauri::AppHandle;

#[tauri::command]
pub async fn load_watchlists(_app: AppHandle) -> Result<Vec<serde_json::Value>, String> {
    if let Some(app_dir) = crate::utils::fs::get_wardian_home() {
        let path = app_dir.join("watchlists.json");
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
    let path = app_dir.join("watchlists.json");
    let json = serde_json::to_string_pretty(&watchlists).map_err(|e| e.to_string())?;
    std::fs::write(path, json).map_err(|e| e.to_string())?;
    Ok(())
}
