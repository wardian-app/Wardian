//! Read-only source eligibility for exact OpenCode continuation.
use rusqlite::{Connection, OpenFlags};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenCodeConfirmedRecord {
    pub key: String,
    pub sha256: String,
}

/// Stable physical database identity plus immutable, completed provider rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenCodeResumeBinding {
    pub source_path: PathBuf,
    pub session_id: String,
    pub native_identity: (u64, u64),
    pub confirmed_records: Vec<OpenCodeConfirmedRecord>,
}
use wardian_core::models::AgentConfig;

const MAX_SOURCE_BYTES: u64 = 128 * 1024 * 1024;
const MAX_RECORD_BYTES: usize = 16 * 1024 * 1024;
const MAX_HISTORY_BYTES: usize = 64 * 1024 * 1024;
const MAX_ROWS: usize = 10_000;

/// Validate source/options without invoking OpenCode or opening its live database.
/// Repeat this check at launch: the private snapshot is eligibility evidence,
/// not a lease on an active provider writer or a permanent source binding.
pub(super) fn validate(config: &AgentConfig) -> Result<(), String> {
    prepare_exact_continuation(config).map(|_| ())
}

/// Capture authoritative source identity and confirmed history before disposal.
pub fn prepare_exact_continuation(config: &AgentConfig) -> Result<OpenCodeResumeBinding, String> {
    config.validate_exact_resume_identity()?;
    let id = config
        .resume_session
        .as_deref()
        .ok_or("OpenCode exact resume requires a materialized session ID")?;
    validate_options(config, id)?;
    let root = match std::env::var_os("XDG_DATA_HOME").filter(|v| !v.is_empty()) {
        Some(root) => PathBuf::from(root),
        None => dirs::home_dir()
            .ok_or("OpenCode data home unavailable")?
            .join(".local/share"),
    };
    if !root.is_absolute() {
        return Err("OpenCode exact resume requires an absolute XDG data home".into());
    }
    let database = database_path(&root, std::env::var_os("OPENCODE_DB"))?;
    inspect_database(&database, id)
}

fn database_path(
    root: &Path,
    override_path: Option<std::ffi::OsString>,
) -> Result<PathBuf, String> {
    match override_path.filter(|path| !path.is_empty()) {
        Some(path) if path == ":memory:" => {
            Err("OpenCode exact resume requires a durable authoritative database".into())
        }
        Some(path) => {
            let path = PathBuf::from(path);
            if !path.is_absolute()
                && path
                    .components()
                    .any(|component| matches!(component, std::path::Component::Prefix(_)))
            {
                return Err(
                    "OpenCode exact resume drive-relative database override is unsupported".into(),
                );
            }
            Ok(if path.is_absolute() {
                path
            } else {
                root.join("opencode").join(path)
            })
        }
        None => Ok(root.join("opencode/opencode.db")),
    }
}
fn validate_options(config: &AgentConfig, id: &str) -> Result<(), String> {
    if !id.starts_with("ses_")
        || id.len() <= 4
        || id.len() > 128
        || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
    {
        return Err("OpenCode exact resume has an invalid session ID".into());
    }
    let destination = Path::new(&config.folder);
    if !destination.is_absolute() || !destination.is_dir() {
        return Err(
            "OpenCode exact resume requires an existing absolute destination directory".into(),
        );
    }
    // Unknown custom options can select another server/session/project, fork,
    // or provide a positional directory. Never silently ignore bad quoting.
    if let Some(custom) = config
        .custom_args
        .as_deref()
        .filter(|s| !s.trim().is_empty())
    {
        shlex::split(custom)
            .ok_or("OpenCode exact resume custom arguments have invalid quoting")?;
        return Err("OpenCode exact resume does not support custom launch arguments".into());
    }
    Ok(())
}

fn read_source(path: &Path) -> Result<Option<Vec<u8>>, String> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("OpenCode authoritative source is unreadable".into()),
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > MAX_SOURCE_BYTES
    {
        return Err("OpenCode authoritative source is not a bounded ordinary file".into());
    }
    let mut bytes = Vec::new();
    use std::io::Read;
    std::fs::File::open(path)
        .map_err(|_| "OpenCode authoritative source is unreadable")?
        .take(MAX_SOURCE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "OpenCode source read failed")?;
    if bytes.len() as u64 > MAX_SOURCE_BYTES {
        return Err("OpenCode authoritative source exceeds snapshot budget".into());
    }
    Ok(Some(bytes))
}

fn inspect_database(database: &Path, id: &str) -> Result<OpenCodeResumeBinding, String> {
    let canonical = database
        .canonicalize()
        .map_err(|_| "OpenCode exact resume authoritative database is missing")?;
    let identity = native_identity(
        &std::fs::File::open(&canonical).map_err(|_| "OpenCode database unavailable")?,
    )
    .map_err(|_| "OpenCode database native identity unavailable")?;
    let suffix = |suffix: &str| {
        let mut path = database.as_os_str().to_os_string();
        path.push(suffix);
        PathBuf::from(path)
    };
    let paths = [database.to_path_buf(), suffix("-wal"), suffix("-journal")];
    let before = paths
        .iter()
        .map(|path| read_source(path))
        .collect::<Result<Vec<_>, _>>()?;
    if before[0].is_none() {
        return Err("OpenCode exact resume authoritative database is missing".into());
    }
    if before[2].is_some() {
        return Err("OpenCode authoritative source requires journal recovery".into());
    }
    if before.iter().flatten().map(Vec::len).sum::<usize>() as u64 > MAX_SOURCE_BYTES {
        return Err("OpenCode source snapshot exceeds combined budget".into());
    }
    let snapshot = tempfile::tempdir().map_err(|_| "OpenCode private snapshot unavailable")?;
    let copy = snapshot.path().join("opencode.db");
    std::fs::write(&copy, before[0].as_ref().unwrap())
        .map_err(|_| "OpenCode private snapshot write failed")?;
    if let Some(wal) = &before[1] {
        std::fs::write(snapshot.path().join("opencode.db-wal"), wal)
            .map_err(|_| "OpenCode private WAL snapshot write failed")?;
    }
    // Fail closed if live capture changed. SQLite may create SHM only beside our
    // disposable copy, never in the authoritative provider data directory.
    for (path, bytes) in paths.iter().zip(&before) {
        if read_source(path)? != *bytes {
            return Err(
                "OpenCode authoritative source changed during snapshot; retry before disposal"
                    .into(),
            );
        }
    }
    let connection = Connection::open_with_flags(
        &copy,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|_| "OpenCode authoritative database is malformed")?;
    connection
        .busy_timeout(std::time::Duration::from_millis(250))
        .map_err(|_| "OpenCode snapshot busy timeout failed")?;
    let integrity: String = connection
        .query_row("PRAGMA quick_check(1)", [], |row| row.get(0))
        .map_err(|_| "OpenCode snapshot integrity check failed")?;
    if integrity != "ok" {
        return Err("OpenCode authoritative database failed integrity check".into());
    }
    let mut session = connection
        .prepare("SELECT id, directory FROM session WHERE id = ?1 LIMIT 2")
        .map_err(|_| "OpenCode authoritative session schema is unsupported")?;
    let rows = session
        .query_map([id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|_| "OpenCode authoritative session lookup failed")?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "OpenCode authoritative session row is invalid")?;
    if rows.len() != 1 || rows[0].0 != id || rows[0].1.trim().is_empty() {
        return Err(
            "OpenCode exact resume requires exactly one materialized authoritative session".into(),
        );
    }
    let has_v2: bool = connection.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='session_message')", [], |row| row.get(0)).map_err(|_| "OpenCode storage schema inspection failed")?;
    if has_v2 {
        let v2_history: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM session_message WHERE session_id=?1)",
                [id],
                |row| row.get(0),
            )
            .map_err(|_| "OpenCode alternate history schema unsupported")?;
        if v2_history {
            return Err("OpenCode exact resume alternate session history is not qualified".into());
        }
    }
    // A valid zero-message session is materialized. For existing history, every
    // bounded record must parse and every part must reference the same session.
    let mut total = 0;
    for query in ["SELECT data FROM message WHERE session_id = ?1 LIMIT 10001", "SELECT p.data FROM part p LEFT JOIN message m ON m.id = p.message_id WHERE (p.session_id = ?1 AND (m.id IS NULL OR m.session_id != ?1)) OR (m.session_id = ?1 AND p.session_id != ?1) LIMIT 1"] {
        let mut statement = connection.prepare(query).map_err(|_| "OpenCode authoritative history schema is unsupported")?;
        let mut records = statement.query([id]).map_err(|_| "OpenCode history query failed")?;
        let mut count = 0;
        while let Some(row) = records.next().map_err(|_| "OpenCode history read failed")? {
            count += 1;
            if query.contains("LEFT JOIN") { return Err("OpenCode history contains an orphan or foreign-session part".into()); }
            let value: String = row.get(0).map_err(|_| "OpenCode history record is invalid")?;
            total += value.len();
            if count > MAX_ROWS || value.len() > MAX_RECORD_BYTES || total > MAX_HISTORY_BYTES { return Err("OpenCode exact resume history exceeds validation budget".into()); }
            let value: serde_json::Value = serde_json::from_str(&value).map_err(|_| "OpenCode history contains malformed JSON")?;
            if !value.is_object() { return Err("OpenCode history record must be an object".into()); }
        }
    }
    let mut statement = connection
        .prepare("SELECT data FROM part WHERE session_id = ?1 LIMIT 10001")
        .map_err(|_| "OpenCode part schema unsupported")?;
    let mut records = statement
        .query([id])
        .map_err(|_| "OpenCode part query failed")?;
    let mut count = 0;
    while let Some(row) = records.next().map_err(|_| "OpenCode part read failed")? {
        count += 1;
        let value: String = row.get(0).map_err(|_| "OpenCode part record is invalid")?;
        total += value.len();
        if count > MAX_ROWS || value.len() > MAX_RECORD_BYTES || total > MAX_HISTORY_BYTES {
            return Err("OpenCode exact resume history exceeds validation budget".into());
        }
        if !serde_json::from_str::<serde_json::Value>(&value)
            .map_err(|_| "OpenCode part contains malformed JSON")?
            .is_object()
        {
            return Err("OpenCode part record must be an object".into());
        }
    }
    let mut confirmed_records = Vec::new();
    let mut receipt_bytes = 0usize;
    for query in [
        "SELECT id, data FROM message WHERE session_id=?1 AND (json_extract(data, '$.role')='user' OR (json_extract(data, '$.role')='assistant' AND json_type(data, '$.time.completed')='integer' AND json_extract(data, '$.time.completed')>0)) ORDER BY id LIMIT 10001",
        "SELECT p.id, p.message_id || ':' || p.data FROM part p JOIN message m ON m.id=p.message_id WHERE p.session_id=?1 AND m.session_id=?1 AND (json_extract(m.data, '$.role')='user' OR (json_extract(m.data, '$.role')='assistant' AND json_type(m.data, '$.time.completed')='integer' AND json_extract(m.data, '$.time.completed')>0)) ORDER BY p.id LIMIT 10001"
    ] {
        let mut statement = connection.prepare(query).map_err(|_| "OpenCode completion schema unavailable")?;
        let mut rows = statement.query([id]).map_err(|_| "OpenCode completed-history lookup failed")?;
        let prefix = if query.contains("FROM message") { "message:" } else { "part:" };
        while let Some(row) = rows.next().map_err(|_| "OpenCode completed-history read failed")? {
            let key = format!("{prefix}{}", row.get::<_,String>(0).map_err(|_| "OpenCode row identity invalid")?);
            let data: String = row.get(1).map_err(|_| "OpenCode confirmed record invalid")?;
            receipt_bytes += key.len() + 64;
            if confirmed_records.len() >= MAX_ROWS || receipt_bytes > 1024*1024 { return Err("OpenCode confirmed history exceeds receipt budget".into()); }
            if confirmed_records.iter().any(|record: &OpenCodeConfirmedRecord| record.key == key) { return Err("OpenCode completed history has duplicate row identity".into()); }
            confirmed_records.push(OpenCodeConfirmedRecord { key, sha256: format!("{:x}", Sha256::digest(data.as_bytes())) });
        }
    }
    let current = std::fs::File::open(&canonical).map_err(|_| "OpenCode source disappeared")?;
    if native_identity(&current).map_err(|_| "OpenCode source identity unavailable")? != identity {
        return Err("OpenCode source replaced during preparation".into());
    }
    Ok(OpenCodeResumeBinding {
        source_path: canonical,
        session_id: id.into(),
        native_identity: identity,
        confirmed_records,
    })
}

/// Require every already-confirmed record and the same physical source after drain.
/// Newly appended records and previously incomplete assistant completion may appear.
pub fn revalidate_exact_continuation(
    config: &AgentConfig,
    previous: &OpenCodeResumeBinding,
) -> Result<OpenCodeResumeBinding, String> {
    let current = prepare_exact_continuation(config)?;
    validate_binding(&current, previous)?;
    Ok(current)
}

fn validate_binding(
    current: &OpenCodeResumeBinding,
    previous: &OpenCodeResumeBinding,
) -> Result<(), String> {
    if current.source_path != previous.source_path
        || current.session_id != previous.session_id
        || current.native_identity != previous.native_identity
    {
        return Err("OpenCode authoritative source identity changed after preparation".into());
    }
    for old in &previous.confirmed_records {
        if !current.confirmed_records.iter().any(|new| new == old) {
            return Err(
                "OpenCode confirmed history changed or disappeared after preparation".into(),
            );
        }
    }
    Ok(())
}

#[cfg(unix)]
fn native_identity(file: &std::fs::File) -> std::io::Result<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    let metadata = file.metadata()?;
    Ok((metadata.dev(), metadata.ino()))
}
#[cfg(windows)]
fn native_identity(file: &std::fs::File) -> std::io::Result<(u64, u64)> {
    use std::os::windows::io::AsRawHandle;
    use winapi::um::fileapi::{GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION};
    let mut information = std::mem::MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::uninit();
    // SAFETY: owned file handle and the Win32 BY_HANDLE_FILE_INFORMATION ABI.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle().cast(), information.as_mut_ptr()) }
        == 0
    {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: successful API call initialized the structure.
    let information = unsafe { information.assume_init() };
    Ok((
        information.dwVolumeSerialNumber as u64,
        ((information.nFileIndexHigh as u64) << 32) | information.nFileIndexLow as u64,
    ))
}
#[cfg(not(any(unix, windows)))]
fn native_identity(_file: &std::fs::File) -> std::io::Result<(u64, u64)> {
    Err(std::io::Error::other(
        "OpenCode native source identity unsupported",
    ))
}

#[cfg(test)]
mod tests;
