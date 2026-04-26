use crate::models::AgentClassDefinition;
use crate::utils::fs::*;
use tauri::{AppHandle, Manager};

pub fn get_all_agent_classes(_app: &AppHandle) -> Vec<AgentClassDefinition> {
    if let Some(app_dir) = get_wardian_home() {
        let classes_path = app_dir.join("classes.json");
        if let Ok(data) = std::fs::read_to_string(&classes_path) {
            return serde_json::from_str::<Vec<AgentClassDefinition>>(&data).unwrap_or_default();
        }
    }
    Vec::new()
}

pub fn save_classes(_app: &AppHandle, classes: &[AgentClassDefinition]) -> Result<(), String> {
    let app_dir = get_wardian_home().ok_or("No home dir")?;
    let json = serde_json::to_string_pretty(classes).map_err(|e| e.to_string())?;
    std::fs::write(app_dir.join("classes.json"), json).map_err(|e| e.to_string())?;
    Ok(())
}

pub fn init_agent_classes(app: &AppHandle) {
    if let Some(app_dir) = get_wardian_home() {
        let classes_dir = app_dir.join("classes");
        let _ = std::fs::create_dir_all(&classes_dir);
        let _ = std::fs::create_dir_all(app_dir.join("common/desk"));
        let _ = std::fs::create_dir_all(app_dir.join("common/lineages"));

        // Ensure Claude can discover skills from the canonical .agents/skills/ location
        ensure_claude_skills_link(&app_dir.join("common"));

        let classes_path = app_dir.join("classes.json");

        // Migration and Initialization
        if !classes_path.exists() {
            let mut defaults: Vec<AgentClassDefinition> =
                serde_json::from_str(include_str!("../default_classes.json")).unwrap_or_default();
            for d in defaults.iter_mut() {
                d.is_default = true;
            }

            let custom_path = app_dir.join("custom_classes.json");
            if custom_path.exists() {
                if let Ok(data) = std::fs::read_to_string(&custom_path) {
                    let mut custom = serde_json::from_str::<Vec<AgentClassDefinition>>(&data)
                        .unwrap_or_default();
                    for c in custom.iter_mut() {
                        c.is_default = false;
                    }
                    defaults.extend(custom);
                }
                // We've successfully merged. We could delete custom_classes.json here.
                let _ = std::fs::remove_file(&custom_path);
            }

            let _ = save_classes(app, &defaults);
        }

        let classes = get_all_agent_classes(app);
        for cls in &classes {
            let role_dir = classes_dir.join(&cls.name);
            let _ = std::fs::create_dir_all(&role_dir);

            // 1. Create AGENTS.md master file
            let agents_md_path = role_dir.join("AGENTS.md");
            if !agents_md_path.exists() {
                let content = if cls.is_default {
                    app.path()
                        .resolve(
                            format!("agent_prompts/{}.md", cls.name),
                            tauri::path::BaseDirectory::Resource,
                        )
                        .ok()
                        .and_then(|p| std::fs::read_to_string(p).ok())
                        .unwrap_or_default()
                } else {
                    format!("# {} Agent\n\n{}\n", cls.name, cls.description)
                };
                let _ = std::fs::write(agents_md_path, content);
            }

            // 2. Symlink .claude/skills/ → .agents/skills/ for Claude discovery
            ensure_claude_skills_link(&role_dir);

            // 3. Create provider stub files for providers that do not read AGENTS.md directly
            for stub_name in &["GEMINI.md", "CLAUDE.md"] {
                let stub_path = role_dir.join(stub_name);
                if !stub_path.exists() {
                    let _ = std::fs::write(stub_path, "@AGENTS.md\n");
                }
            }
        }
    }
}

pub fn get_agent_class_default_instruction(app: &AppHandle, class_name: &str) -> Option<String> {
    app.path()
        .resolve(
            format!("agent_prompts/{}.md", class_name),
            tauri::path::BaseDirectory::Resource,
        )
        .ok()
        .and_then(|p| std::fs::read_to_string(p).ok())
}
