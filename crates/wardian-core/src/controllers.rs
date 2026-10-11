//! Owner-issued capabilities for external controllers, separate from managed agents.
//! Credentials are printed only by explicit issuance; only SHA-256 digests are stored.
use chrono::Utc;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs, io,
    path::{Path, PathBuf},
    str::FromStr,
};
use subtle::ConstantTimeEq;
use uuid::Uuid;

/// Explicit controller permissions. Target operations also require a selected UUID.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ControllerScope {
    #[serde(rename = "agents:read")]
    AgentsRead,
    #[serde(rename = "messages:send")]
    MessagesSend,
    #[serde(rename = "tasks:submit")]
    TasksSubmit,
    #[serde(rename = "agents:wake")]
    AgentsWake,
    #[serde(rename = "agents:interrupt")]
    AgentsInterrupt,
    #[serde(rename = "controller:receive")]
    ControllerReceive,
    #[serde(rename = "controller:ack")]
    ControllerAck,
    #[serde(rename = "controller:wait")]
    ControllerWait,
}
impl FromStr for ControllerScope {
    type Err = io::Error;
    fn from_str(value: &str) -> io::Result<Self> {
        serde_json::from_value(serde_json::Value::String(value.into()))
            .map_err(|_| invalid("unknown controller scope"))
    }
}

/// Public, secret-free attribution and policy of one owner-issued controller.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControllerPrincipal {
    pub id: String,
    pub name: String,
    /// Exact HTTPS protected-resource URI this grant is intended for.
    pub resource: String,
    pub agent_ids: Vec<String>,
    pub scopes: Vec<ControllerScope>,
    pub created_at: i64,
    pub expires_at: Option<i64>,
    pub revoked_at: Option<i64>,
}
impl ControllerPrincipal {
    pub fn mailbox_owner(&self) -> String {
        format!("controller:{}", self.id)
    }
    /// Validate explicit scope and exact target. Roster reads may omit a target;
    /// callers must then filter the roster by `agent_ids` before returning it.
    pub fn authorize(&self, scope: ControllerScope, target: Option<&str>) -> io::Result<()> {
        if !self.scopes.contains(&scope)
            || self.revoked_at.is_some()
            || self
                .expires_at
                .is_some_and(|expiry| expiry <= Utc::now().timestamp())
        {
            return Err(denied());
        }
        let requires_target = matches!(
            scope,
            ControllerScope::MessagesSend
                | ControllerScope::TasksSubmit
                | ControllerScope::AgentsWake
                | ControllerScope::AgentsInterrupt
        );
        if requires_target && target.is_none() {
            return Err(denied());
        }
        if let Some(target) = target {
            canonical_uuid(target)?;
            if !self.agent_ids.iter().any(|id| id == target) {
                return Err(denied());
            }
        }
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct GrantRecord {
    version: u8,
    principal: ControllerPrincipal,
    token_sha256: String,
}

/// Result of explicit local issuance. Debug intentionally never includes bearer material.
pub struct IssuedController {
    pub principal: ControllerPrincipal,
    pub token: String,
}
impl std::fmt::Debug for IssuedController {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IssuedController")
            .field("principal", &self.principal)
            .field("token", &"[REDACTED]")
            .finish()
    }
}

/// Disk-backed authority. Every authentication reloads its record, so revocation
/// and expiry apply to already connected clients on their next operation.
/// Unix storage requires owner-only permission bits; Windows inherits the
/// user-scoped Wardian home ACL, matching the existing listener secret store.
/// This is a remote capability boundary, not isolation from the local OS owner.
#[derive(Debug, Clone)]
pub struct ControllerStore {
    directory: PathBuf,
}
impl ControllerStore {
    pub fn at(home: impl Into<PathBuf>) -> Self {
        Self {
            directory: home.into().join("controllers"),
        }
    }
    pub fn current() -> io::Result<Self> {
        crate::paths::wardian_home()
            .map(Self::at)
            .ok_or_else(|| invalid("Wardian home unavailable"))
    }

    /// Explicit local owner action; never call automatically during connection.
    pub fn create(
        &self,
        name: &str,
        resource: &str,
        agent_ids: Vec<String>,
        scopes: Vec<ControllerScope>,
        expires_at: Option<i64>,
    ) -> io::Result<IssuedController> {
        validate_resource(resource)?;
        if name.trim().is_empty()
            || name.len() > 100
            || name.chars().any(char::is_control)
            || agent_ids.is_empty()
            || agent_ids.len() > 100
            || scopes.is_empty()
            || expires_at.is_some_and(|expiry| expiry <= Utc::now().timestamp())
        {
            return Err(invalid("invalid controller grant"));
        }
        for id in &agent_ids {
            canonical_uuid(id)?;
        }
        let mut agent_ids = agent_ids;
        agent_ids.sort();
        agent_ids.dedup();
        let mut scopes_unique = Vec::new();
        for scope in scopes {
            if !scopes_unique.contains(&scope) {
                scopes_unique.push(scope);
            }
        }
        let principal = ControllerPrincipal {
            id: Uuid::new_v4().to_string(),
            name: name.trim().into(),
            resource: resource.into(),
            agent_ids,
            scopes: scopes_unique,
            created_at: Utc::now().timestamp(),
            expires_at,
            revoked_at: None,
        };
        let mut secret = [0u8; 32];
        rand::rngs::OsRng
            .try_fill_bytes(&mut secret)
            .map_err(|_| io::Error::other("secure randomness unavailable"))?;
        let token = format!("wardian-controller.{}.{}", principal.id, hex(&secret));
        let record = GrantRecord {
            version: 1,
            principal: principal.clone(),
            token_sha256: hex(&Sha256::digest(token.as_bytes())),
        };
        self.prepare_directory()?;
        let path = self.path(&principal.id)?;
        use std::io::Write;
        let mut options = fs::OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(path)?;
        file.write_all(&serde_json::to_vec_pretty(&record).map_err(io::Error::other)?)?;
        file.sync_all()?;
        Ok(IssuedController { principal, token })
    }

    pub fn authenticate_token(&self, token: &str) -> io::Result<ControllerPrincipal> {
        let mut parts = token.split('.');
        if parts.next() != Some("wardian-controller") {
            return Err(denied());
        }
        let id = parts.next().ok_or_else(denied)?;
        let secret = parts.next().ok_or_else(denied)?;
        if parts.next().is_some()
            || secret.len() != 64
            || !secret
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(denied());
        }
        let record = self.read(id).map_err(|_| denied())?;
        let digest = hex(&Sha256::digest(token.as_bytes()));
        if !bool::from(digest.as_bytes().ct_eq(record.token_sha256.as_bytes()))
            || record.principal.revoked_at.is_some()
            || record
                .principal
                .expires_at
                .is_some_and(|expiry| expiry <= Utc::now().timestamp())
        {
            return Err(denied());
        }
        Ok(record.principal)
    }

    /// Authenticate only for the exact owner-selected protected resource URI.
    pub fn authenticate_token_for_resource(
        &self,
        token: &str,
        resource: &str,
    ) -> io::Result<ControllerPrincipal> {
        validate_resource(resource)?;
        let principal = self.authenticate_token(token)?;
        if principal.resource != resource {
            return Err(denied());
        }
        Ok(principal)
    }

    /// Registration survives revocation for durable replies; it conveys no access.
    pub fn registered(&self, mailbox_owner: &str) -> io::Result<bool> {
        let Some(id) = mailbox_owner.strip_prefix("controller:") else {
            return Ok(false);
        };
        if canonical_uuid(id).is_err() {
            return Ok(false);
        }
        match self.read(id) {
            Ok(_) => Ok(true),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e),
        }
    }
    /// Admit unsolicited non-waking information only from a selected worker to
    /// an active controller with receive permission. Correlated durable replies
    /// deliberately use `registered` instead, so revocation does not erase them.
    pub fn authorize_mailbox_sender(
        &self,
        mailbox_owner: &str,
        sender: &str,
    ) -> io::Result<ControllerPrincipal> {
        let id = mailbox_owner
            .strip_prefix("controller:")
            .ok_or_else(denied)?;
        let record = self.read(id)?;
        record
            .principal
            .authorize(ControllerScope::ControllerReceive, Some(sender))?;
        Ok(record.principal)
    }
    pub fn list(&self) -> io::Result<Vec<ControllerPrincipal>> {
        if !self.directory.exists() {
            return Ok(Vec::new());
        }
        check_path(&self.directory, true)?;
        let mut result = Vec::new();
        for entry in fs::read_dir(&self.directory)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "json") {
                let id = path
                    .file_stem()
                    .and_then(|v| v.to_str())
                    .ok_or_else(|| invalid("invalid grant filename"))?;
                result.push(self.read(id)?.principal);
            }
        }
        result.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(result)
    }
    pub fn revoke(&self, id: &str) -> io::Result<ControllerPrincipal> {
        // Shared lock serializes owner modifications, including repeated revocation.
        self.prepare_directory()?;
        let lock_path = self.directory.join(".owner.lock");
        if lock_path.exists() {
            check_path(&lock_path, false)?;
        }
        let mut options = fs::OpenOptions::new();
        options.create(true).truncate(false).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let lock = options.open(lock_path)?;
        fs2::FileExt::lock_exclusive(&lock)?;
        let mut record = self.read(id)?;
        record
            .principal
            .revoked_at
            .get_or_insert_with(|| Utc::now().timestamp());
        let path = self.path(id)?;
        let bytes = serde_json::to_vec_pretty(&record).map_err(io::Error::other)?;
        let staged = crate::atomic_file::stage_bytes_atomic(&path, &bytes)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&staged, fs::Permissions::from_mode(0o600))?;
        }
        crate::atomic_file::replace_staged_atomic_durable(&staged, &path)?;
        Ok(record.principal)
    }
    fn path(&self, id: &str) -> io::Result<PathBuf> {
        canonical_uuid(id)?;
        Ok(self.directory.join(format!("{id}.json")))
    }
    fn prepare_directory(&self) -> io::Result<()> {
        if !self.directory.exists() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                fs::DirBuilder::new().mode(0o700).create(&self.directory)?;
            }
            #[cfg(not(unix))]
            fs::create_dir(&self.directory)?;
        }
        check_path(&self.directory, true)
    }
    fn read(&self, id: &str) -> io::Result<GrantRecord> {
        check_path(&self.directory, true)?;
        let path = self.path(id)?;
        check_path(&path, false)?;
        if fs::metadata(&path)?.len() > 65536 {
            return Err(invalid("oversized grant record"));
        }
        let record: GrantRecord = serde_json::from_slice(&fs::read(path)?)
            .map_err(|_| invalid("invalid controller grant record"))?;
        if record.version != 1
            || record.principal.id != id
            || record.token_sha256.len() != 64
            || !record
                .token_sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            || record.principal.name.trim().is_empty()
            || record.principal.name.len() > 100
            || record.principal.name.chars().any(char::is_control)
            || record.principal.agent_ids.is_empty()
            || record.principal.agent_ids.len() > 100
            || record.principal.scopes.is_empty()
        {
            return Err(invalid("invalid controller grant record"));
        }
        for target in &record.principal.agent_ids {
            canonical_uuid(target)?;
        }
        validate_resource(&record.principal.resource)?;
        Ok(record)
    }
}
fn canonical_uuid(id: &str) -> io::Result<()> {
    if Uuid::parse_str(id).is_ok_and(|uuid| uuid.to_string() == id) {
        Ok(())
    } else {
        Err(invalid("expected canonical UUID"))
    }
}
fn validate_resource(resource: &str) -> io::Result<()> {
    let parsed = url::Url::parse(resource)
        .map_err(|_| invalid("controller resource must be an exact HTTPS URI"))?;
    if parsed.scheme() != "https"
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.fragment().is_some()
        || parsed.query().is_some()
        || parsed.as_str() != resource
        || resource.len() > 2048
    {
        return Err(invalid("controller resource must be a canonical HTTPS URI without credentials, query, or fragment"));
    }
    Ok(())
}
fn check_path(path: &Path, directory: bool) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink()
        || (directory && !metadata.is_dir())
        || (!directory && !metadata.is_file())
    {
        return Err(invalid("controller storage must be a regular owned path"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(denied());
        }
    }
    Ok(())
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}
fn denied() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "controller credential or capability denied",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (tempfile::TempDir, ControllerStore, IssuedController, String) {
        let temp = tempfile::tempdir().unwrap();
        let store = ControllerStore::at(temp.path());
        let agent = Uuid::new_v4().to_string();
        let issued = store
            .create(
                "test controller",
                "https://bridge.example/mcp",
                vec![agent.clone()],
                vec![
                    ControllerScope::MessagesSend,
                    ControllerScope::ControllerReceive,
                ],
                None,
            )
            .unwrap();
        (temp, store, issued, agent)
    }
    #[test]
    fn issuance_authentication_and_exact_capabilities() {
        let (_temp, store, issued, agent) = fixture();
        let principal = store.authenticate_token(&issued.token).unwrap();
        assert!(principal
            .authorize(ControllerScope::MessagesSend, Some(&agent))
            .is_ok());
        assert!(principal
            .authorize(ControllerScope::MessagesSend, None)
            .is_err());
        assert!(principal
            .authorize(
                ControllerScope::MessagesSend,
                Some(&Uuid::new_v4().to_string())
            )
            .is_err());
        assert!(principal
            .authorize(ControllerScope::TasksSubmit, Some(&agent))
            .is_err());
        assert!(principal
            .authorize(ControllerScope::ControllerReceive, None)
            .is_ok());
    }
    #[test]
    fn revocation_denies_access_but_preserves_reply_registration() {
        let (_temp, store, issued, _) = fixture();
        store.revoke(&issued.principal.id).unwrap();
        assert!(store.authenticate_token(&issued.token).is_err());
        assert!(store.registered(&issued.principal.mailbox_owner()).unwrap());
        assert!(!store.registered("controller:../../other").unwrap());
    }
    #[test]
    fn unsolicited_mailbox_sender_requires_active_selected_receive_grant() {
        let (_temp, store, issued, agent) = fixture();
        let owner = issued.principal.mailbox_owner();
        assert!(store.authorize_mailbox_sender(&owner, &agent).is_ok());
        assert!(store
            .authorize_mailbox_sender(&owner, &Uuid::new_v4().to_string())
            .is_err());
        assert!(store
            .authorize_mailbox_sender(&owner, "not-a-uuid")
            .is_err());
        assert!(store
            .authorize_mailbox_sender(&issued.principal.id, &agent)
            .is_err());
        let path = store.path(&issued.principal.id).unwrap();
        let mut record = store.read(&issued.principal.id).unwrap();
        record.principal.scopes = vec![ControllerScope::MessagesSend];
        fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
        assert!(store.authorize_mailbox_sender(&owner, &agent).is_err());
        record.principal.scopes = vec![ControllerScope::ControllerReceive];
        record.principal.expires_at = Some(0);
        fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
        assert!(store.authorize_mailbox_sender(&owner, &agent).is_err());
        record.principal.expires_at = None;
        fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
        store.revoke(&issued.principal.id).unwrap();
        assert!(store.authorize_mailbox_sender(&owner, &agent).is_err());
        assert!(store.registered(&owner).unwrap());
    }
    #[test]
    fn no_secret_persisted_or_debugged() {
        let (_temp, store, issued, _) = fixture();
        let saved = fs::read_to_string(store.path(&issued.principal.id).unwrap()).unwrap();
        assert!(!saved.contains(&issued.token));
        assert!(!format!("{issued:?}").contains(&issued.token));
        assert!(!serde_json::to_string(&store.list().unwrap())
            .unwrap()
            .contains("token"));
    }
    #[test]
    fn malformed_wrong_and_expired_credentials_fail_closed() {
        let (_temp, store, issued, _) = fixture();
        for token in [
            "bad",
            "wardian-controller...",
            &format!("{}.extra", issued.token),
            &format!(
                "wardian-controller.{}.{}",
                issued.principal.id,
                "0".repeat(64)
            ),
        ] {
            assert!(store.authenticate_token(token).is_err());
        }
        let path = store.path(&issued.principal.id).unwrap();
        let mut record = store.read(&issued.principal.id).unwrap();
        record.principal.expires_at = Some(0);
        fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
        assert!(store.authenticate_token(&issued.token).is_err());
        fs::write(&path, b"broken").unwrap();
        assert!(store.authenticate_token(&issued.token).is_err());
    }
    #[test]
    fn grant_input_rejects_names_targets_scopes_and_expired_lifetimes() {
        let temp = tempfile::tempdir().unwrap();
        let store = ControllerStore::at(temp.path());
        for ids in [
            vec![],
            vec!["not-an-agent".into()],
            vec!["../escape".into()],
        ] {
            assert!(store
                .create(
                    "test",
                    "https://bridge.example/mcp",
                    ids,
                    vec![ControllerScope::AgentsRead],
                    None
                )
                .is_err());
        }
        assert!(store
            .create(
                "",
                "https://bridge.example/mcp",
                vec![Uuid::new_v4().to_string()],
                vec![ControllerScope::AgentsRead],
                None
            )
            .is_err());
        assert!(ControllerScope::from_str("*").is_err());
        assert!(store
            .create(
                "test",
                "https://bridge.example/mcp",
                vec![Uuid::new_v4().to_string()],
                vec![],
                None
            )
            .is_err());
        assert!(store
            .create(
                "test",
                "https://bridge.example/mcp",
                vec![Uuid::new_v4().to_string()],
                vec![ControllerScope::AgentsRead],
                Some(0)
            )
            .is_err());
    }
    #[test]
    fn absent_corrupt_and_oversized_records_never_register_or_authenticate() {
        let (_temp, store, issued, _) = fixture();
        assert!(!store
            .registered(&format!("controller:{}", Uuid::new_v4()))
            .unwrap());
        let path = store.path(&issued.principal.id).unwrap();
        fs::write(&path, vec![b' '; 65537]).unwrap();
        assert!(store.registered(&issued.principal.mailbox_owner()).is_err());
        assert!(store.authenticate_token(&issued.token).is_err());
    }
    #[test]
    fn credential_is_bound_to_exact_https_resource() {
        let (_temp, store, issued, _) = fixture();
        assert!(store
            .authenticate_token_for_resource(&issued.token, "https://bridge.example/mcp")
            .is_ok());
        assert!(store
            .authenticate_token_for_resource(&issued.token, "https://other.example/mcp")
            .is_err());
        for resource in [
            "http://bridge.example/mcp",
            "https://user:secret@bridge.example/mcp",
            "https://bridge.example/mcp#fragment",
            "https://bridge.example/mcp?query",
            "https://BRIDGE.example/mcp",
            "https:opaque",
            "https://bridge.example/mcp?",
            "https://bridge.example/mcp#",
        ] {
            assert!(validate_resource(resource).is_err());
        }
    }
    #[test]
    fn grant_and_record_bounds_match_transport_limits() {
        let temp = tempfile::tempdir().unwrap();
        let store = ControllerStore::at(temp.path());
        let agents: Vec<String> = (0..100).map(|_| Uuid::new_v4().to_string()).collect();
        let issued = store
            .create(
                &"é".repeat(50),
                "https://bridge.example/mcp",
                agents.clone(),
                vec![ControllerScope::AgentsRead],
                None,
            )
            .unwrap();
        assert_eq!(
            store
                .authenticate_token(&issued.token)
                .unwrap()
                .agent_ids
                .len(),
            100
        );
        assert!(store
            .create(
                &"é".repeat(51),
                "https://bridge.example/mcp",
                agents.clone(),
                vec![ControllerScope::AgentsRead],
                None
            )
            .is_err());
        assert!(store
            .create(
                &"a".repeat(101),
                "https://bridge.example/mcp",
                agents.clone(),
                vec![ControllerScope::AgentsRead],
                None
            )
            .is_err());
        let mut too_many = agents;
        too_many.push(Uuid::new_v4().to_string());
        assert!(store
            .create(
                "test",
                "https://bridge.example/mcp",
                too_many.clone(),
                vec![ControllerScope::AgentsRead],
                None
            )
            .is_err());
        let mut record = store.read(&issued.principal.id).unwrap();
        record.principal.agent_ids = too_many;
        fs::write(
            store.path(&issued.principal.id).unwrap(),
            serde_json::to_vec(&record).unwrap(),
        )
        .unwrap();
        assert!(store.authenticate_token(&issued.token).is_err());
        record.principal.agent_ids = issued.principal.agent_ids.clone();
        record.principal.name = "a".repeat(101);
        fs::write(
            store.path(&issued.principal.id).unwrap(),
            serde_json::to_vec(&record).unwrap(),
        )
        .unwrap();
        assert!(store.authenticate_token(&issued.token).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn unsafe_permissions_are_rejected() {
        use std::os::unix::fs::PermissionsExt;
        let (_temp, store, issued, _) = fixture();
        fs::set_permissions(
            store.path(&issued.principal.id).unwrap(),
            fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        assert!(store.authenticate_token(&issued.token).is_err());
    }
}
