use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallOutcome {
    Installed(PathBuf),
    AlreadyInstalled(PathBuf),
}

pub fn bundled_cli_file_name() -> &'static str {
    if cfg!(windows) {
        "wardian-cli.exe"
    } else {
        "wardian-cli"
    }
}

pub fn launcher_file_name() -> &'static str {
    if cfg!(windows) {
        "wardian.cmd"
    } else {
        "wardian"
    }
}

pub fn install_cli_from_resources(_resources_dir: &Path) -> Result<InstallOutcome, String> {
    install_cli_from_resources_with_path_update(_resources_dir, ensure_cli_bin_on_path)
}

fn install_cli_from_resources_with_path_update<F>(
    resources_dir: &Path,
    mut update_path: F,
) -> Result<InstallOutcome, String>
where
    F: FnMut(&Path) -> Result<(), String>,
{
    let source = resources_dir.join("bin").join(bundled_cli_file_name());
    if !source.is_file() {
        return Err(format!(
            "CLI resource was not found at {}",
            source.display()
        ));
    }

    let launcher = wardian_core::paths::cli_bin_path()
        .ok_or_else(|| "Could not resolve Wardian CLI install path".to_string())?;
    let target_dir = launcher
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| "Could not resolve Wardian CLI install directory".to_string())?;
    let target = target_dir.join(bundled_cli_file_name());

    std::fs::create_dir_all(&target_dir)
        .map_err(|err| format!("Failed to create {}: {err}", target_dir.display()))?;

    let should_copy_binary = match (std::fs::metadata(&source), std::fs::metadata(&target)) {
        (Ok(source_meta), Ok(target_meta)) => source_meta.len() != target_meta.len(),
        (Ok(_), Err(_)) => true,
        (Err(err), _) => return Err(format!("Failed to inspect {}: {err}", source.display())),
    };
    let should_write_launcher = launcher_needs_update(&launcher);

    let outcome = if should_copy_binary || should_write_launcher {
        if should_copy_binary {
            std::fs::copy(&source, &target).map_err(|err| {
                format!(
                    "Failed to copy CLI from {} to {}: {err}",
                    source.display(),
                    target.display()
                )
            })?;
            make_executable(&target)?;
        }

        write_launcher(&launcher)?;
        make_executable(&launcher)?;
        InstallOutcome::Installed(launcher)
    } else {
        InstallOutcome::AlreadyInstalled(launcher)
    };

    update_path(&target_dir)?;
    Ok(outcome)
}

fn launcher_needs_update(path: &Path) -> bool {
    match std::fs::read_to_string(path) {
        Ok(existing) => normalize_line_endings(&existing) != launcher_contents(),
        Err(_) => true,
    }
}

fn write_launcher(path: &Path) -> Result<(), String> {
    let contents = if cfg!(windows) {
        launcher_contents().replace('\n', "\r\n")
    } else {
        launcher_contents()
    };

    std::fs::write(path, contents)
        .map_err(|err| format!("Failed to write CLI launcher {}: {err}", path.display()))
}

fn normalize_line_endings(value: &str) -> String {
    value.replace("\r\n", "\n")
}

fn launcher_contents() -> String {
    if cfg!(windows) {
        "@echo off\n\"%~dp0wardian-cli.exe\" %*\n".to_string()
    } else {
        "#!/usr/bin/env sh\nexec \"$(dirname \"$0\")/wardian-cli\" \"$@\"\n".to_string()
    }
}

#[cfg(windows)]
fn path_contains_dir(path_value: &str, dir: &Path) -> bool {
    let target = normalize_windows_path_segment(dir);
    path_value
        .split(';')
        .any(|segment| !segment.trim().is_empty() && normalize_windows_path_text(segment) == target)
}

#[cfg(unix)]
fn append_unix_path_marker(profile_path: &Path, _bin_dir: &Path) -> Result<(), String> {
    const MARKER_START: &str = "# wardian-cli";
    const MARKER_BLOCK: &str =
        "# wardian-cli\nexport PATH=\"$HOME/.wardian/bin:$PATH\"\n# /wardian-cli\n";

    let existing = match std::fs::read_to_string(profile_path) {
        Ok(content) => content,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(err) => {
            return Err(format!(
                "Failed to read shell profile {}: {err}",
                profile_path.display()
            ))
        }
    };

    if existing.contains(MARKER_START) {
        return Ok(());
    }

    if let Some(parent) = profile_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|err| format!("Failed to create {}: {err}", parent.display()))?;
    }

    let mut next = existing;
    if !next.is_empty() && !next.ends_with('\n') {
        next.push('\n');
    }
    next.push_str(MARKER_BLOCK);
    std::fs::write(profile_path, next).map_err(|err| {
        format!(
            "Failed to update shell profile {}: {err}",
            profile_path.display()
        )
    })
}

#[cfg(windows)]
fn normalize_windows_path_segment(path: &Path) -> String {
    normalize_windows_path_text(&path.display().to_string())
}

#[cfg(windows)]
fn normalize_windows_path_text(value: &str) -> String {
    value
        .trim()
        .trim_matches('"')
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_ascii_lowercase()
}

#[cfg(windows)]
fn ensure_cli_bin_on_path(bin_dir: &Path) -> Result<(), String> {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let (environment, _) = hkcu
        .create_subkey("Environment")
        .map_err(|err| format!("Failed to open HKCU\\Environment: {err}"))?;
    let current_path = environment
        .get_value::<String, _>("Path")
        .unwrap_or_default();

    if path_contains_dir(&current_path, bin_dir) {
        return Ok(());
    }

    let bin_value = bin_dir.display().to_string();
    let next_path = if current_path.trim().is_empty() {
        bin_value
    } else {
        format!("{current_path};{bin_value}")
    };
    environment
        .set_value("Path", &next_path)
        .map_err(|err| format!("Failed to update user PATH: {err}"))?;
    broadcast_windows_environment_change();
    Ok(())
}

#[cfg(windows)]
fn broadcast_windows_environment_change() {
    use windows::core::w;
    use windows::Win32::Foundation::{LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        SendMessageTimeoutW, HWND_BROADCAST, SMTO_ABORTIFHUNG, WM_SETTINGCHANGE,
    };

    let mut result = 0usize;
    unsafe {
        let _ = SendMessageTimeoutW(
            HWND_BROADCAST,
            WM_SETTINGCHANGE,
            WPARAM(0),
            LPARAM(w!("Environment").as_ptr() as isize),
            SMTO_ABORTIFHUNG,
            5000,
            Some(&mut result),
        );
    }
}

#[cfg(unix)]
fn ensure_cli_bin_on_path(bin_dir: &Path) -> Result<(), String> {
    let home = dirs::home_dir().ok_or_else(|| "Could not resolve home directory".to_string())?;
    let candidates = [".zshrc", ".bashrc", ".profile"].map(|name| home.join(name));
    let profile = candidates
        .iter()
        .find(|candidate| candidate.exists())
        .cloned()
        .unwrap_or_else(|| home.join(".profile"));

    append_unix_path_marker(&profile, bin_dir)
}

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;

    let mut permissions = std::fs::metadata(path)
        .map_err(|err| format!("Failed to inspect {}: {err}", path.display()))?
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(path, permissions)
        .map_err(|err| format!("Failed to mark {} executable: {err}", path.display()))
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn target_binary_name_matches_platform() {
        let expected = if cfg!(windows) { "wardian.cmd" } else { "wardian" };

        assert_eq!(launcher_file_name(), expected);
    }

    #[test]
    fn implementation_binary_name_matches_platform() {
        let expected = if cfg!(windows) {
            "wardian-cli.exe"
        } else {
            "wardian-cli"
        };

        assert_eq!(bundled_cli_file_name(), expected);
    }

    #[test]
    fn installer_copies_cli_binary_to_wardian_home_bin() {
        let _guard = crate::utils::wardian_test_env_lock();
        let home = TempDir::new().unwrap();
        let resources = TempDir::new().unwrap();
        let source_dir = resources.path().join("bin");
        std::fs::create_dir_all(&source_dir).unwrap();
        std::fs::write(source_dir.join(bundled_cli_file_name()), b"wardian cli").unwrap();
        std::env::set_var("WARDIAN_HOME", home.path());

        let mut path_updates = 0;
        let outcome = install_cli_from_resources_with_path_update(resources.path(), |_bin_dir| {
            path_updates += 1;
            Ok(())
        })
        .unwrap();

        let impl_target = home.path().join("bin").join(bundled_cli_file_name());
        let launcher_target = home.path().join("bin").join(launcher_file_name());
        assert_eq!(outcome, InstallOutcome::Installed(launcher_target.clone()));
        assert_eq!(std::fs::read(impl_target).unwrap(), b"wardian cli");
        assert!(std::fs::read_to_string(launcher_target)
            .unwrap()
            .contains("wardian-cli"));
        assert_eq!(path_updates, 1);

        std::env::remove_var("WARDIAN_HOME");
    }

    #[test]
    fn installer_reports_already_installed_when_binary_matches() {
        let _guard = crate::utils::wardian_test_env_lock();
        let home = TempDir::new().unwrap();
        let resources = TempDir::new().unwrap();
        let source_dir = resources.path().join("bin");
        let target_dir = home.path().join("bin");
        std::fs::create_dir_all(&source_dir).unwrap();
        std::fs::create_dir_all(&target_dir).unwrap();
        std::fs::write(source_dir.join(bundled_cli_file_name()), b"wardian cli").unwrap();
        std::fs::write(target_dir.join(bundled_cli_file_name()), b"wardian cli").unwrap();
        std::fs::write(
            target_dir.join(launcher_file_name()),
            launcher_contents().replace('\n', "\r\n"),
        )
        .unwrap();
        std::env::set_var("WARDIAN_HOME", home.path());

        let outcome =
            install_cli_from_resources_with_path_update(resources.path(), |_bin_dir| Ok(()))
                .unwrap();

        assert_eq!(
            outcome,
            InstallOutcome::AlreadyInstalled(target_dir.join(launcher_file_name()))
        );

        std::env::remove_var("WARDIAN_HOME");
    }

    #[cfg(windows)]
    #[test]
    fn windows_path_detection_is_case_insensitive() {
        let dir = PathBuf::from(r"C:\Users\Alice\.wardian\bin");
        let value = r"C:\Windows\System32;c:\users\alice\.WARDIAN\BIN";

        assert!(path_contains_dir(value, &dir));
    }

    #[cfg(unix)]
    #[test]
    fn unix_profile_marker_is_idempotent() {
        let profile = TempDir::new().unwrap().path().join(".profile");
        let bin_dir = PathBuf::from("/home/alice/.wardian/bin");

        append_unix_path_marker(&profile, &bin_dir).unwrap();
        append_unix_path_marker(&profile, &bin_dir).unwrap();

        let content = std::fs::read_to_string(profile).unwrap();
        assert_eq!(content.matches("# wardian-cli").count(), 1);
        assert!(content.contains(r#"export PATH="$HOME/.wardian/bin:$PATH""#));
    }
}
