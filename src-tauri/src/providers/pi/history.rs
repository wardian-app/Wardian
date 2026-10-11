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
