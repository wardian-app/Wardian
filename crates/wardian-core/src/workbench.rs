//! Versioned workbench load, compare-and-swap save, reset, and recovery.

use crate::{
    atomic_file::{
        cleanup_atomic_temps, replace_staged_atomic_durable_with_hook,
        stage_bytes_atomic_with_hook, write_bytes_atomic_durable_with_hook, AtomicFaultHook,
        AtomicWriteRole, NoAtomicFault,
    },
    models::workbench::{
        WorkbenchDocumentV1, WorkbenchValidationErrors, MAX_SAFE_INTEGER,
        MAX_WORKBENCH_DOCUMENT_BYTES, WORKBENCH_SCHEMA_VERSION,
    },
    paths::{workbench_backup_path_for_home, workbench_path_for_home},
};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, VecDeque},
    fs, io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard, Weak},
};
use thiserror::Error;

const MAX_REQUEST_LEDGER_ENTRIES_PER_HOME: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
/// Authoritative slot used for a workbench load.
pub enum WorkbenchLoadSource {
    Primary,
    Backup,
    Default,
    FutureSchema,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
/// Structured workbench load response exposed through Tauri.
pub struct WorkbenchLoadResult {
    pub source: WorkbenchLoadSource,
    pub document: Option<WorkbenchDocumentV1>,
    pub notice: Option<String>,
    pub durable_revision: Option<u64>,
    pub durable_token: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
/// Durable-state outcome for save and reset requests.
pub enum WorkbenchPersistenceOutcome {
    Saved,
    RevisionConflict,
    FutureSchema,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkbenchSaveRequest {
    pub document: WorkbenchDocumentV1,
    pub expected_revision: u64,
    pub expected_token: String,
    pub request_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkbenchSaveResult {
    pub outcome: WorkbenchPersistenceOutcome,
    pub durable_revision: Option<u64>,
    pub durable_token: Option<String>,
    pub request_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkbenchResetRequest {
    pub expected_revision: u64,
    pub expected_token: String,
    pub request_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkbenchResetResult {
    pub outcome: WorkbenchPersistenceOutcome,
    pub durable_revision: Option<u64>,
    pub durable_token: Option<String>,
    pub request_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub document: Option<WorkbenchDocumentV1>,
}

#[derive(Debug, Error)]
pub enum WorkbenchIoError {
    #[error("invalid workbench document: {0}")]
    InvalidDocument(#[from] WorkbenchValidationErrors),
    #[error("could not serialize workbench document: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("workbench filesystem operation failed: {0}")]
    Io(#[from] io::Error),
}

#[derive(Debug)]
enum SlotState {
    Missing,
    Invalid,
    FutureSchema,
    Valid {
        document: Box<WorkbenchDocumentV1>,
        bytes: Vec<u8>,
        token: String,
    },
}

#[derive(Debug)]
enum ResolvedState {
    FutureSchema {
        notice: String,
    },
    V1 {
        source: WorkbenchLoadSource,
        document: Box<WorkbenchDocumentV1>,
        token: String,
        notice: Option<String>,
        valid_primary_bytes: Option<Vec<u8>>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RequestKind {
    Save,
    Reset,
}

#[derive(Debug, Clone)]
enum StoredResult {
    Save(WorkbenchSaveResult),
    Reset(Box<WorkbenchResetResult>),
}

#[derive(Debug, Clone)]
struct RequestLedgerEntry {
    kind: RequestKind,
    fingerprint: String,
    result: StoredResult,
}

#[derive(Debug, Default)]
struct HomeRequestLedger {
    entries: HashMap<String, RequestLedgerEntry>,
    order: VecDeque<String>,
}

static HOME_LOCKS: Lazy<Mutex<HashMap<PathBuf, Weak<Mutex<()>>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));
static REQUEST_LEDGERS: Lazy<Mutex<HashMap<PathBuf, HomeRequestLedger>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

/// Loads the primary, last-known-good backup, or deterministic default for a home.
pub fn load_workbench_for_home(home: &Path) -> Result<WorkbenchLoadResult, WorkbenchIoError> {
    let home_key = home.to_path_buf();
    let home_lock = home_lock(&home_key);
    let _guard = lock_unpoisoned(&home_lock);
    resolved_to_load_result(resolve_state(&home_key)?)
}

/// Validates and durably saves one CAS-proposed workbench revision.
pub fn save_workbench_for_home(
    home: &Path,
    request: WorkbenchSaveRequest,
) -> Result<WorkbenchSaveResult, WorkbenchIoError> {
    let home_key = home.to_path_buf();
    let home_lock = home_lock(&home_key);
    let _guard = lock_unpoisoned(&home_lock);
    save_workbench_locked(&home_key, request)
}

/// Replaces workbench presentation state with a CAS-proposed default revision.
pub fn reset_workbench_for_home(
    home: &Path,
    request: WorkbenchResetRequest,
) -> Result<WorkbenchResetResult, WorkbenchIoError> {
    let home_key = home.to_path_buf();
    let home_lock = home_lock(&home_key);
    let _guard = lock_unpoisoned(&home_lock);
    reset_workbench_locked(&home_key, request)
}

fn save_workbench_locked(
    home: &Path,
    request: WorkbenchSaveRequest,
) -> Result<WorkbenchSaveResult, WorkbenchIoError> {
    let resolved = resolve_state(home)?;
    if matches!(resolved, ResolvedState::FutureSchema { .. }) {
        return Ok(save_result_for_resolved(
            &resolved,
            WorkbenchPersistenceOutcome::FutureSchema,
            request.request_id,
        ));
    }

    request.document.validate()?;
    let incoming_bytes = serde_json::to_vec(&request.document)?;
    let incoming_token = token_for(&incoming_bytes);
    let fingerprint = format!("{}:{incoming_token}", request.document.revision);

    if let Some(entry) = ledger_entry(home, &request.request_id) {
        if entry.kind == RequestKind::Save && entry.fingerprint == fingerprint {
            if let StoredResult::Save(result) = entry.result {
                if saved_result_matches_current_primary(
                    &resolved,
                    result.durable_revision,
                    result.durable_token.as_deref(),
                ) {
                    return Ok(result);
                }
            }
        }
        return Ok(save_result_for_resolved(
            &resolved,
            WorkbenchPersistenceOutcome::RevisionConflict,
            request.request_id,
        ));
    }

    let (durable_document, durable_token) = match &resolved {
        ResolvedState::V1 {
            document, token, ..
        } => (document, token),
        ResolvedState::FutureSchema { .. } => unreachable!(),
    };

    if matches!(
        resolved,
        ResolvedState::V1 {
            source: WorkbenchLoadSource::Primary,
            ..
        }
    ) && request.document.revision == durable_document.revision
        && incoming_token == *durable_token
    {
        let result = WorkbenchSaveResult {
            outcome: WorkbenchPersistenceOutcome::Saved,
            durable_revision: Some(durable_document.revision),
            durable_token: Some(durable_token.clone()),
            request_id: request.request_id.clone(),
        };
        remember_request(
            home,
            request.request_id,
            RequestLedgerEntry {
                kind: RequestKind::Save,
                fingerprint,
                result: StoredResult::Save(result.clone()),
            },
        );
        return Ok(result);
    }

    let successor_revision = request.expected_revision.checked_add(1);
    let cas_matches = request.expected_revision == durable_document.revision
        && request.expected_token == *durable_token
        && request.expected_revision < MAX_SAFE_INTEGER
        && successor_revision == Some(request.document.revision);
    if !cas_matches {
        return Ok(save_result_for_resolved(
            &resolved,
            WorkbenchPersistenceOutcome::RevisionConflict,
            request.request_id,
        ));
    }

    persist_bytes(home, &resolved, &incoming_bytes)?;
    let result = WorkbenchSaveResult {
        outcome: WorkbenchPersistenceOutcome::Saved,
        durable_revision: Some(request.document.revision),
        durable_token: Some(incoming_token),
        request_id: request.request_id.clone(),
    };
    remember_request(
        home,
        request.request_id,
        RequestLedgerEntry {
            kind: RequestKind::Save,
            fingerprint,
            result: StoredResult::Save(result.clone()),
        },
    );
    Ok(result)
}

fn reset_workbench_locked(
    home: &Path,
    request: WorkbenchResetRequest,
) -> Result<WorkbenchResetResult, WorkbenchIoError> {
    let resolved = resolve_state(home)?;
    if matches!(resolved, ResolvedState::FutureSchema { .. }) {
        return Ok(reset_result_for_resolved(
            &resolved,
            WorkbenchPersistenceOutcome::FutureSchema,
            request.request_id,
            None,
        ));
    }

    let fingerprint = format!("{}:{}", request.expected_revision, request.expected_token);
    if let Some(entry) = ledger_entry(home, &request.request_id) {
        if entry.kind == RequestKind::Reset && entry.fingerprint == fingerprint {
            if let StoredResult::Reset(result) = entry.result {
                if saved_result_matches_current_primary(
                    &resolved,
                    result.durable_revision,
                    result.durable_token.as_deref(),
                ) {
                    return Ok(*result);
                }
            }
        }
        return Ok(reset_result_for_resolved(
            &resolved,
            WorkbenchPersistenceOutcome::RevisionConflict,
            request.request_id,
            None,
        ));
    }

    let (durable_document, durable_token, source) = match &resolved {
        ResolvedState::V1 {
            source,
            document,
            token,
            ..
        } => (document, token, *source),
        ResolvedState::FutureSchema { .. } => unreachable!(),
    };

    if source == WorkbenchLoadSource::Primary
        && request.expected_revision < MAX_SAFE_INTEGER
        && durable_document.revision == request.expected_revision + 1
        && is_reset_target(durable_document, durable_document.revision)
        && reset_retry_token_matches(home, request.expected_revision, &request.expected_token)?
    {
        let result = WorkbenchResetResult {
            outcome: WorkbenchPersistenceOutcome::Saved,
            durable_revision: Some(durable_document.revision),
            durable_token: Some(durable_token.clone()),
            request_id: request.request_id.clone(),
            document: Some(durable_document.as_ref().clone()),
        };
        remember_request(
            home,
            request.request_id,
            RequestLedgerEntry {
                kind: RequestKind::Reset,
                fingerprint,
                result: StoredResult::Reset(Box::new(result.clone())),
            },
        );
        return Ok(result);
    }

    let cas_matches = request.expected_revision == durable_document.revision
        && request.expected_token == *durable_token
        && request.expected_revision < MAX_SAFE_INTEGER;
    if !cas_matches {
        return Ok(reset_result_for_resolved(
            &resolved,
            WorkbenchPersistenceOutcome::RevisionConflict,
            request.request_id,
            None,
        ));
    }

    let mut document = WorkbenchDocumentV1::default_document();
    document.revision = request.expected_revision + 1;
    document.saved_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    document.validate()?;
    let bytes = serde_json::to_vec(&document)?;
    let token = token_for(&bytes);
    persist_bytes(home, &resolved, &bytes)?;

    let result = WorkbenchResetResult {
        outcome: WorkbenchPersistenceOutcome::Saved,
        durable_revision: Some(document.revision),
        durable_token: Some(token),
        request_id: request.request_id.clone(),
        document: Some(document),
    };
    remember_request(
        home,
        request.request_id,
        RequestLedgerEntry {
            kind: RequestKind::Reset,
            fingerprint,
            result: StoredResult::Reset(Box::new(result.clone())),
        },
    );
    Ok(result)
}

fn resolve_state(home: &Path) -> Result<ResolvedState, WorkbenchIoError> {
    let primary_path = workbench_path_for_home(home);
    let backup_path = workbench_backup_path_for_home(home);
    cleanup_atomic_temps(&primary_path)?;
    cleanup_atomic_temps(&backup_path)?;

    let primary = classify_slot(&primary_path)?;
    match primary {
        SlotState::FutureSchema => Ok(ResolvedState::FutureSchema {
            notice: "This workbench was written by a newer Wardian version and is read-only."
                .to_string(),
        }),
        SlotState::Valid {
            document,
            bytes,
            token,
        } => {
            if matches!(classify_slot(&backup_path)?, SlotState::FutureSchema) {
                return Ok(ResolvedState::FutureSchema {
                    notice:
                        "The workbench backup was written by a newer Wardian version and is read-only."
                            .to_string(),
                });
            }
            Ok(ResolvedState::V1 {
                source: WorkbenchLoadSource::Primary,
                document,
                valid_primary_bytes: Some(bytes.clone()),
                token,
                notice: None,
            })
        }
        primary @ (SlotState::Missing | SlotState::Invalid) => {
            let backup = classify_slot(&backup_path)?;
            match backup {
                SlotState::FutureSchema => Ok(ResolvedState::FutureSchema {
                    notice:
                        "The workbench backup was written by a newer Wardian version and is read-only."
                            .to_string(),
                }),
                SlotState::Valid {
                    document,
                    bytes: _,
                    token,
                } => Ok(ResolvedState::V1 {
                    source: WorkbenchLoadSource::Backup,
                    document,
                    token,
                    notice: Some(
                        "Wardian recovered the workbench from its last-known-good backup."
                            .to_string(),
                    ),
                    valid_primary_bytes: None,
                }),
                backup @ (SlotState::Missing | SlotState::Invalid) => {
                    let document = WorkbenchDocumentV1::default_document();
                    document.validate()?;
                    let bytes = serde_json::to_vec(&document)?;
                    let token = token_for(&bytes);
                    let notice = matches!(primary, SlotState::Invalid)
                        .then(|| {
                            "Wardian could not restore the saved workbench and opened a default."
                                .to_string()
                        })
                        .or_else(|| {
                            matches!(backup, SlotState::Invalid).then(|| {
                                "Wardian could not restore the workbench backup and opened a default."
                                    .to_string()
                            })
                        });
                    Ok(ResolvedState::V1 {
                        source: WorkbenchLoadSource::Default,
                        document: Box::new(document),
                        token,
                        notice,
                        valid_primary_bytes: None,
                    })
                }
            }
        }
    }
}

fn classify_slot(path: &Path) -> Result<SlotState, WorkbenchIoError> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(SlotState::Missing),
        Err(error) => return Err(error.into()),
    };
    let value: serde_json::Value = match serde_json::from_slice(&bytes) {
        Ok(value) => value,
        Err(_) => return Ok(SlotState::Invalid),
    };
    let Some(schema_version) = value
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
    else {
        return Ok(SlotState::Invalid);
    };
    if schema_version > WORKBENCH_SCHEMA_VERSION {
        return Ok(SlotState::FutureSchema);
    }
    if bytes.len() > MAX_WORKBENCH_DOCUMENT_BYTES {
        return Ok(SlotState::Invalid);
    }
    if schema_version != WORKBENCH_SCHEMA_VERSION {
        return Ok(SlotState::Invalid);
    }
    let document: WorkbenchDocumentV1 = match serde_json::from_value(value) {
        Ok(document) => document,
        Err(_) => return Ok(SlotState::Invalid),
    };
    if document.validate().is_err() {
        return Ok(SlotState::Invalid);
    }
    let token = token_for(&bytes);
    Ok(SlotState::Valid {
        document: Box::new(document),
        bytes,
        token,
    })
}

fn persist_bytes(
    home: &Path,
    resolved: &ResolvedState,
    incoming_bytes: &[u8],
) -> Result<(), WorkbenchIoError> {
    persist_bytes_with_hook(home, resolved, incoming_bytes, &mut NoAtomicFault)
}

fn persist_bytes_with_hook(
    home: &Path,
    resolved: &ResolvedState,
    incoming_bytes: &[u8],
    hook: &mut impl AtomicFaultHook,
) -> Result<(), WorkbenchIoError> {
    let primary_path = workbench_path_for_home(home);
    let primary_temp = stage_bytes_atomic_with_hook(
        &primary_path,
        incoming_bytes,
        AtomicWriteRole::Primary,
        hook,
    )?;
    if let ResolvedState::V1 {
        valid_primary_bytes: Some(current_primary),
        ..
    } = resolved
    {
        write_bytes_atomic_durable_with_hook(
            &workbench_backup_path_for_home(home),
            current_primary,
            AtomicWriteRole::Backup,
            hook,
        )?;
    }
    replace_staged_atomic_durable_with_hook(
        &primary_temp,
        &primary_path,
        AtomicWriteRole::Primary,
        hook,
    )?;
    Ok(())
}

fn resolved_to_load_result(
    resolved: ResolvedState,
) -> Result<WorkbenchLoadResult, WorkbenchIoError> {
    Ok(match resolved {
        ResolvedState::FutureSchema { notice } => WorkbenchLoadResult {
            source: WorkbenchLoadSource::FutureSchema,
            document: None,
            notice: Some(notice),
            durable_revision: None,
            durable_token: None,
        },
        ResolvedState::V1 {
            source,
            document,
            token,
            notice,
            ..
        } => WorkbenchLoadResult {
            source,
            durable_revision: Some(document.revision),
            document: Some(*document),
            notice,
            durable_token: Some(token),
        },
    })
}

fn save_result_for_resolved(
    resolved: &ResolvedState,
    outcome: WorkbenchPersistenceOutcome,
    request_id: String,
) -> WorkbenchSaveResult {
    match resolved {
        ResolvedState::FutureSchema { .. } => WorkbenchSaveResult {
            outcome,
            durable_revision: None,
            durable_token: None,
            request_id,
        },
        ResolvedState::V1 {
            document, token, ..
        } => WorkbenchSaveResult {
            outcome,
            durable_revision: Some(document.revision),
            durable_token: Some(token.clone()),
            request_id,
        },
    }
}

fn reset_result_for_resolved(
    resolved: &ResolvedState,
    outcome: WorkbenchPersistenceOutcome,
    request_id: String,
    document: Option<WorkbenchDocumentV1>,
) -> WorkbenchResetResult {
    match resolved {
        ResolvedState::FutureSchema { .. } => WorkbenchResetResult {
            outcome,
            durable_revision: None,
            durable_token: None,
            request_id,
            document,
        },
        ResolvedState::V1 {
            document: durable,
            token,
            ..
        } => WorkbenchResetResult {
            outcome,
            durable_revision: Some(durable.revision),
            durable_token: Some(token.clone()),
            request_id,
            document,
        },
    }
}

fn token_for(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn saved_result_matches_current_primary(
    resolved: &ResolvedState,
    revision: Option<u64>,
    token: Option<&str>,
) -> bool {
    matches!(
        resolved,
        ResolvedState::V1 {
            source: WorkbenchLoadSource::Primary,
            document,
            token: current_token,
            ..
        } if revision == Some(document.revision) && token == Some(current_token.as_str())
    )
}

fn is_reset_target(document: &WorkbenchDocumentV1, revision: u64) -> bool {
    let mut expected = WorkbenchDocumentV1::default_document();
    expected.revision = revision;
    expected.saved_at.clone_from(&document.saved_at);
    document == &expected
}

fn reset_retry_token_matches(
    home: &Path,
    expected_revision: u64,
    expected_token: &str,
) -> Result<bool, WorkbenchIoError> {
    if let SlotState::Valid {
        document, token, ..
    } = classify_slot(&workbench_backup_path_for_home(home))?
    {
        if document.revision == expected_revision {
            return Ok(token == expected_token);
        }
    }
    if expected_revision != 0 {
        return Ok(false);
    }
    let default_bytes = serde_json::to_vec(&WorkbenchDocumentV1::default_document())?;
    Ok(token_for(&default_bytes) == expected_token)
}

fn home_lock(home: &Path) -> Arc<Mutex<()>> {
    let mut locks = lock_unpoisoned(&HOME_LOCKS);
    if let Some(lock) = locks.get(home).and_then(Weak::upgrade) {
        return lock;
    }
    let lock = Arc::new(Mutex::new(()));
    locks.insert(home.to_path_buf(), Arc::downgrade(&lock));
    lock
}

fn ledger_entry(home: &Path, request_id: &str) -> Option<RequestLedgerEntry> {
    lock_unpoisoned(&REQUEST_LEDGERS)
        .get(home)
        .and_then(|ledger| ledger.entries.get(request_id))
        .cloned()
}

fn remember_request(home: &Path, request_id: String, entry: RequestLedgerEntry) {
    let mut ledgers = lock_unpoisoned(&REQUEST_LEDGERS);
    let ledger = ledgers.entry(home.to_path_buf()).or_default();
    if !ledger.entries.contains_key(&request_id) {
        ledger.order.push_back(request_id.clone());
    }
    ledger.entries.insert(request_id, entry);
    while ledger.entries.len() > MAX_REQUEST_LEDGER_ENTRIES_PER_HOME {
        if let Some(oldest) = ledger.order.pop_front() {
            ledger.entries.remove(&oldest);
        }
    }
}

fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::atomic_file::{AtomicFaultHook, AtomicFaultPoint, AtomicWriteRole};

    struct FailAt {
        target: AtomicFaultPoint,
    }

    impl AtomicFaultHook for FailAt {
        fn check(&mut self, point: AtomicFaultPoint) -> io::Result<()> {
            if point == self.target {
                Err(io::Error::other(format!("injected fault at {point:?}")))
            } else {
                Ok(())
            }
        }
    }

    fn crash_points() -> Vec<AtomicFaultPoint> {
        let points = vec![
            AtomicFaultPoint::BeforeTempSync(AtomicWriteRole::Primary),
            AtomicFaultPoint::AfterTempSync(AtomicWriteRole::Primary),
            AtomicFaultPoint::BeforeTempSync(AtomicWriteRole::Backup),
            AtomicFaultPoint::AfterTempSync(AtomicWriteRole::Backup),
            AtomicFaultPoint::BeforeReplace(AtomicWriteRole::Backup),
            AtomicFaultPoint::AfterReplace(AtomicWriteRole::Backup),
            AtomicFaultPoint::BeforeReplace(AtomicWriteRole::Primary),
            AtomicFaultPoint::AfterReplace(AtomicWriteRole::Primary),
        ];
        #[cfg(unix)]
        {
            return points
                .into_iter()
                .chain([
                    AtomicFaultPoint::BeforeParentSync(AtomicWriteRole::Backup),
                    AtomicFaultPoint::AfterParentSync(AtomicWriteRole::Backup),
                    AtomicFaultPoint::BeforeParentSync(AtomicWriteRole::Primary),
                    AtomicFaultPoint::AfterParentSync(AtomicWriteRole::Primary),
                ])
                .collect();
        }
        #[cfg(not(unix))]
        {
            points
        }
    }

    #[test]
    fn workbench_crash_matrix_always_recovers_old_new_or_last_good_state() {
        let fixture_bytes = include_bytes!("../tests/fixtures/workbench-v1.json");
        for point in crash_points() {
            let home = tempfile::tempdir().expect("temp home");
            let primary_path = workbench_path_for_home(home.path());
            fs::create_dir_all(primary_path.parent().expect("settings parent"))
                .expect("settings dir");
            fs::write(&primary_path, fixture_bytes).expect("seed primary");

            let resolved = resolve_state(home.path()).expect("resolve old primary");
            let mut incoming = match &resolved {
                ResolvedState::V1 { document, .. } => document.as_ref().clone(),
                ResolvedState::FutureSchema { .. } => panic!("unexpected future schema"),
            };
            incoming.revision = 1;
            incoming.saved_at = "2026-07-10T12:34:56.789Z".to_string();
            incoming.shell.left_sidebar_width = 333.0;
            incoming.validate().expect("valid incoming document");
            let incoming_bytes = serde_json::to_vec(&incoming).expect("incoming bytes");

            let error = persist_bytes_with_hook(
                home.path(),
                &resolved,
                &incoming_bytes,
                &mut FailAt { target: point },
            )
            .expect_err("fault must prevent acknowledgement");
            assert!(error.to_string().contains("injected fault"));

            let recovered = load_workbench_for_home(home.path()).expect("recover after fault");
            let recovered_document = recovered.document.expect("recoverable V1 document");
            assert!(
                recovered_document.revision == 0 || recovered_document == incoming,
                "unexpected recovered state at {point:?}: revision {}",
                recovered_document.revision
            );
            if recovered_document.revision == 0 {
                assert_eq!(recovered_document, fixture_document());
            }

            let stale_temps: Vec<_> = fs::read_dir(primary_path.parent().unwrap())
                .expect("settings entries")
                .filter_map(Result::ok)
                .filter(|entry| {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    name.starts_with(".workbench") && name.ends_with(".tmp")
                })
                .collect();
            assert!(
                stale_temps.is_empty(),
                "stale workbench temps remain at {point:?}"
            );
        }
    }

    fn fixture_document() -> WorkbenchDocumentV1 {
        serde_json::from_slice(include_bytes!("../tests/fixtures/workbench-v1.json"))
            .expect("fixture document")
    }
}
