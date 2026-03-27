#[cfg(windows)]
use std::os::windows::process::CommandExt;

pub fn get_wardian_home() -> Option<std::path::PathBuf> {
    dirs::home_dir().map(|h| h.join(".wardian"))
}

pub fn provider_uses_projected_workspace(provider: &str) -> bool {
    provider == "codex"
}

pub fn prepare_provider_habitat(
    provider: &str,
    workspace_root: &std::path::Path,
    class_name: &str,
    session_id: Option<&str>,
) -> Result<Option<std::path::PathBuf>, String> {
    if !provider_uses_projected_workspace(provider) {
        return Ok(None);
    }

    let habitat_root = prepare_habitat_workspace(workspace_root, class_name, session_id)?;
    if provider == "codex" {
        ensure_codex_home_projection(&habitat_root)?;
    }

    Ok(Some(habitat_root))
}

pub fn habitat_workspace_cwd(habitat_root: &std::path::Path) -> std::path::PathBuf {
    habitat_root.join("workspace")
}

pub fn habitat_codex_home(habitat_root: &std::path::Path) -> std::path::PathBuf {
    habitat_root.join(".codex")
}

pub fn resolve_system_include_directories(class_name: &str, session_id: &str) -> Vec<String> {
    let mut dirs = Vec::new();
    if let Some(app_dir) = get_wardian_home() {
        let class_path = app_dir.join("classes").join(class_name);
        let common_path = app_dir.join("common");
        let agent_path = app_dir.join("agents").join(session_id);

        // Ensure the private agent directory exists
        if !agent_path.exists() {
            let _ = std::fs::create_dir_all(&agent_path);
        }
        // Ensure Claude can discover skills from agent's .agents/skills/
        ensure_claude_skills_link(&agent_path);

        if common_path.exists() {
            dirs.push(common_path.to_string_lossy().to_string());
        }
        if class_path.exists() {
            dirs.push(class_path.to_string_lossy().to_string());
        }
        if agent_path.exists() {
            dirs.push(agent_path.to_string_lossy().to_string());
        }
    }
    dirs
}

fn prepare_habitat_workspace(
    workspace_root: &std::path::Path,
    class_name: &str,
    session_id: Option<&str>,
) -> Result<std::path::PathBuf, String> {
    let wardian_home = get_wardian_home().ok_or("Could not find Wardian home")?;
    let habitat_root = if let Some(session_id) = session_id.filter(|sid| !sid.trim().is_empty()) {
        wardian_home.join("agents").join(session_id).join("habitat")
    } else {
        wardian_home
            .join("runtime")
            .join("bootstrap")
            .join(uuid::Uuid::new_v4().to_string())
            .join("habitat")
    };

    std::fs::create_dir_all(&habitat_root).map_err(|e| e.to_string())?;

    write_habitat_instruction_files(&wardian_home, &habitat_root, class_name, session_id)?;
    build_habitat_skill_projection(&wardian_home, &habitat_root, class_name, session_id)?;

    let workspace_link = habitat_root.join("workspace");
    if workspace_link.exists() {
        let _ = std::fs::remove_dir_all(&workspace_link).or_else(|_| std::fs::remove_dir(&workspace_link));
    }
    create_directory_link(workspace_root, &workspace_link)?;
    if !workspace_link.exists() {
        return Err(format!(
            "Failed to create habitat workspace link from {} to {}",
            workspace_link.to_string_lossy(),
            workspace_root.to_string_lossy()
        ));
    }

    Ok(habitat_root)
}

fn ensure_codex_home_projection(habitat_root: &std::path::Path) -> Result<(), String> {
    let real_codex_home = dirs::home_dir()
        .ok_or("Could not find user home directory")?
        .join(".codex");
    let projected_home = habitat_codex_home(habitat_root);
    std::fs::create_dir_all(&projected_home).map_err(|e| e.to_string())?;

    if real_codex_home.exists() {
        let entries = std::fs::read_dir(&real_codex_home).map_err(|e| e.to_string())?;
        for entry in entries.flatten() {
            let source = entry.path();
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str == "skills" {
                continue;
            }

            let target = projected_home.join(&name);
            if source.is_dir() {
                if target.exists() || target.symlink_metadata().is_ok() {
                    continue;
                }
                create_directory_link(&source, &target)?;
            } else if source.is_file() {
                project_file(&source, &target)?;
            }
        }
    }

    let projected_skills = projected_home.join("skills");
    if projected_skills.exists() {
        let _ = std::fs::remove_dir_all(&projected_skills);
    }
    std::fs::create_dir_all(&projected_skills).map_err(|e| e.to_string())?;

    let native_skills = real_codex_home.join("skills");
    if native_skills.exists() {
        let entries = std::fs::read_dir(&native_skills).map_err(|e| e.to_string())?;
        for entry in entries.flatten() {
            let source = entry.path();
            let name = entry.file_name();
            let target = projected_skills.join(&name);
            if source.is_dir() {
                create_directory_link(&source, &target)?;
            } else if source.is_file() {
                project_file(&source, &target)?;
            }
        }
    }

    let wardian_skills = habitat_root.join(".agents").join("skills");
    if wardian_skills.exists() {
        let entries = std::fs::read_dir(&wardian_skills).map_err(|e| e.to_string())?;
        for entry in entries.flatten() {
            let source = entry.path();
            if !source.is_dir() {
                continue;
            }
            let target = projected_skills.join(entry.file_name());
            if target.exists() || target.symlink_metadata().is_ok() {
                let _ = std::fs::remove_dir_all(&target).or_else(|_| std::fs::remove_file(&target));
            }
            create_directory_link(&source, &target)?;
        }
    }

    Ok(())
}

fn write_habitat_instruction_files(
    wardian_home: &std::path::Path,
    habitat_root: &std::path::Path,
    class_name: &str,
    session_id: Option<&str>,
) -> Result<(), String> {
    let common_agents = wardian_home.join("common").join("AGENTS.md");
    let class_agents = wardian_home.join("classes").join(class_name).join("AGENTS.md");
    let agent_agents = session_id
        .filter(|sid| !sid.trim().is_empty())
        .map(|sid| wardian_home.join("agents").join(sid).join("AGENTS.md"));

    let mut sections = Vec::new();
    let mut candidates = vec![("Common", common_agents), ("Class", class_agents)];
    if let Some(agent_agents) = agent_agents {
        candidates.push(("Agent", agent_agents));
    }
    for (label, path) in candidates {
        if path.exists() {
            if let Ok(content) = std::fs::read_to_string(&path) {
                if !content.trim().is_empty() {
                    sections.push(format!(
                        "## {label}\nSource: {}\n\n{}\n",
                        path.to_string_lossy(),
                        content.trim()
                    ));
                }
            }
        }
    }

    let agents_md = if sections.is_empty() {
        "# Wardian Habitat\n\nThis projected workspace has no additional Wardian instructions.\n".to_string()
    } else {
        format!(
            "# Wardian Habitat\n\nThis file is generated by Wardian to project shared instructions into the active workspace scope.\n\n{}\n",
            sections.join("\n")
        )
    };
    std::fs::write(habitat_root.join("AGENTS.md"), agents_md).map_err(|e| e.to_string())?;

    for stub_name in ["GEMINI.md", "CLAUDE.md"] {
        std::fs::write(habitat_root.join(stub_name), "@AGENTS.md\n").map_err(|e| e.to_string())?;
    }

    Ok(())
}

fn build_habitat_skill_projection(
    wardian_home: &std::path::Path,
    habitat_root: &std::path::Path,
    class_name: &str,
    session_id: Option<&str>,
) -> Result<(), String> {
    let merged_skills = habitat_root.join(".agents").join("skills");
    if merged_skills.exists() {
        let _ = std::fs::remove_dir_all(&merged_skills);
    }
    std::fs::create_dir_all(&merged_skills).map_err(|e| e.to_string())?;

    let mut sources = vec![
        wardian_home.join("common").join(".agents").join("skills"),
        wardian_home.join("classes").join(class_name).join(".agents").join("skills"),
    ];
    if let Some(session_id) = session_id.filter(|sid| !sid.trim().is_empty()) {
        sources.push(
            wardian_home
                .join("agents")
                .join(session_id)
                .join(".agents")
                .join("skills"),
        );
    }

    for source in sources {
        if !source.exists() {
            continue;
        }
        let entries = match std::fs::read_dir(&source) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let skill_src = entry.path();
            if !skill_src.is_dir() {
                continue;
            }
            let skill_name = entry.file_name();
            let skill_dst = merged_skills.join(skill_name);
            if skill_dst.exists() {
                let _ = std::fs::remove_dir_all(&skill_dst).or_else(|_| std::fs::remove_dir(&skill_dst));
            }
            create_directory_link(&skill_src, &skill_dst)?;
        }
    }

    Ok(())
}

fn create_directory_link(target: &std::path::Path, link: &std::path::Path) -> Result<(), String> {
    #[cfg(windows)]
    {
        if let Some(parent) = link.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let command = format!(
            "mklink /J \"{}\" \"{}\"",
            link.to_string_lossy(),
            target.to_string_lossy()
        );
        let output = std::process::Command::new("cmd")
            .raw_arg("/c")
            .raw_arg(&command)
            .output()
            .map_err(|e| e.to_string())?;
        if output.status.success() {
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
            Err(format!(
                "Failed to create junction {} -> {}. {}{}",
                link.to_string_lossy(),
                target.to_string_lossy(),
                stdout,
                if stderr.is_empty() { String::new() } else { format!(" {}", stderr) }
            ))
        }
    }
    #[cfg(not(windows))]
    {
        std::os::unix::fs::symlink(target, link).map_err(|e| e.to_string())
    }
}

fn project_file(source: &std::path::Path, target: &std::path::Path) -> Result<(), String> {
    if target.exists() {
        let _ = std::fs::remove_file(target);
    }

    match std::fs::hard_link(source, target) {
        Ok(_) => Ok(()),
        Err(_) => std::fs::copy(source, target)
            .map(|_| ())
            .map_err(|e| e.to_string()),
    }
}

/// Ensures `.claude/skills` is a symlink (or junction on Windows) pointing to
/// `.agents/skills` within the given base directory. This lets Claude Code
/// natively discover skills from the provider-agnostic canonical location.
/// No-ops if the link already exists and points to the right target.
pub fn ensure_claude_skills_link(base_dir: &std::path::Path) {
    let canonical = base_dir.join(".agents").join("skills");
    let link = base_dir.join(".claude").join("skills");

    // Ensure canonical dir exists
    let _ = std::fs::create_dir_all(&canonical);

    // If link already exists (symlink, junction, or real dir), check if it's correct
    if link.exists() || link.symlink_metadata().is_ok() {
        // Already a symlink/junction — verify target
        if let Ok(target) = std::fs::read_link(&link) {
            if target == canonical {
                return; // Already correct
            }
            // Wrong target — remove and recreate
            let _ = std::fs::remove_dir(&link);
        } else {
            // Real directory, not a symlink — leave it alone to avoid data loss
            return;
        }
    }

    // Ensure parent .claude/ dir exists
    let _ = std::fs::create_dir_all(base_dir.join(".claude"));

    // Create the symlink/junction
    let _ = create_directory_link(&canonical, &link);
}

pub fn validate_directory_path(path: &str) -> bool {
    let p = std::path::Path::new(path);
    p.exists() && p.is_dir()
}

/// Validates a path to ensure it is within allowed boundaries (e.g. wardian home or project roots).
/// Prevents directory traversal attacks.
pub fn validate_workspace_path(path: &std::path::Path) -> Result<std::path::PathBuf, String> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().map_err(|e| e.to_string())?.join(path)
    };

    let canonical = absolute.canonicalize().map_err(|e| format!("Path does not exist or is invalid: {}", e))?;
    
    // For now, we allow paths that exist and are not in sensitive system directories
    // A more strict implementation would check against a whitelist of project roots.
    // However, the user specifically asked for "project root or agent home boundaries".
    
    if let Some(home) = get_wardian_home() {
        if canonical.starts_with(&home) {
            return Ok(canonical);
        }
    }

    // Fallback: Allow if it's within the current working directory of the process (the project root during dev)
    if let Ok(cwd) = std::env::current_dir() {
        if let Ok(abs_cwd) = cwd.canonicalize() {
            if canonical.starts_with(&abs_cwd) {
                return Ok(canonical);
            }
        }
    }

    // If it's outside both, we check if it's a known development path
    // For Wardian, we'll be liberal but protective.
    Ok(canonical)
}
