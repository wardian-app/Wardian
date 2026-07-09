use std::path::Path;

use crate::library::is_single_normal_component;
use crate::library::MetadataStore;
use crate::models::AgentClassDefinition;

const DEFAULT_CLASSES_JSON: &str = include_str!("../../../src-tauri/src/default_classes.json");
const DEFAULT_CLASS_PROMPTS: &[(&str, &str)] = &[
    (
        "Architect",
        include_str!("../../../src-tauri/agent_prompts/Architect.md"),
    ),
    (
        "Coder",
        include_str!("../../../src-tauri/agent_prompts/Coder.md"),
    ),
    (
        "Editor",
        include_str!("../../../src-tauri/agent_prompts/Editor.md"),
    ),
    (
        "Evolver",
        include_str!("../../../src-tauri/agent_prompts/Evolver.md"),
    ),
    (
        "Generalist",
        include_str!("../../../src-tauri/agent_prompts/Generalist.md"),
    ),
    (
        "Orchestrator",
        include_str!("../../../src-tauri/agent_prompts/Orchestrator.md"),
    ),
    (
        "Personal Assistant",
        include_str!("../../../src-tauri/agent_prompts/Personal Assistant.md"),
    ),
    ("QA", include_str!("../../../src-tauri/agent_prompts/QA.md")),
    (
        "Researcher",
        include_str!("../../../src-tauri/agent_prompts/Researcher.md"),
    ),
    (
        "Reviewer",
        include_str!("../../../src-tauri/agent_prompts/Reviewer.md"),
    ),
];

pub fn default_class_definitions() -> Vec<AgentClassDefinition> {
    let mut defaults: Vec<AgentClassDefinition> =
        serde_json::from_str(DEFAULT_CLASSES_JSON).unwrap_or_default();
    for class in &mut defaults {
        class.is_default = true;
        class.instruction_content = None;
    }
    defaults
}

pub fn default_class_instruction(name: &str) -> Option<&'static str> {
    DEFAULT_CLASS_PROMPTS
        .iter()
        .find(|(class_name, _)| class_name.eq_ignore_ascii_case(name))
        .map(|(_, content)| *content)
}

pub fn load_class_definitions(home: &Path) -> Result<Vec<AgentClassDefinition>, String> {
    let classes_path = home.join("classes.json");
    if !classes_path.exists() {
        return Ok(Vec::new());
    }

    let data = std::fs::read_to_string(&classes_path).map_err(|error| error.to_string())?;
    serde_json::from_str::<Vec<AgentClassDefinition>>(&data).map_err(|error| error.to_string())
}

pub fn save_class_definitions(home: &Path, classes: &[AgentClassDefinition]) -> Result<(), String> {
    crate::atomic_file::write_json_atomic(&home.join("classes.json"), classes)
        .map_err(|error| error.to_string())
}

pub fn initialize_classes(home: &Path) -> Result<Vec<AgentClassDefinition>, String> {
    let classes = if home.join("classes.json").exists() {
        load_class_definitions(home)?
    } else {
        let defaults = default_class_definitions();
        save_class_definitions(home, &defaults)?;
        defaults
    };

    for class in &classes {
        ensure_class_directory(home, class, None)?;
    }
    Ok(classes)
}

pub fn ensure_class_directory(
    home: &Path,
    class: &AgentClassDefinition,
    instruction_content: Option<&str>,
) -> Result<(), String> {
    validate_class_name(&class.name)?;

    let role_dir = home.join("classes").join(&class.name);
    std::fs::create_dir_all(&role_dir).map_err(|error| error.to_string())?;

    let agents_md_path = role_dir.join("AGENTS.md");
    if !agents_md_path.exists() {
        let content = instruction_content
            .filter(|content| !content.trim().is_empty())
            .map(ToOwned::to_owned)
            .or_else(|| {
                class
                    .is_default
                    .then(|| default_class_instruction(&class.name).map(ToOwned::to_owned))
                    .flatten()
            })
            .unwrap_or_else(|| format!("# Role: {}\n\n{}\n", class.name, class.description));
        std::fs::write(agents_md_path, content).map_err(|error| error.to_string())?;
    }

    for stub_name in ["GEMINI.md", "CLAUDE.md"] {
        let stub_path = role_dir.join(stub_name);
        if !stub_path.exists() {
            std::fs::write(stub_path, "@AGENTS.md\n").map_err(|error| error.to_string())?;
        }
    }

    Ok(())
}

pub fn create_class(
    home: &Path,
    name: &str,
    description: &str,
    instruction_content: Option<&str>,
) -> Result<AgentClassDefinition, String> {
    let trimmed_name = name.trim();
    validate_class_name(trimmed_name)?;

    let classes_path = home.join("classes.json");
    let mut classes = if classes_path.exists() {
        load_class_definitions(home)?
    } else {
        default_class_definitions()
    };

    if classes
        .iter()
        .any(|class| class.name.eq_ignore_ascii_case(trimmed_name))
    {
        return Err(format!("A class named '{trimmed_name}' already exists"));
    }

    let new_class = AgentClassDefinition {
        name: trimmed_name.to_string(),
        description: description.trim().to_string(),
        is_default: false,
        instruction_content: None,
        assigned_skills: None,
    };

    classes.push(new_class.clone());
    save_class_definitions(home, &classes)?;
    ensure_class_directory(home, &new_class, instruction_content)?;
    Ok(new_class)
}

pub fn delete_class(home: &Path, name: &str) -> Result<(), String> {
    let mut classes = load_class_definitions(home)?;
    let found = classes
        .iter()
        .find(|class| class.name.eq_ignore_ascii_case(name))
        .cloned()
        .ok_or_else(|| format!("Class '{name}' not found"))?;

    if found.is_default {
        return Err("Cannot delete a default class".to_string());
    }
    validate_class_name(&found.name)?;

    classes.retain(|class| !class.name.eq_ignore_ascii_case(name));
    save_class_definitions(home, &classes)?;

    let role_dir = home.join("classes").join(&found.name);
    if role_dir.exists() {
        std::fs::remove_dir_all(role_dir).map_err(|error| error.to_string())?;
    }

    let mut metadata = MetadataStore::load(home);
    metadata.remove(&format!("classes/{}", found.name));
    metadata.save(home)?;

    Ok(())
}

pub fn restore_default_instruction(home: &Path, name: &str) -> Result<(), String> {
    let classes = load_class_definitions(home)?;
    let class = classes
        .iter()
        .find(|class| class.name.eq_ignore_ascii_case(name))
        .ok_or_else(|| format!("Class '{name}' not found"))?;

    if !class.is_default {
        return Err(format!(
            "'{}' is not a default class and cannot be reset.",
            class.name
        ));
    }
    validate_class_name(&class.name)?;

    let content = default_class_instruction(&class.name)
        .ok_or_else(|| format!("System default for '{}' not found", class.name))?;
    let role_dir = home.join("classes").join(&class.name);
    std::fs::create_dir_all(&role_dir).map_err(|error| error.to_string())?;
    std::fs::write(role_dir.join("AGENTS.md"), content).map_err(|error| error.to_string())
}

fn validate_class_name(name: &str) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err("Class name cannot be empty".to_string());
    }
    if !is_single_normal_component(name) {
        return Err(format!("Invalid class name: {name}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::library::MetadataStore;
    use crate::models::{AgentClassDefinition, LibraryItemMetadata};

    #[test]
    fn default_classes_are_marked_default() {
        let classes = super::default_class_definitions();

        assert!(classes
            .iter()
            .any(|class| class.name == "Reviewer" && class.is_default));
        assert!(classes.iter().all(|class| class.is_default));
    }

    #[test]
    fn ensure_class_directory_creates_agents_file_and_provider_stubs() {
        let temp = tempfile::tempdir().expect("temp dir");
        let class = AgentClassDefinition {
            name: "Pair Programmer".to_string(),
            description: "Collaborates on code".to_string(),
            is_default: false,
            instruction_content: None,
            assigned_skills: None,
        };

        super::ensure_class_directory(temp.path(), &class, Some("# Pair\n"))
            .expect("class directory");

        let root = temp.path().join("classes").join("Pair Programmer");
        assert_eq!(
            std::fs::read_to_string(root.join("AGENTS.md")).expect("agents file"),
            "# Pair\n"
        );
        assert_eq!(
            std::fs::read_to_string(root.join("GEMINI.md")).expect("gemini stub"),
            "@AGENTS.md\n"
        );
        assert_eq!(
            std::fs::read_to_string(root.join("CLAUDE.md")).expect("claude stub"),
            "@AGENTS.md\n"
        );
    }

    #[test]
    fn initialize_classes_materializes_defaults_in_a_fresh_home() {
        let temp = tempfile::tempdir().expect("temp dir");

        let classes = super::initialize_classes(temp.path()).expect("initialize classes");

        assert!(classes.iter().any(|class| class.name == "Reviewer"));
        let root = temp.path().join("classes").join("Reviewer");
        assert!(root.join("AGENTS.md").is_file());
        assert_eq!(
            std::fs::read_to_string(root.join("GEMINI.md")).expect("gemini stub"),
            "@AGENTS.md\n"
        );
        assert_eq!(
            std::fs::read_to_string(root.join("CLAUDE.md")).expect("claude stub"),
            "@AGENTS.md\n"
        );
    }

    #[test]
    fn create_class_rejects_duplicate_names_case_insensitively() {
        let temp = tempfile::tempdir().expect("temp dir");
        let mut classes = super::default_class_definitions();
        classes.push(AgentClassDefinition {
            name: "Custom".to_string(),
            description: "Custom class".to_string(),
            is_default: false,
            instruction_content: None,
            assigned_skills: None,
        });
        super::save_class_definitions(temp.path(), &classes).expect("save classes");

        let error = super::create_class(temp.path(), "custom", "duplicate", None)
            .expect_err("duplicate class should fail");

        assert!(error.contains("already exists"));
        assert!(!temp.path().join("classes").join("custom").exists());
    }

    #[test]
    fn delete_class_removes_custom_class_directory_but_rejects_defaults() {
        let temp = tempfile::tempdir().expect("temp dir");
        let mut classes = super::default_class_definitions();
        classes.push(AgentClassDefinition {
            name: "Custom".to_string(),
            description: "Custom class".to_string(),
            is_default: false,
            instruction_content: None,
            assigned_skills: None,
        });
        super::save_class_definitions(temp.path(), &classes).expect("save classes");
        std::fs::create_dir_all(temp.path().join("classes").join("Custom")).expect("class dir");

        super::delete_class(temp.path(), "Custom").expect("delete custom");
        assert!(!temp.path().join("classes").join("Custom").exists());

        let error = super::delete_class(temp.path(), "Reviewer")
            .expect_err("default class should not be deleted");
        assert!(error.contains("Cannot delete a default class"));
    }

    #[test]
    fn delete_class_removes_metadata_before_same_name_is_recreated() {
        let temp = tempfile::tempdir().expect("temp dir");
        super::initialize_classes(temp.path()).expect("initialize classes");
        super::create_class(temp.path(), "Custom", "Custom class", Some("# Custom\n"))
            .expect("create class");
        let mut metadata = MetadataStore::load(temp.path());
        metadata.set(
            "classes/Custom".to_string(),
            LibraryItemMetadata {
                id: "classes/Custom".to_string(),
                tags: vec!["stale".to_string()],
                is_starred: true,
                last_used: None,
            },
        );
        metadata.save(temp.path()).expect("save metadata");

        super::delete_class(temp.path(), "Custom").expect("delete class");
        super::create_class(
            temp.path(),
            "Custom",
            "Replacement",
            Some("# Replacement\n"),
        )
        .expect("recreate class");

        assert!(MetadataStore::load(temp.path())
            .get("classes/Custom")
            .is_none());
    }

    #[test]
    fn restore_default_instruction_rewrites_only_default_agents_file() {
        let temp = tempfile::tempdir().expect("temp dir");
        let classes = super::default_class_definitions();
        super::save_class_definitions(temp.path(), &classes).expect("save classes");
        let reviewer = classes
            .iter()
            .find(|class| class.name == "Reviewer")
            .expect("reviewer class");
        super::ensure_class_directory(temp.path(), reviewer, Some("# Edited\n"))
            .expect("class directory");

        super::restore_default_instruction(temp.path(), "Reviewer").expect("restore default");

        let content = std::fs::read_to_string(
            temp.path()
                .join("classes")
                .join("Reviewer")
                .join("AGENTS.md"),
        )
        .expect("agents file");
        assert!(content.contains("Skeptical Auditor"));
    }
}
