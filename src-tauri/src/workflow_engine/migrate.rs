use crate::manager::log_debug;
use crate::workflow_engine::{get_workflows_dir, list_workflows};
use std::collections::HashMap;

fn synthesize_role_name(base: &str, node_id: &str, existing: &HashMap<String, String>) -> String {
    let sanitized = base
        .trim()
        .to_lowercase()
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
        .collect::<String>()
        .trim_matches('_')
        .to_string();

    let base_role = if sanitized.is_empty() {
        format!("agent_{}", node_id)
    } else {
        sanitized
    };

    if !existing.contains_key(&base_role) {
        return base_role;
    }

    let node_suffix_role = format!("{}_{}", base_role, node_id);
    if !existing.contains_key(&node_suffix_role) {
        return node_suffix_role;
    }

    let mut counter = 2;
    loop {
        let candidate = format!("{}_{}", base_role, counter);
        if !existing.contains_key(&candidate) {
            return candidate;
        }
        counter += 1;
    }
}

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
            continue;
        }

        let mut mappings: HashMap<String, String> = HashMap::new();
        let mut modified = false;

        for node in &mut wf.nodes {
            if node.r#type != "agent" {
                continue;
            }

            let has_role = node
                .config
                .get("role")
                .and_then(|v| v.as_str())
                .is_some_and(|r| !r.is_empty());

            if has_role {
                continue;
            }

            let agent_id = node
                .config
                .get("agent_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            if agent_id.is_empty() {
                continue;
            }

            let base_name = node
                .name
                .clone()
                .unwrap_or_else(|| format!("agent_{}", node.id));
            let role_name = synthesize_role_name(&base_name, &node.id, &mappings);

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
                log_debug(&format!(
                    "[Wardian] Migrated workflow '{}' with role_mappings",
                    wf.name
                ));
            }
        }
    }
}
