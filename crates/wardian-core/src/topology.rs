use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::io;
use std::path::Path;

pub const TOPOLOGY_SCHEMA_VERSION: u8 = 2;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TopologyEdge {
    pub a: String,
    pub b: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IgnoredPair {
    pub a: String,
    pub b: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Topology {
    pub version: u8,
    #[serde(default)]
    pub edges: Vec<TopologyEdge>,
    #[serde(default)]
    pub ignored_pairs: Vec<IgnoredPair>,
}

impl Default for Topology {
    fn default() -> Self {
        Self { version: TOPOLOGY_SCHEMA_VERSION, edges: Vec::new(), ignored_pairs: Vec::new() }
    }
}

/// Canonical undirected pair: lexicographic order, rejects self/empty pairs.
pub fn canonical_pair(x: &str, y: &str) -> Option<(String, String)> {
    let x = x.trim();
    let y = y.trim();
    if x.is_empty() || y.is_empty() || x == y {
        return None;
    }
    let (a, b) = if x < y { (x, y) } else { (y, x) };
    Some((a.to_string(), b.to_string()))
}

/// Missing or corrupt file resolves to an empty topology — never an error.
pub fn load_topology(home: &Path) -> Topology {
    let path = crate::paths::topology_path_for_home(home);
    let Ok(content) = std::fs::read_to_string(&path) else {
        return Topology::default();
    };
    let Ok(mut topology) = serde_json::from_str::<Topology>(&content) else {
        return Topology::default();
    };
    canonicalize(&mut topology);
    topology
}

/// Canonicalizes + dedupes, then writes atomically (temp file + rename).
pub fn save_topology(home: &Path, topology: &Topology) -> io::Result<()> {
    let mut canonical = topology.clone();
    canonicalize(&mut canonical);
    canonical.version = TOPOLOGY_SCHEMA_VERSION;
    let path = crate::paths::topology_path_for_home(home);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(&canonical).map_err(io::Error::other)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

fn canonicalize(topology: &mut Topology) {
    let mut seen = BTreeSet::new();
    let mut edges = Vec::new();
    for edge in topology.edges.drain(..) {
        let Some((a, b)) = canonical_pair(&edge.a, &edge.b) else { continue };
        if seen.insert((a.clone(), b.clone())) {
            edges.push(TopologyEdge { a, b, created_at: edge.created_at });
        }
    }
    edges.sort_by(|left, right| (&left.a, &left.b).cmp(&(&right.a, &right.b)));
    topology.edges = edges;

    let mut seen = BTreeSet::new();
    let mut ignored = Vec::new();
    for pair in topology.ignored_pairs.drain(..) {
        let Some((a, b)) = canonical_pair(&pair.a, &pair.b) else { continue };
        if seen.insert((a.clone(), b.clone())) {
            ignored.push(IgnoredPair { a, b });
        }
    }
    ignored.sort_by(|left, right| (&left.a, &left.b).cmp(&(&right.a, &right.b)));
    topology.ignored_pairs = ignored;
}

impl Topology {
    /// Returns true if the edge was added (false: invalid pair or duplicate).
    pub fn add_edge(&mut self, x: &str, y: &str, created_at: &str) -> bool {
        let Some((a, b)) = canonical_pair(x, y) else { return false };
        if self.edges.iter().any(|edge| edge.a == a && edge.b == b) {
            return false;
        }
        self.edges.push(TopologyEdge { a, b, created_at: created_at.to_string() });
        true
    }

    /// Returns true if an edge was removed.
    pub fn remove_edge(&mut self, x: &str, y: &str) -> bool {
        let Some((a, b)) = canonical_pair(x, y) else { return false };
        let before = self.edges.len();
        self.edges.retain(|edge| !(edge.a == a && edge.b == b));
        self.edges.len() != before
    }

    pub fn ignore_pair(&mut self, x: &str, y: &str) -> bool {
        let Some((a, b)) = canonical_pair(x, y) else { return false };
        if self.ignored_pairs.iter().any(|pair| pair.a == a && pair.b == b) {
            return false;
        }
        self.ignored_pairs.push(IgnoredPair { a, b });
        true
    }

    pub fn unignore_pair(&mut self, x: &str, y: &str) -> bool {
        let Some((a, b)) = canonical_pair(x, y) else { return false };
        let before = self.ignored_pairs.len();
        self.ignored_pairs.retain(|pair| !(pair.a == a && pair.b == b));
        self.ignored_pairs.len() != before
    }

    pub fn is_ignored(&self, x: &str, y: &str) -> bool {
        canonical_pair(x, y)
            .map(|(a, b)| self.ignored_pairs.iter().any(|pair| pair.a == a && pair.b == b))
            .unwrap_or(false)
    }

    /// Drop edges/ignores referencing agents not in `known` (deleted-agent GC).
    pub fn retain_agents(&mut self, known: &BTreeSet<String>) {
        self.edges.retain(|edge| known.contains(&edge.a) && known.contains(&edge.b));
        self.ignored_pairs.retain(|pair| known.contains(&pair.a) && known.contains(&pair.b));
    }

    /// Manual neighbors of `uuid`, sorted, deduped.
    pub fn neighbors(&self, uuid: &str) -> Vec<String> {
        let mut result: Vec<String> = self
            .edges
            .iter()
            .filter_map(|edge| {
                if edge.a == uuid { Some(edge.b.clone()) }
                else if edge.b == uuid { Some(edge.a.clone()) }
                else { None }
            })
            .collect();
        result.sort();
        result.dedup();
        result
    }
}

/// Minimal team input, decoupled from CLI/app team representations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamMembership {
    pub id: String,
    pub agent_ids: Vec<String>,
}

/// Minimal agent input for the resolver.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRef {
    pub uuid: String,
    pub workspace: Option<String>,
}

pub const REASON_MANUAL: &str = "manual";
pub const RULE_WORKSPACE_FALLBACK: &str = "workspace-fallback";

/// Stale asks older than this window no longer count as active. An AwaitingReply task
/// created more than 1 hour ago is considered resolved/abandoned and won't trigger
/// edge animation in the UI.
pub const ACTIVE_ASK_WINDOW_MS: i64 = 60 * 60 * 1000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Neighbor {
    pub uuid: String,
    /// "manual", "rule:workspace-fallback"
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NeighborsView {
    pub agent_uuid: String,
    pub members: Vec<Neighbor>,
}

impl NeighborsView {
    pub fn member_uuids(&self) -> BTreeSet<String> {
        self.members.iter().map(|member| member.uuid.clone()).collect()
    }
}

/// Resolve an agent's neighbor set: manual neighbors, with workspace-fallback
/// engaging only when the agent has no manual edges. Excludes the agent itself
/// and any UUID not present in `agents`. Each member carries the reasons it is
/// visible ("manual", "rule:workspace-fallback").
///
/// Ignored pairs suppress workspace-fallback reasons for those pairs. Manual
/// edges are NOT affected by ignores; creating a manual edge implies intent.
/// Teams no longer contribute to the resolver; they seed manual edges instead.
pub fn resolve_neighbors(
    agent_uuid: &str,
    topology: &Topology,
    agents: &[AgentRef],
) -> NeighborsView {
    use std::collections::BTreeMap;

    let known: BTreeMap<&str, &AgentRef> =
        agents.iter().map(|agent| (agent.uuid.as_str(), agent)).collect();
    let mut reasons_by_member: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let push_reason = |uuid: &str, reason: String, map: &mut BTreeMap<String, Vec<String>>| {
        if uuid == agent_uuid || !known.contains_key(uuid) {
            return;
        }
        let entry = map.entry(uuid.to_string()).or_default();
        if !entry.contains(&reason) {
            entry.push(reason);
        }
    };

    let manual_neighbors = topology.neighbors(agent_uuid);
    for neighbor in &manual_neighbors {
        push_reason(neighbor, REASON_MANUAL.to_string(), &mut reasons_by_member);
    }

    let fallback_engaged = manual_neighbors.is_empty();
    if fallback_engaged {
        if let Some(workspace) = known
            .get(agent_uuid)
            .and_then(|me| me.workspace.as_deref())
            .filter(|workspace| !workspace.is_empty())
        {
            for other in agents {
                if other.workspace.as_deref() == Some(workspace) {
                    // Workspace-fallback reason is suppressed if the pair is ignored.
                    if !topology.is_ignored(agent_uuid, &other.uuid) {
                        push_reason(
                            &other.uuid,
                            format!("rule:{RULE_WORKSPACE_FALLBACK}"),
                            &mut reasons_by_member,
                        );
                    }
                }
            }
        }
    }

    NeighborsView {
        agent_uuid: agent_uuid.to_string(),
        members: reasons_by_member
            .into_iter()
            .map(|(uuid, reasons)| Neighbor { uuid, reasons })
            .collect(),
    }
}

/// Seed team cliques: add pairwise edges among member UUIDs.
/// Returns the number of edges actually added (excluding pre-existing duplicates).
/// Uses the existing `add_edge` method (idempotent, canonicalized, dedupes).
pub fn seed_team_clique(topology: &mut Topology, member_uuids: &[String], created_at: &str) -> usize {
    let mut added = 0;
    for i in 0..member_uuids.len() {
        for j in (i + 1)..member_uuids.len() {
            if topology.add_edge(&member_uuids[i], &member_uuids[j], created_at) {
                added += 1;
            }
        }
    }
    added
}

/// Check if topology needs one-time migration from version 1.
pub fn needs_team_seed_migration(topology: &Topology) -> bool {
    topology.version < 2
}

/// Read team memberships from `<home>/watchlists/index.json`. Missing/corrupt → empty.
pub fn load_team_memberships(home: &Path) -> Vec<TeamMembership> {
    let path = home.join("watchlists").join("index.json");
    let Ok(content) = std::fs::read_to_string(path) else { return Vec::new() };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&content) else { return Vec::new() };
    value
        .get("teams")
        .and_then(|teams| teams.as_array())
        .map(|teams| {
            teams
                .iter()
                .filter_map(|team| {
                    let id = team.get("id")?.as_str()?.to_string();
                    let agent_ids = team
                        .get("agentIds")
                        .or_else(|| team.get("agent_ids"))?
                        .as_array()?
                        .iter()
                        .filter_map(|id| id.as_str().map(str::to_string))
                        .collect();
                    Some(TeamMembership { id, agent_ids })
                })
                .collect()
        })
        .unwrap_or_default()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PairActivity {
    pub a: String,
    pub b: String,
    pub last_message_at: String,
    pub active_ask: bool,
    /// UUID of the agent that owes a reply on an open ask, if any.
    pub awaiting_reply_from: Option<String>,
}

/// Aggregate interaction records into per-pair activity.
///
/// `last_message_at` is selected by lexicographic comparison of `created_at`,
/// which is correct only because interaction timestamps are written by a
/// single source in UTC RFC3339 with a fixed format. Mixed offsets would
/// compare incorrectly.
///
/// `now_ms` is the current epoch-milliseconds reference. An AwaitingReply task
/// record is only marked `active_ask: true` if its `created_at` is within
/// ACTIVE_ASK_WINDOW_MS of `now_ms`; older records contribute `last_message_at`
/// but do not set `active_ask`. Unparseable timestamps are treated as stale.
pub fn pair_activity_from_records(
    records: &[crate::control::InteractionRecord],
    now_ms: i64,
) -> Vec<PairActivity> {
    use crate::control::{InteractionKind, InteractionStatus};
    use std::collections::BTreeMap;

    let mut by_pair: BTreeMap<(String, String), PairActivity> = BTreeMap::new();
    for record in records {
        let Some(sender) = record.sender_session_id.as_deref() else { continue };
        for target in &record.target_session_ids {
            let Some((a, b)) = canonical_pair(sender, target) else { continue };
            let entry = by_pair.entry((a.clone(), b.clone())).or_insert(PairActivity {
                a,
                b,
                last_message_at: record.created_at.clone(),
                active_ask: false,
                awaiting_reply_from: None,
            });
            if record.created_at > entry.last_message_at {
                entry.last_message_at = record.created_at.clone();
            }
            if record.kind == InteractionKind::Task
                && record.status == InteractionStatus::AwaitingReply
            {
                // Parse created_at to check if it's within the active ask window.
                let is_within_window = chrono::DateTime::parse_from_rfc3339(&record.created_at)
                    .ok()
                    .map(|dt| {
                        let record_ms = dt.timestamp_millis();
                        let age_ms = now_ms - record_ms;
                        (0..=ACTIVE_ASK_WINDOW_MS).contains(&age_ms)
                    })
                    .unwrap_or(false);

                if is_within_window {
                    entry.active_ask = true;
                    entry.awaiting_reply_from = Some(target.clone());
                }
            }
        }
    }
    by_pair.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn canonical_pair_orders_and_rejects_self_and_empty() {
        assert_eq!(canonical_pair("b", "a"), Some(("a".into(), "b".into())));
        assert_eq!(canonical_pair("a", "a"), None);
        assert_eq!(canonical_pair("", "a"), None);
        assert_eq!(canonical_pair("a", "  "), None);
    }

    #[test]
    fn add_edge_canonicalizes_and_dedupes() {
        let mut topology = Topology::default();
        assert!(topology.add_edge("b", "a", "2026-07-02T00:00:00Z"));
        assert!(!topology.add_edge("a", "b", "2026-07-02T00:00:01Z"));
        assert!(!topology.add_edge("a", "a", "2026-07-02T00:00:02Z"));
        assert_eq!(topology.edges.len(), 1);
        assert_eq!(topology.edges[0].a, "a");
        assert_eq!(topology.edges[0].b, "b");
        assert_eq!(topology.neighbors("a"), vec!["b".to_string()]);
        assert_eq!(topology.neighbors("b"), vec!["a".to_string()]);
        assert!(topology.neighbors("c").is_empty());
    }

    #[test]
    fn remove_edge_accepts_either_order() {
        let mut topology = Topology::default();
        topology.add_edge("a", "b", "t");
        assert!(topology.remove_edge("b", "a"));
        assert!(!topology.remove_edge("b", "a"));
        assert!(topology.edges.is_empty());
    }

    #[test]
    fn ignore_pair_roundtrip() {
        let mut topology = Topology::default();
        assert!(topology.ignore_pair("b", "a"));
        assert!(!topology.ignore_pair("a", "b"));
        assert!(topology.is_ignored("a", "b"));
        assert!(topology.is_ignored("b", "a"));
        assert!(topology.unignore_pair("a", "b"));
        assert!(!topology.is_ignored("a", "b"));
    }

    #[test]
    fn retain_agents_drops_unknown_references() {
        let mut topology = Topology::default();
        topology.add_edge("a", "b", "t");
        topology.add_edge("a", "gone", "t");
        topology.ignore_pair("gone", "b");
        let known: BTreeSet<String> = ["a".to_string(), "b".to_string()].into();
        topology.retain_agents(&known);
        assert_eq!(topology.edges.len(), 1);
        assert!(topology.ignored_pairs.is_empty());
    }

    #[test]
    fn load_missing_or_corrupt_file_yields_empty_topology() {
        let temp = tempfile::tempdir().unwrap();
        assert_eq!(load_topology(temp.path()), Topology::default());
        std::fs::write(temp.path().join("topology.json"), "{not json").unwrap();
        assert_eq!(load_topology(temp.path()), Topology::default());
    }

    #[test]
    fn save_and_load_roundtrip_canonicalizes() {
        let temp = tempfile::tempdir().unwrap();
        let mut topology = Topology::default();
        topology.edges.push(TopologyEdge { a: "z".into(), b: "a".into(), created_at: "t".into() });
        topology.edges.push(TopologyEdge { a: "a".into(), b: "z".into(), created_at: "t2".into() });
        save_topology(temp.path(), &topology).unwrap();
        let loaded = load_topology(temp.path());
        assert_eq!(loaded.edges.len(), 1);
        assert_eq!(loaded.edges[0].a, "a");
        assert_eq!(loaded.edges[0].b, "z");
        assert!(!temp.path().join("topology.json.tmp").exists());
    }

    fn agent(uuid: &str, workspace: Option<&str>) -> AgentRef {
        AgentRef { uuid: uuid.to_string(), workspace: workspace.map(str::to_string) }
    }

    #[test]
    fn neighbors_returns_manual_edges_only() {
        let mut topology = Topology::default();
        topology.add_edge("me", "friend", "t");
        let agents = vec![agent("me", None), agent("friend", None), agent("mate", None)];

        let view = resolve_neighbors("me", &topology, &agents);

        let friend = view.members.iter().find(|m| m.uuid == "friend").unwrap();
        assert_eq!(friend.reasons, vec!["manual"]);
        assert_eq!(view.members.len(), 1);
    }

    #[test]
    fn workspace_fallback_engages_only_when_edgeless() {
        let topology = Topology::default();
        let agents = vec![
            agent("me", Some("D:/ws")),
            agent("mate", Some("D:/ws")),
            agent("other", Some("D:/elsewhere")),
            agent("floating", None),
        ];

        let view = resolve_neighbors("me", &topology, &agents);

        assert_eq!(view.members.len(), 1);
        assert_eq!(view.members[0].uuid, "mate");
        assert_eq!(view.members[0].reasons, vec!["rule:workspace-fallback"]);
    }

    #[test]
    fn first_manual_edge_disengages_workspace_fallback() {
        let mut topology = Topology::default();
        topology.add_edge("me", "remote", "t");
        let agents = vec![
            agent("me", Some("D:/ws")),
            agent("mate", Some("D:/ws")),
            agent("remote", Some("D:/elsewhere")),
        ];

        let view = resolve_neighbors("me", &topology, &agents);

        assert_eq!(view.member_uuids(), ["remote".to_string()].into());
    }

    #[test]
    fn neighbors_excludes_self_and_unknown_agents() {
        let mut topology = Topology::default();
        topology.add_edge("me", "me-too-deleted", "t");
        let agents = vec![agent("me", None)];

        let view = resolve_neighbors("me", &topology, &agents);

        assert!(view.members.is_empty());
    }

    #[test]
    fn load_team_memberships_reads_v2_index() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("watchlists");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("index.json"),
            r#"{"version":2,"teams":[{"id":"t1","name":"Review","agentIds":["a","b"]}],"watchlists":[]}"#,
        )
        .unwrap();

        let teams = load_team_memberships(temp.path());

        assert_eq!(teams.len(), 1);
        assert_eq!(teams[0].id, "t1");
        assert_eq!(teams[0].agent_ids, vec!["a", "b"]);
    }

    #[test]
    fn load_team_memberships_missing_file_is_empty() {
        let temp = tempfile::tempdir().unwrap();
        assert!(load_team_memberships(temp.path()).is_empty());
    }

    fn record(
        kind: crate::control::InteractionKind,
        sender: &str,
        target: &str,
        status: crate::control::InteractionStatus,
        created_at: &str,
    ) -> crate::control::InteractionRecord {
        crate::control::InteractionRecord {
            id: format!("int_{created_at}"),
            kind,
            sender_session_id: Some(sender.to_string()),
            target_session_ids: vec![target.to_string()],
            status,
            trigger_policy: crate::control::InteractionTriggerPolicy::NotifyOnly,
            body_ref: crate::control::InteractionBodyRef::Inline { body: "x".into() },
            parent_interaction_id: None,
            created_at: created_at.to_string(),
            updated_at: created_at.to_string(),
            completed_at: None,
        }
    }

    #[test]
    fn pair_activity_aggregates_last_message_and_open_asks() {
        use crate::control::{InteractionKind, InteractionStatus};
        let now_ms = 1782990000000i64; // 2026-07-02T11:00:00Z in epoch ms
        let records = vec![
            record(InteractionKind::Message, "a", "b", InteractionStatus::Completed, "2026-07-02T10:00:00Z"),
            record(InteractionKind::Message, "b", "a", InteractionStatus::Completed, "2026-07-02T11:00:00Z"),
            record(InteractionKind::Task, "a", "c", InteractionStatus::AwaitingReply, "2026-07-02T10:30:00Z"),
        ];

        let mut activity = pair_activity_from_records(&records, now_ms);
        activity.sort_by(|l, r| (&l.a, &l.b).cmp(&(&r.a, &r.b)));

        assert_eq!(activity.len(), 2);
        let ab = &activity[0];
        assert_eq!((ab.a.as_str(), ab.b.as_str()), ("a", "b"));
        assert_eq!(ab.last_message_at, "2026-07-02T11:00:00Z");
        assert!(!ab.active_ask);
        let ac = &activity[1];
        assert!(ac.active_ask, "Task from 10:30:00 should be active as it's within 1 hour of 11:00:00");
        assert_eq!(ac.awaiting_reply_from.as_deref(), Some("c"));
    }

    #[test]
    fn pair_activity_skips_senderless_records() {
        use crate::control::{InteractionKind, InteractionStatus};
        let now_ms = 1782990000000i64;
        let mut senderless = record(InteractionKind::Message, "a", "b", InteractionStatus::Completed, "2026-07-02T11:00:00Z");
        senderless.sender_session_id = None;
        assert!(pair_activity_from_records(&[senderless], now_ms).is_empty());
    }

    #[test]
    fn ignored_pair_does_not_suppress_manual_reason() {
        let mut topology = Topology::default();
        topology.add_edge("me", "mate", "t");
        topology.ignore_pair("me", "mate");
        let agents = vec![agent("me", None), agent("mate", None)];

        let view = resolve_neighbors("me", &topology, &agents);

        assert_eq!(view.members.len(), 1);
        let mate = view.members.iter().find(|m| m.uuid == "mate").unwrap();
        assert_eq!(mate.reasons, vec!["manual"]);
    }

    #[test]
    fn ignored_pair_suppresses_workspace_fallback_member() {
        let mut topology = Topology::default();
        topology.ignore_pair("me", "mate");
        let agents = vec![
            agent("me", Some("D:/ws")),
            agent("mate", Some("D:/ws")),
            agent("other", Some("D:/ws")),
        ];

        let view = resolve_neighbors("me", &topology, &agents);

        // Should include "other" but not "mate" (ignored)
        assert_eq!(view.members.len(), 1);
        assert_eq!(view.members[0].uuid, "other");
        assert_eq!(view.members[0].reasons, vec!["rule:workspace-fallback"]);
    }

    #[test]
    fn ignored_pairs_do_not_affect_fallback_engagement() {
        let mut topology = Topology::default();
        topology.ignore_pair("me", "mate");
        let agents = vec![
            agent("me", Some("D:/ws")),
            agent("mate", Some("D:/ws")),
        ];

        let view = resolve_neighbors("me", &topology, &agents);

        // Since mate is the only workspace-fallback candidate and is ignored,
        // the agent is still considered "fallback engaged" (no manual edges, no teams).
        // This tests that engagement is determined from raw input, not filtered by ignores.
        // With only 1 agent in workspace, fallback engagement is determined by emptiness of manual/teams,
        // but members might be empty due to ignores. The key is: fallback_engaged computed correctly.
        assert!(view.members.is_empty(), "mate is the only fallback candidate, but ignored");
    }

    #[test]
    fn pair_activity_old_awaiting_reply_not_active_ask() {
        use crate::control::{InteractionKind, InteractionStatus};

        // now = 2026-07-02T11:00:00Z
        let now_ms = 1782990000000i64;
        let hour_ms = 60 * 60 * 1000;
        let old_timestamp = (now_ms - 2 * hour_ms) as u64;

        // Convert epoch ms to RFC3339
        let old_dt = chrono::DateTime::<chrono::Utc>::from(
            std::time::UNIX_EPOCH + std::time::Duration::from_millis(old_timestamp)
        );
        let old_timestamp_rfc3339 = old_dt.to_rfc3339();

        let records = vec![
            record(InteractionKind::Task, "a", "b", InteractionStatus::AwaitingReply, &old_timestamp_rfc3339),
        ];

        let activity = pair_activity_from_records(&records, now_ms);

        assert_eq!(activity.len(), 1);
        assert!(!activity[0].active_ask, "old awaiting_reply should not be active_ask");
        assert_eq!(activity[0].last_message_at, old_timestamp_rfc3339, "last_message_at should still be set");
    }

    #[test]
    fn pair_activity_fresh_awaiting_reply_is_active_ask() {
        use crate::control::{InteractionKind, InteractionStatus};

        let now_ms = 1782990000000i64; // 2026-07-02T11:00:00Z
        let thirty_min_ago = (now_ms - 30 * 60 * 1000) as u64;
        let dt = chrono::DateTime::<chrono::Utc>::from(
            std::time::UNIX_EPOCH + std::time::Duration::from_millis(thirty_min_ago)
        );
        let fresh_timestamp_rfc3339 = dt.to_rfc3339();

        let records = vec![
            record(InteractionKind::Task, "a", "b", InteractionStatus::AwaitingReply, &fresh_timestamp_rfc3339),
        ];

        let activity = pair_activity_from_records(&records, now_ms);

        assert_eq!(activity.len(), 1);
        assert!(activity[0].active_ask, "fresh awaiting_reply should be active_ask");
        assert_eq!(activity[0].awaiting_reply_from.as_deref(), Some("b"));
    }

    #[test]
    fn seed_team_clique_creates_pairwise_edges() {
        let mut topology = Topology::default();
        let members = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let added = seed_team_clique(&mut topology, &members, "2026-07-02T00:00:00Z");

        // 3 members -> 3 edges: (a,b), (a,c), (b,c)
        assert_eq!(added, 3);
        assert_eq!(topology.edges.len(), 3);
        assert!(topology.neighbors("a").contains(&"b".to_string()));
        assert!(topology.neighbors("a").contains(&"c".to_string()));
        assert!(topology.neighbors("b").contains(&"c".to_string()));
    }

    #[test]
    fn seed_team_clique_skips_existing_edges() {
        let mut topology = Topology::default();
        topology.add_edge("a", "b", "old");

        let members = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let added = seed_team_clique(&mut topology, &members, "2026-07-02T00:00:00Z");

        // Should only add (a,c) and (b,c); (a,b) was already there
        assert_eq!(added, 2);
        assert_eq!(topology.edges.len(), 3);
    }

    #[test]
    fn needs_team_seed_migration_detects_version_1() {
        let topology = Topology { version: 1, edges: vec![], ignored_pairs: vec![] };
        assert!(needs_team_seed_migration(&topology));
    }

    #[test]
    fn needs_team_seed_migration_false_for_version_2() {
        let topology = Topology { version: 2, edges: vec![], ignored_pairs: vec![] };
        assert!(!needs_team_seed_migration(&topology));
    }
}
