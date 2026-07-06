use std::path::{Component, Path, PathBuf};

pub const DEPLOYED_SKILL_SOURCE_FILE: &str = ".wardian-skill-source";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LibrarySectionId {
    Skills,
    Prompts,
    Workflows,
    Classes,
    Mcps,
}

impl LibrarySectionId {
    pub const ALL: [LibrarySectionId; 5] = [
        LibrarySectionId::Skills,
        LibrarySectionId::Prompts,
        LibrarySectionId::Workflows,
        LibrarySectionId::Classes,
        LibrarySectionId::Mcps,
    ];

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "skills" => Some(Self::Skills),
            "prompts" => Some(Self::Prompts),
            "workflows" => Some(Self::Workflows),
            "classes" => Some(Self::Classes),
            "mcps" => Some(Self::Mcps),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Skills => "skills",
            Self::Prompts => "prompts",
            Self::Workflows => "workflows",
            Self::Classes => "classes",
            Self::Mcps => "mcps",
        }
    }

    pub fn root_for_home(&self, home: &Path) -> PathBuf {
        match self {
            Self::Skills => home.join("library").join("skills"),
            Self::Prompts => home.join("library").join("prompts"),
            Self::Workflows => home.join("library").join("workflows"),
            Self::Classes => home.join("classes"),
            // Stubbed: no directory is created until the MCP feature lands.
            Self::Mcps => home.join("library").join("mcps"),
        }
    }
}

/// Resolve a section-relative entry path, rejecting traversal, absolute
/// paths, empty paths, reserved file names, and the stubbed MCP section.
pub fn resolve_entry_path(
    home: &Path,
    section: LibrarySectionId,
    rel: &str,
) -> Result<PathBuf, String> {
    if section == LibrarySectionId::Mcps {
        return Err("The MCP section is not yet writable".to_string());
    }
    let normalized = rel.replace('\\', "/");
    if normalized.trim().is_empty() {
        return Err("Entry path must not be empty".to_string());
    }
    let candidate = Path::new(&normalized);
    if candidate.is_absolute() {
        return Err(format!("Entry path must be relative: {rel}"));
    }
    for component in candidate.components() {
        match component {
            Component::Normal(part) => {
                let text = part.to_string_lossy();
                if text == DEPLOYED_SKILL_SOURCE_FILE {
                    return Err(format!("Reserved name in entry path: {text}"));
                }
            }
            _ => return Err(format!("Invalid entry path: {rel}")),
        }
    }
    Ok(section.root_for_home(home).join(candidate))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn section_roots_resolve_under_home() {
        let home = Path::new("/tmp/wh");
        assert_eq!(LibrarySectionId::Skills.root_for_home(home), home.join("library").join("skills"));
        assert_eq!(LibrarySectionId::Prompts.root_for_home(home), home.join("library").join("prompts"));
        assert_eq!(LibrarySectionId::Workflows.root_for_home(home), home.join("library").join("workflows"));
        assert_eq!(LibrarySectionId::Classes.root_for_home(home), home.join("classes"));
    }

    #[test]
    fn parse_round_trips() {
        for id in ["skills", "prompts", "workflows", "classes", "mcps"] {
            assert_eq!(LibrarySectionId::parse(id).unwrap().as_str(), id);
        }
        assert!(LibrarySectionId::parse("plugins").is_none());
    }

    #[test]
    fn resolve_entry_path_rejects_escapes() {
        let home = Path::new("/tmp/wh");
        assert!(resolve_entry_path(home, LibrarySectionId::Skills, "../evil").is_err());
        assert!(resolve_entry_path(home, LibrarySectionId::Skills, "a/../../evil").is_err());
        assert!(resolve_entry_path(home, LibrarySectionId::Skills, "/abs").is_err());
        assert!(resolve_entry_path(home, LibrarySectionId::Skills, "").is_err());
        assert!(resolve_entry_path(home, LibrarySectionId::Skills, "dev/planner").is_ok());
    }

    #[test]
    fn resolve_entry_path_rejects_reserved_names() {
        let home = Path::new("/tmp/wh");
        assert!(resolve_entry_path(home, LibrarySectionId::Skills, "dev/.wardian-skill-source").is_err());
        assert!(resolve_entry_path(home, LibrarySectionId::Mcps, "anything").is_err()); // stubbed section: no paths
    }
}
