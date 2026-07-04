use serde::Serialize;
use std::collections::BTreeMap;
use tauri::{AppHandle, Emitter};
use wardian_core::topology::{
    load_topology, pair_activity_from_records, resolve_neighbors, save_topology, PairActivity,
    Topology,
};

#[derive(Debug, Clone, Serialize)]
pub struct TopologyEdgeDto {
    pub a: String,
    pub b: String,
    /// Always "manual" in schema v3: team edges are seeded as manual edges at
    /// write time, never computed from rules at read time.
    pub origin: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct TopologySnapshot {
    pub edges: Vec<TopologyEdgeDto>,
    pub ignored_pairs: Vec<[String; 2]>,
    /// Groups of agent UUIDs visible to each other only via workspace-fallback.
    /// Not currently consumed by the frontend (the halo rendering it fed was
    /// removed); kept for API stability until a consumer returns or it's retired.
    pub fallback_groups: Vec<Vec<String>>,
}

fn home() -> Result<std::path::PathBuf, String> {
    crate::utils::fs::get_wardian_home().ok_or_else(|| "WARDIAN_HOME not resolvable".to_string())
}

#[tauri::command]
pub async fn get_topology(
    state: tauri::State<'_, crate::state::AppState>,
) -> Result<TopologySnapshot, String> {
    let home = home()?;
    let topology = load_topology(&home);
    let refs = agent_refs(&state).await;

    let edges = snapshot_edges(&topology);

    // Fallback groups: agents whose neighbors come only from workspace-fallback.
    let mut groups: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for agent in &refs {
        let view = resolve_neighbors(&agent.uuid, &topology, &refs);
        let only_fallback = !view.members.is_empty()
            && view.members.iter().all(|m| {
                m.reasons
                    .iter()
                    .all(|r| r.starts_with("rule:workspace-fallback"))
            });
        if only_fallback {
            if let Some(ws) = agent.workspace.clone() {
                groups.entry(ws).or_default().push(agent.uuid.clone());
            }
        }
    }

    Ok(TopologySnapshot {
        edges,
        ignored_pairs: topology
            .ignored_pairs
            .iter()
            .map(|p| [p.a.clone(), p.b.clone()])
            .collect(),
        fallback_groups: groups.into_values().filter(|g| g.len() > 1).collect(),
    })
}

/// Manual edges only. Teams have been seeded as manual edges at write time.
pub(crate) fn snapshot_edges(topology: &Topology) -> Vec<TopologyEdgeDto> {
    topology
        .edges
        .iter()
        .map(|edge| TopologyEdgeDto {
            a: edge.a.clone(),
            b: edge.b.clone(),
            origin: "manual".into(),
        })
        .collect()
}

#[tauri::command]
pub async fn add_topology_edge(app: AppHandle, a: String, b: String) -> Result<bool, String> {
    mutate(&app, |topology| {
        topology.add_edge(&a, &b, &chrono::Utc::now().to_rfc3339())
    })
}

#[tauri::command]
pub async fn remove_topology_edge(app: AppHandle, a: String, b: String) -> Result<bool, String> {
    let home = home()?;
    let teams = wardian_core::topology::load_team_memberships(&home);
    mutate(&app, |topology| {
        topology.remove_edge_and_suppress_seed_if_team_pair(&a, &b, &teams)
    })
}

#[tauri::command]
pub async fn ignore_topology_pair(app: AppHandle, a: String, b: String) -> Result<bool, String> {
    mutate(&app, |topology| topology.ignore_pair(&a, &b))
}

#[tauri::command]
pub async fn unignore_topology_pair(app: AppHandle, a: String, b: String) -> Result<bool, String> {
    mutate(&app, |topology| topology.unignore_pair(&a, &b))
}

#[tauri::command]
pub async fn get_pair_activity() -> Result<Vec<PairActivity>, String> {
    let records = wardian_core::db::list_interaction_records().map_err(|e| e.to_string())?;
    let now_ms = chrono::Utc::now().timestamp_millis();
    Ok(pair_activity_from_records(&records, now_ms))
}

fn mutate(app: &AppHandle, apply: impl FnOnce(&mut Topology) -> bool) -> Result<bool, String> {
    let home = home()?;
    let mut topology = load_topology(&home);
    let changed = apply(&mut topology);
    if changed {
        save_topology(&home, &topology).map_err(|e| e.to_string())?;
        let _ = app.emit("topology-changed", ());
    }
    Ok(changed)
}

async fn agent_refs(
    state: &tauri::State<'_, crate::state::AppState>,
) -> Vec<wardian_core::topology::AgentRef> {
    state.topology_agent_refs().await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_edges_manual_only() {
        let topology = Topology {
            version: 2,
            edges: vec![wardian_core::topology::TopologyEdge {
                a: "a".to_string(),
                b: "b".to_string(),
                created_at: "2026-07-02T00:00:00Z".to_string(),
            }],
            ignored_pairs: vec![],
            suppressed_seed_pairs: vec![],
        };

        let edges = snapshot_edges(&topology);

        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].a, "a");
        assert_eq!(edges[0].b, "b");
        assert_eq!(edges[0].origin, "manual");
    }
}
