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
}
