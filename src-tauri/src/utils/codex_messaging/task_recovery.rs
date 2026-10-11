//! Own one private static hook. User hook files and trust decisions are never adopted.
use super::*;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use wardian_core::agent_messaging::TASK_CONTEXT_OUTPUT_TOKENS;

const RECOVERY_RECORD: &str = ".wardian-task-recovery.json";

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RecoveryRegistration {
    pub version: u32,
    pub agent_id: String,
    pub executable: String,
    pub executable_sha256: String,
    pub cli_executable: String,
    pub cli_sha256: String,
    pub hook_sha256: String,
    pub hook_key: String,
    pub hook_hash: String,
    pub hook_command: String,
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn hook_command(executable: &str) -> Option<String> {
    if executable.contains(['\r', '\n', '\0']) {
        return None;
    }
    if cfg!(windows) {
        // Default cmd.exe expands percent substitutions even inside quotes.
        if executable.contains(['"', '%', '!', '^', '&', '|', '<', '>']) {
            return None;
        }
        Some(format!("\"{executable}\" mcp recovery-pointer"))
    } else {
        Some(format!(
            "'{}' mcp recovery-pointer",
            executable.replace('\'', "'\\''")
        ))
    }
}

/// The pinned provider hashes sorted JSON after TOML normalization. These are
/// the exact non-null normalized fields for this sole synchronous handler.
fn hook_document(command: &str) -> (serde_json::Value, String) {
    let handler = BTreeMap::from([
        ("additionalContextLimit", serde_json::json!(0)),
        ("async", serde_json::json!(false)),
        ("command", serde_json::json!(command)),
        ("timeout", serde_json::json!(5)),
        ("type", serde_json::json!("command")),
    ]);
    let identity = BTreeMap::from([
        ("event_name", serde_json::json!("session_start")),
        ("hooks", serde_json::json!([handler])),
        ("matcher", serde_json::json!("compact")),
    ]);
    let hash = format!(
        "sha256:{}",
        digest(&serde_json::to_vec(&identity).expect("static hook identity"))
    );
    (
        serde_json::json!({"hooks":{"SessionStart":[{"matcher":"compact","hooks":[handler]}]}}),
        hash,
    )
}

fn owns_hook(record: &RecoveryRegistration, text: &str) -> bool {
    let Some(command) = hook_command(&record.executable) else {
        return false;
    };
    let (document, hash) = hook_document(&command);
    record.hook_command == command
        && record.hook_hash == hash
        && record.hook_sha256 == digest(text.as_bytes())
        && serde_json::from_str::<serde_json::Value>(text)
            .ok()
            .as_ref()
            == Some(&document)
}

/// Prepare future owners under the existing managed-home gate. A collision,
/// deletion, disable, lower limit or user trust edit disables recovery only.
/// No generic hook merge or provider approval is performed.
pub(crate) fn ensure_task_recovery(
    wardian_home: &Path,
    agent_id: &str,
) -> Result<Registration, String> {
    let codex_home = super::super::codex_home::resolve_managed_home(wardian_home, agent_id)?;
    let config_path = codex_home.join("config.toml");
    let hook_path = codex_home.join("hooks.json");
    let record_path = codex_home.join(RECOVERY_RECORD);
    for path in [&config_path, &hook_path, &record_path] {
        reject_link(path)?;
    }
    let original = read_optional(&config_path)?.unwrap_or_default();
    let hook_original = read_optional(&hook_path)?;
    let record_original = read_optional(&record_path)?;
    let previous: Option<RecoveryRegistration> = record_original
        .as_deref()
        .map(serde_json::from_str)
        .transpose()
        .map_err(|e| e.to_string())?;
    let messaging: Ownership = serde_json::from_str(
        &read_optional(&codex_home.join(RECORD))?.ok_or("Managed MCP ownership is missing")?,
    )
    .map_err(|e| e.to_string())?;
    let mut config = original.parse::<DocumentMut>().map_err(|e| e.to_string())?;
    let Some(entry) = config
        .get("mcp_servers")
        .and_then(|servers| servers.get(SERVER))
    else {
        return Ok(Registration::Unavailable(
            "managed MCP registration is missing",
        ));
    };
    if messaging.version != 1 || messaging.agent_id != agent_id || !owns(entry, &messaging) {
        return Ok(Registration::Unavailable("managed MCP ownership changed"));
    }
    if entry.get("enabled").and_then(Item::as_bool) == Some(false)
        || config
            .get("features")
            .and_then(|features| features.get("codex_hooks"))
            .and_then(Item::as_bool)
            == Some(false)
        || entry
            .get("disabled_tools")
            .and_then(Item::as_array)
            .is_some_and(|tools| {
                tools
                    .iter()
                    .any(|tool| tool.as_str() == Some("read_task_context"))
            })
        || entry
            .get("enabled_tools")
            .and_then(Item::as_array)
            .is_some_and(|tools| {
                !tools
                    .iter()
                    .any(|tool| tool.as_str() == Some("read_task_context"))
            })
    {
        return Ok(Registration::Unavailable("user disabled recovery"));
    }
    let tools = entry.get("tools");
    let tool = tools.and_then(|tools| tools.get("read_task_context"));
    if tools.is_some_and(|tools| tools.as_table_like().is_none())
        || tool.is_some_and(|tool| tool.as_table_like().is_none())
    {
        return Ok(Registration::Unavailable(
            "custom recovery tool configuration is unsupported",
        ));
    }
    let limit = tool.and_then(|tool| tool.get("output_token_limit"));
    if limit.is_some_and(|limit| {
        limit
            .as_integer()
            .is_none_or(|limit| limit < TASK_CONTEXT_OUTPUT_TOKENS as i64)
    }) || tool
        .and_then(|tool| tool.get("enabled"))
        .and_then(Item::as_bool)
        == Some(false)
    {
        return Ok(Registration::Unavailable(
            "user recovery output limit or disable preserved",
        ));
    }
    let Some(command) = hook_command(&messaging.command) else {
        return Ok(Registration::Unavailable(
            "hook executable cannot be represented safely",
        ));
    };
    let (document, hook_hash) = hook_document(&command);
    let hook_text = serde_json::to_string_pretty(&document).map_err(|e| e.to_string())? + "\n";
    let executable_sha256 = digest(&std::fs::read(&messaging.command).map_err(|e| e.to_string())?);
    let cli_executable = path_text(
        &Path::new(&messaging.command)
            .with_file_name(super::super::cli_install::bundled_cli_file_name()),
    )?;
    let cli_sha256 = digest(&std::fs::read(&cli_executable).map_err(|e| e.to_string())?);
    let desired = RecoveryRegistration {
        version: 1,
        agent_id: agent_id.into(),
        executable: messaging.command,
        executable_sha256,
        cli_executable,
        cli_sha256,
        hook_sha256: digest(hook_text.as_bytes()),
        hook_key: format!("{}:session_start:0:0", hook_path.display()),
        hook_hash,
        hook_command: command,
    };
    match (&previous, &hook_original) {
        (None, None) => {}
        (Some(previous), Some(hook))
            if previous.version == 1
                && previous.agent_id == agent_id
                && previous.executable == desired.executable
                && owns_hook(previous, hook) => {}
        _ => {
            return Ok(Registration::Unavailable(
                "user hook file, edit or removal preserved",
            ))
        }
    }
    for key in ["hooks", "state"] {
        let item = if key == "hooks" {
            config.get(key)
        } else {
            config.get("hooks").and_then(|hooks| hooks.get(key))
        };
        if item.is_some_and(|item| item.as_table_like().is_none()) {
            return Ok(Registration::Unavailable("custom hook state preserved"));
        }
    }
    let state = config
        .get("hooks")
        .and_then(|hooks| hooks.get("state"))
        .and_then(|state| state.get(&desired.hook_key));
    match (state, previous.as_ref()) {
        (None, None) => {}
        (Some(state), Some(previous))
            if state.as_table_like().is_some()
                && state.get("enabled").and_then(Item::as_bool) != Some(false)
                && state.get("trusted_hash").and_then(Item::as_str)
                    == Some(previous.hook_hash.as_str()) => {}
        _ => {
            return Ok(Registration::Unavailable(
                "user hook trust edit, disable or removal preserved",
            ))
        }
    }
    if limit.is_none() {
        config["mcp_servers"][SERVER]["tools"]["read_task_context"]["output_token_limit"] =
            value(TASK_CONTEXT_OUTPUT_TOKENS as i64);
    }
    config["hooks"]["state"][&desired.hook_key]["trusted_hash"] = value(&desired.hook_hash);
    if read_optional(&config_path)?.unwrap_or_default() != original
        || read_optional(&hook_path)? != hook_original
        || read_optional(&record_path)? != record_original
    {
        return Err("Recovery configuration changed during preparation".into());
    }
    let rendered = config.to_string();
    let rendered = if original.contains("\r\n") {
        rendered.replace("\r\n", "\n").replace('\n', "\r\n")
    } else {
        rendered
    };
    let record = serde_json::to_vec(&desired).map_err(|e| e.to_string())?;
    if rendered == original
        && hook_original.as_deref() == Some(hook_text.as_str())
        && record_original.as_deref() == std::str::from_utf8(&record).ok()
    {
        return Ok(Registration::Unchanged);
    }
    // An interrupted publication leaves an unsupported collision on retry.
    // Never adopt an unrecorded handler or broaden its trust to recover.
    atomic_replace(&hook_path, hook_text.as_bytes())?;
    atomic_replace(&config_path, rendered.as_bytes())?;
    atomic_replace(&record_path, &record)?;
    Ok(Registration::Updated)
}

/// Read back exact private ownership for one future native generation.
pub(crate) fn recovery_registration(
    codex_home: &Path,
    agent_id: &str,
) -> Result<RecoveryRegistration, String> {
    let record_path = codex_home.join(RECOVERY_RECORD);
    let hook_path = codex_home.join("hooks.json");
    for path in [&record_path, &hook_path] {
        reject_link(path)?;
    }
    let record: RecoveryRegistration =
        serde_json::from_str(&read_optional(&record_path)?.ok_or("Recovery ownership is missing")?)
            .map_err(|e| e.to_string())?;
    if record.version != 1
        || record.agent_id != agent_id
        || !owns_hook(
            &record,
            &std::fs::read_to_string(&hook_path).map_err(|e| e.to_string())?,
        )
        || digest(&std::fs::read(&record.executable).map_err(|e| e.to_string())?)
            != record.executable_sha256
        || digest(&std::fs::read(&record.cli_executable).map_err(|e| e.to_string())?)
            != record.cli_sha256
    {
        return Err("Recovery ownership or executable changed".into());
    }
    Ok(record)
}

#[cfg(test)]
mod tests;
