use crate::utils::get_wardian_home;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

const SHELL_SETTINGS_FILE: &str = "shell_settings.json";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ShellOption {
    pub id: String,
    pub label: String,
    pub executable: String,
    #[serde(default)]
    pub default_args: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ShellSettings {
    pub shell_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_executable: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_args: Option<String>,
}

impl Default for ShellSettings {
    fn default() -> Self {
        Self {
            shell_id: "auto".to_string(),
            custom_executable: None,
            custom_args: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellLaunchSpec {
    pub executable: String,
    pub args: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ResolvedShell {
    id: String,
    executable: String,
    default_args: Vec<String>,
}

pub fn list_available_shells() -> Vec<ShellOption> {
    #[cfg(windows)]
    {
        discover_windows_shells()
    }
    #[cfg(not(windows))]
    {
        discover_unix_shells()
    }
}

pub fn load_shell_settings() -> Result<ShellSettings, String> {
    let path = shell_settings_path()?;
    load_shell_settings_from_path(&path)
}

pub fn save_shell_settings(settings: &ShellSettings) -> Result<ShellSettings, String> {
    let path = shell_settings_path()?;
    save_shell_settings_to_path(&path, settings)
}

pub fn build_shell_command(command: &str) -> Result<ShellLaunchSpec, String> {
    let settings = load_shell_settings().unwrap_or_default();
    let available = list_available_shells();
    build_shell_command_with_settings(command, &settings, &available)
}

pub fn build_program_launch(program: &str, args: &[String]) -> Result<ShellLaunchSpec, String> {
    let settings = load_shell_settings().unwrap_or_default();
    let available = list_available_shells();
    build_program_launch_with_settings(program, args, &settings, &available)
}

fn shell_settings_path() -> Result<PathBuf, String> {
    let wardian_home = get_wardian_home().ok_or("Could not find Wardian home")?;
    Ok(wardian_home.join(SHELL_SETTINGS_FILE))
}

fn load_shell_settings_from_path(path: &Path) -> Result<ShellSettings, String> {
    if !path.exists() {
        return Ok(ShellSettings::default());
    }

    let content = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let settings = serde_json::from_str::<ShellSettings>(&content).map_err(|e| e.to_string())?;
    Ok(normalize_settings(settings))
}

fn save_shell_settings_to_path(
    path: &Path,
    settings: &ShellSettings,
) -> Result<ShellSettings, String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }

    let normalized = normalize_settings(settings.clone());
    validate_shell_settings(&normalized, &list_available_shells())?;
    let content = serde_json::to_string_pretty(&normalized).map_err(|e| e.to_string())?;
    std::fs::write(path, content).map_err(|e| e.to_string())?;
    Ok(normalized)
}

fn normalize_settings(mut settings: ShellSettings) -> ShellSettings {
    settings.shell_id = if settings.shell_id.trim().is_empty() {
        "auto".to_string()
    } else {
        settings.shell_id.trim().to_string()
    };
    settings.custom_executable = settings
        .custom_executable
        .and_then(|value| trim_to_option(&value));
    settings.custom_args = settings
        .custom_args
        .and_then(|value| trim_to_option(&value));
    settings
}

fn trim_to_option(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn validate_shell_settings(
    settings: &ShellSettings,
    available: &[ShellOption],
) -> Result<(), String> {
    resolve_shell(settings, available).map(|_| ())
}

pub fn build_shell_command_with_settings(
    command: &str,
    settings: &ShellSettings,
    available: &[ShellOption],
) -> Result<ShellLaunchSpec, String> {
    if command.trim().is_empty() {
        return Err("Missing shell command".to_string());
    }

    let shell = resolve_shell(settings, available)?;
    let mut args = shell.default_args;
    if args.is_empty() {
        return Err("Selected shell does not define how to execute a command string".to_string());
    }
    args.push(command.to_string());

    Ok(ShellLaunchSpec {
        executable: shell.executable,
        args,
    })
}

pub fn build_program_launch_with_settings(
    program: &str,
    program_args: &[String],
    settings: &ShellSettings,
    available: &[ShellOption],
) -> Result<ShellLaunchSpec, String> {
    if program.trim().is_empty() {
        return Err("Missing program executable".to_string());
    }

    let shell = resolve_shell(settings, available)?;
    let mut args = shell.default_args;
    if args.is_empty() {
        return Err("Selected shell does not define how to execute a command string".to_string());
    }

    if uses_positional_program_forwarding(&shell.id) {
        args.extend(build_forwarded_program_args(
            &shell.id,
            program,
            program_args,
        ));
    } else {
        let command_text = build_program_command_text(&shell.id, program, program_args);
        args.push(command_text);
    }

    Ok(ShellLaunchSpec {
        executable: shell.executable,
        args,
    })
}

fn resolve_shell(
    settings: &ShellSettings,
    available: &[ShellOption],
) -> Result<ResolvedShell, String> {
    match settings.shell_id.as_str() {
        "auto" => resolve_auto_shell(available)
            .cloned()
            .map(shell_option_to_resolved)
            .ok_or("No compatible shell was discovered on this system".to_string()),
        "custom" => {
            let executable = settings
                .custom_executable
                .clone()
                .ok_or("Custom shell requires an executable path")?;
            let default_args = inferred_custom_args(&executable, settings.custom_args.as_deref())?;
            if default_args.is_empty() {
                return Err(
                    "Custom shell must provide command arguments such as -Command, -c, or /C"
                        .to_string(),
                );
            }

            Ok(ResolvedShell {
                id: shell_id_from_executable(Path::new(&executable)),
                executable,
                default_args,
            })
        }
        shell_id => available
            .iter()
            .find(|option| option.id == shell_id)
            .cloned()
            .map(shell_option_to_resolved)
            .ok_or_else(|| {
                format!(
                    "Selected shell '{}' is not available on this system",
                    shell_id
                )
            }),
    }
}

fn shell_option_to_resolved(option: ShellOption) -> ResolvedShell {
    ResolvedShell {
        id: option.id,
        executable: option.executable,
        default_args: option.default_args,
    }
}

fn inferred_custom_args(
    executable: &str,
    custom_args: Option<&str>,
) -> Result<Vec<String>, String> {
    if let Some(custom_args) = custom_args {
        let parsed =
            shlex::split(custom_args).ok_or("Custom shell arguments could not be parsed")?;
        if !parsed.is_empty() {
            return Ok(parsed);
        }
    }

    Ok(default_shell_args(&shell_id_from_executable(Path::new(
        executable,
    ))))
}

fn uses_positional_program_forwarding(shell_id: &str) -> bool {
    matches!(
        shell_id,
        "bash" | "git-bash" | "zsh" | "sh" | "dash" | "ksh"
    )
}

fn build_forwarded_program_args(
    shell_id: &str,
    program: &str,
    program_args: &[String],
) -> Vec<String> {
    let mut args = vec!["exec \"$@\"".to_string(), "wardian-shell".to_string()];

    if shell_id != "wsl" && is_windows_cmd_shim(program) {
        args.push("cmd.exe".to_string());
        args.push("/d".to_string());
        args.push("/c".to_string());
    }

    args.push(program.to_string());
    args.extend(program_args.iter().cloned());
    args
}

fn build_program_command_text(shell_id: &str, program: &str, program_args: &[String]) -> String {
    #[cfg(windows)]
    if shell_id == "wsl" {
        let cmd_invocation = build_cmd_invocation(program, program_args);
        return format!("exec cmd.exe /c {}", quote_posix_arg(&cmd_invocation));
    }

    match shell_id {
        "cmd" => build_cmd_invocation(program, program_args),
        "powershell" | "pwsh" => build_powershell_invocation(program, program_args),
        _ => {
            #[cfg(windows)]
            {
                if is_windows_cmd_shim(program) {
                    let cmd_invocation = build_cmd_invocation(program, program_args);
                    return format!("exec cmd.exe /c {}", quote_posix_arg(&cmd_invocation));
                }
            }
            build_posix_invocation(program, program_args)
        }
    }
}

fn build_cmd_invocation(program: &str, program_args: &[String]) -> String {
    let mut fragments = Vec::with_capacity(program_args.len() + 2);
    if is_windows_cmd_shim(program) {
        fragments.push("call".to_string());
    }
    fragments.push(quote_cmd_arg(program));
    fragments.extend(program_args.iter().map(|arg| quote_cmd_arg(arg)));
    fragments.join(" ")
}

fn build_powershell_invocation(program: &str, program_args: &[String]) -> String {
    #[cfg(windows)]
    if is_windows_cmd_shim(program) {
        let cmd_invocation = build_cmd_invocation(program, program_args);
        return format!(
            "& $env:ComSpec /d /c {}",
            quote_powershell_arg(&cmd_invocation)
        );
    }

    let mut fragments = Vec::with_capacity(program_args.len() + 2);
    fragments.push("&".to_string());
    fragments.push(quote_powershell_arg(program));
    fragments.extend(program_args.iter().map(|arg| quote_powershell_arg(arg)));
    fragments.join(" ")
}

fn build_posix_invocation(program: &str, program_args: &[String]) -> String {
    let mut fragments = Vec::with_capacity(program_args.len() + 2);
    fragments.push("exec".to_string());
    fragments.push(quote_posix_arg(program));
    fragments.extend(program_args.iter().map(|arg| quote_posix_arg(arg)));
    fragments.join(" ")
}

fn quote_cmd_arg(value: &str) -> String {
    let escaped = value.replace('"', r#"\""#);
    if escaped.is_empty()
        || escaped
            .chars()
            .any(|ch| ch.is_whitespace() || matches!(ch, '^' | '&' | '|' | '<' | '>' | '(' | ')'))
    {
        format!("\"{}\"", escaped)
    } else {
        escaped
    }
}

fn quote_powershell_arg(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn quote_posix_arg(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn resolve_auto_shell(available: &[ShellOption]) -> Option<&ShellOption> {
    #[cfg(windows)]
    let preferred_ids = ["pwsh", "powershell", "cmd", "git-bash", "wsl", "bash"];
    #[cfg(not(windows))]
    let preferred_ids = ["zsh", "bash", "sh", "fish"];

    #[cfg(not(windows))]
    if let Ok(current_shell) = std::env::var("SHELL") {
        let current_id = shell_id_from_executable(Path::new(&current_shell));
        if let Some(shell) = available.iter().find(|option| option.id == current_id) {
            return Some(shell);
        }
    }

    preferred_ids
        .iter()
        .find_map(|id| available.iter().find(|option| option.id == *id))
        .or_else(|| available.first())
}

#[cfg(windows)]
fn discover_windows_shells() -> Vec<ShellOption> {
    let mut shells = Vec::new();

    if let Some(executable) = system_executable("cmd.exe") {
        shells.push(shell_option(
            "cmd",
            "Command Prompt",
            executable,
            default_shell_args("cmd"),
        ));
    }
    if let Some(executable) = find_executable("pwsh") {
        shells.push(shell_option(
            "pwsh",
            "PowerShell 7",
            executable,
            default_shell_args("pwsh"),
        ));
    }
    if let Some(executable) = system_executable("WindowsPowerShell\\v1.0\\powershell.exe") {
        shells.push(shell_option(
            "powershell",
            "Windows PowerShell",
            executable,
            default_shell_args("powershell"),
        ));
    }
    if let Some(executable) = discover_git_bash() {
        shells.push(shell_option(
            "git-bash",
            "Git Bash",
            executable,
            default_shell_args("git-bash"),
        ));
    } else if let Some(executable) = find_executable("bash") {
        shells.push(shell_option(
            "bash",
            "Bash",
            executable,
            default_shell_args("bash"),
        ));
    }
    if let Some(executable) = system_executable("wsl.exe") {
        shells.push(shell_option(
            "wsl",
            "WSL",
            executable,
            default_shell_args("wsl"),
        ));
    }

    dedupe_shells(shells)
}

#[cfg(not(windows))]
fn discover_unix_shells() -> Vec<ShellOption> {
    let mut candidates = Vec::new();

    if let Ok(current_shell) = std::env::var("SHELL") {
        candidates.push(PathBuf::from(current_shell));
    }

    if let Ok(content) = std::fs::read_to_string("/etc/shells") {
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            candidates.push(PathBuf::from(trimmed));
        }
    }

    let shells = candidates
        .into_iter()
        .filter(|candidate| candidate.exists())
        .map(|candidate| {
            let id = shell_id_from_executable(&candidate);
            let label = shell_label_for_id(&id);
            shell_option(id.as_str(), &label, candidate, default_shell_args(&id))
        })
        .collect::<Vec<_>>();

    dedupe_shells(shells)
}

fn dedupe_shells(shells: Vec<ShellOption>) -> Vec<ShellOption> {
    let mut seen = HashSet::new();
    let mut deduped = Vec::new();
    for shell in shells {
        if seen.insert(shell.id.clone()) {
            deduped.push(shell);
        }
    }
    deduped
}

fn shell_option(
    id: &str,
    label: &str,
    executable: PathBuf,
    default_args: Vec<String>,
) -> ShellOption {
    ShellOption {
        id: id.to_string(),
        label: label.to_string(),
        executable: executable.to_string_lossy().to_string(),
        default_args,
    }
}

fn default_shell_args(shell_id: &str) -> Vec<String> {
    match shell_id {
        "cmd" => vec!["/C".to_string()],
        "powershell" | "pwsh" => vec!["-NoProfile".to_string(), "-Command".to_string()],
        "bash" | "git-bash" | "zsh" => vec!["-lc".to_string()],
        "wsl" => vec!["-e".to_string(), "bash".to_string(), "-lc".to_string()],
        "sh" | "dash" | "ksh" | "fish" => vec!["-c".to_string()],
        _ => vec!["-c".to_string()],
    }
}

fn shell_id_from_executable(path: &Path) -> String {
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();

    match file_name.as_str() {
        "cmd" | "cmd.exe" => "cmd".to_string(),
        "pwsh" | "pwsh.exe" => "pwsh".to_string(),
        "powershell" | "powershell.exe" => "powershell".to_string(),
        "wsl" | "wsl.exe" => "wsl".to_string(),
        "bash" | "bash.exe" => {
            if path.to_string_lossy().to_ascii_lowercase().contains("git") {
                "git-bash".to_string()
            } else {
                "bash".to_string()
            }
        }
        "zsh" => "zsh".to_string(),
        "sh" => "sh".to_string(),
        "fish" => "fish".to_string(),
        "dash" => "dash".to_string(),
        "ksh" => "ksh".to_string(),
        other => other.trim_end_matches(".exe").to_string(),
    }
}

#[cfg(not(windows))]
fn shell_label_for_id(shell_id: &str) -> String {
    match shell_id {
        "cmd" => "Command Prompt".to_string(),
        "pwsh" => "PowerShell 7".to_string(),
        "powershell" => "Windows PowerShell".to_string(),
        "git-bash" => "Git Bash".to_string(),
        "wsl" => "WSL".to_string(),
        "bash" => "Bash".to_string(),
        "zsh" => "Zsh".to_string(),
        "sh" => "Sh".to_string(),
        "fish" => "Fish".to_string(),
        "dash" => "Dash".to_string(),
        "ksh" => "Ksh".to_string(),
        other => other.to_string(),
    }
}

#[cfg(windows)]
fn discover_git_bash() -> Option<PathBuf> {
    let mut candidates = Vec::new();

    if let Ok(program_files) = std::env::var("ProgramFiles") {
        candidates.push(
            PathBuf::from(&program_files)
                .join("Git")
                .join("bin")
                .join("bash.exe"),
        );
        candidates.push(
            PathBuf::from(program_files)
                .join("Git")
                .join("usr")
                .join("bin")
                .join("bash.exe"),
        );
    }
    if let Ok(program_files_x86) = std::env::var("ProgramFiles(x86)") {
        candidates.push(
            PathBuf::from(&program_files_x86)
                .join("Git")
                .join("bin")
                .join("bash.exe"),
        );
        candidates.push(
            PathBuf::from(program_files_x86)
                .join("Git")
                .join("usr")
                .join("bin")
                .join("bash.exe"),
        );
    }
    if let Ok(local_app_data) = std::env::var("LocalAppData") {
        candidates.push(
            PathBuf::from(&local_app_data)
                .join("Programs")
                .join("Git")
                .join("bin")
                .join("bash.exe"),
        );
        candidates.push(
            PathBuf::from(local_app_data)
                .join("Programs")
                .join("Git")
                .join("usr")
                .join("bin")
                .join("bash.exe"),
        );
    }

    candidates.into_iter().find(|candidate| candidate.exists())
}

#[cfg(windows)]
fn system_executable(relative_path: &str) -> Option<PathBuf> {
    std::env::var("SystemRoot")
        .ok()
        .map(PathBuf::from)
        .map(|root| root.join("System32").join(relative_path))
        .filter(|candidate| candidate.exists())
}

fn find_executable(name: &str) -> Option<PathBuf> {
    #[cfg(windows)]
    let mut candidate_names = vec![name.to_string()];
    #[cfg(not(windows))]
    let candidate_names = vec![name.to_string()];

    #[cfg(windows)]
    {
        let has_extension = Path::new(name).extension().is_some();
        if !has_extension {
            for extension in executable_extensions() {
                candidate_names.push(format!("{name}{extension}"));
            }
        }
    }

    std::env::var_os("PATH").and_then(|path| {
        for directory in std::env::split_paths(&path) {
            for candidate_name in &candidate_names {
                let candidate = directory.join(candidate_name);
                if candidate.exists() {
                    return Some(candidate);
                }
            }
        }
        None
    })
}

#[cfg(windows)]
fn executable_extensions() -> Vec<String> {
    std::env::var("PATHEXT")
        .ok()
        .map(|value| {
            value
                .split(';')
                .filter_map(|segment| {
                    let trimmed = segment.trim();
                    if trimmed.is_empty() {
                        None
                    } else {
                        Some(trimmed.to_ascii_lowercase())
                    }
                })
                .collect::<Vec<_>>()
        })
        .filter(|extensions| !extensions.is_empty())
        .unwrap_or_else(|| vec![".exe".to_string(), ".cmd".to_string(), ".bat".to_string()])
}

fn is_windows_cmd_shim(program: &str) -> bool {
    #[cfg(windows)]
    {
        Path::new(program)
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("cmd") || ext.eq_ignore_ascii_case("bat"))
    }
    #[cfg(not(windows))]
    {
        let _ = program;
        false
    }
}

#[cfg(test)]
mod tests {
    use super::{
        build_program_launch_with_settings, build_shell_command_with_settings, default_shell_args,
        load_shell_settings_from_path, save_shell_settings_to_path, shell_id_from_executable,
        ShellOption, ShellSettings,
    };
    use std::path::Path;
    use tempfile::tempdir;

    #[test]
    fn shell_settings_round_trip_through_json_file() {
        let temp_dir = tempdir().expect("temp dir");
        let path = temp_dir.path().join("shell_settings.json");
        let settings = ShellSettings {
            shell_id: "custom".to_string(),
            custom_executable: Some("C:/Program Files/PowerShell/7/pwsh.exe".to_string()),
            custom_args: Some("-NoProfile -Command".to_string()),
        };

        let saved = save_shell_settings_to_path(&path, &settings).expect("save settings");
        let loaded = load_shell_settings_from_path(&path).expect("load settings");

        assert_eq!(saved, settings);
        assert_eq!(loaded, settings);
    }

    #[test]
    fn custom_shell_uses_inferred_args_when_not_provided() {
        let settings = ShellSettings {
            shell_id: "custom".to_string(),
            custom_executable: Some(if cfg!(windows) {
                "powershell.exe".to_string()
            } else {
                "/bin/bash".to_string()
            }),
            custom_args: None,
        };

        let spec = build_shell_command_with_settings("echo hello", &settings, &[])
            .expect("build shell command");

        if cfg!(windows) {
            assert_eq!(spec.executable, "powershell.exe");
            assert_eq!(
                spec.args,
                vec![
                    "-NoProfile".to_string(),
                    "-Command".to_string(),
                    "echo hello".to_string()
                ]
            );
        } else {
            assert_eq!(spec.executable, "/bin/bash");
            assert_eq!(spec.args, vec!["-lc".to_string(), "echo hello".to_string()]);
        }
    }

    #[test]
    fn selected_shell_uses_its_registered_command_args() {
        let available = vec![ShellOption {
            id: "pwsh".to_string(),
            label: "PowerShell 7".to_string(),
            executable: "pwsh".to_string(),
            default_args: vec!["-NoProfile".to_string(), "-Command".to_string()],
        }];

        let spec = build_shell_command_with_settings(
            "$PSVersionTable.PSVersion",
            &ShellSettings {
                shell_id: "pwsh".to_string(),
                custom_executable: None,
                custom_args: None,
            },
            &available,
        )
        .expect("build shell command");

        assert_eq!(spec.executable, "pwsh");
        assert_eq!(
            spec.args,
            vec![
                "-NoProfile".to_string(),
                "-Command".to_string(),
                "$PSVersionTable.PSVersion".to_string()
            ]
        );
    }

    #[test]
    fn program_launch_wraps_provider_in_powershell_command() {
        let available = vec![ShellOption {
            id: "pwsh".to_string(),
            label: "PowerShell 7".to_string(),
            executable: "pwsh".to_string(),
            default_args: vec!["-NoProfile".to_string(), "-Command".to_string()],
        }];
        let args = vec!["--resume".to_string(), "session-123".to_string()];

        let spec = build_program_launch_with_settings(
            "codex.cmd",
            &args,
            &ShellSettings {
                shell_id: "pwsh".to_string(),
                custom_executable: None,
                custom_args: None,
            },
            &available,
        )
        .expect("build program launch");

        assert_eq!(spec.executable, "pwsh");
        assert_eq!(spec.args[0..2], ["-NoProfile", "-Command"]);
        if cfg!(windows) {
            assert!(spec.args[2].contains("$env:ComSpec"));
            assert!(spec.args[2].contains("codex.cmd"));
            assert!(spec.args[2].contains("--resume"));
        } else {
            assert!(spec.args[2].contains("codex.cmd"));
        }
    }

    #[test]
    fn auto_shell_falls_back_to_first_available_option() {
        let available = vec![ShellOption {
            id: "custom-sh".to_string(),
            label: "Custom Sh".to_string(),
            executable: "custom-sh".to_string(),
            default_args: vec!["-c".to_string()],
        }];

        let spec = build_shell_command_with_settings(
            "echo fallback",
            &ShellSettings::default(),
            &available,
        )
        .expect("build shell command");

        assert_eq!(spec.executable, "custom-sh");
        assert_eq!(
            spec.args,
            vec!["-c".to_string(), "echo fallback".to_string()]
        );
    }

    #[test]
    fn shell_id_normalizes_known_executable_names() {
        assert_eq!(
            shell_id_from_executable(Path::new("C:/Windows/System32/cmd.exe")),
            "cmd"
        );
        assert_eq!(shell_id_from_executable(Path::new("/bin/bash")), "bash");
        assert_eq!(
            shell_id_from_executable(Path::new("C:/Program Files/PowerShell/7/pwsh.exe")),
            "pwsh"
        );
    }

    #[test]
    fn default_shell_args_cover_standard_shell_families() {
        assert_eq!(default_shell_args("cmd"), vec!["/C".to_string()]);
        assert_eq!(
            default_shell_args("pwsh"),
            vec!["-NoProfile".to_string(), "-Command".to_string()]
        );
        assert_eq!(default_shell_args("bash"), vec!["-lc".to_string()]);
        assert_eq!(default_shell_args("sh"), vec!["-c".to_string()]);
    }

    #[test]
    fn windows_cmd_shims_are_wrapped_for_posix_hosts() {
        let available = vec![ShellOption {
            id: "git-bash".to_string(),
            label: "Git Bash".to_string(),
            executable: "C:/Program Files/Git/bin/bash.exe".to_string(),
            default_args: vec!["-lc".to_string()],
        }];
        let args = vec!["--verbose".to_string()];

        let spec = build_program_launch_with_settings(
            "claude.cmd",
            &args,
            &ShellSettings {
                shell_id: "git-bash".to_string(),
                custom_executable: None,
                custom_args: None,
            },
            &available,
        )
        .expect("build program launch");

        assert_eq!(spec.executable, "C:/Program Files/Git/bin/bash.exe");
        assert_eq!(spec.args[0], "-lc");
        if cfg!(windows) {
            assert_eq!(spec.args[1], "exec \"$@\"");
            assert_eq!(spec.args[2], "wardian-shell");
            assert_eq!(spec.args[3], "cmd.exe");
            assert_eq!(spec.args[4], "/d");
            assert_eq!(spec.args[5], "/c");
            assert_eq!(spec.args[6], "claude.cmd");
            assert_eq!(spec.args[7], "--verbose");
        } else {
            assert!(spec.args[1].starts_with("exec 'claude.cmd'"));
        }
    }

    #[test]
    fn git_bash_preserves_json_arguments_without_flattening() {
        let available = vec![ShellOption {
            id: "git-bash".to_string(),
            label: "Git Bash".to_string(),
            executable: "C:/Program Files/Git/bin/bash.exe".to_string(),
            default_args: vec!["-lc".to_string()],
        }];
        let args = vec![
            "--settings".to_string(),
            "{\"hooks\":{\"PermissionRequest\":[{\"matcher\":\"*\"}]}}".to_string(),
        ];

        let spec = build_program_launch_with_settings(
            "claude.exe",
            &args,
            &ShellSettings {
                shell_id: "git-bash".to_string(),
                custom_executable: None,
                custom_args: None,
            },
            &available,
        )
        .expect("build program launch");

        assert_eq!(spec.executable, "C:/Program Files/Git/bin/bash.exe");
        assert_eq!(spec.args[0], "-lc");
        if cfg!(windows) {
            assert_eq!(spec.args[1], "exec \"$@\"");
            assert_eq!(spec.args[2], "wardian-shell");
            assert_eq!(spec.args[3], "claude.exe");
            assert_eq!(spec.args[4], "--settings");
            assert_eq!(
                spec.args[5],
                "{\"hooks\":{\"PermissionRequest\":[{\"matcher\":\"*\"}]}}"
            );
        }
    }
    #[test]
    fn wsl_wraps_windows_programs_through_cmd() {
        let available = vec![ShellOption {
            id: "wsl".to_string(),
            label: "WSL".to_string(),
            executable: "C:/Windows/System32/wsl.exe".to_string(),
            default_args: vec!["-e".to_string(), "bash".to_string(), "-lc".to_string()],
        }];
        let args = vec!["--version".to_string()];

        let spec = build_program_launch_with_settings(
            "node",
            &args,
            &ShellSettings {
                shell_id: "wsl".to_string(),
                custom_executable: None,
                custom_args: None,
            },
            &available,
        )
        .expect("build program launch");

        assert_eq!(spec.executable, "C:/Windows/System32/wsl.exe");
        assert_eq!(spec.args[0..3], ["-e", "bash", "-lc"]);
        if cfg!(windows) {
            assert!(spec.args[3].starts_with("exec cmd.exe /c "));
            assert!(spec.args[3].contains("node"));
        }
    }
}

pub fn get_opencode_tui_path() -> Option<std::path::PathBuf> {
    dirs::config_dir().map(|p| p.join("opencode").join("tui.json"))
}

pub fn get_opencode_config_path() -> Option<std::path::PathBuf> {
    dirs::config_dir().map(|p| p.join("opencode").join("opencode.json"))
}

fn wardian_theme_to_opencode_theme(theme: &str) -> &str {
    match theme {
        "dark" | "light" | "system" => "system",
        other => other,
    }
}

fn upsert_json_theme(path: &std::path::Path, schema: &str, theme: &str) -> Result<(), String> {
    let mut config = if path.exists() {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|content| serde_json::from_str::<serde_json::Value>(&content).ok())
            .and_then(|value| value.as_object().cloned())
            .unwrap_or_default()
    } else {
        serde_json::Map::new()
    };

    config.insert("$schema".to_string(), serde_json::Value::String(schema.to_string()));
    config.insert(
        "theme".to_string(),
        serde_json::Value::String(theme.to_string()),
    );

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }

    let content = serde_json::to_string_pretty(&serde_json::Value::Object(config))
        .map_err(|e| e.to_string())?;
    std::fs::write(path, content).map_err(|e| e.to_string())?;

    Ok(())
}

pub fn save_opencode_theme(theme: &str) -> Result<(), String> {
    let normalized = wardian_theme_to_opencode_theme(theme);
    let tui_path = get_opencode_tui_path().ok_or("Could not find config directory")?;
    upsert_json_theme(&tui_path, "https://opencode.ai/tui.json", normalized)?;

    if let Some(config_path) = get_opencode_config_path() {
        upsert_json_theme(&config_path, "https://opencode.ai/config.json", normalized)?;
    }

    Ok(())
}

#[cfg(test)]
mod opencode_theme_tests {
    use super::{upsert_json_theme, wardian_theme_to_opencode_theme};

    #[test]
    fn wardian_theme_maps_to_opencode_system_theme() {
        assert_eq!(wardian_theme_to_opencode_theme("dark"), "system");
        assert_eq!(wardian_theme_to_opencode_theme("light"), "system");
        assert_eq!(wardian_theme_to_opencode_theme("system"), "system");
    }

    #[test]
    fn non_wardian_theme_passthrough_is_preserved() {
        assert_eq!(wardian_theme_to_opencode_theme("opencode"), "opencode");
    }

    #[test]
    fn upsert_json_theme_preserves_existing_config_keys() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("opencode.json");
        std::fs::write(
            &path,
            r#"{"$schema":"https://opencode.ai/config.json","provider":{"lmstudio":{"name":"LM Studio"}}}"#,
        )
        .expect("write initial config");

        upsert_json_theme(&path, "https://opencode.ai/config.json", "system")
            .expect("update theme");

        let parsed: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("read config"))
                .expect("parse config");
        assert_eq!(parsed.get("theme").and_then(|value| value.as_str()), Some("system"));
        assert_eq!(
            parsed["provider"]["lmstudio"]["name"].as_str(),
            Some("LM Studio")
        );
    }
}
