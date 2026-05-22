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
    let target_home = wardian_core::paths::wardian_home()
        .ok_or_else(|| "Could not resolve Wardian home".to_string())?;
    install_cli_from_resources_to_home_with_path_update(
        _resources_dir,
        &target_home,
        ensure_cli_bin_on_path,
    )
}

pub fn install_cli_from_resources_to_home(
    resources_dir: &Path,
    target_home: &Path,
) -> Result<InstallOutcome, String> {
    install_cli_from_resources_to_home_with_path_update(
        resources_dir,
        target_home,
        ensure_cli_bin_on_path,
    )
}

#[cfg(test)]
fn install_cli_from_resources_with_path_update<F>(
    resources_dir: &Path,
    update_path: F,
) -> Result<InstallOutcome, String>
where
    F: FnMut(&Path) -> Result<(), String>,
{
    let target_home = wardian_core::paths::wardian_home()
        .ok_or_else(|| "Could not resolve Wardian home".to_string())?;
    install_cli_from_resources_to_home_with_path_update(resources_dir, &target_home, update_path)
}

fn install_cli_from_resources_to_home_with_path_update<F>(
    resources_dir: &Path,
    target_home: &Path,
    mut update_path: F,
) -> Result<InstallOutcome, String>
where
    F: FnMut(&Path) -> Result<(), String>,
{
    let source = bundled_cli_source_path(resources_dir);
    if !source.is_file() {
        return Err(format!(
            "CLI resource was not found at {}",
            source.display()
        ));
    }

    let target_dir = target_home.join("bin");
    let launcher = target_dir.join(launcher_file_name());
    let target = target_dir.join(bundled_cli_file_name());

    std::fs::create_dir_all(&target_dir)
        .map_err(|err| format!("Failed to create {}: {err}", target_dir.display()))?;

    let should_copy_binary = binary_needs_update(&source, &target)?;
    let should_write_launcher = launcher_needs_update(&launcher, &launcher_contents());
    #[cfg(windows)]
    let posix_launcher = target_dir.join("wardian");
    #[cfg(windows)]
    let should_write_posix_launcher =
        launcher_needs_update(&posix_launcher, &windows_posix_launcher_contents());
    #[cfg(not(windows))]
    let should_write_posix_launcher = false;

    let outcome = if should_copy_binary || should_write_launcher || should_write_posix_launcher {
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
        #[cfg(windows)]
        {
            write_launcher_contents(&posix_launcher, &windows_posix_launcher_contents())?;
            make_executable(&posix_launcher)?;
        }
        InstallOutcome::Installed(launcher)
    } else {
        InstallOutcome::AlreadyInstalled(launcher)
    };

    let is_default_home = wardian_core::paths::wardian_home()
        .as_deref()
        .is_some_and(|home| home == target_home);
    if std::env::var_os("WARDIAN_HOME").is_none() && is_default_home {
        update_path(&target_dir)?;
    }
    Ok(outcome)
}

fn bundled_cli_source_path(resources_dir: &Path) -> PathBuf {
    let direct = resources_dir.join("bin").join(bundled_cli_file_name());
    if direct.is_file() {
        return direct;
    }

    resources_dir
        .join("resources")
        .join("bin")
        .join(bundled_cli_file_name())
}

fn launcher_needs_update(path: &Path, expected_contents: &str) -> bool {
    match std::fs::read_to_string(path) {
        Ok(existing) => normalize_line_endings(&existing) != expected_contents,
        Err(_) => true,
    }
}

fn binary_needs_update(source: &Path, target: &Path) -> Result<bool, String> {
    let source_contents = std::fs::read(source)
        .map_err(|err| format!("Failed to read {}: {err}", source.display()))?;
    let target_contents = match std::fs::read(target) {
        Ok(contents) => contents,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(true),
        Err(err) => return Err(format!("Failed to read {}: {err}", target.display())),
    };

    Ok(source_contents != target_contents)
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

fn write_launcher_contents(path: &Path, contents: &str) -> Result<(), String> {
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
fn windows_posix_launcher_contents() -> String {
    "#!/usr/bin/env sh\nexec \"$(dirname \"$0\")/wardian-cli.exe\" \"$@\"\n".to_string()
}

#[cfg_attr(not(unix), allow(dead_code))]
fn unix_path_marker(bin_dir: &Path) -> String {
    format!(
        "# wardian-cli\nexport PATH={}:\"$PATH\"\n# /wardian-cli\n",
        shell_quote_path(bin_dir)
    )
}

#[cfg_attr(not(unix), allow(dead_code))]
fn shell_quote_path(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', "'\"'\"'"))
}

#[cfg(windows)]
fn path_contains_dir(path_value: &str, dir: &Path) -> bool {
    let target = normalize_windows_path_segment(dir);
    path_value
        .split(';')
        .any(|segment| !segment.trim().is_empty() && normalize_windows_path_text(segment) == target)
}

#[cfg(windows)]
pub(crate) fn child_path_with_cli_bin(current_path: Option<&str>) -> Option<String> {
    let bin_dir = wardian_core::paths::cli_bin_dir()?;
    let bin_value = bin_dir.display().to_string();
    let current_path = current_path.unwrap_or("");

    if path_contains_dir(current_path, &bin_dir) {
        return Some(current_path.to_string());
    }

    if current_path.trim().is_empty() {
        Some(bin_value)
    } else {
        Some(format!("{bin_value};{current_path}"))
    }
}

#[cfg(unix)]
fn append_unix_path_marker(profile_path: &Path, bin_dir: &Path) -> Result<(), String> {
    const MARKER_START: &str = "# wardian-cli";

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
    next.push_str(&unix_path_marker(bin_dir));
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
        let expected = if cfg!(windows) {
            "wardian.cmd"
        } else {
            "wardian"
        };

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
        assert_eq!(path_updates, 0);

        std::env::remove_var("WARDIAN_HOME");
    }

    #[test]
    fn installer_can_target_explicit_app_home_without_user_path_update() {
        let _guard = crate::utils::wardian_test_env_lock();
        let previous_home = std::env::var_os("WARDIAN_HOME");
        std::env::remove_var("WARDIAN_HOME");

        let app_home = TempDir::new().unwrap();
        let resources = TempDir::new().unwrap();
        let source_dir = resources.path().join("bin");
        std::fs::create_dir_all(&source_dir).unwrap();
        std::fs::write(source_dir.join(bundled_cli_file_name()), b"debug cli").unwrap();

        let mut path_updates = 0;
        let outcome = install_cli_from_resources_to_home_with_path_update(
            resources.path(),
            app_home.path(),
            |_bin_dir| {
                path_updates += 1;
                Ok(())
            },
        )
        .unwrap();

        let target_dir = app_home.path().join("bin");
        assert_eq!(
            outcome,
            InstallOutcome::Installed(target_dir.join(launcher_file_name()))
        );
        assert_eq!(
            std::fs::read(target_dir.join(bundled_cli_file_name())).unwrap(),
            b"debug cli"
        );
        assert_eq!(path_updates, 0);

        match previous_home {
            Some(value) => std::env::set_var("WARDIAN_HOME", value),
            None => std::env::remove_var("WARDIAN_HOME"),
        }
    }

    #[test]
    fn installer_skips_user_path_update_for_custom_wardian_home() {
        let _guard = crate::utils::wardian_test_env_lock();
        let home = TempDir::new().unwrap();
        let resources = TempDir::new().unwrap();
        let source_dir = resources.path().join("bin");
        std::fs::create_dir_all(&source_dir).unwrap();
        std::fs::write(source_dir.join(bundled_cli_file_name()), b"wardian cli").unwrap();
        std::env::set_var("WARDIAN_HOME", home.path());

        let mut path_updates = 0;
        install_cli_from_resources_with_path_update(resources.path(), |_bin_dir| {
            path_updates += 1;
            Ok(())
        })
        .unwrap();

        assert_eq!(path_updates, 0);

        std::env::remove_var("WARDIAN_HOME");
    }

    #[test]
    fn installer_finds_cli_under_packaged_resources_subdirectory() {
        let _guard = crate::utils::wardian_test_env_lock();
        let home = TempDir::new().unwrap();
        let resources = TempDir::new().unwrap();
        let source_dir = resources.path().join("resources").join("bin");
        std::fs::create_dir_all(&source_dir).unwrap();
        std::fs::write(source_dir.join(bundled_cli_file_name()), b"packaged cli").unwrap();
        std::env::set_var("WARDIAN_HOME", home.path());

        let outcome =
            install_cli_from_resources_with_path_update(resources.path(), |_bin_dir| Ok(()))
                .unwrap();

        let target_dir = home.path().join("bin");
        assert_eq!(
            outcome,
            InstallOutcome::Installed(target_dir.join(launcher_file_name()))
        );
        assert_eq!(
            std::fs::read(target_dir.join(bundled_cli_file_name())).unwrap(),
            b"packaged cli"
        );

        std::env::remove_var("WARDIAN_HOME");
    }

    #[test]
    fn installer_prefers_direct_resource_bin_when_present() {
        let resources = TempDir::new().unwrap();
        let direct_dir = resources.path().join("bin");
        let packaged_dir = resources.path().join("resources").join("bin");
        std::fs::create_dir_all(&direct_dir).unwrap();
        std::fs::create_dir_all(&packaged_dir).unwrap();
        let direct = direct_dir.join(bundled_cli_file_name());
        let packaged = packaged_dir.join(bundled_cli_file_name());
        std::fs::write(&direct, b"direct cli").unwrap();
        std::fs::write(&packaged, b"packaged cli").unwrap();

        assert_eq!(bundled_cli_source_path(resources.path()), direct);
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
        #[cfg(windows)]
        std::fs::write(
            target_dir.join("wardian"),
            windows_posix_launcher_contents(),
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

    #[test]
    fn installer_replaces_same_size_stale_binary() {
        let _guard = crate::utils::wardian_test_env_lock();
        let home = TempDir::new().unwrap();
        let resources = TempDir::new().unwrap();
        let source_dir = resources.path().join("bin");
        let target_dir = home.path().join("bin");
        std::fs::create_dir_all(&source_dir).unwrap();
        std::fs::create_dir_all(&target_dir).unwrap();
        std::fs::write(source_dir.join(bundled_cli_file_name()), b"new cli").unwrap();
        std::fs::write(target_dir.join(bundled_cli_file_name()), b"old cli").unwrap();
        std::fs::write(target_dir.join(launcher_file_name()), launcher_contents()).unwrap();
        std::env::set_var("WARDIAN_HOME", home.path());

        let outcome =
            install_cli_from_resources_with_path_update(resources.path(), |_bin_dir| Ok(()))
                .unwrap();

        assert_eq!(
            outcome,
            InstallOutcome::Installed(target_dir.join(launcher_file_name()))
        );
        assert_eq!(
            std::fs::read(target_dir.join(bundled_cli_file_name())).unwrap(),
            b"new cli"
        );

        std::env::remove_var("WARDIAN_HOME");
    }

    #[test]
    fn unix_path_marker_uses_custom_bin_dir() {
        let marker = unix_path_marker(Path::new("/tmp/custom wardian/bin"));

        assert!(marker.contains("export PATH='/tmp/custom wardian/bin':\"$PATH\""));
        assert!(!marker.contains("$HOME/.wardian/bin"));
    }

    #[cfg(windows)]
    #[test]
    fn windows_path_detection_is_case_insensitive() {
        let dir = PathBuf::from(r"C:\Users\Alice\.wardian\bin");
        let value = r"C:\Windows\System32;c:\users\alice\.WARDIAN\BIN";

        assert!(path_contains_dir(value, &dir));
    }

    #[cfg(windows)]
    #[test]
    fn windows_installer_writes_cmd_and_posix_launchers() {
        let _guard = crate::utils::wardian_test_env_lock();
        let home = TempDir::new().unwrap();
        let resources = TempDir::new().unwrap();
        let source_dir = resources.path().join("bin");
        std::fs::create_dir_all(&source_dir).unwrap();
        std::fs::write(source_dir.join(bundled_cli_file_name()), b"wardian cli").unwrap();
        std::env::set_var("WARDIAN_HOME", home.path());

        install_cli_from_resources_with_path_update(resources.path(), |_bin_dir| Ok(())).unwrap();

        let target_dir = home.path().join("bin");
        let cmd_launcher = std::fs::read_to_string(target_dir.join("wardian.cmd")).unwrap();
        let posix_launcher = std::fs::read_to_string(target_dir.join("wardian")).unwrap();

        assert!(cmd_launcher.contains("wardian-cli.exe"));
        assert!(posix_launcher.contains("#!/usr/bin/env sh"));
        assert!(posix_launcher.contains("wardian-cli.exe"));

        std::env::remove_var("WARDIAN_HOME");
    }

    #[cfg(windows)]
    #[test]
    fn windows_child_path_prepends_cli_bin_once() {
        let _guard = crate::utils::wardian_test_env_lock();
        let home = TempDir::new().unwrap();
        std::env::set_var("WARDIAN_HOME", home.path());
        let bin_dir = home.path().join("bin");
        let existing = r"C:\Windows\System32".to_string();

        let first = child_path_with_cli_bin(Some(&existing)).unwrap();
        let second = child_path_with_cli_bin(Some(&first)).unwrap();

        assert!(first.starts_with(&bin_dir.display().to_string()));
        assert_eq!(first.matches(&bin_dir.display().to_string()).count(), 1);
        assert_eq!(second, first);

        std::env::remove_var("WARDIAN_HOME");
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
        assert!(content.contains(r#"export PATH='/home/alice/.wardian/bin':"$PATH""#));
    }
}
