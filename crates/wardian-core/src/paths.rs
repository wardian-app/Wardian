use std::path::{Path, PathBuf};

pub fn wardian_home() -> Option<PathBuf> {
    if let Some(home) = wardian_home_env() {
        return Some(home);
    }

    default_production_home()
}

pub fn wardian_home_for_manifest(manifest_dir: &Path) -> Option<PathBuf> {
    #[cfg(debug_assertions)]
    {
        let debug_home = debug_home_for_manifest(manifest_dir);
        if let Some(env_home) = wardian_home_env() {
            let production_home = default_production_home();
            if debug_production_home_allowed()
                || production_home
                    .as_ref()
                    .is_none_or(|home| !same_path_lexically(&env_home, home))
            {
                return Some(env_home);
            }
        }

        Some(debug_home)
    }

    #[cfg(not(debug_assertions))]
    {
        let _ = manifest_dir;
        wardian_home()
    }
}

fn wardian_home_env() -> Option<PathBuf> {
    std::env::var("WARDIAN_HOME").ok().and_then(|value| {
        let trimmed = value.trim();
        (!trimmed.is_empty()).then(|| PathBuf::from(trimmed))
    })
}

fn default_production_home() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(".wardian"))
}

#[cfg(debug_assertions)]
fn debug_home_for_manifest(manifest_dir: &Path) -> PathBuf {
    debug_target_dir_for_manifest(manifest_dir)
        .join("debug")
        .join(".wardian")
}

#[cfg(debug_assertions)]
fn debug_production_home_allowed() -> bool {
    std::env::var("WARDIAN_DEBUG_ALLOW_PRODUCTION_HOME")
        .map(|value| value.trim() == "1")
        .unwrap_or(false)
}

#[cfg(debug_assertions)]
fn same_path_lexically(left: &Path, right: &Path) -> bool {
    let normalize = |path: &Path| {
        let mut text = path
            .components()
            .collect::<PathBuf>()
            .to_string_lossy()
            .replace('\\', "/");
        while text.ends_with('/') && text.len() > 1 {
            text.pop();
        }
        #[cfg(windows)]
        {
            text = text.to_ascii_lowercase();
        }
        text
    };

    normalize(left) == normalize(right)
}

#[cfg(debug_assertions)]
fn debug_target_dir_for_manifest(manifest_dir: &Path) -> PathBuf {
    if manifest_dir
        .file_name()
        .is_some_and(|name| name == "src-tauri")
    {
        if let Some(workspace_root) = manifest_dir.parent() {
            return workspace_root.join("target");
        }
    }

    manifest_dir.join("target")
}

pub fn state_db_path() -> Option<PathBuf> {
    wardian_home().map(|home| home.join("state.db"))
}

pub fn cli_bin_dir() -> Option<PathBuf> {
    wardian_home().map(|home| home.join("bin"))
}

pub fn cli_bin_path() -> Option<PathBuf> {
    let name = if cfg!(windows) {
        "wardian.cmd"
    } else {
        "wardian"
    };
    cli_bin_dir().map(|dir| dir.join(name))
}

pub fn agents_dir() -> Option<PathBuf> {
    wardian_home().map(|home| home.join("agents"))
}

/// `<wardian-home>/agents/<agent-id>/conversations`.
pub fn agent_conversations_dir(agent_id: &str) -> Option<PathBuf> {
    if !is_safe_path_component(agent_id) {
        return None;
    }
    agents_dir().map(|dir| dir.join(agent_id).join("conversations"))
}

/// `<wardian-home>/agents/<agent-id>/conversations/<conversation-id>`.
pub fn agent_conversation_dir(agent_id: &str, conversation_id: &str) -> Option<PathBuf> {
    if !is_safe_path_component(conversation_id) {
        return None;
    }
    agent_conversations_dir(agent_id).map(|dir| dir.join(conversation_id))
}

/// `<wardian-home>/logs/workflows` — root of all workflow run logs.
pub fn workflow_runs_dir() -> Option<PathBuf> {
    wardian_home().map(|home| home.join("logs").join("workflows"))
}

/// `<wardian-home>/logs/workflows/<blueprint_id>/<run_id>` — one run's durable root.
pub fn workflow_run_dir(blueprint_id: &str, run_id: &str) -> Option<PathBuf> {
    if !is_safe_path_component(blueprint_id) || !is_safe_path_component(run_id) {
        return None;
    }
    workflow_runs_dir().map(|dir| dir.join(blueprint_id).join(run_id))
}

pub fn is_safe_path_component(value: &str) -> bool {
    let value = value.trim();
    if value.is_empty() || value == "." || value == ".." {
        return false;
    }
    value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
}

/// `<wardian-home>/library/workflows` — where workflow blueprints live.
pub fn library_workflows_dir() -> Option<PathBuf> {
    wardian_home().map(|home| home.join("library").join("workflows"))
}

/// `<wardian-home>/library/workflows/<blueprint_id>.md`.
pub fn blueprint_path(blueprint_id: &str) -> Option<PathBuf> {
    library_workflows_dir().map(|dir| dir.join(format!("{blueprint_id}.md")))
}

/// `<wardian-home>/library/schedules.json` — the workflow schedule index.
pub fn schedules_path() -> Option<PathBuf> {
    wardian_home().map(|home| home.join("library").join("schedules.json"))
}

/// `<wardian-home>/topology.json` — manual communication-topology edges.
pub fn topology_path_for_home(home: &Path) -> PathBuf {
    home.join("topology.json")
}

/// `<wardian-home>/settings/workbench.json`.
pub fn workbench_path_for_home(home: &Path) -> PathBuf {
    home.join("settings").join("workbench.json")
}

/// `<wardian-home>/settings/workbench.backup.json`.
pub fn workbench_backup_path_for_home(home: &Path) -> PathBuf {
    home.join("settings").join("workbench.backup.json")
}

/// `<wardian-home>/library`.
pub fn library_dir_for_home(home: &Path) -> PathBuf {
    home.join("library")
}

/// `<wardian-home>/library/library.json` — tags/stars metadata index.
pub fn library_metadata_path_for_home(home: &Path) -> PathBuf {
    home.join("library").join("library.json")
}

/// `<wardian-home>/classes`.
pub fn classes_dir_for_home(home: &Path) -> PathBuf {
    home.join("classes")
}

/// `<wardian-home>/common`.
pub fn common_dir_for_home(home: &Path) -> PathBuf {
    home.join("common")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wardian_home_respects_env_override() {
        let _guard = crate::tests::env_lock();
        std::env::set_var("WARDIAN_HOME", "/tmp/wardian-cli-plan");
        assert_eq!(
            wardian_home().unwrap(),
            PathBuf::from("/tmp/wardian-cli-plan")
        );
        std::env::remove_var("WARDIAN_HOME");
    }

    #[test]
    fn cli_bin_path_uses_platform_binary_name() {
        let _guard = crate::tests::env_lock();
        std::env::set_var("WARDIAN_HOME", "/tmp/wardian-cli-bin");
        let expected = if cfg!(windows) {
            "wardian.cmd"
        } else {
            "wardian"
        };
        assert!(cli_bin_path().unwrap().ends_with(expected));
        std::env::remove_var("WARDIAN_HOME");
    }

    #[test]
    fn workflow_run_paths_use_home_logs_workflows_layout() {
        let _guard = crate::tests::env_lock();
        std::env::set_var("WARDIAN_HOME", "/tmp/wardian-run-view");

        assert_eq!(
            workflow_runs_dir().unwrap(),
            PathBuf::from("/tmp/wardian-run-view")
                .join("logs")
                .join("workflows")
        );
        assert_eq!(
            workflow_run_dir("wf", "run-1").unwrap(),
            PathBuf::from("/tmp/wardian-run-view")
                .join("logs")
                .join("workflows")
                .join("wf")
                .join("run-1")
        );

        std::env::remove_var("WARDIAN_HOME");
    }

    #[test]
    fn workflow_run_dir_rejects_path_traversal_components() {
        let _guard = crate::tests::env_lock();
        std::env::set_var("WARDIAN_HOME", "/tmp/wardian-run-view");

        assert!(workflow_run_dir("../../outside", "run-1").is_none());
        assert!(workflow_run_dir("wf", "../outside").is_none());
        assert!(workflow_run_dir("wf/name", "run-1").is_none());
        assert!(workflow_run_dir("wf", "run\\one").is_none());
        assert!(workflow_run_dir("wf.name_1", "run-1").is_some());

        std::env::remove_var("WARDIAN_HOME");
    }

    #[test]
    fn agent_conversation_paths_use_agent_owned_layout() {
        let _guard = crate::tests::env_lock();
        std::env::set_var("WARDIAN_HOME", "/tmp/wardian-conversations");

        assert_eq!(
            agent_conversations_dir("agent-1").unwrap(),
            PathBuf::from("/tmp/wardian-conversations")
                .join("agents")
                .join("agent-1")
                .join("conversations")
        );
        assert_eq!(
            agent_conversation_dir("agent-1", "conv_20260615_000000_agent_1").unwrap(),
            PathBuf::from("/tmp/wardian-conversations")
                .join("agents")
                .join("agent-1")
                .join("conversations")
                .join("conv_20260615_000000_agent_1")
        );

        std::env::remove_var("WARDIAN_HOME");
    }

    #[test]
    fn agent_conversation_paths_reject_unsafe_components() {
        let _guard = crate::tests::env_lock();
        std::env::set_var("WARDIAN_HOME", "/tmp/wardian-conversations");

        assert!(agent_conversations_dir("../agent").is_none());
        assert!(agent_conversation_dir("agent-1", "../conv").is_none());
        assert!(agent_conversation_dir("agent-1", "conv/name").is_none());
        assert!(agent_conversation_dir("agent-1", "conv\\name").is_none());
        assert!(agent_conversation_dir("agent-1", "conv.name_1").is_some());

        std::env::remove_var("WARDIAN_HOME");
    }

    #[test]
    fn debug_home_for_tauri_manifest_uses_workspace_target_dir() {
        let _guard = crate::tests::env_lock();
        std::env::remove_var("WARDIAN_HOME");
        std::env::remove_var("WARDIAN_DEBUG_ALLOW_PRODUCTION_HOME");

        let workspace_root = PathBuf::from("/repo/Wardian");
        let manifest_dir = workspace_root.join("src-tauri");

        assert_eq!(
            wardian_home_for_manifest(&manifest_dir).unwrap(),
            workspace_root.join("target").join("debug").join(".wardian")
        );
    }

    #[cfg(debug_assertions)]
    #[test]
    fn debug_app_home_ignores_inherited_default_production_home() {
        let _guard = crate::tests::env_lock();
        let production_home = dirs::home_dir().unwrap().join(".wardian");
        let workspace_root = PathBuf::from("/repo/Wardian");
        let manifest_dir = workspace_root.join("src-tauri");

        std::env::set_var("WARDIAN_HOME", &production_home);
        std::env::remove_var("WARDIAN_DEBUG_ALLOW_PRODUCTION_HOME");

        assert_eq!(
            wardian_home_for_manifest(&manifest_dir).unwrap(),
            workspace_root.join("target").join("debug").join(".wardian")
        );

        std::env::remove_var("WARDIAN_HOME");
    }

    #[cfg(debug_assertions)]
    #[test]
    fn debug_app_home_honors_explicit_non_production_home() {
        let _guard = crate::tests::env_lock();
        let explicit_home = PathBuf::from("/tmp/wardian-dev-home");
        let workspace_root = PathBuf::from("/repo/Wardian");
        let manifest_dir = workspace_root.join("src-tauri");

        std::env::set_var("WARDIAN_HOME", &explicit_home);
        std::env::remove_var("WARDIAN_DEBUG_ALLOW_PRODUCTION_HOME");

        assert_eq!(
            wardian_home_for_manifest(&manifest_dir).unwrap(),
            explicit_home
        );

        std::env::remove_var("WARDIAN_HOME");
    }

    #[cfg(debug_assertions)]
    #[test]
    fn debug_app_home_can_intentionally_use_production_home() {
        let _guard = crate::tests::env_lock();
        let production_home = dirs::home_dir().unwrap().join(".wardian");
        let workspace_root = PathBuf::from("/repo/Wardian");
        let manifest_dir = workspace_root.join("src-tauri");

        std::env::set_var("WARDIAN_HOME", &production_home);
        std::env::set_var("WARDIAN_DEBUG_ALLOW_PRODUCTION_HOME", "1");

        assert_eq!(
            wardian_home_for_manifest(&manifest_dir).unwrap(),
            production_home
        );

        std::env::remove_var("WARDIAN_DEBUG_ALLOW_PRODUCTION_HOME");
        std::env::remove_var("WARDIAN_HOME");
    }
}
