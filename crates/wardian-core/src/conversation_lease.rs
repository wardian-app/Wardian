use fs2::FileExt;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::{
    fs::{File, OpenOptions},
    sync::Mutex,
};

static LEASE_FILE_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

/// Process-local coordination avoids redundant attempts to acquire the OS lock
/// in one runtime. The lock file itself is authoritative across Wardian
/// processes sharing a home.
struct ConversationLeaseFileLock {
    file: File,
}

impl Drop for ConversationLeaseFileLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConversationLease {
    pub agent_id: String,
    pub provider: String,
    pub resume_session: String,
    pub owner_kind: String,
    pub owner_id: String,
    /// Unique for each successful acquisition attempt. A stale process must
    /// never be able to renew or release a later lease that reused the same
    /// human-readable owner id (for example after an automation run resumes).
    #[serde(default)]
    pub acquisition_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_node_id: Option<String>,
    pub mode: String,
    pub started_at: String,
    pub heartbeat_at: String,
    pub expires_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationLeaseOwner {
    pub owner_kind: String,
    pub owner_id: String,
    pub acquisition_id: String,
}

impl ConversationLease {
    pub fn owner(&self) -> ConversationLeaseOwner {
        ConversationLeaseOwner {
            owner_kind: self.owner_kind.clone(),
            owner_id: self.owner_id.clone(),
            acquisition_id: self.acquisition_id.clone(),
        }
    }
}

/// Releases a persisted lease when its owning operation ends or is cancelled.
///
/// The guard deliberately performs a best-effort release in `Drop`: dropping an
/// async request must not leave a provider conversation blocked until expiry.
#[derive(Debug)]
pub struct PersistedConversationLeaseGuard {
    owner: ConversationLeaseOwner,
    released: bool,
}

impl PersistedConversationLeaseGuard {
    pub fn new(lease: &ConversationLease) -> Self {
        Self {
            owner: lease.owner(),
            released: false,
        }
    }

    pub fn owner(&self) -> &ConversationLeaseOwner {
        &self.owner
    }

    pub fn release(&mut self) -> Result<(), String> {
        release_lease_owner_persisted(&self.owner)?;
        self.released = true;
        Ok(())
    }
}

impl Drop for PersistedConversationLeaseGuard {
    fn drop(&mut self) {
        if !self.released {
            let _ = release_lease_owner_persisted(&self.owner);
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConversationLeaseAcquireOutcome {
    Acquired,
    Conflict(Box<ConversationLease>),
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

fn validate_lease(lease: &ConversationLease, index: usize) -> Result<(), String> {
    for (field, value) in [
        ("agent_id", lease.agent_id.as_str()),
        ("provider", lease.provider.as_str()),
        ("owner_kind", lease.owner_kind.as_str()),
        ("owner_id", lease.owner_id.as_str()),
    ] {
        if value.trim().is_empty() {
            return Err(format!("conversation lease {index} has empty {field}"));
        }
    }
    if !matches!(
        lease.mode.as_str(),
        "background_resume" | "background_fresh" | "lifecycle_transition"
    ) {
        return Err(format!("conversation lease {index} has unknown mode"));
    }
    if lease.mode == "background_resume" && lease.resume_session.trim().is_empty() {
        return Err(format!(
            "conversation lease {index} has empty resume_session"
        ));
    }
    for (field, value) in [
        ("started_at", lease.started_at.as_str()),
        ("heartbeat_at", lease.heartbeat_at.as_str()),
        ("expires_at", lease.expires_at.as_str()),
    ] {
        if parse_rfc3339_utc(value).is_none() {
            return Err(format!("conversation lease {index} has invalid {field}"));
        }
    }
    // Legacy leases have no acquisition_id, but still exclude by agent and expiry.
    Ok(())
}

pub fn find_active_conflict<'a>(
    leases: &'a [ConversationLease],
    agent_id: &str,
    resume_session: &str,
    now_rfc3339: &str,
) -> Option<&'a ConversationLease> {
    let now = parse_rfc3339_utc(now_rfc3339).unwrap_or_else(chrono::Utc::now);
    leases
        .iter()
        .find(|lease| lease_conflicts(lease, agent_id, resume_session, now))
}

/// Finds an active provider-execution lease. Lifecycle transition leases use
/// the same exclusion mechanism but must not make an agent appear purple
/// `headless` while it is merely being restarted, cleared, paused, or removed.
pub fn find_active_execution_conflict<'a>(
    leases: &'a [ConversationLease],
    agent_id: &str,
    resume_session: &str,
    now_rfc3339: &str,
) -> Option<&'a ConversationLease> {
    let now = parse_rfc3339_utc(now_rfc3339).unwrap_or_else(chrono::Utc::now);
    leases.iter().find(|lease| {
        is_headless_execution_lease(lease) && lease_conflicts(lease, agent_id, resume_session, now)
    })
}

/// Whether a lease represents a provider process actively using the saved
/// conversation, rather than a short lifecycle transition that excludes such a
/// process from starting.
pub fn is_headless_execution_lease(lease: &ConversationLease) -> bool {
    matches!(
        lease.mode.as_str(),
        "background_resume" | "background_fresh"
    )
}

fn lease_conflicts(
    lease: &ConversationLease,
    agent_id: &str,
    resume_session: &str,
    now: chrono::DateTime<chrono::Utc>,
) -> bool {
    parse_rfc3339_utc(&lease.expires_at).is_some_and(|expires_at| expires_at > now)
        && (lease.agent_id == agent_id
            || (!resume_session.trim().is_empty() && lease.resume_session == resume_session))
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

fn release_lease_owner(leases: &mut Vec<ConversationLease>, owner: &ConversationLeaseOwner) {
    leases.retain(|lease| !lease_matches_owner(lease, owner));
}

fn lease_matches_owner(lease: &ConversationLease, owner: &ConversationLeaseOwner) -> bool {
    lease.owner_kind == owner.owner_kind
        && lease.owner_id == owner.owner_id
        && lease.acquisition_id == owner.acquisition_id
}

pub fn lease_path() -> Option<std::path::PathBuf> {
    crate::paths::wardian_home().map(|home| home.join("runtime").join("conversation-leases.json"))
}

fn lease_lock_path() -> Option<std::path::PathBuf> {
    lease_path().map(|path| path.with_file_name("conversation-leases.lock"))
}

fn acquire_lease_file_lock() -> Result<ConversationLeaseFileLock, String> {
    let path = lease_lock_path()
        .ok_or_else(|| "failed to resolve conversation lease lock path".to_string())?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            format!("failed to create conversation lease lock directory: {error}")
        })?;
    }
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&path)
        .map_err(|error| format!("failed to open conversation lease lock: {error}"))?;
    FileExt::lock_exclusive(&file)
        .map_err(|error| format!("failed to lock conversation leases: {error}"))?;
    Ok(ConversationLeaseFileLock { file })
}

pub fn load_leases() -> Vec<ConversationLease> {
    load_leases_checked().unwrap_or_default()
}

/// Loads persisted leases without treating unreadable or malformed ownership
/// state as an empty lease set. Startup recovery must fail closed on errors.
pub fn load_leases_checked() -> Result<Vec<ConversationLease>, String> {
    let Some(path) = lease_path() else {
        return Err("failed to resolve conversation lease path".to_string());
    };
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("failed to read conversation leases: {error}")),
    };
    let file = serde_json::from_str::<ConversationLeaseFile>(&content)
        .map_err(|error| format!("failed to parse conversation leases: {error}"))?;
    if file.schema != default_schema() {
        return Err(format!(
            "unsupported conversation lease schema {}",
            file.schema
        ));
    }
    for (index, lease) in file.leases.iter().enumerate() {
        validate_lease(lease, index)?;
    }
    Ok(file.leases)
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

pub fn try_acquire_lease(
    lease: ConversationLease,
    now_rfc3339: &str,
) -> Result<ConversationLeaseAcquireOutcome, String> {
    if lease.acquisition_id.trim().is_empty() {
        return Err("conversation lease acquisition id is required".to_string());
    }
    validate_lease(&lease, 0)?;
    let _process_guard = LEASE_FILE_LOCK
        .lock()
        .map_err(|_| "conversation lease lock poisoned".to_string())?;
    let _file_guard = acquire_lease_file_lock()?;
    let mut leases = load_leases_checked()?;
    if let Some(conflict) =
        find_active_conflict(&leases, &lease.agent_id, &lease.resume_session, now_rfc3339)
    {
        return Ok(ConversationLeaseAcquireOutcome::Conflict(Box::new(
            conflict.clone(),
        )));
    }
    add_or_replace_owner(&mut leases, lease);
    save_leases(&leases).map_err(|error| format!("failed to save conversation lease: {error}"))?;
    Ok(ConversationLeaseAcquireOutcome::Acquired)
}

pub fn acquire_lease(lease: ConversationLease, now_rfc3339: &str) -> Result<(), String> {
    let agent_id = lease.agent_id.clone();
    match try_acquire_lease(lease, now_rfc3339)? {
        ConversationLeaseAcquireOutcome::Acquired => Ok(()),
        ConversationLeaseAcquireOutcome::Conflict(conflict) => Err(format!(
            "agent {agent_id} saved conversation is already leased by {} {}",
            conflict.owner_kind, conflict.owner_id
        )),
    }
}

pub fn release_owner_persisted(owner_kind: &str, owner_id: &str) -> Result<(), String> {
    let _process_guard = LEASE_FILE_LOCK
        .lock()
        .map_err(|_| "conversation lease lock poisoned".to_string())?;
    let _file_guard = acquire_lease_file_lock()?;
    let mut leases = load_leases_checked()?;
    release_owner(&mut leases, owner_kind, owner_id);
    save_leases(&leases)
        .map_err(|error| format!("failed to save conversation lease release: {error}"))
}

/// Releases exactly one acquisition attempt. Unlike the legacy owner-id-only
/// cleanup helper, this cannot remove a newer lease that reused the same owner
/// id after the earlier attempt expired.
pub fn release_lease_owner_persisted(owner: &ConversationLeaseOwner) -> Result<(), String> {
    let _process_guard = LEASE_FILE_LOCK
        .lock()
        .map_err(|_| "conversation lease lock poisoned".to_string())?;
    let _file_guard = acquire_lease_file_lock()?;
    let mut leases = load_leases_checked()?;
    release_lease_owner(&mut leases, owner);
    save_leases(&leases)
        .map_err(|error| format!("failed to save conversation lease release: {error}"))
}

/// Extends a currently-owned lease without ever reviving one that has expired.
///
/// Returning `Ok(false)` means the owner no longer has an active lease and the
/// caller must stop using the provider conversation before another operation
/// can overlap it.
pub fn renew_owner_persisted(
    owner_kind: &str,
    owner_id: &str,
    heartbeat_at: &str,
    expires_at: &str,
) -> Result<bool, String> {
    renew_lease_owner_persisted(
        &ConversationLeaseOwner {
            owner_kind: owner_kind.to_string(),
            owner_id: owner_id.to_string(),
            acquisition_id: String::new(),
        },
        heartbeat_at,
        expires_at,
    )
}

/// Renews exactly one acquisition attempt without ever reviving an expired
/// lease. Callers that started provider work must use this fenced form rather
/// than the legacy owner-id-only helper above.
pub fn renew_lease_owner_persisted(
    owner: &ConversationLeaseOwner,
    heartbeat_at: &str,
    expires_at: &str,
) -> Result<bool, String> {
    let now = parse_rfc3339_utc(heartbeat_at)
        .ok_or_else(|| "invalid conversation lease heartbeat_at".to_string())?;
    if parse_rfc3339_utc(expires_at).is_none() {
        return Err("invalid conversation lease expires_at".to_string());
    }
    let _process_guard = LEASE_FILE_LOCK
        .lock()
        .map_err(|_| "conversation lease lock poisoned".to_string())?;
    let _file_guard = acquire_lease_file_lock()?;
    let mut leases = load_leases_checked()?;
    let Some(lease) = leases
        .iter_mut()
        .find(|lease| lease_matches_owner(lease, owner))
    else {
        return Ok(false);
    };
    let active = parse_rfc3339_utc(&lease.expires_at).is_some_and(|expires| expires > now);
    if !active {
        return Ok(false);
    }

    lease.heartbeat_at = heartbeat_at.to_string();
    lease.expires_at = expires_at.to_string();
    save_leases(&leases)
        .map_err(|error| format!("failed to save conversation lease renewal: {error}"))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lease(agent_id: &str, resume_session: &str) -> ConversationLease {
        ConversationLease {
            agent_id: agent_id.to_string(),
            provider: "gemini".to_string(),
            resume_session: resume_session.to_string(),
            owner_kind: "automation_run".to_string(),
            owner_id: "wf/run-1".to_string(),
            acquisition_id: "test-acquisition".to_string(),
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
    fn lifecycle_transition_excludes_execution_without_reporting_headless() {
        let mut lifecycle = lease("agent-1", "resume-1");
        lifecycle.owner_kind = "agent_lifecycle".to_string();
        lifecycle.mode = "lifecycle_transition".to_string();
        let leases = vec![lifecycle];

        assert!(
            find_active_conflict(&leases, "agent-1", "resume-1", "2026-06-01T00:05:00Z").is_some()
        );
        assert!(find_active_execution_conflict(
            &leases,
            "agent-1",
            "resume-1",
            "2026-06-01T00:05:00Z"
        )
        .is_none());
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
    fn release_owner_removes_only_matching_automation_owner() {
        let mut leases = vec![lease("agent-1", "resume-1"), lease("agent-2", "resume-2")];
        leases[1].owner_id = "other/run-2".to_string();

        release_owner(&mut leases, "automation_run", "wf/run-1");

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

    #[test]
    fn try_acquire_lease_reports_conflicting_owner_without_error() {
        let _guard = crate::tests::env_lock();
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("WARDIAN_HOME", dir.path());
        acquire_lease(lease("agent-1", "resume-1"), "2026-06-01T00:05:00Z").expect("first lease");

        let outcome = try_acquire_lease(lease("agent-1", "resume-2"), "2026-06-01T00:05:00Z")
            .expect("conflict is a routing outcome");

        assert!(matches!(
            outcome,
            ConversationLeaseAcquireOutcome::Conflict(ref conflict)
                if conflict.owner_id == "wf/run-1"
        ));
        std::env::remove_var("WARDIAN_HOME");
    }

    #[test]
    fn lease_file_lock_blocks_mutation_from_another_process() {
        const CHILD_ENV: &str = "WARDIAN_TEST_CONVERSATION_LEASE_LOCK_CHILD";
        const TEST_NAME: &str =
            "conversation_lease::tests::lease_file_lock_blocks_mutation_from_another_process";

        if std::env::var_os(CHILD_ENV).is_some() {
            try_acquire_lease(lease("agent-1", "resume-1"), "2026-06-01T00:05:00Z")
                .expect("child should acquire after parent releases the file lock");
            return;
        }

        let _guard = crate::tests::env_lock();
        let dir = tempfile::tempdir().unwrap();
        let previous_home = std::env::var_os("WARDIAN_HOME");
        std::env::set_var("WARDIAN_HOME", dir.path());
        let file_lock = acquire_lease_file_lock().expect("parent file lock");
        let mut child =
            std::process::Command::new(std::env::current_exe().expect("current test executable"))
                .arg("--exact")
                .arg(TEST_NAME)
                .env(CHILD_ENV, "1")
                .env("WARDIAN_HOME", dir.path())
                .spawn()
                .expect("spawn child lease attempt");

        std::thread::sleep(std::time::Duration::from_millis(150));
        assert!(
            child
                .try_wait()
                .expect("poll child lease attempt")
                .is_none(),
            "a second Wardian process acquired the lease while the file lock was held"
        );

        drop(file_lock);
        assert!(
            child
                .wait()
                .expect("wait for child lease attempt")
                .success(),
            "child should complete once the parent releases the file lock"
        );
        assert_eq!(load_leases().len(), 1);
        match previous_home {
            Some(home) => std::env::set_var("WARDIAN_HOME", home),
            None => std::env::remove_var("WARDIAN_HOME"),
        }
    }

    #[test]
    fn fenced_renewal_extends_only_its_active_lease() {
        let _guard = crate::tests::env_lock();
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("WARDIAN_HOME", dir.path());
        let lease = lease("agent-1", "resume-1");
        let owner = lease.owner();
        acquire_lease(lease, "2026-06-01T00:05:00Z").expect("lease");

        assert!(renew_lease_owner_persisted(
            &owner,
            "2026-06-01T00:06:00Z",
            "2026-06-01T00:16:00Z",
        )
        .expect("renewal"));
        assert_eq!(load_leases()[0].expires_at, "2026-06-01T00:16:00Z");

        assert!(!renew_lease_owner_persisted(
            &owner,
            "2026-06-01T00:17:00Z",
            "2026-06-01T00:27:00Z",
        )
        .expect("expired lease is not revived"));
        std::env::remove_var("WARDIAN_HOME");
    }

    #[test]
    fn stale_acquisition_cannot_renew_or_release_a_replacement_lease() {
        let _guard = crate::tests::env_lock();
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("WARDIAN_HOME", dir.path());

        let mut stale = lease("agent-1", "resume-1");
        stale.acquisition_id = "attempt-old".to_string();
        stale.expires_at = "2026-06-01T00:04:00Z".to_string();
        let stale_owner = stale.owner();
        acquire_lease(stale, "2026-06-01T00:05:00Z").expect("expired predecessor lease");

        let mut replacement = lease("agent-1", "resume-1");
        replacement.acquisition_id = "attempt-new".to_string();
        replacement.started_at = "2026-06-01T00:05:00Z".to_string();
        replacement.heartbeat_at = "2026-06-01T00:05:00Z".to_string();
        replacement.expires_at = "2026-06-01T00:15:00Z".to_string();
        let replacement_owner = replacement.owner();
        acquire_lease(replacement, "2026-06-01T00:05:00Z").expect("replacement lease");

        assert!(!renew_lease_owner_persisted(
            &stale_owner,
            "2026-06-01T00:06:00Z",
            "2026-06-01T00:16:00Z",
        )
        .expect("stale renewal should be a clean loss"));
        release_lease_owner_persisted(&stale_owner).expect("stale release is harmless");

        let leases = load_leases();
        assert_eq!(leases.len(), 1);
        assert!(lease_matches_owner(&leases[0], &replacement_owner));
        std::env::remove_var("WARDIAN_HOME");
    }

    #[test]
    fn guard_releases_lease_when_dropped() {
        let _guard = crate::tests::env_lock();
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("WARDIAN_HOME", dir.path());
        let lease = lease("agent-1", "resume-1");
        acquire_lease(lease.clone(), "2026-06-01T00:05:00Z").expect("lease");

        drop(PersistedConversationLeaseGuard::new(&lease));

        assert!(load_leases().is_empty());
        std::env::remove_var("WARDIAN_HOME");
    }

    #[test]
    fn malformed_lease_store_blocks_acquisition_without_overwriting_it() {
        let _guard = crate::tests::env_lock();
        let dir = tempfile::tempdir().unwrap();
        let previous_home = std::env::var_os("WARDIAN_HOME");
        std::env::set_var("WARDIAN_HOME", dir.path());
        let path = lease_path().expect("isolated lease path");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "not json").unwrap();

        assert!(load_leases_checked().is_err());
        assert!(try_acquire_lease(lease("agent-1", "resume-1"), "2026-06-01T00:05:00Z").is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "not json");

        match previous_home {
            Some(home) => std::env::set_var("WARDIAN_HOME", home),
            None => std::env::remove_var("WARDIAN_HOME"),
        }
    }

    #[test]
    fn semantically_invalid_lease_store_blocks_acquisition_without_overwriting_it() {
        let _guard = crate::tests::env_lock();
        let dir = tempfile::tempdir().unwrap();
        let previous_home = std::env::var_os("WARDIAN_HOME");
        std::env::set_var("WARDIAN_HOME", dir.path());
        let path = lease_path().expect("isolated lease path");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();

        for (field, bad_value) in [
            ("agent_id", " "),
            ("provider", ""),
            ("resume_session", ""),
            ("owner_kind", ""),
            ("owner_id", ""),
            ("mode", "unknown"),
            ("started_at", "not-a-date"),
            ("heartbeat_at", "not-a-date"),
            ("expires_at", "not-a-date"),
        ] {
            let mut file = serde_json::to_value(ConversationLeaseFile {
                schema: 1,
                leases: vec![lease("agent-1", "resume-1")],
            })
            .unwrap();
            file["leases"][0][field] = serde_json::Value::String(bad_value.to_string());
            let body = serde_json::to_string(&file).unwrap();
            std::fs::write(&path, &body).unwrap();

            let error = load_leases_checked().expect_err("invalid lease must fail closed");
            assert!(error.contains(field), "{error}");
            assert!(
                try_acquire_lease(lease("agent-1", "resume-1"), "2026-06-01T00:05:00Z").is_err(),
                "invalid {field} must block acquisition"
            );
            assert_eq!(std::fs::read_to_string(&path).unwrap(), body);
        }

        let body = serde_json::to_string(&ConversationLeaseFile {
            schema: 2,
            leases: vec![lease("agent-1", "resume-1")],
        })
        .unwrap();
        std::fs::write(&path, &body).unwrap();
        assert!(load_leases_checked().unwrap_err().contains("schema"));
        assert!(try_acquire_lease(lease("agent-1", "resume-1"), "2026-06-01T00:05:00Z").is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), body);

        match previous_home {
            Some(home) => std::env::set_var("WARDIAN_HOME", home),
            None => std::env::remove_var("WARDIAN_HOME"),
        }
    }

    #[test]
    fn legacy_lease_without_acquisition_id_still_excludes_a_second_owner() {
        let _guard = crate::tests::env_lock();
        let dir = tempfile::tempdir().unwrap();
        let previous_home = std::env::var_os("WARDIAN_HOME");
        std::env::set_var("WARDIAN_HOME", dir.path());
        let path = lease_path().expect("isolated lease path");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut file = serde_json::to_value(ConversationLeaseFile {
            schema: 1,
            leases: vec![lease("agent-1", "resume-1")],
        })
        .unwrap();
        file["leases"][0]
            .as_object_mut()
            .unwrap()
            .remove("acquisition_id");
        std::fs::write(&path, serde_json::to_string(&file).unwrap()).unwrap();

        assert_eq!(load_leases_checked().unwrap()[0].acquisition_id, "");
        assert!(matches!(
            try_acquire_lease(lease("agent-1", "resume-2"), "2026-06-01T00:05:00Z"),
            Ok(ConversationLeaseAcquireOutcome::Conflict(_))
        ));

        match previous_home {
            Some(home) => std::env::set_var("WARDIAN_HOME", home),
            None => std::env::remove_var("WARDIAN_HOME"),
        }
    }
}
