use crate::remote::models::RemoteAuditRecord;
use std::io::Write;
use std::path::{Path, PathBuf};

const AUDIT_LOG_FILE: &str = "remote-access/audit.jsonl";
const AUDIT_LOG_ROTATE_BYTES: u64 = 10 * 1024 * 1024;

pub fn audit_log_path(home: &Path) -> PathBuf {
    home.join(AUDIT_LOG_FILE)
}

fn audit_log_archive_path(home: &Path) -> PathBuf {
    home.join("remote-access/audit.jsonl.1")
}

pub fn append_audit_record(record: &RemoteAuditRecord) -> Result<(), String> {
    let home = crate::utils::get_wardian_home().ok_or("Could not find Wardian home")?;
    append_audit_record_at(&home, record)
}

pub fn append_audit_record_at(home: &Path, record: &RemoteAuditRecord) -> Result<(), String> {
    append_audit_record_at_with_limit(home, record, AUDIT_LOG_ROTATE_BYTES)
}

fn append_audit_record_at_with_limit(
    home: &Path,
    record: &RemoteAuditRecord,
    rotate_bytes: u64,
) -> Result<(), String> {
    let path = audit_log_path(home);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    rotate_audit_log_if_needed(home, rotate_bytes)?;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|error| error.to_string())?;
    let line = serde_json::to_string(record).map_err(|error| error.to_string())?;
    file.write_all(line.as_bytes())
        .map_err(|error| error.to_string())?;
    file.write_all(b"\n").map_err(|error| error.to_string())
}

fn rotate_audit_log_if_needed(home: &Path, rotate_bytes: u64) -> Result<(), String> {
    let path = audit_log_path(home);
    let Ok(metadata) = std::fs::metadata(&path) else {
        return Ok(());
    };
    if metadata.len() < rotate_bytes {
        return Ok(());
    }
    let archive_path = audit_log_archive_path(home);
    if archive_path.exists() {
        std::fs::remove_file(&archive_path).map_err(|error| error.to_string())?;
    }
    std::fs::rename(path, archive_path).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::models::REMOTE_AUDIT_SCHEMA_VERSION;

    #[test]
    fn audit_append_writes_jsonl_under_remote_access() {
        let temp = tempfile::tempdir().expect("temp dir");
        let record = RemoteAuditRecord {
            schema_version: REMOTE_AUDIT_SCHEMA_VERSION,
            event_id: "evt-1".to_string(),
            timestamp: "2026-05-21T00:00:00Z".to_string(),
            request_id: "req-1".to_string(),
            device_id: Some("dev-1".to_string()),
            session_id: Some("sess-1".to_string()),
            origin: Some("https://wardian.tailnet.ts.net".to_string()),
            event_type: "agent_action".to_string(),
            action: "send_prompt".to_string(),
            target_type: Some("agent".to_string()),
            target_id: Some("agent-1".to_string()),
            outcome: "accepted".to_string(),
            error_code: None,
        };

        append_audit_record_at(temp.path(), &record).expect("append audit");
        let log = std::fs::read_to_string(audit_log_path(temp.path())).expect("read audit log");
        let parsed: serde_json::Value = serde_json::from_str(log.trim()).expect("json audit");

        assert_eq!(parsed["schema_version"], REMOTE_AUDIT_SCHEMA_VERSION);
        assert_eq!(parsed["event_type"], "agent_action");
        assert_eq!(parsed["action"], "send_prompt");
    }

    #[test]
    fn audit_append_rotates_existing_log_when_limit_is_exceeded() {
        let temp = tempfile::tempdir().expect("temp dir");
        let record = RemoteAuditRecord {
            schema_version: REMOTE_AUDIT_SCHEMA_VERSION,
            event_id: "evt-2".to_string(),
            timestamp: "2026-05-21T00:00:00Z".to_string(),
            request_id: "req-2".to_string(),
            device_id: None,
            session_id: None,
            origin: Some("https://wardian.tailnet.ts.net".to_string()),
            event_type: "gateway_policy".to_string(),
            action: "request_boundary".to_string(),
            target_type: None,
            target_id: None,
            outcome: "rejected".to_string(),
            error_code: Some("origin_forbidden".to_string()),
        };
        let log_path = audit_log_path(temp.path());
        std::fs::create_dir_all(log_path.parent().expect("audit parent")).expect("mkdir");
        std::fs::write(&log_path, "older-entry\n".repeat(8)).expect("seed audit log");

        append_audit_record_at_with_limit(temp.path(), &record, 16).expect("append audit");

        assert!(audit_log_archive_path(temp.path()).exists());
        let current = std::fs::read_to_string(&log_path).expect("read current audit log");
        let archived =
            std::fs::read_to_string(audit_log_archive_path(temp.path())).expect("read archive");
        assert_eq!(current.lines().count(), 1);
        assert!(current.contains("\"event_id\":\"evt-2\""));
        assert!(archived.contains("older-entry"));
    }
}
