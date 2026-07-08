use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LibraryItemMetadata {
    pub id: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub is_starred: bool,
    pub last_used: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub struct LibraryPrompt {
    pub path: String,
    pub name: String,
    pub content: String,
    pub metadata: LibraryItemMetadata,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub struct LibrarySkill {
    pub path: String,
    pub name: String,
    pub description: String,
    pub content: String,
    pub metadata: LibraryItemMetadata,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LibraryFolder {
    pub path: String,
    pub name: String,
    pub children: Vec<LibraryNode>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum LibraryNode {
    Folder(LibraryFolder),
    Prompt(LibraryPrompt),
    Skill(LibrarySkill),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillDeployment {
    pub target_type: String,
    pub target_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeployedSkillRef {
    pub name: String,
    pub source_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LibraryEntry {
    pub kind: String,
    pub path: String,
    pub entry_ref: String,
    pub name: String,
    pub description: String,
    pub tags: Vec<String>,
    pub is_starred: bool,
    pub deployment_count: u32,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LibraryIndexFolder {
    pub path: String,
    pub name: String,
    pub children: Vec<LibraryIndexNode>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum LibraryIndexNode {
    Folder(LibraryIndexFolder),
    Entry(LibraryEntry),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LibrarySection {
    pub tree: LibraryIndexFolder,
    pub stubbed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DeploymentTarget {
    pub target_type: String,
    pub target_id: String,
    pub linked: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OrphanDeployment {
    pub target_type: String,
    pub target_id: String,
    pub skill_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LibraryIndex {
    pub sections: std::collections::HashMap<String, LibrarySection>,
    pub deployments: std::collections::HashMap<String, Vec<DeploymentTarget>>,
    pub orphans: Vec<OrphanDeployment>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_library_item_metadata_serialization() {
        let metadata = LibraryItemMetadata {
            id: "uuid-123".to_string(),
            tags: vec!["rust".to_string(), "tauri".to_string()],
            is_starred: true,
            last_used: None,
        };

        let json = serde_json::to_string(&metadata).unwrap();
        let deserialized: LibraryItemMetadata = serde_json::from_str(&json).unwrap();

        assert_eq!(metadata.id, deserialized.id);
        assert_eq!(metadata.tags, deserialized.tags);
        assert_eq!(metadata.is_starred, deserialized.is_starred);
    }

    #[test]
    fn library_index_round_trips_with_snake_case() {
        let entry = LibraryEntry {
            kind: "skill".into(),
            path: "dev/planner".into(),
            entry_ref: "skills/dev/planner".into(),
            name: "planner".into(),
            description: "Plans work".into(),
            tags: vec!["dev".into()],
            is_starred: false,
            deployment_count: 2,
            error: None,
        };
        let node = LibraryIndexNode::Entry(entry.clone());
        let json = serde_json::to_string(&node).unwrap();
        assert!(json.contains("\"entry_ref\""));
        match serde_json::from_str::<LibraryIndexNode>(&json).unwrap() {
            LibraryIndexNode::Entry(parsed) => assert_eq!(parsed, entry),
            LibraryIndexNode::Folder(_) => panic!("entry must not parse as folder"),
        }

        let folder = LibraryIndexFolder {
            path: "".into(),
            name: "Root".into(),
            children: vec![node],
        };
        let json = serde_json::to_string(&folder).unwrap();
        let parsed: LibraryIndexFolder = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.children.len(), 1);
    }
}
