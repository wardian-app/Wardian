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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillDeployment {
    pub target_type: String,
    pub target_id: String,
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
}
