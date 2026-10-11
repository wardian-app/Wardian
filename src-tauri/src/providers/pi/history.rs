//! Read-only proof of Pi's deferred first session flush in the agent-owned directory.
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs::File;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};

// Startup examines the first persisted assistant, never an unbounded transcript.
const MAX_RECORD_BYTES: u64 = 16 * 1024 * 1024;
const MAX_PREFIX_BYTES: u64 = 64 * 1024 * 1024;
const MAX_FILES: usize = 4096;

pub(crate) struct SessionFileBinding {
    pub(crate) path: PathBuf,
    identity: same_file::Handle,
    prefix_len: u64,
    prefix_hash: [u8; 32],
}

impl SessionFileBinding {
    /// Read-only SDK opening receipt. Old histories must never enter an API
    /// which may migrate and rewrite them during open.
    pub(crate) fn sdk_open_receipt(&self, native_id: &str) -> Result<serde_json::Value, String> {
        self.revalidate()?;
        let mut reader = BufReader::new(plain_file(&self.path)?);
        let mut line = String::new();
        bounded_line(&mut reader, &mut line)?;
        let header: serde_json::Value =
            serde_json::from_str(line.trim_end()).map_err(history_error)?;
        if header["type"] != "session" || header["id"].as_str() != Some(native_id) {
            return Err("Pi exact SDK resume history identity changed".into());
        }
        if header["version"].as_u64() != Some(3) {
            return Err(
                "Pi exact SDK resume requires current v3 history; migration is not permitted"
                    .into(),
            );
        }
        Ok(serde_json::json!({
            "prefixBytes": self.prefix_len,
            "prefixSha256": self.prefix_hash.iter().map(|byte| format!("{byte:02x}")).collect::<String>(),
        }))
    }

    /// Append-only progress is allowed; replacement or changed confirmation
    /// bytes must fail before the provider receives an exact-file selector.
    pub(crate) fn revalidate(&self) -> Result<(), String> {
        let file = plain_file(&self.path)?;
        let identity = same_file::Handle::from_file(file.try_clone().map_err(history_error)?)
            .map_err(history_error)?;
        if identity != self.identity {
            return Err("Pi history file changed before launch".into());
        }
        let mut prefix = Vec::new();
        file.take(self.prefix_len)
            .read_to_end(&mut prefix)
            .map_err(history_error)?;
        let hash: [u8; 32] = Sha256::digest(&prefix).into();
        if prefix.len() as u64 != self.prefix_len || hash != self.prefix_hash {
            return Err("Pi history confirmation changed before launch".into());
        }
        Ok(())
    }
}

/// Serializable source receipt retained across shutdown and Off restoration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PiContinuationBinding {
    pub(crate) config_root: PathBuf,
    pub(crate) canonical_path: PathBuf,
    pub(crate) session_id: String,
    pub(crate) native_identity: (u64, u64),
    pub(crate) confirmed_bytes: u64,
    pub(crate) prefix_sha256: String,
}

/// Capture a unique, current-format, complete transcript without provider writes.
pub(crate) fn prepare_continuation_binding(
    directory: &Path,
    native_id: &str,
) -> Result<PiContinuationBinding, String> {
    let history = inspect_owned_history(directory, native_id)?
        .complete
        .ok_or("Pi exact continuation requires unique complete owned history")?;
    history.sdk_open_receipt(native_id)?;
    let config_root = std::fs::canonicalize(directory).map_err(history_error)?;
    let canonical_path = std::fs::canonicalize(&history.path).map_err(history_error)?;
    if canonical_path.parent() != Some(config_root.as_path()) {
        return Err("Pi exact continuation source escaped the owned directory".into());
    }
    let file = plain_file(&canonical_path)?;
    let identity = native_identity(&file).map_err(history_error)?;
    let mut bytes = Vec::new();
    file.take(MAX_PREFIX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(history_error)?;
    if bytes.len() as u64 > MAX_PREFIX_BYTES || !bytes.ends_with(b"\n") {
        return Err("Pi exact continuation requires bounded complete history".into());
    }
    for line in bytes.split_inclusive(|byte| *byte == b'\n') {
        if line.len() as u64 > MAX_RECORD_BYTES {
            return Err("Pi history record exceeds startup inspection limit".into());
        }
        serde_json::from_slice::<serde_json::Value>(line).map_err(history_error)?;
    }
    let binding = PiContinuationBinding {
        config_root,
        canonical_path,
        session_id: native_id.into(),
        native_identity: identity,
        confirmed_bytes: bytes.len() as u64,
        prefix_sha256: hex_digest(&bytes),
    };
    // Bind the original inspection to this receipt, including concurrent replacement.
    history.revalidate()?;
    validate_continuation_prefix(&binding)?;
    Ok(binding)
}

fn hex_digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn validate_continuation_prefix(binding: &PiContinuationBinding) -> Result<(), String> {
    if binding.confirmed_bytes == 0 || binding.confirmed_bytes > MAX_PREFIX_BYTES {
        return Err("Pi exact continuation has an invalid confirmed prefix".into());
    }
    let file = plain_file(&binding.canonical_path)?;
    if native_identity(&file).map_err(history_error)? != binding.native_identity {
        return Err("Pi exact continuation source physical identity changed".into());
    }
    let mut bytes = Vec::new();
    file.take(binding.confirmed_bytes)
        .read_to_end(&mut bytes)
        .map_err(history_error)?;
    if bytes.len() as u64 != binding.confirmed_bytes || hex_digest(&bytes) != binding.prefix_sha256
    {
        return Err("Pi exact continuation confirmed prefix changed".into());
    }
    Ok(())
}

/// Revalidate persisted identity and prefix; valid append-only progress extends it.
pub(crate) fn revalidate_continuation_binding(
    directory: &Path,
    native_id: &str,
    binding: &PiContinuationBinding,
) -> Result<PiContinuationBinding, String> {
    if native_id != binding.session_id
        || std::fs::canonicalize(directory).map_err(history_error)? != binding.config_root
    {
        return Err("Pi exact continuation source context changed".into());
    }
    validate_continuation_prefix(binding)?;
    let updated = prepare_continuation_binding(directory, native_id)?;
    if updated.canonical_path != binding.canonical_path
        || updated.native_identity != binding.native_identity
        || updated.confirmed_bytes < binding.confirmed_bytes
    {
        return Err("Pi exact continuation canonical source changed".into());
    }
    validate_continuation_prefix(binding)?;
    Ok(updated)
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
    // SAFETY: owned handle and the Win32 file information ABI.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle().cast(), information.as_mut_ptr()) }
        == 0
    {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: success initialized the output structure.
    let information = unsafe { information.assume_init() };
    Ok((
        u64::from(information.dwVolumeSerialNumber),
        (u64::from(information.nFileIndexHigh) << 32) | u64::from(information.nFileIndexLow),
    ))
}
#[cfg(not(any(unix, windows)))]
fn native_identity(_file: &File) -> std::io::Result<(u64, u64)> {
    Err(std::io::Error::other(
        "Pi transcript physical identity is unsupported",
    ))
}

#[derive(Default)]
pub(crate) struct OwnedHistory {
    pub(crate) complete: Option<SessionFileBinding>,
    pub(crate) preexisting: HashSet<PathBuf>,
}

fn history_error(error: impl std::fmt::Display) -> String {
    format!("Could not inspect owned Pi history: {error}")
}

fn plain_file(path: &Path) -> Result<File, String> {
    if !std::fs::symlink_metadata(path)
        .map_err(history_error)?
        .file_type()
        .is_file()
    {
        return Err("Pi history is not a plain owned file".into());
    }
    File::open(path).map_err(history_error)
}

fn bounded_line(reader: &mut BufReader<File>, line: &mut String) -> Result<usize, String> {
    line.clear();
    let read = reader
        .by_ref()
        .take(MAX_RECORD_BYTES + 1)
        .read_line(line)
        .map_err(history_error)?;
    if read as u64 > MAX_RECORD_BYTES {
        return Err("Pi history record exceeds startup inspection limit".into());
    }
    Ok(read)
}

/// Positive unique history can recover a paused PendingFresh session. A known
/// header-only file remains pending; unreadable or malformed evidence cannot
/// justify an absence conclusion or a fresh fallback.
pub(crate) fn inspect_owned_history(
    directory: &Path,
    native_id: &str,
) -> Result<OwnedHistory, String> {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(OwnedHistory::default())
        }
        Err(error) => return Err(history_error(error)),
    };
    let mut history = OwnedHistory::default();
    let mut inspected_bytes = 0_u64;
    for (index, entry) in entries.enumerate() {
        if index >= MAX_FILES {
            return Err("Pi history directory exceeds startup inspection limit".into());
        }
        let path = entry.map_err(history_error)?.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("jsonl") {
            continue;
        }
        let file = plain_file(&path)?;
        let identity = same_file::Handle::from_file(file.try_clone().map_err(history_error)?)
            .map_err(history_error)?;
        let mut reader = BufReader::new(file);
        let mut line = String::new();
        let mut prefix_len = bounded_line(&mut reader, &mut line)? as u64;
        inspected_bytes += prefix_len;
        if inspected_bytes > MAX_PREFIX_BYTES {
            return Err("Pi history inspection exceeds startup byte limit".into());
        }
        let header: serde_json::Value =
            serde_json::from_str(line.trim_end()).map_err(history_error)?;
        if header["type"] != "session" || header["id"].as_str().is_none() {
            return Err("Pi history has an invalid session header".into());
        }
        if header["id"].as_str() != Some(native_id) {
            continue;
        }
        history.preexisting.insert(path.clone());
        if !line.ends_with('\n') {
            continue;
        }
        let mut hash = Sha256::new();
        hash.update(line.as_bytes());
        loop {
            let read = bounded_line(&mut reader, &mut line)?;
            if read == 0 {
                break;
            }
            prefix_len += read as u64;
            inspected_bytes += read as u64;
            if inspected_bytes > MAX_PREFIX_BYTES {
                return Err("Pi history prefix exceeds startup inspection limit".into());
            }
            let record: serde_json::Value =
                serde_json::from_str(line.trim_end()).map_err(history_error)?;
            if !line.ends_with('\n') {
                break;
            }
            hash.update(line.as_bytes());
            if record["type"] == "message" && record["message"]["role"] == "assistant" {
                if history.complete.is_some() {
                    return Err("Pi session has ambiguous owned complete histories".into());
                }
                history.complete = Some(SessionFileBinding {
                    path: path.clone(),
                    identity,
                    prefix_len,
                    prefix_hash: hash.finalize().into(),
                });
                break;
            }
        }
    }
    Ok(history)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_history(path: &Path, assistant: bool) {
        let mut text = "{\"type\":\"session\",\"id\":\"owned\"}\n".to_owned();
        if assistant {
            text.push_str("{\"type\":\"message\",\"message\":{\"role\":\"assistant\"}}\n");
        }
        std::fs::write(path, text).unwrap();
    }

    fn durable_fixture() -> (tempfile::TempDir, PathBuf) {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("complete.jsonl");
        std::fs::write(&path, "{\"type\":\"session\",\"version\":3,\"id\":\"owned\"}\n{\"type\":\"message\",\"message\":{\"role\":\"assistant\"}}\n").unwrap();
        (root, path)
    }

    #[test]
    fn durable_receipt_extends_append_and_rejects_replacement_and_prefix_changes() {
        let (root, path) = durable_fixture();
        let receipt = prepare_continuation_binding(root.path(), "owned").unwrap();
        writeln!(
            std::fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .unwrap(),
            "{{\"type\":\"message\",\"message\":{{\"role\":\"user\"}}}}"
        )
        .unwrap();
        let extended = revalidate_continuation_binding(root.path(), "owned", &receipt).unwrap();
        assert!(extended.confirmed_bytes > receipt.confirmed_bytes);
        assert_eq!(extended.native_identity, receipt.native_identity);
        let bytes = std::fs::read(&path).unwrap();
        std::fs::rename(&path, root.path().join("old.txt")).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        assert!(
            revalidate_continuation_binding(root.path(), "owned", &receipt)
                .unwrap_err()
                .contains("physical identity")
        );
        let replacement = prepare_continuation_binding(root.path(), "owned").unwrap();
        let mut changed = bytes.clone();
        let index = changed
            .windows(4)
            .position(|window| window == b"user")
            .unwrap();
        changed[index..index + 4].copy_from_slice(b"tool");
        std::fs::write(&path, changed).unwrap();
        assert!(
            revalidate_continuation_binding(root.path(), "owned", &replacement)
                .unwrap_err()
                .contains("prefix changed")
        );
    }

    #[test]
    fn durable_receipt_rejects_context_ambiguity_truncation_and_incomplete_tail() {
        let (root, path) = durable_fixture();
        let receipt = prepare_continuation_binding(root.path(), "owned").unwrap();
        assert!(revalidate_continuation_binding(root.path(), "wrong", &receipt).is_err());
        let other = tempfile::tempdir().unwrap();
        assert!(revalidate_continuation_binding(other.path(), "owned", &receipt).is_err());
        std::fs::copy(&path, root.path().join("duplicate.jsonl")).unwrap();
        assert!(revalidate_continuation_binding(root.path(), "owned", &receipt).is_err());
        std::fs::remove_file(root.path().join("duplicate.jsonl")).unwrap();
        write!(
            std::fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .unwrap(),
            "{{\"type\":\"message\"}}"
        )
        .unwrap();
        assert!(revalidate_continuation_binding(root.path(), "owned", &receipt).is_err());
        std::fs::write(&path, "{}\n").unwrap();
        assert!(revalidate_continuation_binding(root.path(), "owned", &receipt).is_err());
    }

    #[test]
    fn unique_complete_history_wins_over_retained_header_only_file() {
        let dir = tempfile::tempdir().unwrap();
        let partial = dir.path().join("partial.jsonl");
        let complete = dir.path().join("complete.jsonl");
        write_history(&partial, false);
        write_history(&complete, true);
        let inspected = inspect_owned_history(dir.path(), "owned").unwrap();
        assert!(inspected.preexisting.contains(&partial));
        let binding = inspected.complete.unwrap();
        assert_eq!(binding.path, complete);
        binding.revalidate().unwrap();
        writeln!(
            std::fs::OpenOptions::new()
                .append(true)
                .open(&complete)
                .unwrap(),
            "{{\"type\":\"message\",\"message\":{{\"role\":\"user\"}}}}"
        )
        .unwrap();
        binding.revalidate().unwrap();
        write_history(&complete, false);
        assert!(binding.revalidate().is_err());
    }

    #[test]
    fn missing_header_only_and_unterminated_assistant_do_not_confirm() {
        let dir = tempfile::tempdir().unwrap();
        assert!(inspect_owned_history(&dir.path().join("missing"), "owned")
            .unwrap()
            .complete
            .is_none());
        let path = dir.path().join("partial.jsonl");
        write_history(&path, false);
        assert!(inspect_owned_history(dir.path(), "owned")
            .unwrap()
            .complete
            .is_none());
        write!(
            std::fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .unwrap(),
            "{{\"type\":\"message\",\"message\":{{\"role\":\"assistant\"}}}}"
        )
        .unwrap();
        assert!(inspect_owned_history(dir.path(), "owned")
            .unwrap()
            .complete
            .is_none());
    }

    #[test]
    fn ambiguous_and_malformed_owned_evidence_fail_closed() {
        let dir = tempfile::tempdir().unwrap();
        write_history(&dir.path().join("one.jsonl"), true);
        write_history(&dir.path().join("two.jsonl"), true);
        assert!(inspect_owned_history(dir.path(), "owned").is_err());
        std::fs::write(dir.path().join("two.jsonl"), "broken\n").unwrap();
        assert!(inspect_owned_history(dir.path(), "owned").is_err());
    }

    #[test]
    fn changed_physical_file_and_non_plain_or_oversized_evidence_fail_closed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("complete.jsonl");
        write_history(&path, true);
        let binding = inspect_owned_history(dir.path(), "owned")
            .unwrap()
            .complete
            .unwrap();
        let moved = dir.path().join("previous.txt");
        std::fs::rename(&path, &moved).unwrap();
        write_history(&path, true);
        assert!(binding.revalidate().is_err());
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(inspect_owned_history(dir.path(), "owned").is_err());
        std::fs::remove_dir(&path).unwrap();
        std::fs::write(&path, "x".repeat(MAX_RECORD_BYTES as usize + 1)).unwrap();
        assert!(inspect_owned_history(dir.path(), "owned").is_err());
    }
}
