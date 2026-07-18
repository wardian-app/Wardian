use super::models::{
    ArtifactIndexEntryV1, ArtifactIndexV1, ArtifactManifestV1, ArtifactOriginV1,
    ArtifactReviewStatus, ArtifactVersionV1, ARTIFACT_SCHEMA_V1,
};
use crate::{
    atomic_file::{cleanup_atomic_temps, write_bytes_atomic_durable, write_json_atomic},
    paths::is_safe_path_component,
};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ArtifactStoreError {
    #[error("artifact storage I/O failed: {0}")]
    Io(#[from] io::Error),
    #[error("artifact storage JSON is invalid: {0}")]
    Json(#[from] serde_json::Error),
    #[error("unsupported artifact schema {0}")]
    UnsupportedSchema(u8),
    #[error("invalid artifact reference: {0}")]
    InvalidReference(String),
    #[error("artifact not found: {0}")]
    ArtifactNotFound(String),
    #[error("artifact version not found: {artifact_id}/{version_id}")]
    VersionNotFound {
        artifact_id: String,
        version_id: String,
    },
    #[error("artifact store lock is poisoned")]
    Poisoned,
    #[error("artifact thread has no versions: {0}")]
    EmptyThread(String),
    #[error("artifact blob hash mismatch for {0}")]
    BlobHashMismatch(String),
}

#[derive(Debug, Clone)]
pub struct CreateArtifactThread {
    pub canonical_path: String,
    pub title: String,
    pub description: Option<String>,
    pub origin: ArtifactOriginV1,
    pub bytes: Vec<u8>,
    pub addressed_comment_ids: Vec<String>,
    pub presented_at_ms: u64,
}

#[derive(Debug, Clone)]
pub struct AppendArtifactVersion {
    pub artifact_id: String,
    pub bytes: Vec<u8>,
    pub addressed_comment_ids: Vec<String>,
    pub presented_at_ms: u64,
    pub title: Option<String>,
    pub description: Option<Option<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredArtifactVersion {
    pub manifest: ArtifactManifestV1,
    pub version: ArtifactVersionV1,
}

#[derive(Debug, Clone)]
pub struct ArtifactStore {
    root: PathBuf,
    gate: Arc<Mutex<()>>,
}

impl ArtifactStore {
    /// Opens `<wardian-home>/artifacts` (or an equivalent isolated test root).
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, ArtifactStoreError> {
        let store = Self {
            root: root.into(),
            gate: Arc::new(Mutex::new(())),
        };
        store.initialize()?;
        Ok(store)
    }

    pub fn create_thread(
        &self,
        input: CreateArtifactThread,
    ) -> Result<StoredArtifactVersion, ArtifactStoreError> {
        let _guard = self.lock()?;
        validate_nonempty("canonical_path", &input.canonical_path)?;
        validate_nonempty("title", &input.title)?;
        validate_origin(&input.origin)?;

        let artifact_id = uuid::Uuid::new_v4().to_string();
        let version = new_version(
            1,
            &input.bytes,
            input.addressed_comment_ids,
            input.presented_at_ms,
        );
        self.persist_blob(&version.content_hash, &input.bytes)?;
        let manifest = ArtifactManifestV1 {
            schema: ARTIFACT_SCHEMA_V1,
            artifact_id: artifact_id.clone(),
            canonical_path: input.canonical_path,
            title: input.title,
            description: input.description,
            origin: input.origin,
            status: ArtifactReviewStatus::Presented,
            active: true,
            created_at_ms: input.presented_at_ms,
            updated_at_ms: input.presented_at_ms,
            versions: vec![version.clone()],
            latest_review_id: None,
        };
        self.persist_manifest(&manifest)?;
        self.upsert_index(&manifest, true)?;
        Ok(StoredArtifactVersion { manifest, version })
    }

    pub fn append_version(
        &self,
        input: AppendArtifactVersion,
    ) -> Result<StoredArtifactVersion, ArtifactStoreError> {
        validate_id("artifact_id", &input.artifact_id)?;
        let _guard = self.lock()?;
        let mut manifest = self.load_manifest_unlocked(&input.artifact_id)?;
        let sequence = manifest
            .versions
            .last()
            .map_or(1, |version| version.sequence.saturating_add(1));
        let version = new_version(
            sequence,
            &input.bytes,
            input.addressed_comment_ids,
            input.presented_at_ms,
        );
        self.persist_blob(&version.content_hash, &input.bytes)?;
        manifest.status = match manifest.status {
            ArtifactReviewStatus::FeedbackSent => ArtifactReviewStatus::Updated,
            status => status,
        };
        manifest.active = true;
        manifest.updated_at_ms = input.presented_at_ms;
        if let Some(title) = input.title {
            validate_nonempty("title", &title)?;
            manifest.title = title;
        }
        if let Some(description) = input.description {
            manifest.description = description;
        }
        manifest.versions.push(version.clone());
        self.persist_manifest(&manifest)?;
        self.upsert_index(&manifest, true)?;
        Ok(StoredArtifactVersion { manifest, version })
    }

    pub fn load_manifest(
        &self,
        artifact_id: &str,
    ) -> Result<ArtifactManifestV1, ArtifactStoreError> {
        validate_id("artifact_id", artifact_id)?;
        let _guard = self.lock()?;
        self.load_manifest_unlocked(artifact_id)
    }

    pub fn load_version(
        &self,
        artifact_id: &str,
        version_id: Option<&str>,
    ) -> Result<StoredArtifactVersion, ArtifactStoreError> {
        validate_id("artifact_id", artifact_id)?;
        if let Some(version_id) = version_id {
            validate_id("version_id", version_id)?;
        }
        let _guard = self.lock()?;
        let manifest = self.load_manifest_unlocked(artifact_id)?;
        let version = match version_id {
            Some(version_id) => manifest
                .versions
                .iter()
                .find(|version| version.version_id == version_id),
            None => manifest.versions.last(),
        }
        .cloned()
        .ok_or_else(|| match version_id {
            Some(version_id) => ArtifactStoreError::VersionNotFound {
                artifact_id: artifact_id.to_string(),
                version_id: version_id.to_string(),
            },
            None => ArtifactStoreError::EmptyThread(artifact_id.to_string()),
        })?;
        Ok(StoredArtifactVersion { manifest, version })
    }

    pub fn read_version_bytes(
        &self,
        artifact_id: &str,
        version_id: Option<&str>,
    ) -> Result<Vec<u8>, ArtifactStoreError> {
        let stored = self.load_version(artifact_id, version_id)?;
        let bytes = fs::read(self.blob_path(&stored.version.content_hash)?)?;
        let actual = content_hash(&bytes);
        if actual != stored.version.content_hash {
            return Err(ArtifactStoreError::BlobHashMismatch(
                stored.version.content_hash,
            ));
        }
        Ok(bytes)
    }

    pub fn list_recent(
        &self,
        limit: usize,
    ) -> Result<Vec<ArtifactIndexEntryV1>, ArtifactStoreError> {
        let _guard = self.lock()?;
        let mut index = self.load_or_rebuild_index()?;
        index.entries.sort_by(|left, right| {
            right
                .updated_at_ms
                .cmp(&left.updated_at_ms)
                .then_with(|| left.artifact_id.cmp(&right.artifact_id))
        });
        index.entries.truncate(limit);
        Ok(index.entries)
    }

    pub fn find_active_thread(
        &self,
        origin_session_id: &str,
        canonical_path: &str,
    ) -> Result<Option<ArtifactManifestV1>, ArtifactStoreError> {
        let _guard = self.lock()?;
        let index = self.load_or_rebuild_index()?;
        let Some(entry) = index.entries.into_iter().find(|entry| {
            entry.active
                && entry.origin_session_id == origin_session_id
                && entry.canonical_path == canonical_path
        }) else {
            return Ok(None);
        };
        self.load_manifest_unlocked(&entry.artifact_id).map(Some)
    }

    pub fn set_attention(
        &self,
        artifact_id: &str,
        attention: bool,
    ) -> Result<ArtifactIndexEntryV1, ArtifactStoreError> {
        validate_id("artifact_id", artifact_id)?;
        let _guard = self.lock()?;
        let manifest = self.load_manifest_unlocked(artifact_id)?;
        self.upsert_index(&manifest, attention)
    }

    pub fn close_thread(
        &self,
        artifact_id: &str,
    ) -> Result<ArtifactManifestV1, ArtifactStoreError> {
        validate_id("artifact_id", artifact_id)?;
        let _guard = self.lock()?;
        let mut manifest = self.load_manifest_unlocked(artifact_id)?;
        manifest.active = false;
        self.persist_manifest(&manifest)?;
        let attention = self
            .load_or_rebuild_index()?
            .entries
            .iter()
            .find(|entry| entry.artifact_id == artifact_id)
            .is_some_and(|entry| entry.attention);
        self.upsert_index(&manifest, attention)?;
        Ok(manifest)
    }

    pub fn blob_count(&self) -> Result<usize, ArtifactStoreError> {
        let _guard = self.lock()?;
        let entries = fs::read_dir(self.blobs_dir())?;
        Ok(entries
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
            .count())
    }

    fn initialize(&self) -> Result<(), ArtifactStoreError> {
        fs::create_dir_all(self.threads_dir())?;
        fs::create_dir_all(self.root.join("reviews"))?;
        fs::create_dir_all(self.root.join("checkpoints"))?;
        fs::create_dir_all(self.blobs_dir())?;
        cleanup_atomic_temps(&self.index_path())?;
        let _guard = self.lock()?;
        let _ = self.load_or_rebuild_index()?;
        Ok(())
    }

    fn lock(&self) -> Result<MutexGuard<'_, ()>, ArtifactStoreError> {
        self.gate.lock().map_err(|_| ArtifactStoreError::Poisoned)
    }

    fn load_manifest_unlocked(
        &self,
        artifact_id: &str,
    ) -> Result<ArtifactManifestV1, ArtifactStoreError> {
        let path = self.manifest_path(artifact_id)?;
        cleanup_atomic_temps(&path)?;
        let primary = read_manifest_file(&path);
        let manifest = match primary {
            Ok(manifest) => manifest,
            Err(primary_error) => {
                let backup_path = manifest_backup_path(&path);
                match read_manifest_file(&backup_path) {
                    Ok(manifest) => {
                        write_json_atomic(&path, &manifest)?;
                        manifest
                    }
                    Err(_) if matches!(primary_error, ArtifactStoreError::Io(ref error) if error.kind() == io::ErrorKind::NotFound) =>
                    {
                        return Err(ArtifactStoreError::ArtifactNotFound(
                            artifact_id.to_string(),
                        ));
                    }
                    Err(_) => return Err(primary_error),
                }
            }
        };
        validate_manifest(&manifest)?;
        if manifest.artifact_id != artifact_id {
            return Err(ArtifactStoreError::InvalidReference(format!(
                "manifest {} does not match directory {artifact_id}",
                manifest.artifact_id
            )));
        }
        Ok(manifest)
    }

    fn persist_manifest(&self, manifest: &ArtifactManifestV1) -> Result<(), ArtifactStoreError> {
        validate_manifest(manifest)?;
        let path = self.manifest_path(&manifest.artifact_id)?;
        fs::create_dir_all(path.parent().expect("manifest parent"))?;
        if let Ok(existing) = fs::read(&path) {
            let backup = manifest_backup_path(&path);
            write_bytes_atomic_durable(&backup, &existing)?;
        }
        write_json_atomic(&path, manifest)?;
        Ok(())
    }

    fn persist_blob(&self, hash: &str, bytes: &[u8]) -> Result<(), ArtifactStoreError> {
        if content_hash(bytes) != hash {
            return Err(ArtifactStoreError::BlobHashMismatch(hash.to_string()));
        }
        let path = self.blob_path(hash)?;
        match OpenOptions::new().create_new(true).write(true).open(&path) {
            Ok(mut file) => {
                file.write_all(bytes)?;
                file.sync_all()?;
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                let existing = fs::read(&path)?;
                if content_hash(&existing) != hash {
                    return Err(ArtifactStoreError::BlobHashMismatch(hash.to_string()));
                }
            }
            Err(error) => return Err(error.into()),
        }
        Ok(())
    }

    fn upsert_index(
        &self,
        manifest: &ArtifactManifestV1,
        attention: bool,
    ) -> Result<ArtifactIndexEntryV1, ArtifactStoreError> {
        let mut index = self.load_or_rebuild_index()?;
        let entry = index_entry(manifest, attention)?;
        if let Some(existing) = index
            .entries
            .iter_mut()
            .find(|existing| existing.artifact_id == manifest.artifact_id)
        {
            *existing = entry.clone();
        } else {
            index.entries.push(entry.clone());
        }
        write_json_atomic(&self.index_path(), &index)?;
        Ok(entry)
    }

    fn load_or_rebuild_index(&self) -> Result<ArtifactIndexV1, ArtifactStoreError> {
        let path = self.index_path();
        cleanup_atomic_temps(&path)?;
        if let Ok(bytes) = fs::read(&path) {
            if let Ok(index) = serde_json::from_slice::<ArtifactIndexV1>(&bytes) {
                if validate_index(&index).is_ok() && self.index_matches_manifests(&index)? {
                    return Ok(index);
                }
            }
        }
        let index = self.rebuild_index()?;
        write_json_atomic(&path, &index)?;
        Ok(index)
    }

    fn index_matches_manifests(&self, index: &ArtifactIndexV1) -> Result<bool, ArtifactStoreError> {
        let thread_count = fs::read_dir(self.threads_dir())?
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
            .count();
        if thread_count != index.entries.len() {
            return Ok(false);
        }
        for entry in &index.entries {
            let manifest = match self.load_manifest_unlocked(&entry.artifact_id) {
                Ok(manifest) => manifest,
                Err(_) => return Ok(false),
            };
            if index_entry(&manifest, entry.attention)? != *entry {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn rebuild_index(&self) -> Result<ArtifactIndexV1, ArtifactStoreError> {
        let mut entries = Vec::new();
        for directory in fs::read_dir(self.threads_dir())? {
            let directory = directory?;
            if !directory.file_type()?.is_dir() {
                continue;
            }
            let Some(artifact_id) = directory.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            validate_id("artifact_id", &artifact_id)?;
            let manifest = self.load_manifest_unlocked(&artifact_id)?;
            entries.push(index_entry(&manifest, false)?);
        }
        entries.sort_by(|left, right| left.artifact_id.cmp(&right.artifact_id));
        Ok(ArtifactIndexV1 {
            schema: ARTIFACT_SCHEMA_V1,
            entries,
        })
    }

    fn index_path(&self) -> PathBuf {
        self.root.join("index.json")
    }

    fn threads_dir(&self) -> PathBuf {
        self.root.join("threads")
    }

    fn blobs_dir(&self) -> PathBuf {
        self.root.join("blobs")
    }

    fn manifest_path(&self, artifact_id: &str) -> Result<PathBuf, ArtifactStoreError> {
        validate_id("artifact_id", artifact_id)?;
        Ok(self.threads_dir().join(artifact_id).join("manifest.json"))
    }

    fn blob_path(&self, hash: &str) -> Result<PathBuf, ArtifactStoreError> {
        let digest = hash
            .strip_prefix("sha256:")
            .filter(|digest| {
                digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
            })
            .ok_or_else(|| ArtifactStoreError::InvalidReference(format!("content_hash {hash}")))?;
        Ok(self.blobs_dir().join(digest))
    }
}

fn new_version(
    sequence: u64,
    bytes: &[u8],
    addressed_comment_ids: Vec<String>,
    presented_at_ms: u64,
) -> ArtifactVersionV1 {
    ArtifactVersionV1 {
        version_id: uuid::Uuid::new_v4().to_string(),
        sequence,
        content_hash: content_hash(bytes),
        size_bytes: bytes.len() as u64,
        presented_at_ms,
        addressed_comment_ids,
    }
}

fn content_hash(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn manifest_backup_path(path: &Path) -> PathBuf {
    path.with_file_name("manifest.json.bak")
}

fn read_manifest_file(path: &Path) -> Result<ArtifactManifestV1, ArtifactStoreError> {
    let bytes = fs::read(path)?;
    let manifest = serde_json::from_slice(&bytes)?;
    validate_manifest(&manifest)?;
    Ok(manifest)
}

fn validate_manifest(manifest: &ArtifactManifestV1) -> Result<(), ArtifactStoreError> {
    if manifest.schema != ARTIFACT_SCHEMA_V1 {
        return Err(ArtifactStoreError::UnsupportedSchema(manifest.schema));
    }
    validate_id("artifact_id", &manifest.artifact_id)?;
    validate_nonempty("canonical_path", &manifest.canonical_path)?;
    validate_nonempty("title", &manifest.title)?;
    validate_origin(&manifest.origin)?;
    if manifest.versions.is_empty() {
        return Err(ArtifactStoreError::EmptyThread(
            manifest.artifact_id.clone(),
        ));
    }
    for (index, version) in manifest.versions.iter().enumerate() {
        validate_id("version_id", &version.version_id)?;
        if version.sequence != index as u64 + 1 {
            return Err(ArtifactStoreError::InvalidReference(format!(
                "version sequence {} at index {index}",
                version.sequence
            )));
        }
        validate_hash(&version.content_hash)?;
        for comment_id in &version.addressed_comment_ids {
            validate_id("comment_id", comment_id)?;
        }
    }
    if let Some(review_id) = &manifest.latest_review_id {
        validate_id("review_id", review_id)?;
    }
    Ok(())
}

fn validate_index(index: &ArtifactIndexV1) -> Result<(), ArtifactStoreError> {
    if index.schema != ARTIFACT_SCHEMA_V1 {
        return Err(ArtifactStoreError::UnsupportedSchema(index.schema));
    }
    for entry in &index.entries {
        validate_id("artifact_id", &entry.artifact_id)?;
        validate_id("version_id", &entry.latest_version_id)?;
        validate_nonempty("canonical_path", &entry.canonical_path)?;
        validate_nonempty("title", &entry.title)?;
        validate_nonempty("origin_session_id", &entry.origin_session_id)?;
    }
    Ok(())
}

fn validate_origin(origin: &ArtifactOriginV1) -> Result<(), ArtifactStoreError> {
    validate_nonempty("origin.session_id", &origin.session_id)?;
    validate_nonempty("origin.agent_id", &origin.agent_id)?;
    validate_nonempty("origin.agent_name", &origin.agent_name)?;
    validate_nonempty("origin.provider", &origin.provider)
}

fn validate_id(kind: &str, value: &str) -> Result<(), ArtifactStoreError> {
    if !is_safe_path_component(value) || value.contains('/') || value.contains('\\') {
        return Err(ArtifactStoreError::InvalidReference(format!(
            "{kind} {value:?}"
        )));
    }
    Ok(())
}

fn validate_hash(value: &str) -> Result<(), ArtifactStoreError> {
    let digest = value
        .strip_prefix("sha256:")
        .filter(|digest| digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit()));
    if digest.is_none() {
        return Err(ArtifactStoreError::InvalidReference(format!(
            "content_hash {value:?}"
        )));
    }
    Ok(())
}

fn validate_nonempty(kind: &str, value: &str) -> Result<(), ArtifactStoreError> {
    if value.trim().is_empty() {
        return Err(ArtifactStoreError::InvalidReference(format!(
            "{kind} is empty"
        )));
    }
    Ok(())
}

fn index_entry(
    manifest: &ArtifactManifestV1,
    attention: bool,
) -> Result<ArtifactIndexEntryV1, ArtifactStoreError> {
    let latest = manifest
        .versions
        .last()
        .ok_or_else(|| ArtifactStoreError::EmptyThread(manifest.artifact_id.clone()))?;
    Ok(ArtifactIndexEntryV1 {
        artifact_id: manifest.artifact_id.clone(),
        canonical_path: manifest.canonical_path.clone(),
        title: manifest.title.clone(),
        origin_session_id: manifest.origin.session_id.clone(),
        origin_agent_name: manifest.origin.agent_name.clone(),
        latest_version_id: latest.version_id.clone(),
        status: manifest.status,
        active: manifest.active,
        attention,
        updated_at_ms: manifest.updated_at_ms,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn origin(session_id: &str) -> ArtifactOriginV1 {
        ArtifactOriginV1 {
            session_id: session_id.into(),
            agent_id: format!("agent-{session_id}"),
            agent_name: "Writer".into(),
            provider: "codex".into(),
        }
    }

    fn create_input(session_id: &str, path: &str, bytes: &[u8], now: u64) -> CreateArtifactThread {
        CreateArtifactThread {
            canonical_path: path.into(),
            title: "Report".into(),
            description: None,
            origin: origin(session_id),
            bytes: bytes.to_vec(),
            addressed_comment_ids: Vec::new(),
            presented_at_ms: now,
        }
    }

    #[test]
    fn identical_presentations_share_one_blob_but_create_versions() {
        let temp = tempfile::tempdir().expect("temp dir");
        let store = ArtifactStore::open(temp.path().join("artifacts")).expect("store");
        let first = store
            .create_thread(create_input(
                "session-a",
                "/workspace/report.md",
                b"same bytes",
                1,
            ))
            .expect("create");
        let second = store
            .append_version(AppendArtifactVersion {
                artifact_id: first.manifest.artifact_id.clone(),
                bytes: b"same bytes".to_vec(),
                addressed_comment_ids: Vec::new(),
                presented_at_ms: 2,
                title: None,
                description: None,
            })
            .expect("append");

        assert_eq!(first.version.content_hash, second.version.content_hash);
        assert_ne!(first.version.version_id, second.version.version_id);
        assert_eq!(store.blob_count().expect("blob count"), 1);
    }

    #[test]
    fn stale_index_is_rebuilt_from_authoritative_manifests() {
        let temp = tempfile::tempdir().expect("temp dir");
        let root = temp.path().join("artifacts");
        let store = ArtifactStore::open(&root).expect("store");
        let created = store
            .create_thread(create_input("session-a", "/workspace/report.md", b"v1", 1))
            .expect("create");
        fs::write(root.join("index.json"), br#"{"schema":1,"entries":[]}"#).expect("stale index");

        let reopened = ArtifactStore::open(&root).expect("reopen");
        let entries = reopened.list_recent(20).expect("recent");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].artifact_id, created.manifest.artifact_id);
    }

    #[test]
    fn corrupt_primary_manifest_recovers_last_known_good() {
        let temp = tempfile::tempdir().expect("temp dir");
        let root = temp.path().join("artifacts");
        let store = ArtifactStore::open(&root).expect("store");
        let first = store
            .create_thread(create_input("session-a", "/workspace/report.md", b"v1", 1))
            .expect("create");
        store
            .append_version(AppendArtifactVersion {
                artifact_id: first.manifest.artifact_id.clone(),
                bytes: b"v2".to_vec(),
                addressed_comment_ids: Vec::new(),
                presented_at_ms: 2,
                title: None,
                description: None,
            })
            .expect("append");
        let manifest_path = root
            .join("threads")
            .join(&first.manifest.artifact_id)
            .join("manifest.json");
        fs::write(&manifest_path, b"{broken").expect("corrupt primary");

        let recovered = store
            .load_manifest(&first.manifest.artifact_id)
            .expect("recover backup");
        assert_eq!(recovered.versions.len(), 1);
        assert_eq!(recovered.versions[0].version_id, first.version.version_id);
    }

    #[test]
    fn active_lookup_and_close_preserve_history() {
        let temp = tempfile::tempdir().expect("temp dir");
        let store = ArtifactStore::open(temp.path().join("artifacts")).expect("store");
        let created = store
            .create_thread(create_input("session-a", "/workspace/report.md", b"v1", 1))
            .expect("create");

        assert_eq!(
            store
                .find_active_thread("session-a", "/workspace/report.md")
                .expect("lookup")
                .expect("active")
                .artifact_id,
            created.manifest.artifact_id
        );
        store
            .close_thread(&created.manifest.artifact_id)
            .expect("close");
        assert!(store
            .find_active_thread("session-a", "/workspace/report.md")
            .expect("lookup")
            .is_none());
        assert_eq!(
            store
                .load_manifest(&created.manifest.artifact_id)
                .expect("history")
                .versions
                .len(),
            1
        );
    }

    #[test]
    fn path_traversal_references_are_rejected() {
        let temp = tempfile::tempdir().expect("temp dir");
        let store = ArtifactStore::open(temp.path().join("artifacts")).expect("store");
        for id in ["../escape", "..", "a/b", "a\\b", ""] {
            assert!(matches!(
                store.load_manifest(id),
                Err(ArtifactStoreError::InvalidReference(_))
            ));
        }
    }

    #[test]
    fn unsupported_schema_is_not_silently_repaired() {
        let temp = tempfile::tempdir().expect("temp dir");
        let root = temp.path().join("artifacts");
        fs::create_dir_all(root.join("threads").join("artifact-1")).expect("threads");
        fs::create_dir_all(root.join("blobs")).expect("blobs");
        fs::write(
            root.join("threads")
                .join("artifact-1")
                .join("manifest.json"),
            br#"{"schema":2}"#,
        )
        .expect("future manifest");

        assert!(ArtifactStore::open(root).is_err());
    }
}
