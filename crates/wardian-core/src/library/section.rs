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

/// True if `name` is exactly one normal path component: no empty string,
/// no `.`/`..`, no separators, no root/prefix (e.g. Windows `C:`), and no
/// trailing-separator padding that would otherwise normalize away.
///
/// This is deliberately structural rather than purely character-based: a
/// character blocklist alone (rejecting `/`, `\`, `.`, `..`) still lets
/// strings like `"C:"` or `"C:evil"` through, and on Windows
/// `PathBuf::join` treats a joined path with a drive prefix as an
/// absolute replacement of the base rather than a sub-path, letting the
/// join escape the intended directory entirely.
///
/// The structural check alone is not enough, though: `Path::components()`
/// only treats `\` as a separator on Windows, so on Unix a name like
/// `"a\\b"` parses as a single `Normal` component and would otherwise
/// pass here while being rejected on Windows — an OS-dependent result for
/// the same input. Library entry names must be portable across OSes (a
/// name created on Linux may later be read on Windows and vice versa), so
/// both `/` and `\` are rejected explicitly on every platform regardless
/// of what the host OS considers a separator.
pub fn is_single_normal_component(name: &str) -> bool {
    if name.contains('/') || name.contains('\\') {
        return false;
    }
    let mut components = Path::new(name).components();
    matches!(
        (components.next(), components.next()),
        (Some(Component::Normal(part)), None) if part == std::ffi::OsStr::new(name)
    )
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
                if text.eq_ignore_ascii_case(DEPLOYED_SKILL_SOURCE_FILE) {
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
        assert!(resolve_entry_path(home, LibrarySectionId::Skills, "dev/.Wardian-Skill-Source").is_err());
        assert!(resolve_entry_path(home, LibrarySectionId::Mcps, "anything").is_err()); // stubbed section: no paths
    }

    #[test]
    fn is_single_normal_component_accepts_plain_names() {
        assert!(is_single_normal_component("planner"));
        assert!(is_single_normal_component("my-skill_v2"));
    }

    #[test]
    fn is_single_normal_component_rejects_empty_and_dots() {
        assert!(!is_single_normal_component(""));
        assert!(!is_single_normal_component("."));
        assert!(!is_single_normal_component(".."));
    }

    // These must NOT be cfg-gated: `\` is only a path separator on
    // Windows, so a character-blind structural check alone would accept
    // "a\\b" as a single Normal component on Unix while Windows rejects
    // it. Both separators are rejected explicitly on every platform so
    // the result is identical everywhere.
    #[test]
    fn is_single_normal_component_rejects_path_separators_everywhere() {
        assert!(!is_single_normal_component("a/b"));
        assert!(!is_single_normal_component("a\\b"));
    }

    // On Windows, `Path::components()` parses a leading `C:` as a
    // `Prefix` component distinct from the rest, so `"C:evil"` fails the
    // single `Normal` component check and is rejected. On Unix there is
    // no drive prefix concept, so `"C:evil"` is just an ordinary (if odd)
    // file name and is a valid single component — this assertion is
    // Windows-only.
    #[cfg(windows)]
    #[test]
    fn is_single_normal_component_rejects_windows_drive_prefix() {
        assert!(!is_single_normal_component("C:evil"));
    }
}
