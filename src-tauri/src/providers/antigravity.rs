use std::collections::HashSet;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags, OptionalExtension};
use wardian_core::models::chat::AgentChatRole;
use wardian_core::models::provider::{AgentEvent, AgentProvider};
use wardian_core::models::AgentConfig;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AntigravityTranscriptSummary {
    pub conversation_id: Option<String>,
    pub last_text: Option<String>,
    pub last_step_index: Option<u64>,
}

pub struct AntigravityProvider;

fn database_contains_user_message(path: &Path) -> bool {
    let Ok(connection) = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    ) else {
        return false;
    };
    connection
        .query_row(
            "SELECT 1 FROM steps WHERE step_type = 14 LIMIT 1",
            [],
            |_row| Ok(()),
        )
        .is_ok()
}

fn database_contains_steps_table(path: &Path) -> bool {
    let Ok(connection) = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    ) else {
        return false;
    };
    connection
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'steps' LIMIT 1",
            [],
            |_row| Ok(()),
        )
        .is_ok()
}

fn database_metadata_matches_workspace(path: &Path, workspace: &Path) -> bool {
    let Ok(connection) = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    ) else {
        return false;
    };
    let Ok(data) = connection.query_row(
        "SELECT data FROM trajectory_metadata_blob WHERE id = 'main' LIMIT 1",
        [],
        |row| row.get::<_, Vec<u8>>(0),
    ) else {
        return false;
    };
    let workspace_uri = workspace_file_uri(workspace);
    String::from_utf8_lossy(&data)
        .to_ascii_lowercase()
        .contains(&workspace_uri)
}

fn workspace_file_uri(workspace: &Path) -> String {
    let workspace_key = normalize_path_key(workspace);
    if workspace_key.starts_with("//") {
        format!("file:{workspace_key}")
    } else if workspace_key.starts_with('/') {
        format!("file://{workspace_key}")
    } else {
        format!("file:///{workspace_key}")
    }
}

fn update_latest_timestamp(latest: &mut Option<String>, candidate: Option<String>) {
    let Some(candidate) = candidate else {
        return;
    };
    let should_replace = latest.as_deref().is_none_or(|current| {
        match (
            chrono::DateTime::parse_from_rfc3339(current),
            chrono::DateTime::parse_from_rfc3339(&candidate),
        ) {
            (Ok(current), Ok(candidate)) => candidate > current,
            _ => candidate.as_str() > current,
        }
    });
    if should_replace {
        *latest = Some(candidate);
    }
}

/// A user or model message stored in Antigravity's current SQLite
/// conversation format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AntigravityConversationMessage {
    pub step_index: u64,
    pub role: AgentChatRole,
    pub text: String,
}

/// Historical telemetry extracted from one Antigravity SQLite conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AntigravityConversationMetrics {
    pub query_count: usize,
    pub init_timestamp: Option<String>,
    pub last_query_timestamp: Option<String>,
    pub status: Option<&'static str>,
}

pub(crate) fn changed_workspace_conversation(
    before: Option<&str>,
    after: Option<&str>,
) -> Option<String> {
    let after = after.map(str::trim).filter(|value| !value.is_empty())?;
    (before.map(str::trim) != Some(after)).then(|| after.to_string())
}

impl Default for AntigravityProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl AntigravityProvider {
    pub fn new() -> Self {
        AntigravityProvider
    }

    pub fn antigravity_home() -> Option<PathBuf> {
        dirs::home_dir().map(|home| home.join(".gemini").join("antigravity-cli"))
    }

    pub fn transcript_path(home: &Path, conversation_id: &str) -> PathBuf {
        home.join("brain")
            .join(conversation_id)
            .join(".system_generated")
            .join("logs")
            .join("transcript.jsonl")
    }

    pub fn conversation_database_path(home: &Path, conversation_id: &str) -> PathBuf {
        home.join("conversations")
            .join(format!("{conversation_id}.db"))
    }

    pub fn conversation_database_ids(home: &Path) -> HashSet<String> {
        std::fs::read_dir(home.join("conversations"))
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| {
                let path = entry.path();
                (path.extension().is_some_and(|extension| extension == "db"))
                    .then(|| path.file_stem()?.to_str().map(str::to_string))
                    .flatten()
            })
            .collect()
    }

    /// Discovers a fresh provider conversation only from a database created
    /// after this Wardian generation and carrying the exact workspace URI in
    /// Antigravity's own trajectory metadata. Multiple matches are ambiguous
    /// and intentionally produce no identity.
    pub fn fresh_database_conversation_for_workspace(
        home: &Path,
        workspace: &Path,
        launch_baseline: &HashSet<String>,
        excluded_conversations: &[String],
    ) -> Option<String> {
        let mut matches = Self::conversation_database_ids(home)
            .into_iter()
            .filter(|conversation_id| !launch_baseline.contains(conversation_id))
            .filter(|conversation_id| {
                !excluded_conversations
                    .iter()
                    .any(|excluded| excluded.trim() == conversation_id)
            })
            .filter(|conversation_id| {
                let database = Self::conversation_database_path(home, conversation_id);
                database_contains_steps_table(&database)
                    && database_metadata_matches_workspace(&database, workspace)
            });
        let conversation_id = matches.next()?;
        matches.next().is_none().then_some(conversation_id)
    }

    pub fn conversation_for_workspace(home: &Path, workspace: &Path) -> Option<String> {
        let cache = home.join("cache").join("last_conversations.json");
        let content = std::fs::read_to_string(cache).ok()?;
        let parsed: serde_json::Value = serde_json::from_str(&content).ok()?;
        let object = parsed.as_object()?;
        let workspace_key = normalize_path_key(workspace);
        object.iter().find_map(|(key, value)| {
            (normalize_path_text(key) == workspace_key)
                .then(|| value.as_str().map(str::to_string))
                .flatten()
        })
    }

    /// Resolves only the cache entry that Antigravity explicitly associates
    /// with this workspace. Newer Antigravity builds also record the workspace
    /// URI in conversation metadata; require that match when it is available.
    /// There is intentionally no newest-file fallback here.
    pub fn verified_conversation_for_workspace(
        home: &Path,
        workspace: &Path,
        excluded_conversations: &[String],
    ) -> Option<String> {
        let conversation_id = Self::conversation_for_workspace(home, workspace)?;
        if excluded_conversations
            .iter()
            .any(|excluded| excluded.trim() == conversation_id)
        {
            return None;
        }

        match conversation_metadata_matches_workspace(home, &conversation_id, workspace) {
            Some(true) => {}
            Some(false) => return None,
            // A newly-created conversation can reach the workspace cache
            // before its metadata entry is written. Accept only the provider's
            // durable conversation store (SQLite in current builds, JSONL in
            // older ones), never a newest-file fallback.
            None if Self::conversation_log_path(home, &conversation_id).is_none() => return None,
            None => {}
        }

        Self::conversation_log_path(home, &conversation_id).map(|_| conversation_id)
    }

    /// Returns the durable log for one verified provider conversation. Version
    /// 1.1.7 stores interactive turns in SQLite and leaves the legacy JSONL
    /// transcript empty, so prefer a database that contains a user-message
    /// step. This probe intentionally avoids decoding every payload; chat
    /// hydration performs the full message decode after selecting the source.
    pub fn conversation_log_path(home: &Path, conversation_id: &str) -> Option<PathBuf> {
        let database = Self::conversation_database_path(home, conversation_id);
        if database_contains_user_message(&database) {
            return Some(database);
        }

        let transcript = Self::transcript_path(home, conversation_id);
        transcript.is_file().then_some(transcript)
    }

    /// Returns the provider source used for live status. A known conversation
    /// database is valid status evidence as soon as its schema exists, before
    /// the first user-message step makes it useful as a chat transcript.
    pub fn conversation_status_path(home: &Path, conversation_id: &str) -> Option<PathBuf> {
        let database = Self::conversation_database_path(home, conversation_id);
        if database_contains_steps_table(&database) {
            return Some(database);
        }

        let transcript = Self::transcript_path(home, conversation_id);
        transcript.is_file().then_some(transcript)
    }

    pub fn conversation_messages_from_database(
        path: &Path,
    ) -> Result<Vec<AntigravityConversationMessage>, String> {
        Self::conversation_messages_from_database_since(path, None)
    }

    /// Reads projected conversation messages at or after `minimum_step_index`.
    ///
    /// The inclusive boundary lets the live watcher re-read a small overlap so
    /// provider-authored updates to the latest planner step are still emitted,
    /// without decoding every historical payload on every poll.
    pub fn conversation_messages_from_database_since(
        path: &Path,
        minimum_step_index: Option<u64>,
    ) -> Result<Vec<AntigravityConversationMessage>, String> {
        let profile = crate::utils::runtime_profile::RuntimeProfileSpan::start(
            crate::utils::runtime_profile::RuntimeMetric::AntigravityMessageScan,
        );
        let connection = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|error| format!("failed to open Antigravity conversation database: {error}"))?;
        let sql = if minimum_step_index.is_some() {
            "SELECT idx, step_type, step_payload FROM steps WHERE idx >= ?1 ORDER BY idx"
        } else {
            "SELECT idx, step_type, step_payload FROM steps ORDER BY idx"
        };
        let mut statement = connection
            .prepare(sql)
            .map_err(|error| format!("failed to read Antigravity conversation steps: {error}"))?;
        let minimum_step_index = minimum_step_index.map(|index| index.min(i64::MAX as u64) as i64);
        let mut rows = match minimum_step_index {
            Some(index) => statement.query([index]),
            None => statement.query([]),
        }
        .map_err(|error| format!("failed to query Antigravity conversation steps: {error}"))?;

        let mut messages = Vec::new();
        while let Some(row) = rows
            .next()
            .map_err(|error| format!("failed to decode Antigravity conversation step: {error}"))?
        {
            let step_index = row.get::<_, i64>(0).map_err(|error| {
                format!("failed to decode Antigravity conversation step index: {error}")
            })?;
            let step_type = row.get::<_, i64>(1).map_err(|error| {
                format!("failed to decode Antigravity conversation step type: {error}")
            })?;
            let payload = row.get::<_, Vec<u8>>(2).map_err(|error| {
                format!("failed to decode Antigravity conversation step payload: {error}")
            })?;
            let (role, text) = match step_type {
                // USER_MESSAGE: payload.user_message.text
                14 => (
                    AgentChatRole::User,
                    protobuf_string_at_path(&payload, &[19, 2]),
                ),
                // PLANNER_RESPONSE: prefer the final response; progress prose
                // is the only visible response for intermediate planner steps.
                15 => (
                    AgentChatRole::Assistant,
                    protobuf_string_at_path(&payload, &[20, 1])
                        .or_else(|| protobuf_string_at_path(&payload, &[20, 3])),
                ),
                _ => continue,
            };
            let Some(text) = text
                .map(|text| text.trim().to_string())
                .filter(|text| !text.is_empty())
            else {
                continue;
            };
            messages.push(AntigravityConversationMessage {
                step_index: step_index.max(0) as u64,
                role,
                text,
            });
        }

        profile.finish(messages.len() as u64);
        Ok(messages)
    }

    /// Reads durable user-message timestamps from the current Antigravity
    /// SQLite conversation format. Each step's metadata contains a protobuf
    /// timestamp at field `1`, with seconds and nanos at fields `1` and `2`.
    pub fn conversation_metrics_from_database(
        path: &Path,
    ) -> Result<AntigravityConversationMetrics, String> {
        let connection = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|error| format!("failed to open Antigravity conversation database: {error}"))?;
        let has_metadata_column = connection
            .query_row(
                "SELECT 1 FROM pragma_table_info('steps') WHERE name = 'metadata' LIMIT 1",
                [],
                |_row| Ok(()),
            )
            .is_ok();
        let query = if has_metadata_column {
            "SELECT idx, step_type, metadata FROM steps ORDER BY idx"
        } else {
            "SELECT idx, step_type, NULL AS metadata FROM steps ORDER BY idx"
        };
        let mut statement = connection.prepare(query).map_err(|error| {
            format!("failed to read Antigravity conversation metadata: {error}")
        })?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, Option<Vec<u8>>>(2)?,
                ))
            })
            .map_err(|error| {
                format!("failed to query Antigravity conversation metadata: {error}")
            })?;

        let mut metrics = AntigravityConversationMetrics {
            query_count: 0,
            init_timestamp: None,
            last_query_timestamp: None,
            status: None,
        };
        for row in rows {
            let (_step_index, step_type, metadata) = row.map_err(|error| {
                format!("failed to decode Antigravity conversation metadata: {error}")
            })?;
            let timestamp = metadata
                .as_deref()
                .and_then(|metadata| protobuf_timestamp_at_path(metadata, &[1]));
            if metrics.init_timestamp.is_none() {
                metrics.init_timestamp = timestamp.clone();
            }
            match step_type {
                14 => {
                    metrics.query_count += 1;
                    update_latest_timestamp(&mut metrics.last_query_timestamp, timestamp);
                    metrics.status = Some("Processing...");
                }
                15 => metrics.status = Some("Idle"),
                _ => {}
            }
        }

        Ok(metrics)
    }

    /// Returns the newest durable user-message step in an Antigravity
    /// conversation. The step index is the provider-owned receipt cursor used
    /// by the live watcher to acknowledge a submitted turn.
    pub fn latest_user_message_step_index(path: &Path) -> Result<Option<u64>, String> {
        let profile = crate::utils::runtime_profile::RuntimeProfileSpan::start(
            crate::utils::runtime_profile::RuntimeMetric::AntigravityLatestStep,
        );
        let connection = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|error| format!("failed to open Antigravity conversation database: {error}"))?;
        let mut statement = connection
            .prepare("SELECT idx FROM steps WHERE step_type = 14 ORDER BY idx DESC LIMIT 1")
            .map_err(|error| format!("failed to read Antigravity user-message steps: {error}"))?;
        let latest = statement
            .query_row([], |row| row.get::<_, i64>(0))
            .optional()
            .map(|step_index| step_index.map(|index| index.max(0) as u64))
            .map_err(|error| format!("failed to query Antigravity user-message steps: {error}"));
        profile.finish(u64::from(latest.as_ref().is_ok_and(Option::is_some)));
        latest
    }

    pub fn summarize_conversation(
        home: &Path,
        conversation_id: &str,
    ) -> Result<AntigravityTranscriptSummary, String> {
        let path = Self::transcript_path(home, conversation_id);
        let content = std::fs::read_to_string(&path).map_err(|err| {
            format!(
                "Failed to read Antigravity transcript {}: {}",
                path.display(),
                err
            )
        })?;
        let mut summary = Self::summarize_transcript_content(&content);
        summary.conversation_id = Some(conversation_id.to_string());
        Ok(summary)
    }

    pub fn summarize_transcript_content(content: &str) -> AntigravityTranscriptSummary {
        let mut summary = AntigravityTranscriptSummary::default();
        for line in content.lines() {
            let Ok(parsed) = serde_json::from_str::<serde_json::Value>(line) else {
                continue;
            };
            if is_antigravity_model_response(&parsed) {
                if let Some(text) = parsed.get("content").and_then(|value| value.as_str()) {
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        summary.last_text = Some(trimmed.to_string());
                    }
                }
                summary.last_step_index = parsed.get("step_index").and_then(|value| value.as_u64());
            }
        }
        summary
    }
}

fn conversation_metadata_matches_workspace(
    home: &Path,
    conversation_id: &str,
    workspace: &Path,
) -> Option<bool> {
    let metadata =
        std::fs::read_to_string(home.join("cache").join("conversation_metadata.json")).ok()?;
    let parsed: serde_json::Value = serde_json::from_str(&metadata).ok()?;
    let workspace_uris = parsed
        .get("conversations")?
        .get(conversation_id)?
        .get("summary")?
        .get("WorkspaceURIs")?
        .as_array()?;
    let workspace_key = normalize_path_key(workspace);
    Some(workspace_uris.iter().any(|value| {
        value
            .as_str()
            .and_then(file_uri_path_text)
            .is_some_and(|path| normalize_path_text(path) == workspace_key)
    }))
}

fn file_uri_path_text(uri: &str) -> Option<&str> {
    let path = uri.strip_prefix("file://")?;
    if path.len() >= 3 && path.starts_with('/') && path.as_bytes()[2] == b':' {
        Some(&path[1..])
    } else {
        Some(path)
    }
}

fn protobuf_string_at_path(bytes: &[u8], fields: &[u32]) -> Option<String> {
    let current = protobuf_message_at_path(bytes, fields)?;
    String::from_utf8(current.to_vec()).ok()
}

fn protobuf_message_at_path<'a>(bytes: &'a [u8], fields: &[u32]) -> Option<&'a [u8]> {
    fields.iter().try_fold(bytes, |current, field| {
        protobuf_length_delimited_field(current, *field)
    })
}

fn protobuf_varint_field(bytes: &[u8], wanted_field: u32) -> Option<u64> {
    let mut offset = 0;
    while offset < bytes.len() {
        let key = protobuf_varint(bytes, &mut offset)?;
        let field = (key >> 3) as u32;
        match key & 0x07 {
            0 => {
                let value = protobuf_varint(bytes, &mut offset)?;
                if field == wanted_field {
                    return Some(value);
                }
            }
            1 => offset = offset.checked_add(8)?,
            2 => {
                let length = usize::try_from(protobuf_varint(bytes, &mut offset)?).ok()?;
                offset = offset.checked_add(length)?;
            }
            5 => offset = offset.checked_add(4)?,
            _ => return None,
        }
    }
    None
}

fn protobuf_timestamp_at_path(bytes: &[u8], fields: &[u32]) -> Option<String> {
    let timestamp = protobuf_message_at_path(bytes, fields)?;
    let seconds = i64::try_from(protobuf_varint_field(timestamp, 1)?).ok()?;
    let nanos = u32::try_from(protobuf_varint_field(timestamp, 2).unwrap_or_default()).ok()?;
    chrono::DateTime::from_timestamp(seconds, nanos)
        .map(|timestamp| timestamp.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
}

fn protobuf_length_delimited_field(bytes: &[u8], wanted_field: u32) -> Option<&[u8]> {
    let mut offset = 0;
    while offset < bytes.len() {
        let key = protobuf_varint(bytes, &mut offset)?;
        let field = (key >> 3) as u32;
        match key & 0x07 {
            0 => {
                protobuf_varint(bytes, &mut offset)?;
            }
            1 => offset = offset.checked_add(8)?,
            2 => {
                let length = usize::try_from(protobuf_varint(bytes, &mut offset)?).ok()?;
                let end = offset.checked_add(length)?;
                let value = bytes.get(offset..end)?;
                offset = end;
                if field == wanted_field {
                    return Some(value);
                }
            }
            5 => offset = offset.checked_add(4)?,
            _ => return None,
        }
    }
    None
}

fn protobuf_varint(bytes: &[u8], offset: &mut usize) -> Option<u64> {
    let mut value = 0_u64;
    for index in 0..10 {
        let byte = *bytes.get(*offset)?;
        *offset += 1;
        let shift = index * 7;
        if shift >= 64 || (shift == 63 && (byte & 0x7f) > 1) {
            return None;
        }
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Some(value);
        }
    }
    None
}

impl AgentProvider for AntigravityProvider {
    fn name(&self) -> &str {
        "antigravity"
    }

    fn get_executable(&self) -> (String, Vec<String>) {
        #[cfg(target_os = "windows")]
        {
            if let Some(paths) = std::env::var_os("PATH") {
                for path in std::env::split_paths(&paths) {
                    for name in ["agy.exe", "agy.cmd", "agy.bat", "agy"] {
                        let candidate = path.join(name);
                        if candidate.is_file() {
                            if !name.eq_ignore_ascii_case("agy.exe") {
                                if let Some(launch) =
                                    crate::providers::npm::node_launch_from_npm_cmd_shim(
                                        &path, "agy",
                                    )
                                {
                                    return launch;
                                }
                            }
                            return (candidate.to_string_lossy().to_string(), vec![]);
                        }
                    }
                }
            }

            if let Some(local) = dirs::data_local_dir() {
                let candidate = local.join("agy").join("bin").join("agy.exe");
                if candidate.is_file() {
                    return (candidate.to_string_lossy().to_string(), vec![]);
                }
            }

            ("agy".to_string(), vec![])
        }

        #[cfg(not(target_os = "windows"))]
        {
            if let Some(paths) = std::env::var_os("PATH") {
                for path in std::env::split_paths(&paths) {
                    let candidate = path.join("agy");
                    if candidate.is_file() {
                        return (candidate.to_string_lossy().to_string(), vec![]);
                    }
                }
            }

            let home = dirs::home_dir().unwrap_or_default();
            for path in [
                home.join(".local/bin/agy"),
                PathBuf::from("/usr/local/bin/agy"),
                PathBuf::from("/opt/homebrew/bin/agy"),
            ] {
                if path.is_file() {
                    return (path.to_string_lossy().to_string(), vec![]);
                }
            }

            ("agy".to_string(), vec![])
        }
    }

    fn get_spawn_args(&self, config: &AgentConfig, is_resume: bool) -> Vec<String> {
        let mut args = Vec::new();
        let antigravity = config.antigravity_config();

        let mut directories = config
            .system_include_directories
            .clone()
            .unwrap_or_default();
        if let Some(user_dirs) = config.include_directories.as_ref() {
            for dir in user_dirs {
                if !directories.contains(dir) {
                    directories.push(dir.clone());
                }
            }
        }
        let directories = crate::utils::fs::project_antigravity_include_directories(
            &config.session_id,
            directories,
        );
        for dir in directories {
            args.push("--add-dir".to_string());
            args.push(dir);
        }

        if antigravity.sandbox.unwrap_or(false) {
            args.push("--sandbox".to_string());
        }
        if antigravity.dangerously_skip_permissions.unwrap_or(true) {
            args.push("--dangerously-skip-permissions".to_string());
        }
        if let Some(mode) = antigravity
            .mode
            .as_deref()
            .map(str::trim)
            .filter(|mode| matches!(*mode, "accept-edits" | "plan"))
        {
            args.push("--mode".to_string());
            args.push(mode.to_string());
        }
        if let Some(agent) = antigravity
            .agent
            .as_deref()
            .map(str::trim)
            .filter(|agent| !agent.is_empty())
        {
            args.push("--agent".to_string());
            args.push(agent.to_string());
        }
        if let Some(model) = config
            .model
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            args.push("--model".to_string());
            args.push(model.to_string());
        }
        if let Some(effort) = antigravity
            .reasoning_effort
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            args.push("--effort".to_string());
            args.push(effort.to_string());
        }

        if is_resume {
            if let Some(session_id) = config
                .resume_session
                .as_ref()
                .filter(|value| !value.trim().is_empty())
            {
                args.push("--conversation".to_string());
                args.push(session_id.clone());
            }
        }

        if let Some(custom) = config.custom_args.as_ref() {
            if let Some(parsed) = shlex::split(custom) {
                args.extend(parsed);
            }
        }

        args
    }

    fn parse_output(&self, line: &str) -> Option<AgentEvent> {
        let trimmed = line.trim();
        if trimmed.contains("Do you trust the contents of this project?")
            || trimmed.contains("requires permission to read, edit, and execute files here")
            || trimmed.contains("Requesting permission for:")
        {
            return Some(AgentEvent::ActionRequired {
                message: "Antigravity permission required".to_string(),
            });
        }

        let parsed: serde_json::Value = serde_json::from_str(line).ok()?;
        match parsed.get("type").and_then(|value| value.as_str()) {
            Some("USER_INPUT") => Some(AgentEvent::UserQuery),
            // Antigravity emits a DONE PLANNER_RESPONSE for every planner step:
            // tool calls, progress prose, and the final answer. The terminal
            // ready prompt is the only reliable end-of-turn boundary.
            Some("PLANNER_RESPONSE") => Some(AgentEvent::Unknown),
            Some("SYSTEM_MESSAGE") | Some("CONVERSATION_HISTORY") => Some(AgentEvent::Unknown),
            _ => Some(AgentEvent::Unknown),
        }
    }

    fn get_instruction_filename(&self) -> &str {
        "AGENTS.md"
    }
}

fn is_antigravity_model_response(value: &serde_json::Value) -> bool {
    value.get("source").and_then(|value| value.as_str()) == Some("MODEL")
        && value.get("type").and_then(|value| value.as_str()) == Some("PLANNER_RESPONSE")
        && value.get("status").and_then(|value| value.as_str()) == Some("DONE")
}

fn normalize_path_key(path: &Path) -> String {
    normalize_path_text(&path.to_string_lossy())
}

fn normalize_path_text(path: &str) -> String {
    let normalized = path.replace('\\', "/");
    let normalized = normalized
        .strip_prefix("//?/UNC/")
        .map(|path| format!("//{path}"))
        .unwrap_or(normalized);
    normalized
        .strip_prefix("//?/")
        .unwrap_or(&normalized)
        .trim_end_matches('/')
        .to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;

    #[test]
    fn bootstrap_requires_a_changed_workspace_mapping() {
        assert_eq!(
            changed_workspace_conversation(Some("old"), Some("new")).as_deref(),
            Some("new")
        );
        assert_eq!(
            changed_workspace_conversation(Some("same"), Some("same")),
            None
        );
        assert_eq!(changed_workspace_conversation(None, None), None);
    }
    use wardian_core::models::{AntigravityProviderConfig, ProviderConfig};

    fn make_provider() -> AntigravityProvider {
        AntigravityProvider::new()
    }

    fn make_antigravity_config(antigravity: AntigravityProviderConfig) -> AgentConfig {
        AgentConfig {
            provider: "antigravity".into(),
            provider_config: ProviderConfig::Antigravity(antigravity),
            ..Default::default()
        }
    }

    #[test]
    fn name_returns_lowercase_antigravity() {
        assert_eq!(make_provider().name(), "antigravity");
    }

    #[test]
    fn instruction_filename_is_agents_md() {
        assert_eq!(make_provider().get_instruction_filename(), "AGENTS.md");
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_path_resolution_prefers_node_entrypoint_over_cmd_shim() {
        let _lock = crate::utils::wardian_test_env_lock();
        let previous_path = std::env::var_os("PATH");
        let temp = tempfile::tempdir().unwrap();
        let agy_js = temp
            .path()
            .join("node_modules")
            .join("@google")
            .join("antigravity")
            .join("bin")
            .join("agy.js");
        std::fs::create_dir_all(agy_js.parent().unwrap()).unwrap();
        std::fs::write(
            temp.path().join("agy.cmd"),
            r#"@ECHO off
SET dp0=%~dp0
"%dp0%\node.exe" "%dp0%\node_modules\@google\antigravity\bin\agy.js" %*
"#,
        )
        .unwrap();
        std::fs::write(&agy_js, "console.log('agy')").unwrap();

        unsafe {
            std::env::set_var("PATH", temp.path());
        }

        let (executable, args) = AntigravityProvider::new().get_executable();

        assert_eq!(executable, "node");
        assert_eq!(args, vec![agy_js.to_string_lossy().to_string()]);

        match previous_path {
            Some(value) => unsafe { std::env::set_var("PATH", value) },
            None => unsafe { std::env::remove_var("PATH") },
        }
    }

    #[test]
    fn spawn_args_include_context_dirs_sandbox_permissions_and_resume() {
        let provider = make_provider();
        let config = AgentConfig {
            model: Some("gemini-3.8-flash-low".into()),
            system_include_directories: Some(vec!["common".into(), "class".into()]),
            include_directories: Some(vec!["class".into(), "user".into()]),
            resume_session: Some("conversation-123".into()),
            ..make_antigravity_config(AntigravityProviderConfig {
                reasoning_effort: Some("high".into()),
                sandbox: Some(true),
                dangerously_skip_permissions: Some(true),
                mode: Some("plan".into()),
                agent: Some("reviewer".into()),
                ..Default::default()
            })
        };

        let args = provider.get_spawn_args(&config, true);

        assert_eq!(
            args,
            vec![
                "--add-dir",
                "common",
                "--add-dir",
                "class",
                "--add-dir",
                "user",
                "--sandbox",
                "--dangerously-skip-permissions",
                "--mode",
                "plan",
                "--agent",
                "reviewer",
                "--model",
                "gemini-3.8-flash-low",
                "--effort",
                "high",
                "--conversation",
                "conversation-123",
            ]
        );
    }

    #[test]
    fn spawn_args_skip_permissions_by_default_but_honor_explicit_false() {
        let provider = make_provider();
        let default_args = provider.get_spawn_args(
            &make_antigravity_config(AntigravityProviderConfig::default()),
            false,
        );
        assert!(default_args.contains(&"--dangerously-skip-permissions".to_string()));

        let explicit_args = provider.get_spawn_args(
            &make_antigravity_config(AntigravityProviderConfig {
                dangerously_skip_permissions: Some(false),
                ..Default::default()
            }),
            false,
        );
        assert!(!explicit_args.contains(&"--dangerously-skip-permissions".to_string()));
    }

    #[test]
    fn spawn_args_project_hidden_wardian_include_roots_before_add_dir() {
        let provider = make_provider();
        let temp = tempfile::tempdir().expect("temp dir");
        let hidden = temp.path().join(".wardian").join("common");
        std::fs::create_dir_all(hidden.join(".agents").join("skills").join("role-skill"))
            .expect("create skill");
        std::fs::write(hidden.join("AGENTS.md"), "instructions").expect("write agents");
        let config = AgentConfig {
            session_id: "antigravity-session".to_string(),
            system_include_directories: Some(vec![hidden.to_string_lossy().to_string()]),
            ..make_antigravity_config(AntigravityProviderConfig::default())
        };

        let args = provider.get_spawn_args(&config, false);

        assert_eq!(args[0], "--add-dir");
        assert_ne!(args[1], hidden.to_string_lossy());
        let projected = PathBuf::from(&args[1]);
        assert!(projected.join("AGENTS.md").exists());
        assert!(projected
            .join(".agents")
            .join("skills")
            .join("role-skill")
            .exists());
    }

    #[test]
    fn parse_output_leaves_planner_responses_to_terminal_ready_detection() {
        let provider = make_provider();

        assert_eq!(
            provider
                .parse_output(r#"{"type":"USER_INPUT","source":"USER_EXPLICIT"}"#)
                .unwrap(),
            AgentEvent::UserQuery
        );
        assert_eq!(
            provider
                .parse_output(
                    r#"{"source":"MODEL","type":"PLANNER_RESPONSE","status":"DONE","content":"ok"}"#
                )
                .unwrap(),
            AgentEvent::Unknown
        );
    }

    #[test]
    fn parse_output_detects_workspace_trust_prompt_as_action_required() {
        let provider = make_provider();
        let line = "Do you trust the contents of this project? Antigravity CLI requires permission to read, edit, and execute files here.";

        assert!(matches!(
            provider.parse_output(line),
            Some(AgentEvent::ActionRequired { .. })
        ));
    }

    #[test]
    fn summarize_transcript_content_returns_last_model_response() {
        let content = concat!(
            "{\"step_index\":0,\"source\":\"USER_EXPLICIT\",\"type\":\"USER_INPUT\",\"status\":\"DONE\",\"content\":\"hello\"}\n",
            "{\"step_index\":2,\"source\":\"MODEL\",\"type\":\"PLANNER_RESPONSE\",\"status\":\"DONE\",\"content\":\"first\"}\n",
            "{\"step_index\":6,\"source\":\"MODEL\",\"type\":\"PLANNER_RESPONSE\",\"status\":\"DONE\",\"content\":\"second\"}\n",
        );

        let summary = AntigravityProvider::summarize_transcript_content(content);

        assert_eq!(summary.last_text.as_deref(), Some("second"));
        assert_eq!(summary.last_step_index, Some(6));
    }

    #[test]
    fn conversation_for_workspace_reads_cache_with_path_normalization() {
        let temp = tempfile::tempdir().expect("temp dir");
        let home = temp.path();
        std::fs::create_dir_all(home.join("cache")).expect("cache dir");
        std::fs::write(
            home.join("cache").join("last_conversations.json"),
            r#"{"C:\\Project\\Wardian":"conversation-123"}"#,
        )
        .expect("cache");

        let conversation =
            AntigravityProvider::conversation_for_workspace(home, Path::new("C:/Project/Wardian"));

        assert_eq!(conversation.as_deref(), Some("conversation-123"));
    }

    #[test]
    fn file_uri_paths_preserve_posix_and_windows_workspace_paths() {
        assert_eq!(
            file_uri_path_text("file:///tmp/wardian-workspace"),
            Some("/tmp/wardian-workspace")
        );
        assert_eq!(
            file_uri_path_text("file:///C:/Workspace/Wardian"),
            Some("C:/Workspace/Wardian")
        );
        assert_eq!(
            normalize_path_text(r"\\?\C:\Workspace\Wardian"),
            normalize_path_text("C:/Workspace/Wardian")
        );
        assert_eq!(
            normalize_path_text(r"\\?\UNC\server\share\Wardian"),
            normalize_path_text("//server/share/Wardian")
        );
    }

    fn protobuf_varint(value: u64) -> Vec<u8> {
        let mut value = value;
        let mut bytes = Vec::new();
        loop {
            let mut byte = (value & 0x7f) as u8;
            value >>= 7;
            if value != 0 {
                byte |= 0x80;
            }
            bytes.push(byte);
            if value == 0 {
                return bytes;
            }
        }
    }

    fn protobuf_string_field(field: u32, text: &str) -> Vec<u8> {
        let mut bytes = protobuf_varint(u64::from(field << 3 | 2));
        bytes.extend(protobuf_varint(text.len() as u64));
        bytes.extend(text.as_bytes());
        bytes
    }

    fn protobuf_varint_field(field: u32, value: u64) -> Vec<u8> {
        let mut bytes = protobuf_varint(u64::from(field << 3));
        bytes.extend(protobuf_varint(value));
        bytes
    }

    fn protobuf_message_field(field: u32, value: Vec<u8>) -> Vec<u8> {
        let mut bytes = protobuf_varint(u64::from(field << 3 | 2));
        bytes.extend(protobuf_varint(value.len() as u64));
        bytes.extend(value);
        bytes
    }

    fn protobuf_timestamp_metadata(seconds: u64, nanos: u64) -> Vec<u8> {
        let mut timestamp = protobuf_varint_field(1, seconds);
        timestamp.extend(protobuf_varint_field(2, nanos));
        protobuf_message_field(1, timestamp)
    }

    #[test]
    fn verified_workspace_conversation_prefers_current_sqlite_messages() {
        let temp = tempfile::tempdir().expect("temp dir");
        let home = temp.path();
        let conversation_id = "conversation-123";
        std::fs::create_dir_all(home.join("cache")).expect("cache dir");
        std::fs::create_dir_all(home.join("conversations")).expect("conversation dir");
        std::fs::write(
            home.join("cache").join("last_conversations.json"),
            r#"{"C:\\Project\\Wardian":"conversation-123"}"#,
        )
        .expect("cache");
        std::fs::write(
            home.join("cache").join("conversation_metadata.json"),
            r#"{"conversations":{"conversation-123":{"summary":{"WorkspaceURIs":["file:///C:/Project/Wardian"]}}}}"#,
        )
        .expect("metadata");

        let database = AntigravityProvider::conversation_database_path(home, conversation_id);
        let connection = Connection::open(&database).expect("open database");
        connection
            .execute_batch(
                "CREATE TABLE steps (idx INTEGER, step_type INTEGER, step_payload BLOB);",
            )
            .expect("create steps");
        let user = protobuf_message_field(19, protobuf_string_field(2, "Make brownies."));
        let assistant = protobuf_message_field(20, protobuf_string_field(1, "Brownies made."));
        connection
            .execute(
                "INSERT INTO steps (idx, step_type, step_payload) VALUES (?1, ?2, ?3)",
                params![0_i64, 14_i64, user],
            )
            .expect("insert user");
        connection
            .execute(
                "INSERT INTO steps (idx, step_type, step_payload) VALUES (?1, ?2, ?3)",
                params![1_i64, 15_i64, assistant],
            )
            .expect("insert assistant");
        drop(connection);

        let workspace = Path::new("C:/Project/Wardian");
        assert_eq!(
            AntigravityProvider::verified_conversation_for_workspace(home, workspace, &[])
                .as_deref(),
            Some(conversation_id)
        );
        assert_eq!(
            AntigravityProvider::conversation_log_path(home, conversation_id),
            Some(database.clone())
        );
        let messages = AntigravityProvider::conversation_messages_from_database(&database)
            .expect("read conversation database");
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, AgentChatRole::User);
        assert_eq!(messages[0].text, "Make brownies.");
        assert_eq!(messages[1].role, AgentChatRole::Assistant);
        assert_eq!(messages[1].text, "Brownies made.");
        assert!(AntigravityProvider::verified_conversation_for_workspace(
            home,
            workspace,
            &[conversation_id.to_string()],
        )
        .is_none());
    }

    #[test]
    fn verified_workspace_conversation_accepts_sqlite_before_metadata_is_written() {
        let temp = tempfile::tempdir().expect("temp dir");
        let home = temp.path();
        let conversation_id = "conversation-before-metadata";
        std::fs::create_dir_all(home.join("cache")).expect("cache dir");
        std::fs::create_dir_all(home.join("conversations")).expect("conversation dir");
        std::fs::write(
            home.join("cache").join("last_conversations.json"),
            r#"{"C:\\Project\\Wardian":"conversation-before-metadata"}"#,
        )
        .expect("cache");

        let database = AntigravityProvider::conversation_database_path(home, conversation_id);
        let connection = Connection::open(&database).expect("open database");
        connection
            .execute_batch(
                "CREATE TABLE steps (idx INTEGER, step_type INTEGER, step_payload BLOB);",
            )
            .expect("create steps");
        let user = protobuf_message_field(19, protobuf_string_field(2, "Fresh prompt."));
        connection
            .execute(
                "INSERT INTO steps (idx, step_type, step_payload) VALUES (?1, ?2, ?3)",
                params![0_i64, 14_i64, user],
            )
            .expect("insert user");
        drop(connection);

        assert_eq!(
            AntigravityProvider::verified_conversation_for_workspace(
                home,
                Path::new("C:/Project/Wardian"),
                &[],
            )
            .as_deref(),
            Some(conversation_id)
        );
    }

    #[test]
    fn status_path_accepts_empty_conversation_database_without_changing_chat_fallback() {
        let temp = tempfile::tempdir().expect("temp dir");
        let home = temp.path();
        let conversation_id = "conversation-before-first-turn";
        std::fs::create_dir_all(home.join("conversations")).expect("conversation dir");
        let transcript = AntigravityProvider::transcript_path(home, conversation_id);
        std::fs::create_dir_all(transcript.parent().expect("transcript parent"))
            .expect("transcript dir");
        std::fs::write(&transcript, "").expect("empty legacy transcript");
        let database = AntigravityProvider::conversation_database_path(home, conversation_id);
        Connection::open(&database)
            .expect("open database")
            .execute_batch(
                "CREATE TABLE steps (idx INTEGER, step_type INTEGER, step_payload BLOB);",
            )
            .expect("create steps");

        assert_eq!(
            AntigravityProvider::conversation_status_path(home, conversation_id),
            Some(database)
        );
        assert_eq!(
            AntigravityProvider::conversation_log_path(home, conversation_id),
            Some(transcript)
        );
    }

    #[test]
    fn fresh_database_discovery_requires_generation_and_exact_workspace_metadata() {
        let temp = tempfile::tempdir().expect("temp dir");
        let home = temp.path();
        let workspace = home
            .join("agents")
            .join("agent-1")
            .join("habitat")
            .join("workspace");
        std::fs::create_dir_all(home.join("conversations")).expect("conversation dir");
        std::fs::create_dir_all(&workspace).expect("workspace dir");

        let create_database = |conversation_id: &str, metadata_workspace: &Path| {
            let database = AntigravityProvider::conversation_database_path(home, conversation_id);
            let connection = Connection::open(&database).expect("open database");
            connection
                .execute_batch(
                    "CREATE TABLE steps (idx INTEGER, step_type INTEGER, step_payload BLOB);\
                     CREATE TABLE trajectory_metadata_blob (id TEXT PRIMARY KEY, data BLOB);",
                )
                .expect("create schema");
            let uri = workspace_file_uri(metadata_workspace);
            connection
                .execute(
                    "INSERT INTO trajectory_metadata_blob (id, data) VALUES ('main', ?1)",
                    params![uri.as_bytes()],
                )
                .expect("insert metadata");
        };

        create_database("preexisting", &workspace);
        let baseline = AntigravityProvider::conversation_database_ids(home);
        create_database("wrong-workspace", &home.join("other"));
        create_database("fresh-match", &workspace);

        assert_eq!(
            AntigravityProvider::fresh_database_conversation_for_workspace(
                home,
                &workspace,
                &baseline,
                &[],
            )
            .as_deref(),
            Some("fresh-match")
        );
        assert!(
            AntigravityProvider::fresh_database_conversation_for_workspace(
                home,
                &workspace,
                &baseline,
                &["fresh-match".to_string()],
            )
            .is_none()
        );
    }

    #[test]
    fn latest_user_message_step_index_reads_only_user_message_steps() {
        let temp = tempfile::tempdir().expect("temp dir");
        let database = temp.path().join("conversation.db");
        let connection = Connection::open(&database).expect("open database");
        connection
            .execute_batch(
                "CREATE TABLE steps (idx INTEGER, step_type INTEGER, step_payload BLOB);",
            )
            .expect("create steps");
        connection
            .execute(
                "INSERT INTO steps (idx, step_type, step_payload) VALUES (?1, ?2, ?3)",
                params![4_i64, 14_i64, Vec::<u8>::new()],
            )
            .expect("insert first user message");
        connection
            .execute(
                "INSERT INTO steps (idx, step_type, step_payload) VALUES (?1, ?2, ?3)",
                params![9_i64, 15_i64, Vec::<u8>::new()],
            )
            .expect("insert assistant response");
        connection
            .execute(
                "INSERT INTO steps (idx, step_type, step_payload) VALUES (?1, ?2, ?3)",
                params![12_i64, 14_i64, Vec::<u8>::new()],
            )
            .expect("insert second user message");
        drop(connection);

        assert_eq!(
            AntigravityProvider::latest_user_message_step_index(&database)
                .expect("read latest user message"),
            Some(12)
        );
        assert_eq!(
            AntigravityProvider::conversation_metrics_from_database(&database)
                .expect("read metrics without metadata"),
            AntigravityConversationMetrics {
                query_count: 2,
                init_timestamp: None,
                last_query_timestamp: None,
                status: Some("Processing..."),
            }
        );
    }

    #[test]
    fn incremental_message_projection_reads_only_the_requested_overlap() {
        let temp = tempfile::tempdir().expect("temp dir");
        let database = temp.path().join("conversation.db");
        let connection = Connection::open(&database).expect("open database");
        connection
            .execute_batch(
                "CREATE TABLE steps (idx INTEGER, step_type INTEGER, step_payload BLOB);",
            )
            .expect("create steps");
        for (index, text) in [(2_i64, "old"), (20_i64, "overlap"), (21_i64, "new")] {
            let payload = protobuf_message_field(19, protobuf_string_field(2, text));
            connection
                .execute(
                    "INSERT INTO steps (idx, step_type, step_payload) VALUES (?1, 14, ?2)",
                    rusqlite::params![index, payload],
                )
                .expect("insert user message");
        }
        drop(connection);

        let messages =
            AntigravityProvider::conversation_messages_from_database_since(&database, Some(20))
                .expect("read incremental messages");

        assert_eq!(
            messages
                .iter()
                .map(|message| (message.step_index, message.text.as_str()))
                .collect::<Vec<_>>(),
            vec![(20, "overlap"), (21, "new")]
        );
    }

    #[test]
    fn conversation_metrics_from_database_reads_user_message_timestamps() {
        let temp = tempfile::tempdir().expect("temp dir");
        let database = temp.path().join("conversation.db");
        let connection = Connection::open(&database).expect("open database");
        connection
            .execute_batch(
                "CREATE TABLE steps (
                    idx INTEGER,
                    step_type INTEGER,
                    metadata BLOB,
                    step_payload BLOB
                );",
            )
            .expect("create steps");
        for (idx, step_type, seconds, nanos) in [
            (0_i64, 14_i64, 1_787_770_550_u64, 0_u64),
            (1_i64, 15_i64, 1_787_770_551_u64, 0_u64),
            (2_i64, 14_i64, 1_787_770_552_u64, 500_000_000_u64),
            (3_i64, 15_i64, 1_787_770_553_u64, 0_u64),
        ] {
            connection
                .execute(
                    "INSERT INTO steps (idx, step_type, metadata, step_payload)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![
                        idx,
                        step_type,
                        protobuf_timestamp_metadata(seconds, nanos),
                        Vec::<u8>::new()
                    ],
                )
                .expect("insert conversation step");
        }
        drop(connection);

        assert_eq!(
            AntigravityProvider::conversation_metrics_from_database(&database)
                .expect("read conversation metrics"),
            AntigravityConversationMetrics {
                query_count: 2,
                init_timestamp: Some("2026-08-26T18:55:50.000Z".to_string()),
                last_query_timestamp: Some("2026-08-26T18:55:52.500Z".to_string()),
                status: Some("Idle"),
            }
        );
    }

    #[test]
    fn conversation_metrics_from_database_uses_newest_timestamp_not_last_row() {
        let temp = tempfile::tempdir().expect("temp dir");
        let database = temp.path().join("conversation.db");
        let connection = Connection::open(&database).expect("open database");
        connection
            .execute_batch(
                "CREATE TABLE steps (
                    idx INTEGER,
                    step_type INTEGER,
                    metadata BLOB,
                    step_payload BLOB
                );",
            )
            .expect("create steps");
        for (idx, seconds) in [(0_i64, 1_787_770_552_u64), (1_i64, 1_787_770_550_u64)] {
            connection
                .execute(
                    "INSERT INTO steps (idx, step_type, metadata, step_payload)
                     VALUES (?1, 14, ?2, ?3)",
                    params![
                        idx,
                        protobuf_timestamp_metadata(seconds, 0),
                        Vec::<u8>::new()
                    ],
                )
                .expect("insert user message");
        }
        drop(connection);

        let metrics = AntigravityProvider::conversation_metrics_from_database(&database)
            .expect("read conversation metrics");
        assert_eq!(
            metrics.last_query_timestamp.as_deref(),
            Some("2026-08-26T18:55:52.000Z")
        );
    }
}
