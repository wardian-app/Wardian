use crate::{
    args::{LibraryArgs, LibraryCommand, LibraryOrphanCommand},
    errors::CliError,
};
use std::collections::HashSet;
use std::io::Read as _;
use std::path::{Component, Path, PathBuf};
use wardian_core::library::{self, LibrarySectionId, DEPLOYED_SKILL_SOURCE_FILE};
use wardian_core::models::{LibraryEntry, LibraryIndexNode, LibraryItemMetadata, SkillDeployment};

#[derive(Debug, Clone, PartialEq, Eq)]
struct LibraryRef {
    section: LibrarySectionId,
    rel_path: String,
    entry_ref: String,
}

pub fn handle_library(args: LibraryArgs) -> Result<String, CliError> {
    match args.command {
        LibraryCommand::List { section, flat } => handle_list(section.as_deref(), flat),
        LibraryCommand::Show { entry_ref, content } => handle_show(&entry_ref, content),
        LibraryCommand::Read { entry_ref } => handle_read(&entry_ref),
        LibraryCommand::Create {
            entry_ref,
            stdin,
            file,
        } => handle_create(&entry_ref, stdin, file.as_deref()),
        LibraryCommand::Write {
            entry_ref,
            stdin,
            file,
        } => handle_write(&entry_ref, stdin, file.as_deref()),
        LibraryCommand::Move { from_ref, to_ref } => handle_move(&from_ref, &to_ref),
        LibraryCommand::Delete { entry_ref } => handle_delete(&entry_ref),
        LibraryCommand::RestoreDefault { entry_ref } => handle_restore_default(&entry_ref),
        LibraryCommand::Star { entry_ref } => handle_star(&entry_ref, true),
        LibraryCommand::Unstar { entry_ref } => handle_star(&entry_ref, false),
        LibraryCommand::Tags { entry_ref, set } => handle_tags(&entry_ref, set),
        LibraryCommand::Deployments { skill_ref } => handle_deployments(&skill_ref),
        LibraryCommand::Deploy {
            skill_ref,
            targets,
            clear,
        } => handle_deploy(&skill_ref, targets.as_deref(), clear),
        LibraryCommand::Orphans => handle_orphans(),
        LibraryCommand::Orphan { command } => match command {
            LibraryOrphanCommand::Delete { target, skill } => handle_orphan_delete(&target, &skill),
        },
    }
}

fn handle_list(section: Option<&str>, flat: bool) -> Result<String, CliError> {
    let home = wardian_home()?;
    let index = library::build_library_index(&home).map_err(CliError::generic)?;
    if let Some(section_id) = parse_section(section)? {
        let section_name = section_id.as_str();
        let section = index
            .sections
            .get(section_name)
            .ok_or_else(|| CliError::unknown_section(section_name))?;
        let mut body = serde_json::json!({
            "schema": 1,
            "section": section_name,
            "tree": section.tree,
            "stubbed": section.stubbed,
        });
        if flat {
            let mut entries = Vec::new();
            flatten_entries(&section.tree.children, &mut entries);
            body["entries"] = serde_json::json!(entries);
        }
        return render_json(body);
    }

    let body = serde_json::json!({
        "schema": 1,
        "sections": index.sections,
        "deployments": index.deployments,
        "orphans": index.orphans,
    });
    render_json(body)
}

fn handle_show(entry_ref: &str, include_content: bool) -> Result<String, CliError> {
    let home = wardian_home()?;
    let entry_ref = parse_library_ref(entry_ref)?;
    reject_mcps(&entry_ref)?;
    if !entry_exists(&home, &entry_ref) {
        return Err(CliError::library_not_found(&entry_ref.entry_ref));
    }

    let entry = find_entry(&home, &entry_ref.entry_ref)?
        .ok_or_else(|| CliError::library_not_found(&entry_ref.entry_ref))?;
    let absolute_path = target_path_for_ref(&home, &entry_ref)?;
    let absolute_path = display_path(&absolute_path);
    let mut body =
        serde_json::to_value(&entry).map_err(|error| CliError::generic(error.to_string()))?;
    body["schema"] = serde_json::json!(1);
    body["absolute_path"] = serde_json::json!(absolute_path.clone());
    if entry_ref.section == LibrarySectionId::Workflows {
        body["workflow_path"] = serde_json::json!(absolute_path);
    }
    if include_content {
        body["content"] =
            serde_json::json!(
                library::read_item(&home, entry_ref.section, &entry_ref.rel_path)
                    .map_err(CliError::generic)?
            );
    }
    render_json(body)
}

fn handle_read(entry_ref: &str) -> Result<String, CliError> {
    let home = wardian_home()?;
    let entry_ref = parse_library_ref(entry_ref)?;
    reject_mcps(&entry_ref)?;
    if !entry_exists(&home, &entry_ref) {
        return Err(CliError::library_not_found(&entry_ref.entry_ref));
    }
    library::read_item(&home, entry_ref.section, &entry_ref.rel_path).map_err(CliError::generic)
}

fn handle_create(entry_ref: &str, stdin: bool, file: Option<&str>) -> Result<String, CliError> {
    let home = wardian_home()?;
    let entry_ref = parse_library_ref(entry_ref)?;
    reject_mcps(&entry_ref)?;
    if entry_exists(&home, &entry_ref) {
        return Err(CliError::already_exists(&entry_ref.entry_ref));
    }
    library::validate_entry_destination(&home, entry_ref.section, &entry_ref.rel_path)
        .map_err(CliError::invalid_ref)?;
    let content = read_content_arg(stdin, file)?;
    if entry_ref.section == LibrarySectionId::Classes {
        let description = class_description_from_content(&content);
        wardian_core::classes::create_class(
            &home,
            &entry_ref.rel_path,
            &description,
            Some(&content),
        )
        .map_err(CliError::generic)?;
    } else {
        library::save_item(&home, entry_ref.section, &entry_ref.rel_path, &content)
            .map_err(CliError::generic)?;
    }
    render_json(serde_json::json!({
        "schema": 1,
        "ok": true,
        "entry_ref": entry_ref.entry_ref,
    }))
}

fn handle_write(entry_ref: &str, stdin: bool, file: Option<&str>) -> Result<String, CliError> {
    let home = wardian_home()?;
    let entry_ref = parse_library_ref(entry_ref)?;
    reject_mcps(&entry_ref)?;
    if !entry_exists(&home, &entry_ref) {
        return Err(CliError::library_not_found(&entry_ref.entry_ref));
    }
    library::validate_entry_destination(&home, entry_ref.section, &entry_ref.rel_path)
        .map_err(CliError::invalid_ref)?;
    let content = read_content_arg(stdin, file)?;
    library::save_item(&home, entry_ref.section, &entry_ref.rel_path, &content)
        .map_err(CliError::generic)?;
    render_json(serde_json::json!({
        "schema": 1,
        "ok": true,
        "entry_ref": entry_ref.entry_ref,
    }))
}

fn handle_move(from_ref: &str, to_ref: &str) -> Result<String, CliError> {
    let home = wardian_home()?;
    let from_ref = parse_library_ref(from_ref)?;
    let to_ref = parse_library_ref(to_ref)?;
    reject_mcps(&from_ref)?;
    reject_mcps(&to_ref)?;
    if from_ref.section != to_ref.section {
        return Err(CliError::invalid_ref(
            "Moving library entries across sections is not supported",
        ));
    }
    if from_ref.section == LibrarySectionId::Classes {
        return Err(CliError::not_supported(
            "Moving classes is not supported; create a new class and delete the old one if needed.",
        ));
    }
    library::validate_entry_destination(&home, to_ref.section, &to_ref.rel_path)
        .map_err(CliError::invalid_ref)?;
    if !entry_exists(&home, &from_ref) {
        return Err(CliError::library_not_found(&from_ref.entry_ref));
    }
    if entry_exists(&home, &to_ref) {
        return Err(CliError::already_exists(&to_ref.entry_ref));
    }
    library::rename_entry(
        &home,
        from_ref.section,
        &from_ref.rel_path,
        &to_ref.rel_path,
    )
    .map_err(CliError::generic)?;
    render_json(serde_json::json!({
        "schema": 1,
        "ok": true,
        "from_ref": from_ref.entry_ref,
        "to_ref": to_ref.entry_ref,
    }))
}

fn handle_delete(entry_ref: &str) -> Result<String, CliError> {
    let home = wardian_home()?;
    let entry_ref = parse_library_ref(entry_ref)?;
    reject_mcps(&entry_ref)?;
    if entry_ref.section == LibrarySectionId::Classes {
        wardian_core::classes::delete_class(&home, &entry_ref.rel_path)
            .map_err(CliError::generic)?;
    } else {
        if !entry_exists(&home, &entry_ref) {
            return Err(CliError::library_not_found(&entry_ref.entry_ref));
        }
        library::delete_entry(&home, entry_ref.section, &entry_ref.rel_path)
            .map_err(CliError::generic)?;
    }
    render_json(serde_json::json!({
        "schema": 1,
        "ok": true,
        "entry_ref": entry_ref.entry_ref,
    }))
}

fn handle_restore_default(entry_ref: &str) -> Result<String, CliError> {
    let home = wardian_home()?;
    let entry_ref = parse_library_ref(entry_ref)?;
    if entry_ref.section != LibrarySectionId::Classes {
        return Err(CliError::invalid_ref(
            "restore-default requires a classes/<Name> ref",
        ));
    }
    wardian_core::classes::restore_default_instruction(&home, &entry_ref.rel_path)
        .map_err(CliError::generic)?;
    render_json(serde_json::json!({
        "schema": 1,
        "ok": true,
        "entry_ref": entry_ref.entry_ref,
    }))
}

fn handle_star(entry_ref: &str, is_starred: bool) -> Result<String, CliError> {
    let home = wardian_home()?;
    let entry_ref = parse_library_ref(entry_ref)?;
    reject_mcps(&entry_ref)?;
    if !entry_exists(&home, &entry_ref) {
        return Err(CliError::library_not_found(&entry_ref.entry_ref));
    }

    let mut metadata = current_metadata(&home, &entry_ref);
    metadata.is_starred = is_starred;
    write_metadata(&home, &entry_ref, metadata.clone())?;
    render_json(serde_json::json!({
        "schema": 1,
        "ok": true,
        "entry_ref": entry_ref.entry_ref,
        "is_starred": metadata.is_starred,
        "tags": metadata.tags,
    }))
}

fn handle_tags(entry_ref: &str, tags: Vec<String>) -> Result<String, CliError> {
    let home = wardian_home()?;
    let entry_ref = parse_library_ref(entry_ref)?;
    reject_mcps(&entry_ref)?;
    if !entry_exists(&home, &entry_ref) {
        return Err(CliError::library_not_found(&entry_ref.entry_ref));
    }

    let mut metadata = current_metadata(&home, &entry_ref);
    metadata.tags = tags
        .into_iter()
        .map(|tag| tag.trim().to_string())
        .filter(|tag| !tag.is_empty())
        .collect();
    metadata.tags.sort();
    metadata.tags.dedup();
    write_metadata(&home, &entry_ref, metadata.clone())?;
    render_json(serde_json::json!({
        "schema": 1,
        "ok": true,
        "entry_ref": entry_ref.entry_ref,
        "is_starred": metadata.is_starred,
        "tags": metadata.tags,
    }))
}

fn handle_deployments(skill_ref: &str) -> Result<String, CliError> {
    let home = wardian_home()?;
    let skill_ref = parse_skill_ref(skill_ref)?;
    if !entry_exists(&home, &skill_ref) {
        return Err(CliError::library_not_found(&skill_ref.entry_ref));
    }

    let sources = library::collect_skill_sources(&home);
    let scan = library::scan_deployments(&home, &sources);
    let targets = scan
        .deployments
        .get(&skill_ref.rel_path)
        .cloned()
        .unwrap_or_default();
    render_json(serde_json::json!({
        "schema": 1,
        "skill_ref": skill_ref.entry_ref,
        "targets": targets,
    }))
}

fn handle_deploy(skill_ref: &str, targets: Option<&str>, clear: bool) -> Result<String, CliError> {
    let home = wardian_home()?;
    let skill_ref = parse_skill_ref(skill_ref)?;
    if !entry_exists(&home, &skill_ref) {
        return Err(CliError::library_not_found(&skill_ref.entry_ref));
    }
    let targets = if clear {
        Vec::new()
    } else {
        parse_target_refs(targets.unwrap_or_default())?
    };
    validate_deployment_targets(&home, &targets)?;
    let outcome = library::set_skill_deployments(&home, &skill_ref.rel_path, &targets)
        .map_err(CliError::generic)?;
    render_json(serde_json::json!({
        "schema": 1,
        "ok": true,
        "skill_ref": skill_ref.entry_ref,
        "targets": targets,
        "outcome": outcome,
    }))
}

fn handle_orphans() -> Result<String, CliError> {
    let home = wardian_home()?;
    let sources = library::collect_skill_sources(&home);
    let scan = library::scan_deployments(&home, &sources);
    render_json(serde_json::json!({
        "schema": 1,
        "orphans": scan.orphans,
    }))
}

fn handle_orphan_delete(target: &str, skill: &str) -> Result<String, CliError> {
    let home = wardian_home()?;
    let (target_type, target_id) = parse_target_ref(target)?;
    let removed = library::remove_orphan_deployment(&home, &target_type, &target_id, skill)
        .map_err(CliError::generic)?;
    if !removed {
        return Err(CliError::library_not_found(&format!(
            "orphan/{target}/{skill}"
        )));
    }
    render_json(serde_json::json!({
        "schema": 1,
        "ok": true,
        "target_type": target_type,
        "target_id": target_id,
        "skill": skill,
    }))
}

fn parse_section(section: Option<&str>) -> Result<Option<LibrarySectionId>, CliError> {
    section
        .map(|section| {
            LibrarySectionId::parse(section).ok_or_else(|| CliError::unknown_section(section))
        })
        .transpose()
}

fn parse_library_ref(value: &str) -> Result<LibraryRef, CliError> {
    let trimmed = value.trim();
    let (section_name, rel_path) = trimmed.split_once('/').ok_or_else(|| {
        CliError::invalid_ref(format!("Library ref must include a section: {value}"))
    })?;
    let section = LibrarySectionId::parse(section_name)
        .ok_or_else(|| CliError::unknown_section(section_name))?;
    let rel_path = normalize_entry_path(rel_path)?;
    Ok(LibraryRef {
        section,
        entry_ref: format!("{}/{}", section.as_str(), rel_path),
        rel_path,
    })
}

fn normalize_entry_path(rel_path: &str) -> Result<String, CliError> {
    let normalized = rel_path.replace('\\', "/");
    if normalized.trim().is_empty() {
        return Err(CliError::invalid_ref(
            "Library entry path must not be empty",
        ));
    }

    let path = Path::new(&normalized);
    if path.is_absolute() {
        return Err(CliError::invalid_ref(format!(
            "Library entry path must be relative: {rel_path}"
        )));
    }
    for component in path.components() {
        match component {
            Component::Normal(part) => {
                let text = part.to_string_lossy();
                if text.eq_ignore_ascii_case(DEPLOYED_SKILL_SOURCE_FILE) {
                    return Err(CliError::invalid_ref(format!(
                        "Reserved name in library entry path: {text}"
                    )));
                }
            }
            _ => {
                return Err(CliError::invalid_ref(format!(
                    "Invalid library entry path: {rel_path}"
                )))
            }
        }
    }

    Ok(normalized)
}

fn target_path_for_ref(home: &Path, entry: &LibraryRef) -> Result<PathBuf, CliError> {
    library::resolve_entry_path(home, entry.section, &entry.rel_path).map_err(CliError::invalid_ref)
}

fn entry_exists(home: &Path, entry: &LibraryRef) -> bool {
    match entry.section {
        LibrarySectionId::Skills => target_path_for_ref(home, entry)
            .map(|path| path.join("SKILL.md").is_file())
            .unwrap_or(false),
        LibrarySectionId::Prompts | LibrarySectionId::Workflows => target_path_for_ref(home, entry)
            .map(|path| path.is_file())
            .unwrap_or(false),
        LibrarySectionId::Classes => target_path_for_ref(home, entry)
            .map(|path| path.join("AGENTS.md").is_file())
            .unwrap_or(false),
        LibrarySectionId::Mcps => false,
    }
}

fn parse_target_refs(value: &str) -> Result<Vec<SkillDeployment>, CliError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(CliError::invalid_target(value));
    }

    let mut targets = Vec::new();
    let mut seen = HashSet::new();
    for target in trimmed.split(',').map(str::trim) {
        if target.is_empty() {
            return Err(CliError::invalid_target(value));
        }
        let (target_type, target_id) = target
            .split_once(':')
            .ok_or_else(|| CliError::invalid_target(target))?;
        let deployment = match target_type {
            "user" if target_id == "global" => SkillDeployment {
                target_type: "user".to_string(),
                target_id: "global".to_string(),
            },
            "class" if library::is_single_normal_component(target_id) => SkillDeployment {
                target_type: "class".to_string(),
                target_id: target_id.to_string(),
            },
            "agent" if library::is_single_normal_component(target_id) => SkillDeployment {
                target_type: "agent".to_string(),
                target_id: target_id.to_string(),
            },
            _ => return Err(CliError::invalid_target(target)),
        };
        if seen.insert((deployment.target_type.clone(), deployment.target_id.clone())) {
            targets.push(deployment);
        }
    }

    Ok(targets)
}

fn parse_target_ref(value: &str) -> Result<(String, String), CliError> {
    let mut targets = parse_target_refs(value)?;
    if targets.len() != 1 {
        return Err(CliError::invalid_target(value));
    }
    let target = targets.remove(0);
    Ok((target.target_type, target.target_id))
}

fn parse_skill_ref(value: &str) -> Result<LibraryRef, CliError> {
    let entry_ref = parse_library_ref(value)?;
    if entry_ref.section != LibrarySectionId::Skills {
        return Err(CliError::invalid_ref(
            "Skill deployment commands require a skills/<path> ref",
        ));
    }
    Ok(entry_ref)
}

fn validate_deployment_targets(home: &Path, targets: &[SkillDeployment]) -> Result<(), CliError> {
    for target in targets {
        let target_ref = format!("{}:{}", target.target_type, target.target_id);
        match target.target_type.as_str() {
            "user" if target.target_id == "global" => {}
            "class" if class_target_exists(home, &target.target_id) => {}
            "agent" if agent_target_exists(home, &target.target_id) => {}
            _ => return Err(CliError::invalid_target(&target_ref)),
        }
    }
    Ok(())
}

fn class_target_exists(home: &Path, class_name: &str) -> bool {
    wardian_core::classes::load_class_definitions(home)
        .map(|classes| {
            classes
                .iter()
                .any(|class| class.name.eq_ignore_ascii_case(class_name))
        })
        .unwrap_or(false)
        || home
            .join("classes")
            .join(class_name)
            .join("AGENTS.md")
            .is_file()
}

fn agent_target_exists(home: &Path, agent_id: &str) -> bool {
    home.join("agents").join(agent_id).is_dir()
        || persisted_agent_exists(home, agent_id)
        || live_agent_exists(agent_id)
}

fn persisted_agent_exists(home: &Path, agent_id: &str) -> bool {
    let path = home.join("state.db");
    if !path.is_file() {
        return false;
    }
    let Ok(conn) =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
    else {
        return false;
    };
    wardian_core::db::get_agent_by_session_id_with_conn(&conn, agent_id)
        .map(|agent| agent.is_some())
        .unwrap_or(false)
}

fn live_agent_exists(agent_id: &str) -> bool {
    crate::live::list_agents()
        .map(|agents| agents.iter().any(|agent| agent.uuid == agent_id))
        .unwrap_or(false)
}

fn current_metadata(home: &Path, entry_ref: &LibraryRef) -> LibraryItemMetadata {
    library::MetadataStore::load(home)
        .get(&entry_ref.entry_ref)
        .cloned()
        .unwrap_or_else(|| LibraryItemMetadata {
            id: entry_ref.entry_ref.clone(),
            tags: Vec::new(),
            is_starred: false,
            last_used: None,
        })
}

fn write_metadata(
    home: &Path,
    entry_ref: &LibraryRef,
    metadata: LibraryItemMetadata,
) -> Result<(), CliError> {
    library::update_metadata(home, &entry_ref.entry_ref, metadata).map_err(CliError::generic)
}

fn wardian_home() -> Result<PathBuf, CliError> {
    wardian_core::paths::wardian_home()
        .ok_or_else(|| CliError::generic("Could not resolve Wardian home directory"))
}

fn read_content_arg(stdin: bool, file: Option<&str>) -> Result<String, CliError> {
    if stdin {
        let mut content = String::new();
        std::io::stdin()
            .read_to_string(&mut content)
            .map_err(|error| CliError::generic(error.to_string()))?;
        return Ok(content);
    }
    if let Some(file) = file {
        return std::fs::read_to_string(file).map_err(|error| CliError::generic(error.to_string()));
    }
    Err(CliError::generic("Specify --stdin or --file <path>"))
}

fn render_json(body: serde_json::Value) -> Result<String, CliError> {
    serde_json::to_string_pretty(&body)
        .map(|json| format!("{json}\n"))
        .map_err(|error| CliError::generic(error.to_string()))
}

fn reject_mcps(entry_ref: &LibraryRef) -> Result<(), CliError> {
    if entry_ref.section == LibrarySectionId::Mcps {
        return Err(CliError::not_supported(
            "The library mcps section is not implemented yet",
        ));
    }
    Ok(())
}

fn find_entry(home: &Path, entry_ref: &str) -> Result<Option<LibraryEntry>, CliError> {
    let index = library::build_library_index(home).map_err(CliError::generic)?;
    Ok(index
        .sections
        .values()
        .find_map(|section| find_entry_in_nodes(&section.tree.children, entry_ref)))
}

fn find_entry_in_nodes(nodes: &[LibraryIndexNode], entry_ref: &str) -> Option<LibraryEntry> {
    for node in nodes {
        match node {
            LibraryIndexNode::Entry(entry) if entry.entry_ref == entry_ref => {
                return Some(entry.clone());
            }
            LibraryIndexNode::Entry(_) => {}
            LibraryIndexNode::Folder(folder) => {
                if let Some(entry) = find_entry_in_nodes(&folder.children, entry_ref) {
                    return Some(entry);
                }
            }
        }
    }
    None
}

fn flatten_entries(nodes: &[LibraryIndexNode], entries: &mut Vec<LibraryEntry>) {
    for node in nodes {
        match node {
            LibraryIndexNode::Entry(entry) => entries.push(entry.clone()),
            LibraryIndexNode::Folder(folder) => flatten_entries(&folder.children, entries),
        }
    }
}

fn class_description_from_content(content: &str) -> String {
    content
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| line.trim_start_matches('#').trim().to_string())
        .filter(|line| !line.is_empty())
        .unwrap_or_else(|| "Custom agent class".to_string())
}

fn display_path(path: &Path) -> String {
    path.components()
        .collect::<PathBuf>()
        .to_string_lossy()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use wardian_core::library::LibrarySectionId;
    use wardian_core::models::SkillDeployment;

    #[test]
    fn parse_library_ref_requires_known_section_and_relative_path() {
        let parsed = parse_library_ref("skills/review/planner").expect("skill ref");

        assert_eq!(parsed.section, LibrarySectionId::Skills);
        assert_eq!(parsed.rel_path, "review/planner");
        assert_eq!(parsed.entry_ref, "skills/review/planner");

        assert_eq!(
            parse_library_ref("planner").unwrap_err().code,
            "invalid_ref"
        );
        assert_eq!(
            parse_library_ref("plugins/planner").unwrap_err().code,
            "unknown_section"
        );
        assert_eq!(
            parse_library_ref("skills/../planner").unwrap_err().code,
            "invalid_ref"
        );
    }

    #[test]
    fn parse_section_accepts_optional_known_sections() {
        assert_eq!(parse_section(None).unwrap(), None);
        assert_eq!(
            parse_section(Some("classes")).unwrap(),
            Some(LibrarySectionId::Classes)
        );
        assert_eq!(
            parse_section(Some("plugins")).unwrap_err().code,
            "unknown_section"
        );
    }

    #[test]
    fn target_path_and_entry_exists_respect_section_layout() {
        let temp = tempfile::tempdir().expect("temp dir");
        let home = temp.path();

        std::fs::create_dir_all(home.join("library").join("skills").join("review"))
            .expect("skills dir");
        std::fs::write(
            home.join("library")
                .join("skills")
                .join("review")
                .join("planner")
                .join("SKILL.md"),
            "skill",
        )
        .expect_err("parent should not exist yet");
        std::fs::create_dir_all(
            home.join("library")
                .join("skills")
                .join("review")
                .join("planner"),
        )
        .expect("skill dir");
        std::fs::write(
            home.join("library")
                .join("skills")
                .join("review")
                .join("planner")
                .join("SKILL.md"),
            "skill",
        )
        .expect("skill file");
        std::fs::create_dir_all(home.join("classes").join("Reviewer")).expect("class dir");
        std::fs::write(
            home.join("classes").join("Reviewer").join("AGENTS.md"),
            "class",
        )
        .expect("class file");

        let skill = parse_library_ref("skills/review/planner").unwrap();
        let class = parse_library_ref("classes/Reviewer").unwrap();
        let missing = parse_library_ref("workflows/missing.md").unwrap();

        assert!(target_path_for_ref(home, &skill)
            .unwrap()
            .ends_with("library/skills/review/planner"));
        assert!(entry_exists(home, &skill));
        assert!(entry_exists(home, &class));
        assert!(!entry_exists(home, &missing));
    }

    #[test]
    fn parse_target_refs_accepts_user_class_and_agent_targets() {
        assert_eq!(
            parse_target_refs("user:global,class:Reviewer,agent:agent-1").unwrap(),
            vec![
                SkillDeployment {
                    target_type: "user".to_string(),
                    target_id: "global".to_string(),
                },
                SkillDeployment {
                    target_type: "class".to_string(),
                    target_id: "Reviewer".to_string(),
                },
                SkillDeployment {
                    target_type: "agent".to_string(),
                    target_id: "agent-1".to_string(),
                },
            ]
        );
        assert_eq!(parse_target_refs("").unwrap_err().code, "invalid_target");
        assert_eq!(
            parse_target_refs("user:global,").unwrap_err().code,
            "invalid_target"
        );
        assert_eq!(
            parse_target_refs("team:Review").unwrap_err().code,
            "invalid_target"
        );
        assert_eq!(
            parse_target_refs("class:../Reviewer").unwrap_err().code,
            "invalid_target"
        );
    }
}
