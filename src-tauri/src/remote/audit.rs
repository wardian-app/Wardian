use crate::remote::models::RemoteAuditRecord;
use std::io::Write;
use std::path::{Path, PathBuf};

const AUDIT_LOG_FILE: &str = "remote-access/audit.jsonl";

pub fn audit_log_path(home: &Path) -> PathBuf {
    home.join(AUDIT_LOG_FILE)
}

pub fn append_audit_record(record: &RemoteAuditRecord) -> Result<(), String> {
    let home = crate::utils::get_wardian_home().ok_or("Could not find Wardian home")?;
    append_audit_record_at(&home, record)
}

pub fn append_audit_record_at(home: &Path, record: &RemoteAuditRecord) -> Result<(), String> {
    let path = audit_log_path(home);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
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
}
