use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::sync::Mutex;

static LEASE_FILE_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConversationLease {
    pub agent_id: String,
    pub provider: String,
    pub resume_session: String,
    pub owner_kind: String,
    pub owner_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_node_id: Option<String>,
    pub mode: String,
    pub started_at: String,
    pub heartbeat_at: String,
    pub expires_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct ConversationLeaseFile {
    #[serde(default = "default_schema")]
    pub schema: u8,
    #[serde(default)]
    pub leases: Vec<ConversationLease>,
}

fn default_schema() -> u8 {
    1
}

pub fn find_active_conflict<'a>(
    leases: &'a [ConversationLease],
    agent_id: &str,
    resume_session: &str,
    now_rfc3339: &str,
) -> Option<&'a ConversationLease> {
    let now = parse_rfc3339_utc(now_rfc3339).unwrap_or_else(chrono::Utc::now);
    leases.iter().find(|lease| {
        parse_rfc3339_utc(&lease.expires_at).is_some_and(|expires_at| expires_at > now)
            && (lease.agent_id == agent_id
                || (!resume_session.trim().is_empty() && lease.resume_session == resume_session))
    })
}

fn parse_rfc3339_utc(value: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|datetime| datetime.with_timezone(&chrono::Utc))
}

pub fn add_or_replace_owner(leases: &mut Vec<ConversationLease>, lease: ConversationLease) {
    release_owner(leases, &lease.owner_kind, &lease.owner_id);
    leases.push(lease);
}

pub fn release_owner(leases: &mut Vec<ConversationLease>, owner_kind: &str, owner_id: &str) {
    leases.retain(|lease| lease.owner_kind != owner_kind || lease.owner_id != owner_id);
}

pub fn lease_path() -> Option<std::path::PathBuf> {
    crate::paths::wardian_home().map(|home| home.join("runtime").join("conversation-leases.json"))
}

pub fn load_leases() -> Vec<ConversationLease> {
    let Some(path) = lease_path() else {
        return Vec::new();
    };
    let Ok(content) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    serde_json::from_str::<ConversationLeaseFile>(&content)
        .map(|file| file.leases)
        .unwrap_or_default()
}

pub fn save_leases(leases: &[ConversationLease]) -> std::io::Result<()> {
    let path = lease_path()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "no wardian home"))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = ConversationLeaseFile {
        schema: 1,
        leases: leases.to_vec(),
    };
    let body = serde_json::to_string_pretty(&file)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

pub fn acquire_lease(lease: ConversationLease, now_rfc3339: &str) -> Result<(), String> {
    let _guard = LEASE_FILE_LOCK
        .lock()
        .map_err(|_| "conversation lease lock poisoned".to_string())?;
    let mut leases = load_leases();
    if let Some(conflict) =
        find_active_conflict(&leases, &lease.agent_id, &lease.resume_session, now_rfc3339)
    {
        return Err(format!(
            "agent {} saved conversation is already leased by {} {}",
            lease.agent_id, conflict.owner_kind, conflict.owner_id
        ));
    }
    add_or_replace_owner(&mut leases, lease);
    save_leases(&leases).map_err(|error| format!("failed to save conversation lease: {error}"))
}

pub fn release_owner_persisted(owner_kind: &str, owner_id: &str) -> Result<(), String> {
    let _guard = LEASE_FILE_LOCK
        .lock()
        .map_err(|_| "conversation lease lock poisoned".to_string())?;
    let mut leases = load_leases();
    release_owner(&mut leases, owner_kind, owner_id);
    save_leases(&leases)
        .map_err(|error| format!("failed to save conversation lease release: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lease(agent_id: &str, resume_session: &str) -> ConversationLease {
        ConversationLease {
            agent_id: agent_id.to_string(),
            provider: "gemini".to_string(),
            resume_session: resume_session.to_string(),
            owner_kind: "workflow_run".to_string(),
            owner_id: "wf/run-1".to_string(),
            owner_node_id: Some("agent-1".to_string()),
            mode: "background_resume".to_string(),
            started_at: "2026-06-01T00:00:00Z".to_string(),
            heartbeat_at: "2026-06-01T00:00:00Z".to_string(),
            expires_at: "2026-06-01T00:10:00Z".to_string(),
        }
    }

    #[test]
    fn active_lease_conflicts_by_agent_id() {
        let leases = vec![lease("agent-1", "resume-1")];
        let conflict = find_active_conflict(&leases, "agent-1", "resume-2", "2026-06-01T00:05:00Z");
        assert!(conflict.is_some());
    }

    #[test]
    fn active_lease_conflicts_by_resume_session() {
        let leases = vec![lease("agent-1", "resume-1")];
        let conflict = find_active_conflict(&leases, "agent-2", "resume-1", "2026-06-01T00:05:00Z");
        assert!(conflict.is_some());
    }

    #[test]
    fn expired_lease_does_not_conflict() {
        let leases = vec![lease("agent-1", "resume-1")];
        let conflict = find_active_conflict(&leases, "agent-1", "resume-1", "2026-06-01T00:11:00Z");
        assert!(conflict.is_none());
    }

    #[test]
    fn active_lease_conflict_uses_timestamp_order_not_string_order() {
        let mut lease = lease("agent-1", "resume-1");
        lease.expires_at = "2026-06-01T00:10:00+00:00".to_string();
        let leases = vec![lease];

        let conflict = find_active_conflict(&leases, "agent-1", "resume-1", "2026-06-01T00:05:00Z");

        assert!(conflict.is_some());
    }

    #[test]
    fn add_or_replace_owner_records_background_resume_lease() {
        let mut leases = Vec::new();
        let lease = lease("agent-1", "resume-1");

        add_or_replace_owner(&mut leases, lease.clone());

        assert_eq!(leases, vec![lease]);
    }

    #[test]
    fn release_owner_removes_only_matching_workflow_owner() {
        let mut leases = vec![lease("agent-1", "resume-1"), lease("agent-2", "resume-2")];
        leases[1].owner_id = "other/run-2".to_string();

        release_owner(&mut leases, "workflow_run", "wf/run-1");

        assert_eq!(leases.len(), 1);
        assert_eq!(leases[0].agent_id, "agent-2");
    }

    #[test]
    fn acquire_lease_rejects_existing_active_owner() {
        let _guard = crate::tests::env_lock();
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("WARDIAN_HOME", dir.path());
        let first = lease("agent-1", "resume-1");
        acquire_lease(first, "2026-06-01T00:05:00Z").expect("first lease");

        let err = acquire_lease(lease("agent-1", "resume-2"), "2026-06-01T00:05:00Z")
            .expect_err("second lease should conflict");

        assert!(err.contains("already leased"));
        std::env::remove_var("WARDIAN_HOME");
    }
}
