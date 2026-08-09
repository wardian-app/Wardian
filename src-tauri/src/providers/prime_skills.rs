//! Discovers Prime Agent skills so Wardian can offer to promote them.
//!
//! Prime's skill creator writes a new skill into the project as a directory
//! with a `SKILL.md`, which is the same Agent Skills layout Wardian's Library
//! uses. That shared format is what makes the return direction cheap: an agent
//! authoring a first-class Wardian artifact needs a copy, not a conversion.
//!
//! Discovery is read-only. Nothing here writes to a workspace or to the
//! Library; promotion is the user's decision.

use std::path::{Path, PathBuf};

/// Where Prime writes a skill scoped to one project.
///
/// Prime also reads a global `~/.prime/agent/skills` and skills bundled inside
/// npm packages, but only the project directory holds skills authored for the
/// workspace an agent is working in. Promoting a globally installed or
/// package-bundled skill would copy someone else's artifact into the Library.
pub fn project_skills_dir(workspace: &Path) -> PathBuf {
    workspace.join(".prime").join("agent").join("skills")
}

/// A skill found on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredPrimeSkill {
    /// The declared name, which is what Prime and the Library both key on.
    pub name: String,
    pub description: Option<String>,
    pub directory: PathBuf,
    /// True when the skill ships a Python package callable from the kernel,
    /// rather than being instructions alone.
    pub is_python_backed: bool,
}

impl DiscoveredPrimeSkill {
    /// True when the skill carries enough to be listed and understood.
    ///
    /// Prime warns about and refuses a skill with no description, so one that
    /// lacks it is unfinished rather than promotable.
    pub fn is_complete(&self) -> bool {
        self.description
            .as_deref()
            .is_some_and(|description| !description.trim().is_empty())
    }
}

/// Reads the `name` and `description` from a `SKILL.md` frontmatter block.
///
/// Only these two keys are read, because they are the only ones Prime loads
/// into the system prompt at startup and the only ones the Library needs to
/// list an entry. Values may be quoted; a missing or unterminated frontmatter
/// block yields nothing rather than a guess at the body.
pub fn parse_skill_manifest(contents: &str) -> Option<(String, Option<String>)> {
    let mut lines = contents.lines();
    // The block must open on the first non-empty line; a `---` further down is
    // a horizontal rule in the body.
    let opened = lines
        .by_ref()
        .find(|line| !line.trim().is_empty())
        .is_some_and(|line| line.trim() == "---");
    if !opened {
        return None;
    }

    let mut name = None;
    let mut description = None;
    let mut closed = false;

    for line in lines {
        let trimmed = line.trim();
        if trimmed == "---" {
            closed = true;
            break;
        }
        let Some((key, value)) = trimmed.split_once(':') else {
            continue;
        };
        let value = unquote(value.trim());
        match key.trim() {
            "name" if name.is_none() => name = Some(value.to_string()),
            "description" if description.is_none() => description = Some(value.to_string()),
            _ => {}
        }
    }

    // An unterminated block means the file is malformed, and treating its
    // whole body as frontmatter would invent a skill from prose.
    if !closed {
        return None;
    }

    let name = name
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())?;

    Some((
        name,
        description
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty()),
    ))
}

fn unquote(value: &str) -> &str {
    let bytes = value.as_bytes();
    if bytes.len() >= 2
        && ((bytes[0] == b'"' && bytes[bytes.len() - 1] == b'"')
            || (bytes[0] == b'\'' && bytes[bytes.len() - 1] == b'\''))
    {
        return &value[1..value.len() - 1];
    }

    value
}

/// Lists the skills directly inside a skills directory.
///
/// Only one level deep: a skill is a directory holding `SKILL.md`, and a
/// nested match would be a reference or fixture belonging to the skill above
/// it rather than a skill of its own.
pub fn discover_skills(skills_dir: &Path) -> Vec<DiscoveredPrimeSkill> {
    let Ok(entries) = std::fs::read_dir(skills_dir) else {
        return Vec::new();
    };

    let mut skills = entries
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| {
            let directory = entry.path();
            let manifest = std::fs::read_to_string(directory.join("SKILL.md")).ok()?;
            let (name, description) = parse_skill_manifest(&manifest)?;

            Some(DiscoveredPrimeSkill {
                name,
                description,
                is_python_backed: directory.join("pyproject.toml").is_file(),
                directory,
            })
        })
        .collect::<Vec<_>>();

    // Stable order so a promotion prompt does not reshuffle between scans.
    skills.sort_by(|left, right| left.name.cmp(&right.name));
    skills
}

/// Narrows discovered skills to the ones worth offering to promote.
///
/// A skill already in the Library under the same name is left alone: the
/// Library copy is the one deployed to agents, so offering to overwrite it
/// from a workspace would silently take the project's version as authoritative.
pub fn promotable_skills(
    discovered: Vec<DiscoveredPrimeSkill>,
    library_skill_names: &[String],
) -> Vec<DiscoveredPrimeSkill> {
    discovered
        .into_iter()
        .filter(|skill| skill.is_complete())
        .filter(|skill| {
            !library_skill_names
                .iter()
                .any(|existing| existing.eq_ignore_ascii_case(&skill.name))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_skill(root: &Path, name: &str, manifest: &str, python_backed: bool) -> PathBuf {
        let directory = root.join(name);
        std::fs::create_dir_all(&directory).expect("skill dir");
        std::fs::write(directory.join("SKILL.md"), manifest).expect("SKILL.md");
        if python_backed {
            std::fs::write(directory.join("pyproject.toml"), "[project]\n").expect("pyproject");
        }
        directory
    }

    #[test]
    fn reads_the_two_keys_prime_loads_at_startup() {
        // Frontmatter shaped after prime-agent's bundled websearch skill.
        let manifest = "---\nname: websearch\ndescription: Search Google via the Serper API.\n---\n\n# Web Search\n";

        assert_eq!(
            parse_skill_manifest(manifest),
            Some((
                "websearch".to_string(),
                Some("Search Google via the Serper API.".to_string())
            ))
        );
    }

    #[test]
    fn quoted_values_and_colons_in_descriptions_survive() {
        let manifest = "---\nname: \"deploy\"\ndescription: 'Ship it: carefully'\n---\n";

        let (name, description) = parse_skill_manifest(manifest).expect("manifest");
        assert_eq!(name, "deploy");
        // Splitting on the first colon only, so the description keeps its own.
        assert_eq!(description.as_deref(), Some("Ship it: carefully"));
    }

    #[test]
    fn a_horizontal_rule_in_the_body_is_not_frontmatter() {
        // Without the first-line requirement this would read the prose above
        // the rule as a manifest.
        let manifest = "# Notes\n\nSome prose.\n\n---\nname: sneaky\n---\n";

        assert_eq!(parse_skill_manifest(manifest), None);
    }

    #[test]
    fn malformed_frontmatter_yields_nothing() {
        // Unterminated: treating the rest of the file as frontmatter would
        // invent a skill out of prose.
        assert_eq!(parse_skill_manifest("---\nname: half\n\n# Body\n"), None);
        // Nameless: Prime keys on the name, so there is nothing to promote.
        assert_eq!(
            parse_skill_manifest("---\ndescription: no name\n---\n"),
            None
        );
        assert_eq!(parse_skill_manifest("---\nname:   \n---\n"), None);
        assert_eq!(parse_skill_manifest(""), None);
    }

    #[test]
    fn a_description_is_optional_in_the_manifest_but_marks_a_skill_unfinished() {
        let (name, description) = parse_skill_manifest("---\nname: bare\n---\n").expect("manifest");
        assert_eq!(name, "bare");
        assert_eq!(description, None);

        let skill = DiscoveredPrimeSkill {
            name,
            description,
            directory: PathBuf::from("bare"),
            is_python_backed: false,
        };
        // Prime itself warns about and refuses a skill with no description.
        assert!(!skill.is_complete());
    }

    #[test]
    fn discovery_finds_skills_and_notes_which_are_python_backed() {
        let root = tempfile::tempdir().expect("root");
        write_skill(
            root.path(),
            "zeta",
            "---\nname: zeta\ndescription: Last alphabetically.\n---\n",
            false,
        );
        write_skill(
            root.path(),
            "alpha",
            "---\nname: alpha\ndescription: A python skill.\n---\n",
            true,
        );

        let skills = discover_skills(root.path());

        // Stable order, so a promotion prompt does not reshuffle between scans.
        assert_eq!(
            skills.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
            vec!["alpha", "zeta"]
        );
        assert!(skills[0].is_python_backed);
        assert!(!skills[1].is_python_backed);
    }

    #[test]
    fn directories_that_are_not_skills_are_ignored() {
        let root = tempfile::tempdir().expect("root");
        std::fs::create_dir_all(root.path().join("not-a-skill/src")).expect("dirs");
        std::fs::write(root.path().join("not-a-skill/README.md"), "hi").expect("readme");
        // A nested SKILL.md belongs to the skill above it, not to a skill of
        // its own.
        let outer = write_skill(
            root.path(),
            "outer",
            "---\nname: outer\ndescription: Real.\n---\n",
            false,
        );
        std::fs::create_dir_all(outer.join("references")).expect("references");
        std::fs::write(
            outer.join("references/SKILL.md"),
            "---\nname: nested\ndescription: Not a skill.\n---\n",
        )
        .expect("nested");

        let skills = discover_skills(root.path());

        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "outer");
    }

    #[test]
    fn a_missing_directory_is_empty_rather_than_an_error() {
        // The common case: a project that has never used Prime skills.
        assert!(discover_skills(Path::new("does-not-exist")).is_empty());
    }

    #[test]
    fn promotion_skips_unfinished_skills_and_names_the_library_already_has() {
        let discovered = vec![
            DiscoveredPrimeSkill {
                name: "planner".into(),
                description: Some("Already in the Library.".into()),
                directory: PathBuf::from("planner"),
                is_python_backed: false,
            },
            DiscoveredPrimeSkill {
                name: "unfinished".into(),
                description: None,
                directory: PathBuf::from("unfinished"),
                is_python_backed: false,
            },
            DiscoveredPrimeSkill {
                name: "novel".into(),
                description: Some("Worth promoting.".into()),
                directory: PathBuf::from("novel"),
                is_python_backed: true,
            },
        ];

        let promotable = promotable_skills(discovered, &["Planner".to_string()]);

        // The Library copy is the deployed one, so an existing name is left
        // alone rather than offered as an overwrite. Matching is
        // case-insensitive because the Library keys on directory names.
        assert_eq!(
            promotable
                .iter()
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>(),
            vec!["novel"]
        );
    }

    #[test]
    fn project_skills_live_where_prime_writes_them() {
        assert_eq!(
            project_skills_dir(Path::new("/w")),
            Path::new("/w/.prime/agent/skills")
        );
    }
}
