use serde_json::Value;
use std::collections::HashSet;

pub fn load_items() -> Vec<Value> {
    let items = crate::utils::fs::get_wardian_home()
        .and_then(|home| std::fs::read_to_string(home.join("queue").join("items.json")).ok())
        .and_then(|data| serde_json::from_str::<Vec<Value>>(&data).ok())
        .unwrap_or_default();
    let (items, migrated) = normalize_legacy_items(items);
    if migrated {
        let _ = save_items(&items);
    }
    items
}

/// Normalize queue records written before the Workflows-to-Automations rename.
///
/// The item id and all unrelated state are retained.  Legacy keys are removed
/// after their values have been copied to the canonical keys so repeated loads
/// are idempotent and remote and desktop projections see the same shape.
pub fn normalize_legacy_items(items: Vec<Value>) -> (Vec<Value>, bool) {
    let mut migrated = false;
    let items = items
        .into_iter()
        .map(|item| {
            let (item, changed) = normalize_legacy_item(item);
            migrated |= changed;
            item
        })
        .collect();
    (items, migrated)
}

fn normalize_legacy_item(mut item: Value) -> (Value, bool) {
    let Some(object) = item.as_object_mut() else {
        return (item, false);
    };

    let mut migrated = false;
    match object.get("type").and_then(Value::as_str) {
        Some("workflow_completed") => {
            object.insert(
                "type".to_string(),
                Value::String("automation_completed".to_string()),
            );
            migrated = true;
        }
        Some("workflow_failed") => {
            object.insert(
                "type".to_string(),
                Value::String("automation_completed".to_string()),
            );
            if !object.contains_key("status") {
                object.insert("status".to_string(), Value::String("failed".to_string()));
            }
            migrated = true;
        }
        _ => {}
    }

    for (legacy_key, canonical_key) in [
        ("workflow_id", "automation_id"),
        ("workflow_run_id", "automation_run_id"),
        ("workflow_name", "automation_name"),
        ("workflow_approval", "automation_approval"),
    ] {
        if let Some(value) = object.remove(legacy_key) {
            object.entry(canonical_key).or_insert(value);
            migrated = true;
        }
    }

    (item, migrated)
}

pub fn save_items(items: &[Value]) -> Result<(), String> {
    let home = crate::utils::fs::get_wardian_home().ok_or_else(|| "no wardian home".to_string())?;
    let queue_dir = home.join("queue");
    std::fs::create_dir_all(&queue_dir).map_err(|error| error.to_string())?;
    wardian_core::conversations::write_json_atomic(&queue_dir.join("items.json"), items)
        .map_err(|error| error.to_string())
}

/// Merges a desktop queue snapshot with the latest persisted projection.
///
/// The desktop queue historically sends a complete snapshot after each local
/// mutation. When a remote mutation lands after the desktop load, the base
/// snapshot lets us distinguish desktop removals from items added remotely.
/// Read state is monotonic, so a remote read acknowledgement wins conflicts.
pub fn merge_desktop_snapshot(
    base: Option<&[Value]>,
    incoming: &[Value],
    latest: &[Value],
) -> Vec<Value> {
    let Some(base) = base else {
        // Without the load snapshot we cannot distinguish a new desktop item
        // from an item that was removed remotely. Keep the latest persisted
        // projection authoritative until the desktop establishes a baseline.
        return latest.to_vec();
    };
    let base_ids = base.iter().filter_map(item_id).collect::<HashSet<_>>();
    let incoming_by_id = incoming
        .iter()
        .filter_map(|item| item_id(item).map(|id| (id, item)))
        .collect::<std::collections::HashMap<_, _>>();
    let latest_ids = latest.iter().filter_map(item_id).collect::<HashSet<_>>();
    let mut merged = Vec::with_capacity(latest.len() + incoming.len());

    for latest_item in latest {
        let Some(id) = item_id(latest_item) else {
            merged.push(latest_item.clone());
            continue;
        };
        // Provider completion records are written by the backend and can only
        // be removed through the explicit dismissal command. A desktop full
        // snapshot that omits one is stale or belongs to a client that has not
        // observed it yet.
        if latest_item.get("type").and_then(Value::as_str) == Some("agent_completed") {
            let mut item = latest_item.clone();
            if latest_item.get("dismissed").and_then(Value::as_bool) == Some(true) {
                item["read"] = Value::Bool(true);
            } else if let Some(incoming_item) = incoming_by_id.get(id) {
                if incoming_item.get("read").and_then(Value::as_bool) == Some(true) {
                    item["read"] = Value::Bool(true);
                }
            }
            merged.push(item);
            continue;
        }
        if !base_ids.contains(id) {
            merged.push(latest_item.clone());
        } else if let Some(incoming_item) = incoming_by_id.get(id) {
            let mut item = (*incoming_item).clone();
            if latest_item.get("read").and_then(Value::as_bool) == Some(true) {
                item["read"] = Value::Bool(true);
            }
            if let Some(provider_choice_sent) = latest_item.get("provider_choice_sent") {
                item["provider_choice_sent"] = provider_choice_sent.clone();
            }
            if let Some(provider_choice_pending) = latest_item.get("provider_choice_pending") {
                item["provider_choice_pending"] = provider_choice_pending.clone();
            }
            merged.push(item);
        }
    }

    for incoming_item in incoming {
        let Some(id) = item_id(incoming_item) else {
            merged.push(incoming_item.clone());
            continue;
        };
        if !base_ids.contains(id) && !latest_ids.contains(id) {
            merged.push(incoming_item.clone());
        }
    }
    merged
}

fn item_id(item: &Value) -> Option<&str> {
    item.get("id").and_then(Value::as_str)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_legacy_completion_and_approval_fields_without_losing_state() {
        let (items, migrated) = normalize_legacy_items(vec![serde_json::json!({
            "id": "legacy-run",
            "type": "workflow_failed",
            "timestamp": 123,
            "read": true,
            "dismissed": true,
            "workflow_id": "release",
            "workflow_run_id": "run-1",
            "workflow_name": "Release",
            "workflow_approval": {
                "blueprint_id": "release",
                "blueprint_path": "release.md",
                "run_id": "run-1",
                "node": "gate"
            }
        })]);

        assert!(migrated);
        assert_eq!(items[0]["id"], "legacy-run");
        assert_eq!(items[0]["type"], "automation_completed");
        assert_eq!(items[0]["status"], "failed");
        assert_eq!(items[0]["automation_id"], "release");
        assert_eq!(items[0]["automation_run_id"], "run-1");
        assert_eq!(items[0]["automation_name"], "Release");
        assert_eq!(items[0]["automation_approval"]["node"], "gate");
        assert_eq!(items[0]["dismissed"], true);
        assert!(items[0].get("workflow_id").is_none());
        assert!(items[0].get("workflow_run_id").is_none());
        assert!(items[0].get("workflow_name").is_none());
        assert!(items[0].get("workflow_approval").is_none());
    }

    #[test]
    fn stale_desktop_snapshot_preserves_canonical_backend_completion() {
        let base = vec![serde_json::json!({ "id": "existing", "read": false })];
        let incoming = base.clone();
        let latest = vec![
            serde_json::json!({ "id": "existing", "read": false }),
            serde_json::json!({
                "id": "agent-completed:agent-1:message-1",
                "type": "agent_completed",
                "evidence_id": "message-1",
                "timestamp": 123,
                "summary": "canonical preview",
                "response_text": "full provider response",
                "read": false
            }),
        ];

        let merged = merge_desktop_snapshot(Some(&base), &incoming, &latest);

        let completion = merged
            .iter()
            .find(|item| item["id"] == "agent-completed:agent-1:message-1")
            .expect("completion survives stale desktop write");
        assert_eq!(completion["timestamp"], 123);
        assert_eq!(completion["summary"], "canonical preview");
        assert_eq!(completion["response_text"], "full provider response");
    }

    #[test]
    fn desktop_snapshot_cannot_remove_or_resurrect_a_dismissed_completion() {
        let dismissed = serde_json::json!({
            "id": "agent-completed:agent-1:message-1",
            "type": "agent_completed",
            "evidence_id": "message-1",
            "read": true,
            "dismissed": true
        });
        let stale_visible = serde_json::json!({
            "id": dismissed["id"],
            "type": "agent_completed",
            "evidence_id": "message-1",
            "read": false
        });
        let merged = merge_desktop_snapshot(
            Some(std::slice::from_ref(&stale_visible)),
            std::slice::from_ref(&stale_visible),
            std::slice::from_ref(&dismissed),
        );

        assert_eq!(merged, vec![dismissed]);
    }

    #[test]
    fn load_items_persists_the_canonical_queue_shape() {
        let _lock = crate::utils::wardian_test_env_lock();
        let home = tempfile::tempdir().expect("temp home");
        let previous_home = std::env::var_os("WARDIAN_HOME");
        unsafe { std::env::set_var("WARDIAN_HOME", home.path()) };
        std::fs::create_dir_all(home.path().join("queue")).expect("queue directory");
        std::fs::write(
            home.path().join("queue/items.json"),
            serde_json::json!([{
                "id": "legacy-completed",
                "type": "workflow_completed",
                "timestamp": 123,
                "read": false,
                "workflow_id": "release",
                "workflow_run_id": "run-1",
                "workflow_name": "Release"
            }])
            .to_string(),
        )
        .expect("legacy queue");

        let items = load_items();
        let persisted: Vec<Value> = serde_json::from_str(
            &std::fs::read_to_string(home.path().join("queue/items.json")).expect("saved queue"),
        )
        .expect("saved queue json");

        match previous_home {
            Some(value) => unsafe { std::env::set_var("WARDIAN_HOME", value) },
            None => unsafe { std::env::remove_var("WARDIAN_HOME") },
        }
        assert_eq!(items, persisted);
        assert_eq!(persisted[0]["type"], "automation_completed");
        assert_eq!(persisted[0]["automation_run_id"], "run-1");
        assert!(persisted[0].get("workflow_run_id").is_none());
    }
}
