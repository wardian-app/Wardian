use std::path::{Path, PathBuf};

pub fn wardian_home() -> Option<PathBuf> {
    if let Ok(value) = std::env::var("WARDIAN_HOME") {
        if !value.trim().is_empty() {
            return Some(PathBuf::from(value));
        }
    }

    dirs::home_dir().map(|home| home.join(".wardian"))
}

pub fn wardian_home_for_manifest(manifest_dir: &Path) -> Option<PathBuf> {
    if let Ok(value) = std::env::var("WARDIAN_HOME") {
        if !value.trim().is_empty() {
            return Some(PathBuf::from(value));
        }
    }

    #[cfg(debug_assertions)]
    {
        return Some(manifest_dir.join("target").join("debug").join(".wardian"));
    }

    #[cfg(not(debug_assertions))]
    {
        let _ = manifest_dir;
        dirs::home_dir().map(|home| home.join(".wardian"))
    }
}

pub fn state_db_path() -> Option<PathBuf> {
    wardian_home().map(|home| home.join("state.db"))
}

pub fn cli_bin_dir() -> Option<PathBuf> {
    wardian_home().map(|home| home.join("bin"))
}

pub fn cli_bin_path() -> Option<PathBuf> {
    let name = if cfg!(windows) { "wardian.exe" } else { "wardian" };
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
            "wardian.exe"
        } else {
            "wardian"
        };
        assert!(cli_bin_path().unwrap().ends_with(expected));
        std::env::remove_var("WARDIAN_HOME");
    }
}
