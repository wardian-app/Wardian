use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::io;
use std::path::Path;

pub const TOPOLOGY_SCHEMA_VERSION: u8 = 1;

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
pub const RULE_TEAM_CLIQUE: &str = "team-clique";
pub const RULE_WORKSPACE_FALLBACK: &str = "workspace-fallback";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CommunityMember {
    pub uuid: String,
    /// "manual", "rule:team-clique:<team-id>", "rule:workspace-fallback"
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CommunityView {
    pub agent_uuid: String,
    pub members: Vec<CommunityMember>,
}

impl CommunityView {
    pub fn member_uuids(&self) -> BTreeSet<String> {
        self.members.iter().map(|member| member.uuid.clone()).collect()
    }
}

pub fn resolve_community(
    agent_uuid: &str,
    topology: &Topology,
    teams: &[TeamMembership],
    agents: &[AgentRef],
) -> CommunityView {
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

    let my_teams: Vec<&TeamMembership> = teams
        .iter()
        .filter(|team| team.agent_ids.iter().any(|id| id == agent_uuid))
        .collect();
    for team in &my_teams {
        for member in &team.agent_ids {
            push_reason(
                member,
                format!("rule:{RULE_TEAM_CLIQUE}:{}", team.id),
                &mut reasons_by_member,
            );
        }
    }

    let fallback_engaged = manual_neighbors.is_empty() && my_teams.is_empty();
    if fallback_engaged {
        if let Some(workspace) = known
            .get(agent_uuid)
            .and_then(|me| me.workspace.as_deref())
            .filter(|workspace| !workspace.is_empty())
        {
            for other in agents {
                if other.workspace.as_deref() == Some(workspace) {
                    push_reason(
                        &other.uuid,
                        format!("rule:{RULE_WORKSPACE_FALLBACK}"),
                        &mut reasons_by_member,
                    );
                }
            }
        }
    }

    CommunityView {
        agent_uuid: agent_uuid.to_string(),
        members: reasons_by_member
            .into_iter()
            .map(|(uuid, reasons)| CommunityMember { uuid, reasons })
            .collect(),
    }
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
pub fn pair_activity_from_records(
    records: &[crate::control::InteractionRecord],
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
                entry.active_ask = true;
                entry.awaiting_reply_from = Some(target.clone());
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
    fn community_unions_manual_and_team_clique_with_reasons() {
        let mut topology = Topology::default();
        topology.add_edge("me", "friend", "t");
        let teams = vec![TeamMembership {
            id: "team-1".into(),
            agent_ids: vec!["me".into(), "mate".into(), "friend".into()],
        }];
        let agents = vec![agent("me", None), agent("friend", None), agent("mate", None)];

        let view = resolve_community("me", &topology, &teams, &agents);

        let friend = view.members.iter().find(|m| m.uuid == "friend").unwrap();
        assert_eq!(friend.reasons, vec!["manual", "rule:team-clique:team-1"]);
        let mate = view.members.iter().find(|m| m.uuid == "mate").unwrap();
        assert_eq!(mate.reasons, vec!["rule:team-clique:team-1"]);
        assert_eq!(view.members.len(), 2);
    }

    #[test]
    fn workspace_fallback_engages_only_when_edgeless_and_teamless() {
        let topology = Topology::default();
        let agents = vec![
            agent("me", Some("D:/ws")),
            agent("mate", Some("D:/ws")),
            agent("other", Some("D:/elsewhere")),
            agent("floating", None),
        ];

        let view = resolve_community("me", &topology, &[], &agents);

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

        let view = resolve_community("me", &topology, &[], &agents);

        assert_eq!(view.member_uuids(), ["remote".to_string()].into());
    }

    #[test]
    fn team_membership_disengages_workspace_fallback_even_with_no_teammates_visible() {
        let topology = Topology::default();
        let teams = vec![TeamMembership { id: "t1".into(), agent_ids: vec!["me".into()] }];
        let agents = vec![agent("me", Some("D:/ws")), agent("mate", Some("D:/ws"))];

        let view = resolve_community("me", &topology, &teams, &agents);

        assert!(view.members.is_empty());
    }

    #[test]
    fn community_excludes_self_and_unknown_agents() {
        let mut topology = Topology::default();
        topology.add_edge("me", "me-too-deleted", "t");
        let agents = vec![agent("me", None)];

        let view = resolve_community("me", &topology, &[], &agents);

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
        let records = vec![
            record(InteractionKind::Message, "a", "b", InteractionStatus::Completed, "2026-07-02T10:00:00Z"),
            record(InteractionKind::Message, "b", "a", InteractionStatus::Completed, "2026-07-02T11:00:00Z"),
            record(InteractionKind::Task, "a", "c", InteractionStatus::AwaitingReply, "2026-07-02T09:00:00Z"),
        ];

        let mut activity = pair_activity_from_records(&records);
        activity.sort_by(|l, r| (&l.a, &l.b).cmp(&(&r.a, &r.b)));

        assert_eq!(activity.len(), 2);
        let ab = &activity[0];
        assert_eq!((ab.a.as_str(), ab.b.as_str()), ("a", "b"));
        assert_eq!(ab.last_message_at, "2026-07-02T11:00:00Z");
        assert!(!ab.active_ask);
        let ac = &activity[1];
        assert!(ac.active_ask);
        assert_eq!(ac.awaiting_reply_from.as_deref(), Some("c"));
    }

    #[test]
    fn pair_activity_skips_senderless_records() {
        use crate::control::{InteractionKind, InteractionStatus};
        let mut senderless = record(InteractionKind::Message, "a", "b", InteractionStatus::Completed, "t");
        senderless.sender_session_id = None;
        assert!(pair_activity_from_records(&[senderless]).is_empty());
    }
}
