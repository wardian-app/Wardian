use crate::manager::log_debug;
use crate::workflow_engine::{get_workflows_dir, get_scheduled_runs_path, list_workflows};
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

    // Migrate scheduled_runs.json from old format
    migrate_scheduled_runs_if_needed();
}

/// Migrates `scheduled_runs.json` from old flat `{ schedule_type, value, active }`
/// to the new rich ScheduleDefinition format.
fn migrate_scheduled_runs_if_needed() {
    let path = match get_scheduled_runs_path() {
        Some(p) => p,
        None => return,
    };

    if !path.exists() {
        return;
    }

    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return,
    };

    let runs: Vec<serde_json::Value> = match serde_json::from_str(&content) {
        Ok(r) => r,
        Err(_) => return,
    };

    // Check if migration is needed: old format has "value" field in schedule
    let needs_migration = runs.iter().any(|run| {
        run.get("schedule")
            .and_then(|s| s.get("value"))
            .is_some()
    });

    if !needs_migration {
        return;
    }

    let migrated: Vec<serde_json::Value> = runs
        .into_iter()
        .map(|mut run| {
            if let Some(schedule) = run.get("schedule").cloned() {
                if schedule.get("value").is_some() {
                    let old_type = schedule
                        .get("schedule_type")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let old_value = schedule
                        .get("value")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let active = schedule
                        .get("active")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(true);

                    let new_schedule = match old_type {
                        "minutes" => {
                            let mins: u32 = old_value.parse().unwrap_or(5);
                            serde_json::json!({
                                "schedule_type": "interval",
                                "interval_minutes": mins,
                                "end_condition": "never",
                                "repeat_every": 1,
                                "occurrence_count": 0,
                                "active": active
                            })
                        }
                        "hours" => {
                            let hours: u32 = old_value.parse().unwrap_or(1);
                            serde_json::json!({
                                "schedule_type": "interval",
                                "interval_minutes": hours * 60,
                                "end_condition": "never",
                                "repeat_every": 1,
                                "occurrence_count": 0,
                                "active": active
                            })
                        }
                        "daily" => {
                            serde_json::json!({
                                "schedule_type": "daily",
                                "time_of_day": old_value,
                                "end_condition": "never",
                                "repeat_every": 1,
                                "occurrence_count": 0,
                                "active": active
                            })
                        }
                        "weekly" => {
                            // old format: "Mon,Wed@09:00"
                            let parts: Vec<&str> = old_value.split('@').collect();
                            let days: Vec<String> = if parts.len() >= 1 {
                                parts[0].split(',').map(|s| s.trim().to_string()).collect()
                            } else {
                                vec![]
                            };
                            let time = if parts.len() >= 2 {
                                parts[1].to_string()
                            } else {
                                "00:00".to_string()
                            };
                            serde_json::json!({
                                "schedule_type": "weekly",
                                "days_of_week": days,
                                "time_of_day": time,
                                "repeat_every": 1,
                                "end_condition": "never",
                                "occurrence_count": 0,
                                "active": active
                            })
                        }
                        "one_time" => {
                            serde_json::json!({
                                "schedule_type": "one_time",
                                "run_at": old_value,
                                "end_condition": "never",
                                "repeat_every": 1,
                                "occurrence_count": 0,
                                "active": active
                            })
                        }
                        _ => schedule,
                    };

                    if let Some(obj) = run.as_object_mut() {
                        obj.insert("schedule".to_string(), new_schedule);
                    }
                }
            }
            run
        })
        .collect();

    if let Ok(json) = serde_json::to_string_pretty(&migrated) {
        let _ = std::fs::write(&path, json);
        log_debug("[Wardian] Migrated scheduled_runs.json to new ScheduleDefinition format");
    }
}
