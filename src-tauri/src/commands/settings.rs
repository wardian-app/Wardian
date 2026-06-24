use crate::utils::OnboardingHintsState;
use crate::utils::{AppSettingsDocument, ShellOption, ShellSettings, ShellSettingsDocument};
use serde::Serialize;
use tauri::State;
use wardian_core::conversations::ConversationLoggingSetting;
use wardian_core::models::AgentSessionPersistence;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UpdateEligibility {
    pub enabled: bool,
    pub channel: Option<String>,
    pub reason: Option<String>,
    pub windows_handoff: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WindowsInstallRegistryIdentity {
    publisher: String,
    product_name: String,
}

const UPDATE_DOWNLOAD_EVENT: &str = "wardian-update-download";
const MAX_WINDOWS_UPDATE_INSTALLER_BYTES: u64 = 512 * 1024 * 1024;

const WINDOWS_INSTALL_RECORD_MISSING_REASON: &str = "Wardian could not verify its Windows install location. Reinstall the latest Wardian installer manually before using in-app updates.";
const WINDOWS_INSTALL_MISMATCH_REASON: &str = "Wardian is not running from the Windows install location registered for updates. Reinstall the latest Wardian installer manually so shortcuts and updater state point to the same copy.";

fn normalize_windows_install_path(path: &str) -> String {
    let normalized = path
        .trim()
        .trim_matches('"')
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_string();

    normalized
        .strip_prefix("//?/UNC/")
        .map(|path| format!("//{path}"))
        .or_else(|| normalized.strip_prefix("//?/").map(str::to_string))
        .unwrap_or(normalized)
        .to_ascii_lowercase()
}

fn current_exe_install_dir(current_exe: &str) -> Option<String> {
    let normalized = normalize_windows_install_path(current_exe);
    normalized
        .rsplit_once('/')
        .map(|(parent, _)| parent.to_string())
        .filter(|parent| !parent.is_empty())
}

fn resolve_windows_install_mismatch_reason(
    current_exe: &str,
    registered_install_dirs: &[String],
) -> Option<String> {
    let current_dir = current_exe_install_dir(current_exe)?;
    let registered_dirs = registered_install_dirs
        .iter()
        .map(|path| normalize_windows_install_path(path))
        .filter(|path| !path.is_empty())
        .collect::<Vec<_>>();

    if registered_dirs.is_empty() {
        return Some(WINDOWS_INSTALL_RECORD_MISSING_REASON.to_string());
    }

    if registered_dirs.iter().any(|path| path == &current_dir) {
        None
    } else {
        Some(WINDOWS_INSTALL_MISMATCH_REASON.to_string())
    }
}

fn windows_install_registry_paths(identity: &WindowsInstallRegistryIdentity) -> [String; 3] {
    [
        format!(
            "Software\\{}\\{}",
            identity.publisher, identity.product_name
        ),
        format!(
            "Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\{}",
            identity.product_name
        ),
        format!(
            "Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\{}",
            identity.product_name
        ),
    ]
}

fn current_windows_install_registry_identity() -> WindowsInstallRegistryIdentity {
    WindowsInstallRegistryIdentity {
        publisher: option_env!("WARDIAN_UPDATE_REGISTRY_PUBLISHER")
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("wardian")
            .to_string(),
        product_name: option_env!("WARDIAN_UPDATE_REGISTRY_PRODUCT_NAME")
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("Wardian")
            .to_string(),
    }
}

#[cfg(windows)]
fn read_registry_string(root: winreg::HKEY, subkey: &str, value: &str) -> Option<String> {
    use winreg::RegKey;

    RegKey::predef(root)
        .open_subkey(subkey)
        .ok()?
        .get_value::<String, _>(value)
        .ok()
}

#[cfg(windows)]
fn windows_registered_install_dirs() -> Vec<String> {
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};

    let [publisher_install_key, user_uninstall_key, machine_uninstall_key] =
        windows_install_registry_paths(&current_windows_install_registry_identity());

    [
        read_registry_string(HKEY_CURRENT_USER, &publisher_install_key, ""),
        read_registry_string(HKEY_CURRENT_USER, &user_uninstall_key, "InstallLocation"),
        read_registry_string(
            HKEY_LOCAL_MACHINE,
            &machine_uninstall_key,
            "InstallLocation",
        ),
    ]
    .into_iter()
    .flatten()
    .collect()
}

#[cfg(windows)]
fn current_install_mismatch_reason() -> Option<String> {
    let current_exe = std::env::current_exe().ok()?;
    resolve_windows_install_mismatch_reason(
        &current_exe.to_string_lossy(),
        &windows_registered_install_dirs(),
    )
}

#[cfg(not(windows))]
fn current_install_mismatch_reason() -> Option<String> {
    None
}

pub fn resolve_update_eligibility(
    debug_build: bool,
    update_channel: Option<&str>,
    install_mismatch_reason: Option<&str>,
) -> UpdateEligibility {
    let normalized_channel = update_channel
        .map(str::trim)
        .filter(|channel| !channel.is_empty())
        .map(str::to_string);

    if debug_build {
        return UpdateEligibility {
            enabled: false,
            channel: normalized_channel,
            reason: Some("Updates are unavailable in development builds.".to_string()),
            windows_handoff: cfg!(windows),
        };
    }

    if normalized_channel.as_deref() != Some("stable") {
        return UpdateEligibility {
            enabled: false,
            channel: normalized_channel,
            reason: Some(
                "Updates are only available in official installed release builds.".to_string(),
            ),
            windows_handoff: cfg!(windows),
        };
    }

    if let Some(reason) = install_mismatch_reason {
        return UpdateEligibility {
            enabled: false,
            channel: normalized_channel,
            reason: Some(reason.to_string()),
            windows_handoff: cfg!(windows),
        };
    }

    UpdateEligibility {
        enabled: true,
        channel: normalized_channel,
        reason: None,
        windows_handoff: cfg!(windows),
    }
}

fn powershell_single_quoted(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn windows_update_installer_args() -> Vec<&'static str> {
    vec!["/P", "/R", "/UPDATE", "/ARGS"]
}

fn windows_update_handoff_script(parent_pid: u32, installer_path: &std::path::Path) -> String {
    let installer = powershell_single_quoted(&installer_path.display().to_string());
    let installer_args = windows_update_installer_args()
        .into_iter()
        .map(powershell_single_quoted)
        .collect::<Vec<_>>()
        .join(", ");

    format!(
        "$ErrorActionPreference = 'SilentlyContinue';\n\
         $parentExited = $false;\n\
         for ($attempt = 0; $attempt -lt 120; $attempt++) {{\n\
           if (-not (Get-Process -Id {parent_pid} -ErrorAction SilentlyContinue)) {{\n\
             $parentExited = $true;\n\
             break;\n\
           }}\n\
           Start-Sleep -Milliseconds 500;\n\
         }}\n\
         if (-not $parentExited) {{ exit 1; }}\n\
         Start-Sleep -Milliseconds 1500;\n\
         $installer = {installer};\n\
         $installerArgs = @({installer_args});\n\
         Start-Process -FilePath $installer -ArgumentList $installerArgs -WindowStyle Normal;\n",
    )
}

fn sanitized_update_version(version: &str) -> String {
    let sanitized = version
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();

    if sanitized.is_empty() {
        "unknown".to_string()
    } else {
        sanitized
    }
}

fn windows_update_installer_path(version: &str) -> std::path::PathBuf {
    std::env::temp_dir().join("wardian-updater").join(format!(
        "Wardian-{}-setup.exe",
        sanitized_update_version(version)
    ))
}

fn ensure_expected_update_version(
    expected_version: &str,
    actual_version: &str,
) -> Result<(), String> {
    let expected_version = expected_version.trim();
    let actual_version = actual_version.trim();

    if expected_version == actual_version {
        Ok(())
    } else {
        Err(format!(
            "Available update changed from {expected_version} to {actual_version}. Check for updates again."
        ))
    }
}

fn ensure_windows_update_installer_size(byte_len: u64) -> Result<(), String> {
    if byte_len <= MAX_WINDOWS_UPDATE_INSTALLER_BYTES {
        Ok(())
    } else {
        Err(format!(
            "Downloaded update installer is larger than the {} byte safety limit.",
            MAX_WINDOWS_UPDATE_INSTALLER_BYTES
        ))
    }
}

fn write_windows_update_installer(
    installer_path: &std::path::Path,
    bytes: &[u8],
) -> Result<(), String> {
    let parent = installer_path
        .parent()
        .ok_or_else(|| "Could not resolve update installer directory.".to_string())?;
    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    std::fs::write(installer_path, bytes).map_err(|error| error.to_string())
}

#[cfg(windows)]
fn spawn_windows_update_handoff(
    parent_pid: u32,
    installer_path: &std::path::Path,
) -> Result<(), String> {
    use std::os::windows::process::CommandExt;

    let script = windows_update_handoff_script(parent_pid, installer_path);
    std::process::Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-WindowStyle",
            "Hidden",
            "-Command",
            &script,
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .creation_flags(windows_update_handoff_creation_flags())
        .spawn()
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn windows_update_handoff_creation_flags() -> u32 {
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;

    CREATE_NEW_PROCESS_GROUP
        | crate::utils::process::windows_silent_process_creation_flags()
        | CREATE_BREAKAWAY_FROM_JOB
}

#[cfg(test)]
mod update_eligibility_tests {
    use super::{
        ensure_expected_update_version, ensure_windows_update_installer_size,
        normalize_windows_install_path, powershell_single_quoted, resolve_update_eligibility,
        resolve_windows_install_mismatch_reason, windows_install_registry_paths,
        windows_update_handoff_creation_flags, windows_update_handoff_script,
        windows_update_installer_args, WindowsInstallRegistryIdentity,
        MAX_WINDOWS_UPDATE_INSTALLER_BYTES,
    };

    #[test]
    fn update_eligibility_disables_debug_builds() {
        let eligibility = resolve_update_eligibility(true, Some("stable"), None);

        assert!(!eligibility.enabled);
        assert_eq!(eligibility.channel.as_deref(), Some("stable"));
        assert_eq!(
            eligibility.reason.as_deref(),
            Some("Updates are unavailable in development builds.")
        );
    }

    #[test]
    fn update_eligibility_disables_unmarked_release_builds() {
        let eligibility = resolve_update_eligibility(false, None, None);

        assert!(!eligibility.enabled);
        assert_eq!(eligibility.channel, None);
        assert_eq!(
            eligibility.reason.as_deref(),
            Some("Updates are only available in official installed release builds.")
        );
    }

    #[test]
    fn update_eligibility_enables_stable_release_builds() {
        let eligibility = resolve_update_eligibility(false, Some("stable"), None);

        assert!(eligibility.enabled);
        assert_eq!(eligibility.channel.as_deref(), Some("stable"));
        assert_eq!(eligibility.reason, None);
    }

    #[test]
    fn update_eligibility_disables_stable_builds_with_install_mismatch() {
        let eligibility = resolve_update_eligibility(
            false,
            Some("stable"),
            Some("Wardian is not running from the registered install location."),
        );

        assert!(!eligibility.enabled);
        assert_eq!(eligibility.channel.as_deref(), Some("stable"));
        assert_eq!(
            eligibility.reason.as_deref(),
            Some("Wardian is not running from the registered install location.")
        );
    }

    #[test]
    fn install_mismatch_allows_registered_current_exe_directory() {
        let reason = resolve_windows_install_mismatch_reason(
            r"C:\Users\tester\AppData\Local\Wardian\Wardian.exe",
            &[r#""C:\Users\tester\AppData\Local\Wardian""#.to_string()],
        );

        assert_eq!(reason, None);
    }

    #[test]
    fn install_mismatch_allows_extended_length_current_exe_directory() {
        let reason = resolve_windows_install_mismatch_reason(
            r"\\?\C:\Users\tester\AppData\Local\Wardian\Wardian.exe",
            &[r"C:\Users\tester\AppData\Local\Wardian".to_string()],
        );

        assert_eq!(reason, None);
    }

    #[test]
    fn install_mismatch_allows_extended_length_unc_current_exe_directory() {
        let normalized =
            normalize_windows_install_path(r"\\?\UNC\server\share\Wardian\Wardian.exe");

        assert_eq!(normalized, "//server/share/wardian/wardian.exe");
    }

    #[test]
    fn install_mismatch_rejects_unregistered_current_exe_directory() {
        let reason = resolve_windows_install_mismatch_reason(
            r"C:\Users\tester\Downloads\Wardian\Wardian.exe",
            &[r"C:\Users\tester\AppData\Local\Wardian".to_string()],
        );

        assert_eq!(
            reason.as_deref(),
            Some("Wardian is not running from the Windows install location registered for updates. Reinstall the latest Wardian installer manually so shortcuts and updater state point to the same copy.")
        );
    }

    #[test]
    fn install_mismatch_rejects_missing_install_record() {
        let reason = resolve_windows_install_mismatch_reason(
            r"C:\Users\tester\AppData\Local\Wardian\Wardian.exe",
            &[],
        );

        assert_eq!(
            reason.as_deref(),
            Some("Wardian could not verify its Windows install location. Reinstall the latest Wardian installer manually before using in-app updates.")
        );
    }

    #[test]
    fn windows_install_registry_paths_default_to_production_identity_shape() {
        let paths = windows_install_registry_paths(&WindowsInstallRegistryIdentity {
            publisher: "wardian".to_string(),
            product_name: "Wardian".to_string(),
        });

        assert_eq!(
            paths,
            [
                "Software\\wardian\\Wardian".to_string(),
                "Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\Wardian".to_string(),
                "Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\Wardian".to_string(),
            ]
        );
    }

    #[test]
    fn windows_install_registry_paths_support_disposable_update_test_identity() {
        let paths = windows_install_registry_paths(&WindowsInstallRegistryIdentity {
            publisher: "wardian-test".to_string(),
            product_name: "Wardian Updater Test".to_string(),
        });

        assert_eq!(
            paths,
            [
                "Software\\wardian-test\\Wardian Updater Test".to_string(),
                "Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\Wardian Updater Test"
                    .to_string(),
                "Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\Wardian Updater Test"
                    .to_string(),
            ]
        );
    }

    #[test]
    fn powershell_single_quoted_escapes_embedded_quotes() {
        assert_eq!(
            powershell_single_quoted(r"C:\Temp\Wardian Update's.exe"),
            r"'C:\Temp\Wardian Update''s.exe'"
        );
    }

    #[test]
    fn windows_update_handoff_script_waits_for_parent_before_starting_installer() {
        let script =
            windows_update_handoff_script(42, std::path::Path::new(r"C:\Temp\Wardian Update.exe"));

        assert!(script.contains("Get-Process -Id 42"));
        assert!(script.contains("Start-Sleep -Milliseconds 1500"));
        assert!(script.contains(r"$installer = 'C:\Temp\Wardian Update.exe'"));
        assert!(script.contains("Start-Process -FilePath $installer"));
    }

    #[test]
    fn windows_update_handoff_script_aborts_if_parent_is_still_running() {
        let script =
            windows_update_handoff_script(42, std::path::Path::new(r"C:\Temp\Wardian Update.exe"));

        assert!(script.contains("$parentExited = $false"));
        assert!(script.contains("Get-Process -Id 42"));
        assert!(script.contains("if (-not $parentExited) { exit 1; }"));
        assert!(script.find("exit 1").unwrap() < script.find("Start-Process").unwrap());
    }

    #[test]
    fn windows_update_handoff_breaks_away_from_app_supervisor_job() {
        const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;

        assert_eq!(
            windows_update_handoff_creation_flags() & CREATE_BREAKAWAY_FROM_JOB,
            CREATE_BREAKAWAY_FROM_JOB
        );
    }

    #[test]
    fn windows_update_installer_args_preserve_tauri_updater_contract() {
        assert_eq!(
            windows_update_installer_args(),
            vec!["/P", "/R", "/UPDATE", "/ARGS"]
        );
    }

    #[test]
    fn expected_update_version_rejects_changed_release_metadata() {
        let result = ensure_expected_update_version("0.3.7", "0.3.8");

        assert_eq!(
            result.unwrap_err(),
            "Available update changed from 0.3.7 to 0.3.8. Check for updates again."
        );
    }

    #[test]
    fn expected_update_version_matches_trimmed_release_metadata() {
        assert_eq!(ensure_expected_update_version(" 0.3.7 ", "0.3.7"), Ok(()));
    }

    #[test]
    fn oversized_windows_update_installer_is_rejected() {
        let result = ensure_windows_update_installer_size(MAX_WINDOWS_UPDATE_INSTALLER_BYTES + 1);

        assert_eq!(
            result.unwrap_err(),
            format!(
                "Downloaded update installer is larger than the {} byte safety limit.",
                MAX_WINDOWS_UPDATE_INSTALLER_BYTES
            )
        );
    }
}

#[tauri::command]
pub fn list_available_shells() -> Result<Vec<ShellOption>, String> {
    Ok(crate::utils::list_available_shells())
}

fn settings_folder_path_from_home(wardian_home: &std::path::Path) -> String {
    wardian_home.join("settings").to_string_lossy().into_owned()
}

fn ensure_settings_folder_path_from_home(wardian_home: &std::path::Path) -> Result<String, String> {
    let path_string = settings_folder_path_from_home(wardian_home);
    let path = std::path::PathBuf::from(&path_string);
    std::fs::create_dir_all(&path).map_err(|e| e.to_string())?;
    Ok(path_string)
}

#[tauri::command]
pub fn get_settings_folder_path() -> Result<String, String> {
    let wardian_home = crate::utils::get_wardian_home().ok_or("Could not find Wardian home")?;
    ensure_settings_folder_path_from_home(&wardian_home)
}

#[tauri::command]
pub fn get_update_eligibility() -> UpdateEligibility {
    let install_mismatch_reason =
        if !cfg!(debug_assertions) && option_env!("WARDIAN_UPDATE_CHANNEL") == Some("stable") {
            current_install_mismatch_reason()
        } else {
            None
        };
    resolve_update_eligibility(
        cfg!(debug_assertions),
        option_env!("WARDIAN_UPDATE_CHANNEL"),
        install_mismatch_reason.as_deref(),
    )
}

pub fn update_plugins_enabled_for_current_build() -> bool {
    get_update_eligibility().enabled
}

#[tauri::command]
pub async fn install_update_with_windows_handoff(
    app: tauri::AppHandle,
    expected_version: String,
) -> Result<(), String> {
    #[cfg(not(windows))]
    {
        let _ = (app, expected_version);
        Err("Windows update handoff is only available on Windows.".to_string())
    }

    #[cfg(windows)]
    {
        install_update_with_windows_handoff_impl(app, expected_version).await
    }
}

#[cfg(windows)]
async fn install_update_with_windows_handoff_impl(
    app: tauri::AppHandle,
    expected_version: String,
) -> Result<(), String> {
    use tauri::Emitter;
    use tauri_plugin_updater::UpdaterExt;

    let eligibility = get_update_eligibility();
    if !eligibility.enabled {
        return Err(eligibility
            .reason
            .unwrap_or_else(|| "Updates are unavailable for this build.".to_string()));
    }

    let update = app
        .updater()
        .map_err(|error| error.to_string())?
        .check()
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "No update is available.".to_string())?;
    ensure_expected_update_version(&expected_version, &update.version)?;

    let progress_app = app.clone();
    let finish_app = app.clone();
    let mut started = false;
    let bytes = update
        .download(
            move |chunk_length, content_length| {
                if !started {
                    let _ = progress_app.emit(
                        UPDATE_DOWNLOAD_EVENT,
                        serde_json::json!({
                            "event": "Started",
                            "data": { "contentLength": content_length },
                        }),
                    );
                    started = true;
                }

                let _ = progress_app.emit(
                    UPDATE_DOWNLOAD_EVENT,
                    serde_json::json!({
                        "event": "Progress",
                        "data": { "chunkLength": chunk_length },
                    }),
                );
            },
            move || {
                let _ = finish_app.emit(
                    UPDATE_DOWNLOAD_EVENT,
                    serde_json::json!({
                        "event": "Finished",
                    }),
                );
            },
        )
        .await
        .map_err(|error| error.to_string())?;
    // Tauri's verified Rust updater API returns the full installer as bytes.
    // Keep a hard ceiling before writing or launching an unexpectedly large asset.
    ensure_windows_update_installer_size(bytes.len() as u64)?;

    let installer_path = windows_update_installer_path(&update.version);
    write_windows_update_installer(&installer_path, &bytes)?;
    spawn_windows_update_handoff(std::process::id(), &installer_path)?;
    std::process::exit(0);
}

#[tauri::command]
pub fn load_shell_settings() -> Result<ShellSettingsDocument, String> {
    crate::utils::load_shell_settings_document()
}

#[tauri::command]
pub fn load_app_settings() -> Result<AppSettingsDocument, String> {
    crate::utils::load_app_settings_document()
}

#[tauri::command]
pub fn save_app_settings(settings: AppSettingsDocument) -> Result<AppSettingsDocument, String> {
    crate::utils::save_app_settings_document(&settings)
}

#[tauri::command]
pub async fn save_shell_settings(
    settings: ShellSettingsDocument,
    state: State<'_, crate::state::AppState>,
) -> Result<ShellSettingsDocument, String> {
    save_shell_settings_for_state(&state, settings).await
}

pub(crate) async fn save_shell_settings_for_state(
    state: &crate::state::AppState,
    settings: ShellSettingsDocument,
) -> Result<ShellSettingsDocument, String> {
    let previous_logging = crate::utils::load_shell_settings_document()
        .map(|document| document.settings.conversation_logging)
        .unwrap_or_else(|_| ShellSettings::default().conversation_logging);
    let saved = crate::utils::save_shell_settings_document(&settings)?;

    if previous_logging != ConversationLoggingSetting::Disabled
        && saved.settings.conversation_logging == ConversationLoggingSetting::Disabled
    {
        mark_global_conversation_logging_disabled(state).await;
    }

    Ok(saved)
}

async fn mark_global_conversation_logging_disabled(state: &crate::state::AppState) {
    let session_ids = {
        let agents = state.agents.lock().await;
        agents.keys().cloned().collect::<Vec<_>>()
    };

    for session_id in session_ids {
        let snapshot = match crate::commands::chat::agent_archive_capture_snapshot(
            state,
            &session_id,
        )
        .await
        {
            Ok(snapshot) => snapshot,
            Err(error) => {
                crate::manager::log_debug(&format!(
                    "[WARDIAN] conversation archive disabled cutoff snapshot failed for {session_id}: {error}"
                ));
                continue;
            }
        };
        if crate::state::conversation_archive::effective_conversation_logging(
            ConversationLoggingSetting::Disabled,
            snapshot.agent_conversation_logging,
        ) != ConversationLoggingSetting::Disabled
        {
            continue;
        }
        let context = crate::commands::chat::conversation_archive_context_from_snapshot(&snapshot);
        if let Err(error) = state
            .conversation_archive
            .discard_agent_with_context(context, &[])
        {
            crate::manager::log_debug(&format!(
                "[WARDIAN] conversation archive disabled cutoff failed for {session_id}: {error}"
            ));
        }
    }
}

#[tauri::command]
pub fn save_agent_session_persistence(
    persistence: AgentSessionPersistence,
) -> Result<ShellSettings, String> {
    crate::utils::save_agent_session_persistence(persistence)
}

#[tauri::command]
pub fn sync_provider_theme_settings(
    theme: String,
    state: State<'_, crate::state::AppState>,
) -> Result<(), String> {
    state.set_terminal_theme(&theme);
    crate::utils::sync_provider_theme_settings(&theme)
}

#[tauri::command]
pub fn load_onboarding_hints() -> Result<OnboardingHintsState, String> {
    crate::utils::load_onboarding_hints()
}

#[tauri::command]
pub fn dismiss_onboarding_hint(hint_id: String) -> Result<OnboardingHintsState, String> {
    crate::utils::dismiss_onboarding_hint(hint_id)
}

#[cfg(test)]
mod settings_path_tests {
    use super::{
        ensure_settings_folder_path_from_home, save_shell_settings_for_state,
        settings_folder_path_from_home,
    };
    use crate::state::{ActiveAgent, AgentWatchState, AppState};
    use crate::utils::{ShellSettings, ShellSettingsDocument, ShellSettingsOverrides};
    use std::sync::{Arc, Mutex};
    use wardian_core::conversations::{
        AgentConversationLoggingSetting, ConversationLoggingSetting,
    };
    use wardian_core::models::chat::{
        AgentChatEvent, AgentChatEventKind, AgentChatRole, AgentChatStatus,
    };
    use wardian_core::models::AgentConfig;

    #[test]
    fn settings_folder_path_points_under_wardian_home() {
        let home = std::path::Path::new("/tmp/wardian-home");

        assert_eq!(
            settings_folder_path_from_home(home).replace('\\', "/"),
            "/tmp/wardian-home/settings"
        );
    }

    #[test]
    fn settings_folder_path_can_be_created_before_opening() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let path = std::path::PathBuf::from(
            ensure_settings_folder_path_from_home(temp_dir.path()).expect("settings path"),
        );

        assert!(path.is_dir());
    }

    #[test]
    fn save_shell_settings_disabled_cuts_off_default_agent_provider_source() {
        let _guard = crate::utils::wardian_test_env_lock();
        let temp_dir = tempfile::tempdir().expect("temp dir");
        std::env::set_var("WARDIAN_HOME", temp_dir.path());
        let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");

        runtime.block_on(async {
            let state = AppState::new();
            state.agents.lock().await.insert(
                "agent-1".to_string(),
                test_agent(
                    "agent-1",
                    AgentConversationLoggingSetting::Default,
                    Some("provider-session-1"),
                ),
            );

            let settings = ShellSettings {
                conversation_logging: ConversationLoggingSetting::Disabled,
                ..Default::default()
            };
            let saved = save_shell_settings_for_state(
                &state,
                ShellSettingsDocument {
                    schema_version: 2,
                    settings,
                    overrides: ShellSettingsOverrides {
                        conversation_logging: Some(ConversationLoggingSetting::Disabled),
                        ..Default::default()
                    },
                },
            )
            .await
            .expect("save disabled settings");
            assert_eq!(
                saved.settings.conversation_logging,
                ConversationLoggingSetting::Disabled
            );

            let appended = state
                .conversation_archive
                .append_chat_events_with_context(
                    crate::state::conversation_archive::ConversationArchiveContext {
                        agent_id: "agent-1".to_string(),
                        agent_name: "Agent One".to_string(),
                        agent_class: "Coder".to_string(),
                        workspace: "<absolute-workspace-path>".to_string(),
                        provider: "codex".to_string(),
                        provider_session_ids: vec!["provider-session-1".to_string()],
                        provider_source_key: Some("codex:session:provider-session-1".to_string()),
                    },
                    &[provider_event("event-disabled-window")],
                )
                .expect("append after disabled cutoff");

            assert_eq!(appended, 0);
        });
    }

    fn test_agent(
        session_id: &str,
        conversation_logging: AgentConversationLoggingSetting,
        resume_session: Option<&str>,
    ) -> ActiveAgent {
        ActiveAgent {
            config: Arc::new(Mutex::new(AgentConfig {
                session_id: session_id.to_string(),
                session_name: "Agent One".to_string(),
                agent_class: "Coder".to_string(),
                provider: "codex".to_string(),
                folder: "<absolute-workspace-path>".to_string(),
                resume_session: resume_session.map(ToString::to_string),
                conversation_logging,
                ..Default::default()
            })),
            child_process: None,
            background_processes: Vec::new(),
            pty_master: None,
            stdin_tx: None,
            output_buffer: Arc::new(Mutex::new(String::new())),
            process_id: None,
            query_count: Arc::new(Mutex::new(0)),
            init_timestamp: Arc::new(Mutex::new(None)),
            current_status: Arc::new(Mutex::new("Idle".to_string())),
            last_status_at: Arc::new(Mutex::new(None)),
            watch_state: Arc::new(Mutex::new(AgentWatchState::new(
                session_id.to_string(),
                32,
                4096,
            ))),
            terminal_title: Arc::new(Mutex::new(String::new())),
            last_output_at: Arc::new(Mutex::new(None)),
            log_path: Arc::new(Mutex::new(None)),
            log_last_modified: Arc::new(Mutex::new(None)),
            #[cfg(windows)]
            job_object: None,
        }
    }

    fn provider_event(id: &str) -> AgentChatEvent {
        AgentChatEvent {
            id: id.to_string(),
            session_id: "agent-1".to_string(),
            provider: "codex".to_string(),
            kind: AgentChatEventKind::Message,
            role: Some(AgentChatRole::Assistant),
            text: Some("This happened while logging was disabled.".to_string()),
            title: None,
            status: Some(AgentChatStatus::Idle),
            turn_id: Some("turn-disabled".to_string()),
            source: Some("response_item".to_string()),
            command: None,
            exit_code: None,
            path: None,
            language: None,
            created_at: Some("2026-06-15T00:00:00.000Z".to_string()),
            sequence: Some(1),
            metadata: serde_json::json!({}),
        }
    }
}
