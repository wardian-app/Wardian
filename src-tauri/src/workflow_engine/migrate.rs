use crate::workflow_engine::{get_workflows_dir, list_workflows};
use crate::manager::log_debug;
use std::collections::HashMap;

/// Migrates workflow files that lack `role_mappings`.
/// For each agent node with a direct `agent_id` but no `role`,
/// assigns a synthetic role and populates `role_mappings`.
pub fn migrate_workflows_if_needed() {
    let dir = match get_workflows_dir() {
        Some(d) => d,
        None => return,
    };

    let workflows = list_workflows().unwrap_or_default();

    for mut wf in workflows {
        if !wf.role_mappings.is_empty() {
            continue; // Already migrated
        }

        let mut mappings: HashMap<String, String> = HashMap::new();
        let mut modified = false;

        for node in &mut wf.nodes {
            if node.r#type != "agent" {
                continue;
            }

            let has_role = node.config.get("role")
                .and_then(|v| v.as_str())
                .map_or(false, |r| !r.is_empty());

            if has_role {
                continue;
            }

            let agent_id = node.config.get("agent_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            if agent_id.is_empty() {
                continue;
            }

            // Synthesize a role name from the node's display name or ID
            let role_name = node.name.clone()
                .unwrap_or_else(|| format!("agent_{}", node.id))
                .to_lowercase()
                .replace(' ', "_");

            if let Some(obj) = node.config.as_object_mut() {
                obj.insert("role".to_string(), serde_json::json!(role_name));
            }

            mappings.insert(role_name, agent_id);
            modified = true;
        }

        if modified {
            wf.role_mappings = mappings;
            let path = dir.join(format!("{}.json", wf.id));
            if let Ok(content) = serde_json::to_string_pretty(&wf) {
                let _ = std::fs::write(&path, content);
                log_debug(&format!("[Wardian] Migrated workflow '{}' with role_mappings", wf.name));
            }
        }
    }
}
