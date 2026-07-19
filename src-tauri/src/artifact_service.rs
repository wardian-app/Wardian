use crate::state::artifact_runtime::{ArtifactAckWaitError, ArtifactRuntime};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use wardian_core::{
    artifacts::{
        AppendArtifactVersion, ArtifactManifestV1, ArtifactOriginV1, ArtifactStore,
        ArtifactStoreError, ArtifactVersionV1, CreateArtifactThread,
    },
    files::{AuthorizedRootService, FileResourceErrorV1, FileResourceLimits, VerifiedFileSnapshot},
    models::AgentConfig,
};

pub const ARTIFACT_PRESENTED_EVENT: &str = "artifact://presented";
const DEFAULT_ACK_TIMEOUT: Duration = Duration::from_secs(2);
const DEFAULT_CAPTURE_TIMEOUT: Duration = Duration::from_secs(2);
const CAPTURE_RETRY_DELAY: Duration = Duration::from_millis(40);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct ArtifactPresentationRequestV1 {
    pub origin_session_id: String,
    pub path: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub artifact_id: Option<String>,
    pub force_new: bool,
    #[serde(default)]
    pub addressed_comment_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct ArtifactPresentationResultV1 {
    pub schema: u8,
    pub presentation_id: String,
    pub artifact_id: String,
    pub version_id: String,
    pub canonical_path: String,
    pub reused_thread: bool,
    pub persistence_state: String,
    pub ui_delivery_state: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct ArtifactPresentationEventV1 {
    pub schema: u8,
    pub presentation_id: String,
    pub artifact_id: String,
    pub version_id: String,
    pub canonical_path: String,
    pub title: String,
    pub origin_agent_id: String,
    pub origin_agent_name: String,
    pub reused_thread: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct ArtifactPresentationAckV1 {
    pub presentation_id: String,
    pub routed: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct ArtifactShowResultV1 {
    pub schema: u8,
    pub manifest: ArtifactManifestV1,
    pub selected_version: ArtifactVersionV1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct ArtifactServiceError {
    pub code: String,
    pub message: String,
    pub persisted: Option<Box<ArtifactPresentationResultV1>>,
}

impl std::fmt::Display for ArtifactServiceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for ArtifactServiceError {}

#[derive(Debug, Clone)]
struct CapturedArtifactFile {
    canonical_path: String,
    bytes: Vec<u8>,
}

trait ArtifactFileCapture: Send + Sync {
    fn capture(
        &self,
        config: &AgentConfig,
        path: &Path,
        timeout: Duration,
    ) -> Result<CapturedArtifactFile, ArtifactServiceError>;
}

#[derive(Debug, Default)]
struct AuthorizedArtifactFileCapture;

impl ArtifactFileCapture for AuthorizedArtifactFileCapture {
    fn capture(
        &self,
        config: &AgentConfig,
        path: &Path,
        timeout: Duration,
    ) -> Result<CapturedArtifactFile, ArtifactServiceError> {
        let roots = AuthorizedRootService::from_agent_config(config).map_err(map_file_error)?;
        let authorized = roots
            .authorize_existing_file(path)
            .map_err(map_file_error)?;
        let limits = FileResourceLimits::default();
        let started = Instant::now();
        loop {
            match VerifiedFileSnapshot::from_authorized_path(&authorized, &limits) {
                Ok(snapshot) => {
                    let descriptor = snapshot.descriptor();
                    let capacity = usize::try_from(descriptor.size_bytes).map_err(|_| {
                        service_error("unreadable_file", "artifact is too large to snapshot")
                    })?;
                    let mut bytes = Vec::with_capacity(capacity);
                    match authorized
                        .copy_verified_revision_to(snapshot.revision_token(), &mut bytes)
                    {
                        Ok(_) => {
                            return Ok(CapturedArtifactFile {
                                canonical_path: descriptor.canonical_path.clone(),
                                bytes,
                            });
                        }
                        Err(error) if is_unstable(&error) && started.elapsed() < timeout => {
                            std::thread::sleep(CAPTURE_RETRY_DELAY);
                        }
                        Err(error) => return Err(map_file_error(error)),
                    }
                }
                Err(error) if is_unstable(&error) && started.elapsed() < timeout => {
                    std::thread::sleep(CAPTURE_RETRY_DELAY);
                }
                Err(error) => return Err(map_file_error(error)),
            }
        }
    }
}

type ArtifactEventEmitter =
    dyn Fn(ArtifactPresentationEventV1) -> Result<(), String> + Send + Sync + 'static;

#[derive(Clone)]
pub struct ArtifactService {
    store: ArtifactStore,
    runtime: Arc<ArtifactRuntime>,
    emitter: Arc<ArtifactEventEmitter>,
    capture: Arc<dyn ArtifactFileCapture>,
    ack_timeout: Duration,
    capture_timeout: Duration,
}

impl ArtifactService {
    pub fn new(
        store: ArtifactStore,
        runtime: Arc<ArtifactRuntime>,
        emitter: impl Fn(ArtifactPresentationEventV1) -> Result<(), String> + Send + Sync + 'static,
    ) -> Self {
        Self {
            store,
            runtime,
            emitter: Arc::new(emitter),
            capture: Arc::new(AuthorizedArtifactFileCapture),
            ack_timeout: DEFAULT_ACK_TIMEOUT,
            capture_timeout: DEFAULT_CAPTURE_TIMEOUT,
        }
    }

    pub async fn present(
        &self,
        config: AgentConfig,
        request: ArtifactPresentationRequestV1,
    ) -> Result<ArtifactPresentationResultV1, ArtifactServiceError> {
        validate_request(&config, &request)?;
        let path = absolute_path(&request.path)?;
        let capture = self.capture.clone();
        let capture_timeout = self.capture_timeout;
        let capture_config = config.clone();
        let captured = tokio::task::spawn_blocking(move || {
            capture.capture(&capture_config, &path, capture_timeout)
        })
        .await
        .map_err(|error| {
            service_error("unreadable_file", format!("file worker failed: {error}"))
        })??;

        let canonical_path = captured.canonical_path.clone();
        let default_title = Path::new(&canonical_path)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(&canonical_path)
            .to_string();
        let origin = ArtifactOriginV1 {
            session_id: config.session_id.clone(),
            agent_id: config.session_id.clone(),
            agent_name: nonempty_or(&config.session_name, &config.session_id),
            provider: nonempty_or(&config.provider, "unknown"),
        };
        let title = request.title.clone().unwrap_or(default_title);
        let store = self.store.clone();
        let artifact_id = request.artifact_id.clone();
        let force_new = request.force_new;
        let description = request.description.clone();
        let addressed_comment_ids = request.addressed_comment_ids.clone();
        let now = now_ms();
        let stored = tokio::task::spawn_blocking(move || {
            if let Some(artifact_id) = artifact_id {
                let manifest = store.load_manifest(&artifact_id)?;
                if manifest.origin.session_id != origin.session_id
                    || manifest.canonical_path != canonical_path
                {
                    return Err(ArtifactStoreError::InvalidReference(
                        "target artifact origin or canonical path does not match".to_string(),
                    ));
                }
                store
                    .append_version(AppendArtifactVersion {
                        artifact_id,
                        bytes: captured.bytes,
                        addressed_comment_ids,
                        presented_at_ms: now,
                        title: request.title,
                        description: description.map(Some),
                    })
                    .map(|stored| (stored, true))
            } else if !force_new {
                if let Some(existing) =
                    store.find_active_thread(&origin.session_id, &canonical_path)?
                {
                    return store
                        .append_version(AppendArtifactVersion {
                            artifact_id: existing.artifact_id,
                            bytes: captured.bytes,
                            addressed_comment_ids,
                            presented_at_ms: now,
                            title: request.title,
                            description: description.map(Some),
                        })
                        .map(|stored| (stored, true));
                }
                store
                    .create_thread(CreateArtifactThread {
                        canonical_path,
                        title,
                        description,
                        origin,
                        bytes: captured.bytes,
                        addressed_comment_ids,
                        presented_at_ms: now,
                    })
                    .map(|stored| (stored, false))
            } else {
                store
                    .create_thread(CreateArtifactThread {
                        canonical_path,
                        title,
                        description,
                        origin,
                        bytes: captured.bytes,
                        addressed_comment_ids,
                        presented_at_ms: now,
                    })
                    .map(|stored| (stored, false))
            }
        })
        .await
        .map_err(|error| {
            service_error(
                "persistence_failed",
                format!("store worker failed: {error}"),
            )
        })?
        .map_err(map_store_error)?;

        let (stored, reused_thread) = stored;
        let presentation_id = uuid::Uuid::new_v4().to_string();
        let persisted = ArtifactPresentationResultV1 {
            schema: 1,
            presentation_id: presentation_id.clone(),
            artifact_id: stored.manifest.artifact_id.clone(),
            version_id: stored.version.version_id.clone(),
            canonical_path: stored.manifest.canonical_path.clone(),
            reused_thread,
            persistence_state: "persisted".to_string(),
            ui_delivery_state: "pending".to_string(),
        };
        let receiver = self
            .runtime
            .register(&presentation_id)
            .await
            .map_err(|message| delivery_error(message, persisted.clone()))?;
        let event = ArtifactPresentationEventV1 {
            schema: 1,
            presentation_id: presentation_id.clone(),
            artifact_id: stored.manifest.artifact_id,
            version_id: stored.version.version_id,
            canonical_path: stored.manifest.canonical_path,
            title: stored.manifest.title,
            origin_agent_id: stored.manifest.origin.agent_id,
            origin_agent_name: stored.manifest.origin.agent_name,
            reused_thread,
        };
        if let Err(message) = (self.emitter)(event) {
            self.runtime
                .wait(&presentation_id, receiver, Duration::ZERO)
                .await
                .ok();
            return Err(delivery_error(message, persisted));
        }
        let ack = self
            .runtime
            .wait(&presentation_id, receiver, self.ack_timeout)
            .await
            .map_err(|error| match error {
                ArtifactAckWaitError::Timeout => delivery_error(
                    "workbench routing acknowledgement timed out",
                    persisted.clone(),
                ),
                ArtifactAckWaitError::SenderDropped => delivery_error(
                    "workbench routing acknowledgement channel closed",
                    persisted.clone(),
                ),
            })?;
        if !ack.routed {
            return Err(delivery_error(
                ack.error
                    .unwrap_or_else(|| "workbench rejected artifact routing".to_string()),
                persisted,
            ));
        }
        Ok(ArtifactPresentationResultV1 {
            ui_delivery_state: "routed_background".to_string(),
            ..persisted
        })
    }

    pub async fn show(
        &self,
        artifact_id: String,
        version_id: Option<String>,
    ) -> Result<ArtifactShowResultV1, ArtifactServiceError> {
        let store = self.store.clone();
        let stored = tokio::task::spawn_blocking(move || {
            store.load_version(&artifact_id, version_id.as_deref())
        })
        .await
        .map_err(|error| {
            service_error(
                "persistence_failed",
                format!("store worker failed: {error}"),
            )
        })?
        .map_err(map_store_error)?;
        Ok(ArtifactShowResultV1 {
            schema: 1,
            manifest: stored.manifest,
            selected_version: stored.version,
        })
    }
}

fn validate_request(
    config: &AgentConfig,
    request: &ArtifactPresentationRequestV1,
) -> Result<(), ArtifactServiceError> {
    if config.session_id.trim().is_empty()
        || request.origin_session_id.trim().is_empty()
        || config.session_id != request.origin_session_id
    {
        return Err(service_error(
            "invalid_origin",
            "artifact origin does not match a live agent session",
        ));
    }
    if request.path.trim().is_empty() {
        return Err(service_error("unreadable_file", "artifact path is empty"));
    }
    if request.artifact_id.is_some() && request.force_new {
        return Err(service_error(
            "invalid_request",
            "--artifact and --new are mutually exclusive",
        ));
    }
    for comment_id in &request.addressed_comment_ids {
        if comment_id.trim().is_empty()
            || comment_id.contains('/')
            || comment_id.contains('\\')
            || comment_id == ".."
        {
            return Err(service_error(
                "invalid_request",
                "addressed comment id is invalid",
            ));
        }
    }
    Ok(())
}

fn absolute_path(path: &str) -> Result<PathBuf, ArtifactServiceError> {
    let path = PathBuf::from(path);
    if path.is_absolute() {
        Ok(path)
    } else {
        std::env::current_dir()
            .map(|current| current.join(path))
            .map_err(|error| service_error("unreadable_file", error.to_string()))
    }
}

fn is_unstable(error: &FileResourceErrorV1) -> bool {
    matches!(error.code(), "unstable_file" | "stale_revision")
}

fn map_file_error(error: FileResourceErrorV1) -> ArtifactServiceError {
    let code = match error.code() {
        "unauthorized_path" => "unauthorized_path",
        "unstable_file" | "stale_revision" => "unstable_file_timeout",
        _ => "unreadable_file",
    };
    service_error(code, error.message)
}

fn map_store_error(error: ArtifactStoreError) -> ArtifactServiceError {
    let code = match error {
        ArtifactStoreError::ArtifactNotFound(_) | ArtifactStoreError::VersionNotFound { .. } => {
            "artifact_not_found"
        }
        ArtifactStoreError::InvalidReference(_) => "invalid_request",
        _ => "persistence_failed",
    };
    service_error(code, error.to_string())
}

fn service_error(code: impl Into<String>, message: impl Into<String>) -> ArtifactServiceError {
    ArtifactServiceError {
        code: code.into(),
        message: message.into(),
        persisted: None,
    }
}

fn delivery_error(
    message: impl Into<String>,
    persisted: ArtifactPresentationResultV1,
) -> ArtifactServiceError {
    ArtifactServiceError {
        code: "ui_delivery_failed".to_string(),
        message: message.into(),
        persisted: Some(Box::new(persisted)),
    }
}

fn nonempty_or(value: &str, fallback: &str) -> String {
    let value = value.trim();
    if value.is_empty() {
        fallback.to_string()
    } else {
        value.to_string()
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    fn config(root: &Path, session_id: &str) -> AgentConfig {
        AgentConfig {
            session_id: session_id.into(),
            session_name: format!("Agent {session_id}"),
            folder: root.to_string_lossy().into_owned(),
            provider: "codex".into(),
            ..AgentConfig::default()
        }
    }

    fn request(session_id: &str, path: &Path) -> ArtifactPresentationRequestV1 {
        ArtifactPresentationRequestV1 {
            origin_session_id: session_id.into(),
            path: path.to_string_lossy().into_owned(),
            title: None,
            description: None,
            artifact_id: None,
            force_new: false,
            addressed_comment_ids: Vec::new(),
        }
    }

    fn acknowledging_service(root: &Path) -> ArtifactService {
        let store = ArtifactStore::open(root.join("artifacts")).expect("store");
        let runtime = Arc::new(ArtifactRuntime::default());
        let ack_runtime = runtime.clone();
        ArtifactService::new(store, runtime, move |event| {
            let runtime = ack_runtime.clone();
            tokio::spawn(async move {
                runtime
                    .acknowledge(ArtifactPresentationAckV1 {
                        presentation_id: event.presentation_id,
                        routed: true,
                        error: None,
                    })
                    .await;
            });
            Ok(())
        })
    }

    #[tokio::test]
    async fn same_origin_and_path_reuse_active_thread() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("report.md");
        std::fs::write(&path, "first").expect("file");
        let service = acknowledging_service(temp.path());
        let first = service
            .present(
                config(temp.path(), "session-a"),
                request("session-a", &path),
            )
            .await
            .expect("first");
        std::fs::write(&path, "second").expect("update");
        let second = service
            .present(
                config(temp.path(), "session-a"),
                request("session-a", &path),
            )
            .await
            .expect("second");

        assert_eq!(first.artifact_id, second.artifact_id);
        assert!(second.reused_thread);
        assert_ne!(first.version_id, second.version_id);
        assert_eq!(second.ui_delivery_state, "routed_background");
    }

    #[tokio::test]
    async fn different_origin_and_force_new_create_distinct_threads() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("report.md");
        std::fs::write(&path, "content").expect("file");
        let service = acknowledging_service(temp.path());
        let first = service
            .present(
                config(temp.path(), "session-a"),
                request("session-a", &path),
            )
            .await
            .expect("first");
        let other = service
            .present(
                config(temp.path(), "session-b"),
                request("session-b", &path),
            )
            .await
            .expect("other");
        let mut forced = request("session-a", &path);
        forced.force_new = true;
        let forced = service
            .present(config(temp.path(), "session-a"), forced)
            .await
            .expect("forced");

        assert_ne!(first.artifact_id, other.artifact_id);
        assert_ne!(first.artifact_id, forced.artifact_id);
    }

    #[tokio::test]
    async fn explicit_thread_requires_exact_origin_and_path() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("report.md");
        let other_path = temp.path().join("other.md");
        std::fs::write(&path, "content").expect("file");
        std::fs::write(&other_path, "other").expect("other");
        let service = acknowledging_service(temp.path());
        let first = service
            .present(
                config(temp.path(), "session-a"),
                request("session-a", &path),
            )
            .await
            .expect("first");
        let mut mismatch = request("session-a", &other_path);
        mismatch.artifact_id = Some(first.artifact_id);
        let error = service
            .present(config(temp.path(), "session-a"), mismatch)
            .await
            .expect_err("mismatch");
        assert_eq!(error.code, "invalid_request");
    }

    #[tokio::test]
    async fn unauthorized_path_fails_without_persistence() {
        let temp = tempfile::tempdir().expect("temp dir");
        let root = temp.path().join("root");
        std::fs::create_dir(&root).expect("root");
        let outside = temp.path().join("outside.md");
        std::fs::write(&outside, "outside").expect("outside");
        let service = acknowledging_service(temp.path());
        let error = service
            .present(config(&root, "session-a"), request("session-a", &outside))
            .await
            .expect_err("unauthorized");
        assert_eq!(error.code, "unauthorized_path");
    }

    #[tokio::test]
    async fn persistence_precedes_event_and_address_claims_are_retained() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("report.md");
        std::fs::write(&path, "content").expect("file");
        let store = ArtifactStore::open(temp.path().join("artifacts")).expect("store");
        let observed = Arc::new(Mutex::new(false));
        let runtime = Arc::new(ArtifactRuntime::default());
        let ack_runtime = runtime.clone();
        let event_store = store.clone();
        let event_observed = observed.clone();
        let service = ArtifactService::new(store.clone(), runtime, move |event| {
            let manifest = event_store
                .load_manifest(&event.artifact_id)
                .map_err(|error| error.to_string())?;
            *event_observed.lock().expect("observed") = true;
            assert_eq!(
                manifest
                    .versions
                    .last()
                    .expect("version")
                    .addressed_comment_ids,
                vec!["comment-1"]
            );
            let runtime = ack_runtime.clone();
            tokio::spawn(async move {
                runtime
                    .acknowledge(ArtifactPresentationAckV1 {
                        presentation_id: event.presentation_id,
                        routed: true,
                        error: None,
                    })
                    .await;
            });
            Ok(())
        });
        let mut present = request("session-a", &path);
        present.addressed_comment_ids = vec!["comment-1".into()];

        service
            .present(config(temp.path(), "session-a"), present)
            .await
            .expect("present");
        assert!(*observed.lock().expect("observed"));
    }

    #[tokio::test]
    async fn ui_timeout_reports_failure_without_rolling_back_persistence() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("report.md");
        std::fs::write(&path, "content").expect("file");
        let store = ArtifactStore::open(temp.path().join("artifacts")).expect("store");
        let runtime = Arc::new(ArtifactRuntime::default());
        let mut service = ArtifactService::new(store.clone(), runtime, |_| Ok(()));
        service.ack_timeout = Duration::from_millis(5);

        let error = service
            .present(
                config(temp.path(), "session-a"),
                request("session-a", &path),
            )
            .await
            .expect_err("timeout");
        assert_eq!(error.code, "ui_delivery_failed");
        let persisted = error.persisted.expect("persisted result");
        assert_eq!(persisted.persistence_state, "persisted");
        assert_eq!(store.list_recent(10).expect("recent").len(), 1);
    }
}
