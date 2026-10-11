//! Read-only Claude continuation evidence. No transcript migration or fresh fallback.
use std::ffi::OsString;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use wardian_core::models::AgentConfig;

const MAX_PROJECTS: usize = 10_000;
const MAX_TRANSCRIPT_BYTES: u64 = 64 * 1024 * 1024;
const MAX_RECORD_BYTES: usize = 16 * 1024 * 1024;

/// Private continuation authority for a verified append-only transcript prefix.
/// Keep this in trusted lifecycle/broker state; never accept it from custom args.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaudeTranscriptBinding {
    pub config_root: PathBuf,
    pub canonical_path: PathBuf,
    pub session_id: String,
    pub confirmed_bytes: u64,
    pub prefix_sha256: String,
    pub native_identity: (u64, u64),
}

/// The same configuration root must be used by launch, telemetry and watchers.
/// An explicit `CLAUDE_CONFIG_DIR` never falls back when its history is missing.
/// Relative overrides resolve against the actual provider child directory.
pub fn config_root(launch_cwd: &Path) -> Result<PathBuf, String> {
    config_root_with_environment(
        std::env::var_os("CLAUDE_CONFIG_DIR"),
        dirs::home_dir(),
        launch_cwd,
    )
}

fn config_root_with_environment(
    configured: Option<OsString>,
    user_home: Option<PathBuf>,
    launch_cwd: &Path,
) -> Result<PathBuf, String> {
    let root = configured
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| user_home.map(|home| home.join(".claude")))
        .ok_or("Claude configuration directory is unavailable")?;
    if root.is_absolute() {
        Ok(root)
    } else if launch_cwd.is_absolute() {
        Ok(launch_cwd.join(root))
    } else {
        Err("Claude relative configuration requires an absolute provider launch cwd".into())
    }
}

/// Validate exact identity and durable local history before owner disposal.
/// This does not launch a provider or qualify its destination tool behavior.
pub fn prepare_exact_continuation(
    config: &AgentConfig,
    launch_cwd: &Path,
) -> Result<ClaudeTranscriptBinding, String> {
    prepare_exact_continuation_at_root(config, &config_root(launch_cwd)?)
}

/// Validate a source using the root captured for this provider generation. The
/// caller must use this same root in the child environment and its watchers;
/// this API deliberately does not read or modify global environment variables.
pub fn prepare_exact_continuation_at_root(
    config: &AgentConfig,
    effective_root: &Path,
) -> Result<ClaudeTranscriptBinding, String> {
    validate_selectors(config)?;
    let id = exact_id(config)?;
    require_absolute_root(effective_root)?;
    prepare_in(effective_root, id, None)
}

/// Revalidate a persisted receipt against an explicitly selected generation
/// root. Root changes, replacement and prefix mutation cannot create authority.
pub fn revalidate_exact_continuation_at_root(
    config: &AgentConfig,
    effective_root: &Path,
    binding: &ClaudeTranscriptBinding,
) -> Result<ClaudeTranscriptBinding, String> {
    validate_selectors(config)?;
    let id = exact_id(config)?;
    require_absolute_root(effective_root)?;
    validate_binding_shape(binding)?;
    if id != binding.session_id {
        return Err("Claude exact Resume identity changed after preparation".into());
    }
    prepare_in(effective_root, id, Some(binding))
}

/// Workspace rotation cannot preserve an inherited relative configuration root
/// without explicitly pinning it in the child environment. Shared preflight
/// calls this before stopping the source owner. Normal Resume may still use a
/// relative root because it does not rotate the workspace projection.
pub fn validate_workspace_handover_root() -> Result<(), String> {
    validate_workspace_handover_root_with_environment(std::env::var_os("CLAUDE_CONFIG_DIR"))
}

fn validate_workspace_handover_root_with_environment(
    configured: Option<OsString>,
) -> Result<(), String> {
    if configured
        .filter(|value| !value.is_empty())
        .is_some_and(|value| !Path::new(&value).is_absolute())
    {
        return Err("Claude workspace handover requires an absolute configuration root; relative CLAUDE_CONFIG_DIR could rebind its source history".into());
    }
    Ok(())
}

fn require_absolute_root(root: &Path) -> Result<(), String> {
    if !root.is_absolute() {
        return Err("Claude exact Resume requires an absolute effective configuration root".into());
    }
    Ok(())
}

fn validate_binding_shape(binding: &ClaudeTranscriptBinding) -> Result<(), String> {
    if !binding.config_root.is_absolute()
        || !binding.canonical_path.is_absolute()
        || binding.session_id.trim() != binding.session_id
        || uuid::Uuid::parse_str(&binding.session_id).is_err()
        || binding.confirmed_bytes == 0
        || binding.confirmed_bytes > MAX_TRANSCRIPT_BYTES
        || binding.prefix_sha256.len() != 64
        || !binding
            .prefix_sha256
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err("Claude exact Resume transcript receipt is malformed".into());
    }
    Ok(())
}

fn exact_id(config: &AgentConfig) -> Result<&str, String> {
    config.validate_exact_resume_identity()?;
    if config.provider != "claude" {
        return Err("Claude exact Resume received another provider identity".into());
    }
    let id = config
        .resume_session
        .as_deref()
        .ok_or("Claude exact Resume requires a source session identity")?;
    if id.trim() != id || uuid::Uuid::parse_str(id).is_err() {
        return Err("Claude exact Resume requires a provider UUID".into());
    }
    Ok(id)
}

fn validate_selectors(config: &AgentConfig) -> Result<(), String> {
    let arguments = shlex::split(config.custom_args.as_deref().unwrap_or_default())
        .ok_or("Claude exact Resume custom arguments are malformed")?;
    for argument in arguments {
        let flag = argument.split('=').next().unwrap_or_default();
        let selector = matches!(
            flag,
            "--" | "--resume"
                | "--continue"
                | "--session-id"
                | "--fork-session"
                | "--resume-session-at"
                | "--rewind-files"
                | "--from-pr"
                | "--worktree"
                | "--no-session-persistence"
                | "--remote"
                | "--remote-session-id"
                | "--teleport"
                | "--cwd"
        ) || (!argument.starts_with("--")
            && ["-r", "-c", "-w"]
                .iter()
                .any(|alias| argument.starts_with(alias)));
        if selector {
            return Err("Claude exact Resume custom arguments conflict with continuation identity or workspace".into());
        }
    }
    Ok(())
}

fn prepare_in(
    root: &Path,
    id: &str,
    previous: Option<&ClaudeTranscriptBinding>,
) -> Result<ClaudeTranscriptBinding, String> {
    let canonical_root = std::fs::canonicalize(root)
        .map_err(|_| "Claude exact Resume configuration directory is unavailable")?;
    if previous.is_some_and(|binding| binding.config_root != canonical_root) {
        return Err("Claude exact Resume configuration root changed after preparation".into());
    }
    let projects = std::fs::read_dir(canonical_root.join("projects"))
        .map_err(|_| "Claude exact Resume history directory is unavailable")?;
    let name = format!("{id}.jsonl");
    let mut found = None;
    for (index, entry) in projects.enumerate() {
        if index >= MAX_PROJECTS {
            return Err("Claude exact Resume project lookup exceeded its safety bound".into());
        }
        let entry = entry.map_err(|_| "Claude exact Resume project lookup failed")?;
        if !entry
            .file_type()
            .map_err(|_| "Claude exact Resume project metadata is unavailable")?
            .is_dir()
        {
            continue;
        }
        let candidate = entry.path().join(&name);
        match std::fs::metadata(&candidate) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return Err("Claude exact Resume transcript metadata is unavailable".into()),
            Ok(metadata) if !metadata.is_file() => {
                return Err("Claude exact Resume transcript is not a regular file".into())
            }
            Ok(_) => {}
        }
        if found.is_some() {
            return Err("Claude exact Resume history is ambiguous across projects".into());
        }
        found = Some(inspect(&canonical_root, &candidate, id, previous)?);
    }
    found.ok_or_else(|| {
        "Claude exact Resume transcript is missing; fresh fallback is forbidden".into()
    })
}

fn inspect(
    root: &Path,
    candidate: &Path,
    id: &str,
    previous: Option<&ClaudeTranscriptBinding>,
) -> Result<ClaudeTranscriptBinding, String> {
    let canonical_path = std::fs::canonicalize(candidate)
        .map_err(|_| "Claude exact Resume transcript path is unavailable")?;
    let mut file =
        File::open(&canonical_path).map_err(|_| "Claude exact Resume transcript is unreadable")?;
    let identity = native_identity(&file)
        .map_err(|_| "Claude exact Resume transcript identity is unavailable")?;
    let length = file
        .metadata()
        .map_err(|_| "Claude exact Resume transcript metadata is unavailable")?
        .len();
    if length > MAX_TRANSCRIPT_BYTES {
        return Err("Claude exact Resume history exceeded its inspection safety bound".into());
    }
    if let Some(binding) = previous {
        if canonical_path != binding.canonical_path || identity != binding.native_identity {
            return Err("Claude exact Resume transcript was replaced after preparation".into());
        }
        if length < binding.confirmed_bytes {
            return Err("Claude exact Resume transcript was truncated after preparation".into());
        }
    }
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(length)
        .read_to_end(&mut bytes)
        .map_err(|_| "Claude exact Resume transcript read failed")?;
    if bytes.len() as u64 != length {
        return Err("Claude exact Resume transcript changed during inspection".into());
    }
    if let Some(binding) = previous {
        if hash(&bytes[..binding.confirmed_bytes as usize]) != binding.prefix_sha256 {
            return Err("Claude exact Resume confirmed history changed after preparation".into());
        }
    }
    validate_records(&bytes, id)?;
    // A pathname can be replaced while the opened old file remains readable.
    let current = File::open(&canonical_path)
        .map_err(|_| "Claude exact Resume transcript disappeared during inspection")?;
    if native_identity(&current)
        .map_err(|_| "Claude exact Resume transcript identity is unavailable")?
        != identity
    {
        return Err("Claude exact Resume transcript was replaced during inspection".into());
    }
    Ok(ClaudeTranscriptBinding {
        config_root: root.to_path_buf(),
        canonical_path,
        session_id: id.into(),
        confirmed_bytes: length,
        prefix_sha256: hash(&bytes),
        native_identity: identity,
    })
}

fn validate_records(bytes: &[u8], id: &str) -> Result<(), String> {
    if !bytes.is_empty() && !bytes.ends_with(b"\n") {
        return Err("Claude exact Resume transcript has an incomplete final record".into());
    }
    let mut materialized = false;
    for line in bytes.split(|byte| *byte == b'\n') {
        if line.len() > MAX_RECORD_BYTES {
            return Err("Claude exact Resume transcript record exceeds its safety bound".into());
        }
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let record: serde_json::Value = serde_json::from_slice(line)
            .map_err(|_| "Claude exact Resume transcript contains invalid history")?;
        if matches!(
            record.get("type").and_then(|value| value.as_str()),
            Some("user" | "assistant")
        ) {
            if record.get("sessionId").and_then(|value| value.as_str()) != Some(id) {
                return Err(
                    "Claude exact Resume history has a conflicting or absent session identity"
                        .into(),
                );
            }
            materialized |= conversation_content(&record)?;
        }
    }
    if !materialized {
        return Err("Claude exact Resume has no materialized conversation history".into());
    }
    Ok(())
}

fn conversation_content(record: &serde_json::Value) -> Result<bool, String> {
    let message = record
        .get("message")
        .and_then(serde_json::Value::as_object)
        .ok_or("Claude exact Resume conversation message is malformed")?;
    if message.get("role").and_then(serde_json::Value::as_str)
        != record.get("type").and_then(serde_json::Value::as_str)
    {
        return Err("Claude exact Resume conversation role is malformed".into());
    }
    validate_content(
        message
            .get("content")
            .ok_or("Claude exact Resume conversation content is absent")?,
    )
}

fn validate_content(content: &serde_json::Value) -> Result<bool, String> {
    if let Some(text) = content.as_str() {
        return Ok(!text.trim().is_empty());
    }
    let blocks = content
        .as_array()
        .ok_or("Claude exact Resume conversation content is malformed")?;
    for block in blocks {
        validate_block(block)?;
    }
    Ok(!blocks.is_empty())
}

fn validate_block(block: &serde_json::Value) -> Result<(), String> {
    let object = block
        .as_object()
        .ok_or("Claude exact Resume conversation block is malformed")?;
    let string = |key| object.get(key).and_then(serde_json::Value::as_str);
    let nonempty = |key| string(key).is_some_and(|value| !value.is_empty());
    let valid = match string("type") {
        Some("text") => string("text").is_some(),
        // Empty thinking text is supported; the signed/redacted blocks remain
        // untouched and must never be removed or rewritten during continuation.
        Some("thinking") => {
            string("thinking").is_some()
                && object
                    .get("signature")
                    .is_none_or(serde_json::Value::is_string)
        }
        Some("redacted_thinking" | "encrypted_content") => string("data").is_some(),
        Some("tool_use" | "server_tool_use" | "mcp_tool_use") => {
            nonempty("id")
                && nonempty("name")
                && object
                    .get("input")
                    .is_some_and(serde_json::Value::is_object)
        }
        Some("tool_result" | "mcp_tool_result") => {
            let content_valid = match object.get("content") {
                Some(content) => {
                    validate_content(content)?;
                    true
                }
                None => true, // Empty tool results are permitted by the protocol.
            };
            nonempty("tool_use_id")
                && content_valid
                && object
                    .get("is_error")
                    .is_none_or(serde_json::Value::is_boolean)
        }
        Some("image" | "document") => object
            .get("source")
            .and_then(serde_json::Value::as_object)
            .is_some_and(
                |source| match source.get("type").and_then(serde_json::Value::as_str) {
                    Some("base64") => {
                        source
                            .get("media_type")
                            .is_some_and(serde_json::Value::is_string)
                            && source.get("data").is_some_and(serde_json::Value::is_string)
                    }
                    Some("text") => source.get("data").is_some_and(serde_json::Value::is_string),
                    Some("url") => source.get("url").is_some_and(serde_json::Value::is_string),
                    Some("file") => source
                        .get("file_id")
                        .is_some_and(serde_json::Value::is_string),
                    Some("content") => source
                        .get("content")
                        .is_some_and(|content| validate_content(content).is_ok()),
                    _ => false,
                },
            ),
        Some("search_result") => {
            nonempty("source")
                && string("title").is_some()
                && object
                    .get("content")
                    .is_some_and(|content| validate_content(content).is_ok())
        }
        Some(
            "web_search_tool_result"
            | "web_fetch_tool_result"
            | "code_execution_tool_result"
            | "bash_code_execution_tool_result"
            | "text_editor_code_execution_tool_result",
        ) => {
            nonempty("tool_use_id")
                && object.get("content").is_some_and(|content| {
                    content.as_object().is_some_and(|result| {
                        result.get("type").is_some_and(serde_json::Value::is_string)
                    }) || content.as_array().is_some_and(|results| {
                        results.iter().all(|result| {
                            result.as_object().is_some_and(|result| {
                                result.get("type").is_some_and(serde_json::Value::is_string)
                            })
                        })
                    })
                })
        }
        Some("container_upload") => nonempty("file_id"),
        Some("compaction") => string("content").is_some(),
        Some("refusal") => string("refusal").is_some(),
        _ => false,
    };
    if !valid {
        return Err("Claude exact Resume conversation block is malformed or unsupported".into());
    }
    Ok(())
}

fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(unix)]
fn native_identity(file: &File) -> std::io::Result<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    let metadata = file.metadata()?;
    Ok((metadata.dev(), metadata.ino()))
}
#[cfg(windows)]
fn native_identity(file: &File) -> std::io::Result<(u64, u64)> {
    use std::os::windows::io::AsRawHandle;
    use winapi::um::fileapi::{GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION};
    let mut information = std::mem::MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::uninit();
    // SAFETY: owned file handle and the Win32 BY_HANDLE_FILE_INFORMATION ABI.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle().cast(), information.as_mut_ptr()) }
        == 0
    {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: successful GetFileInformationByHandle initialized the structure.
    let information = unsafe { information.assume_init() };
    Ok((
        u64::from(information.dwVolumeSerialNumber),
        (u64::from(information.nFileIndexHigh) << 32) | u64::from(information.nFileIndexLow),
    ))
}
#[cfg(not(any(unix, windows)))]
fn native_identity(_file: &File) -> std::io::Result<(u64, u64)> {
    Err(std::io::Error::other(
        "Claude transcript file identity unsupported on this platform",
    ))
}

#[cfg(test)]
mod tests;
