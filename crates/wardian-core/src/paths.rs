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
