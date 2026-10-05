//! Live-only admission for ordinary provider-log archive capture.
//!
//! One claim owns a capture identity. Observations arriving during that claim
//! remain pending; errors and cancellation release ownership for a later tick.
//! The mutex protects admission only and is never held over archive I/O.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use crate::commands::provider_log_acquisition::ProviderLogNativeIdentity;

#[derive(Clone)]
pub(crate) struct CaptureRequest {
    pub(crate) session_id: String,
    pub(crate) incarnation: Arc<Mutex<String>>,
    pub(crate) provider: String,
    pub(crate) conversation: Option<String>,
    pub(crate) source_path: Option<PathBuf>,
    pub(crate) source: Option<SourceObservation>,
    pub(crate) logging_enabled: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SourceObservation {
    pub(crate) path: PathBuf,
    pub(crate) native_identity: ProviderLogNativeIdentity,
    pub(crate) length: u64,
    pub(crate) modified: Option<SystemTime>,
}

impl CaptureRequest {
    fn same_identity(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.incarnation, &other.incarnation)
            && self.provider == other.provider
            && self.conversation == other.conversation
            && self.source_path == other.source_path
            && self
                .source
                .as_ref()
                .map(|source| (&source.path, &source.native_identity))
                == other
                    .source
                    .as_ref()
                    .map(|source| (&source.path, &source.native_identity))
    }

    fn same_observation(&self, other: &Self) -> bool {
        self.source == other.source && self.logging_enabled == other.logging_enabled
    }
}

/// Private acquisition disposition; a stopped bounded pass is not necessarily EOF.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CaptureStop {
    More,
    Observed {
        status: String,
        reason: Option<String>,
    },
    Disabled,
    Retired,
}

struct Entry {
    request: CaptureRequest,
    epoch: u64,
    serviced_epoch: u64,
    claim: Option<u64>,
    retry: bool,
    await_observation: bool,
}

#[derive(Default)]
struct Inner {
    entries: HashMap<String, Entry>,
    next_claim: u64,
}

#[derive(Clone, Default)]
pub(crate) struct BackgroundCaptureCoordinator {
    inner: Arc<Mutex<Inner>>,
}

impl BackgroundCaptureCoordinator {
    /// Ordinary observations coalesce once quiet. Explicit restore/status
    /// requests can force a pass without fabricating a source change.
    pub(crate) fn admit(&self, request: CaptureRequest, force: bool) -> Option<CaptureClaim> {
        let mut inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        let session_id = request.session_id.clone();
        match inner.entries.get_mut(&session_id) {
            Some(entry) if entry.request.same_identity(&request) => {
                if force || !entry.request.same_observation(&request) {
                    entry.epoch = entry.epoch.checked_add(1)?;
                    entry.request = request;
                }
            }
            Some(entry) => {
                // Retain the current claim's bookkeeping when its identity
                // changes. Admission is validated against the live roster by
                // the caller, so a newer request waits behind the old owner.
                entry.epoch = entry.epoch.checked_add(1)?;
                entry.request = request;
            }
            _ => {
                inner.entries.insert(
                    session_id.clone(),
                    Entry {
                        request,
                        epoch: 1,
                        serviced_epoch: 0,
                        claim: None,
                        retry: false,
                        await_observation: false,
                    },
                );
            }
        }
        let entry = inner.entries.get(&session_id)?;
        if entry.claim.is_some() || (!entry.retry && entry.serviced_epoch == entry.epoch) {
            return None;
        }
        let nonce = inner.next_claim.checked_add(1)?;
        inner.next_claim = nonce;
        let entry = inner.entries.get_mut(&session_id)?;
        entry.claim = Some(nonce);
        entry.retry = false;
        Some(CaptureClaim {
            coordinator: self.clone(),
            request: entry.request.clone(),
            epoch: entry.epoch,
            nonce,
            finished: false,
        })
    }

    /// Drop removed agents' live-only identities. An old claim's nonce cannot
    /// retire an entry subsequently installed for a replacement agent.
    pub(crate) fn retain_agents(&self, sessions: &[(String, Arc<Mutex<String>>)]) {
        self.inner
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .entries
            .retain(|session, entry| {
                sessions.iter().any(|(id, incarnation)| {
                    session == id && Arc::ptr_eq(incarnation, &entry.request.incarnation)
                })
            });
    }

    /// Existing stopped/error owners remain observable through ordinary ticks,
    /// including status-triggered owners whose runtime is currently active.
    pub(crate) fn observation_sessions(&self) -> std::collections::HashSet<String> {
        self.inner
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .entries
            .iter()
            .filter(|(_, entry)| {
                entry.retry || entry.await_observation || entry.serviced_epoch != entry.epoch
            })
            .map(|(session, _)| session.clone())
            .collect()
    }

    /// A successful owner hands a newer request forward without waiting for a
    /// second trigger. Retry intent is excluded: errors only retry on a tick.
    pub(crate) fn take_pending(&self, session_id: &str) -> Option<CaptureClaim> {
        let mut inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        let entry = inner.entries.get(session_id)?;
        if entry.claim.is_some() || entry.retry || entry.serviced_epoch == entry.epoch {
            return None;
        }
        let nonce = inner.next_claim.checked_add(1)?;
        inner.next_claim = nonce;
        let entry = inner.entries.get_mut(session_id)?;
        entry.claim = Some(nonce);
        Some(CaptureClaim {
            coordinator: self.clone(),
            request: entry.request.clone(),
            epoch: entry.epoch,
            nonce,
            finished: false,
        })
    }
}

pub(crate) struct CaptureClaim {
    coordinator: BackgroundCaptureCoordinator,
    pub(crate) request: CaptureRequest,
    epoch: u64,
    nonce: u64,
    finished: bool,
}

impl CaptureClaim {
    /// Only this claim's observed epoch is serviced. A newer request survives
    /// successful completion, while an error waits for a later admission.
    pub(crate) fn finish(mut self, stop: CaptureStop) {
        let mut inner = self
            .coordinator
            .inner
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(entry) = inner.entries.get_mut(&self.request.session_id) {
            if entry.claim == Some(self.nonce) {
                if stop == CaptureStop::Retired {
                    if entry.epoch == self.epoch {
                        inner.entries.remove(&self.request.session_id);
                    } else {
                        entry.serviced_epoch = self.epoch;
                        entry.claim = None;
                    }
                } else {
                    entry.await_observation = match &stop {
                        CaptureStop::Disabled => true,
                        CaptureStop::Observed { status, .. } => {
                            status != "complete" && status != "projection"
                        }
                        _ => false,
                    };
                    if stop == CaptureStop::Disabled && entry.epoch == self.epoch {
                        // Retain the policy actually encountered after the
                        // gate, rather than the policy sampled at admission.
                        entry.request.logging_enabled = false;
                    }
                    entry.serviced_epoch = self.epoch;
                    entry.claim = None;
                }
            }
        }
        self.finished = true;
    }
}

impl Drop for CaptureClaim {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        let mut inner = self
            .coordinator
            .inner
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(entry) = inner.entries.get_mut(&self.request.session_id) {
            if entry.claim == Some(self.nonce) {
                entry.claim = None;
                entry.retry = true;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> CaptureRequest {
        CaptureRequest {
            session_id: "capture-agent".into(),
            incarnation: Arc::new(Mutex::new("Off".into())),
            provider: "codex".into(),
            conversation: Some("conversation".into()),
            source_path: Some(PathBuf::from("provider.jsonl")),
            source: Some(SourceObservation {
                path: PathBuf::from("provider.jsonl"),
                native_identity: ProviderLogNativeIdentity {
                    platform: "test".into(),
                    primary: 1,
                    secondary: 2,
                },
                length: 10,
                modified: None,
            }),
            logging_enabled: true,
        }
    }

    fn settled() -> CaptureStop {
        CaptureStop::Observed {
            status: "complete".into(),
            reason: None,
        }
    }

    #[test]
    fn coalesces_owners_and_preserves_arrival_at_completion() {
        let coordinator = BackgroundCaptureCoordinator::default();
        let first = request();
        let claim = coordinator.admit(first.clone(), false).unwrap();
        assert!(coordinator.admit(first.clone(), false).is_none());
        let mut appended = first.clone();
        appended.source.as_mut().unwrap().length += 1;
        assert!(coordinator.admit(appended.clone(), false).is_none());
        claim.finish(settled());
        coordinator
            .admit(appended.clone(), false)
            .unwrap()
            .finish(settled());
        assert!(coordinator.admit(appended, false).is_none());
    }

    #[test]
    fn cancellation_retries_without_a_parser_or_source_change() {
        let coordinator = BackgroundCaptureCoordinator::default();
        let request = request();
        drop(coordinator.admit(request.clone(), false).unwrap());
        coordinator
            .admit(request.clone(), false)
            .unwrap()
            .finish(settled());
        assert!(coordinator.admit(request, false).is_none());
    }

    #[test]
    fn old_owner_cannot_retire_or_release_replacement() {
        let coordinator = BackgroundCaptureCoordinator::default();
        let original = request();
        let old_claim = coordinator.admit(original.clone(), false).unwrap();
        let mut replacement = original;
        replacement.incarnation = Arc::new(Mutex::new("Off".into()));
        assert!(coordinator.admit(replacement.clone(), false).is_none());
        old_claim.finish(CaptureStop::Retired);
        let new_claim = coordinator.take_pending(&replacement.session_id).unwrap();
        new_claim.finish(settled());
        assert!(coordinator.admit(replacement, false).is_none());
    }

    #[test]
    fn disabled_policy_suspends_until_policy_observation_changes() {
        let coordinator = BackgroundCaptureCoordinator::default();
        let mut disabled = request();
        disabled.logging_enabled = false;
        coordinator
            .admit(disabled.clone(), false)
            .unwrap()
            .finish(CaptureStop::Disabled);
        assert!(coordinator.admit(disabled.clone(), false).is_none());
        disabled.logging_enabled = true;
        assert!(coordinator.admit(disabled, false).is_some());
    }

    #[test]
    fn actual_disabled_policy_preserves_later_unchanged_enabled_request() {
        let coordinator = BackgroundCaptureCoordinator::default();
        let enabled = request();
        coordinator
            .admit(enabled.clone(), false)
            .unwrap()
            .finish(CaptureStop::Disabled);
        assert!(coordinator.admit(enabled, false).is_some());
    }
}
