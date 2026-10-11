//! Retain display samples only, scoped to the same runtime owner as status publication.
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, Weak};

type Values = (f32, f64, u64);

struct CachedResources {
    owner: Weak<Mutex<String>>,
    process_id: Option<u32>,
    values: Values,
}

#[derive(Default)]
struct ResourceCache(HashMap<usize, CachedResources>);

impl ResourceCache {
    fn display_values(
        &mut self,
        owner: &Arc<Mutex<String>>,
        process_id: Option<u32>,
        sample: Option<Values>,
    ) -> Values {
        // Weak owners neither keep deleted runtimes alive nor allow address reuse
        // to transfer samples. Different AppStates also have different owners.
        self.0.retain(|_, cached| cached.owner.strong_count() > 0);
        let key = Arc::as_ptr(owner) as usize;
        if self
            .0
            .get(&key)
            .is_some_and(|cached| cached.process_id != process_id)
        {
            self.0.remove(&key);
        }
        if let Some(values) = sample {
            self.0.insert(
                key,
                CachedResources {
                    owner: Arc::downgrade(owner),
                    process_id,
                    values,
                },
            );
            return values;
        }
        self.0
            .get(&key)
            .filter(|cached| cached.process_id == process_id)
            .map(|cached| cached.values)
            .unwrap_or_default()
    }
}

/// A missing sample may reuse display values, but never provides liveness.
/// The status Arc is the runtime identity used by telemetry publication; PID
/// matching additionally prevents reuse after a root process changes in place.
pub(super) fn display_values(
    owner: &Arc<Mutex<String>>,
    process_id: Option<u32>,
    sample: Option<Values>,
) -> Values {
    static CACHE: OnceLock<Mutex<ResourceCache>> = OnceLock::new();
    CACHE
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .display_values(owner, process_id, sample)
}

/// Withhold rows from a runtime replaced or deleted while OS sampling ran.
/// A retired pass must not overwrite the replacement's published display data.
pub(super) async fn retain_current_runtime_metrics(
    state: &crate::state::AppState,
    observations: &[super::super::TelemetryProviderStatus],
    metrics: &mut Vec<wardian_core::models::AgentTelemetry>,
) {
    let owners: HashMap<_, _> = observations
        .iter()
        .map(|observation| (observation.session_id.as_str(), &observation.current_status))
        .collect();
    let agents = state.agents.lock().await;
    metrics.retain(|metric| {
        agents.get(&metric.session_id).is_some_and(|agent| {
            owners
                .get(metric.session_id.as_str())
                .is_some_and(|owner| Arc::ptr_eq(owner, &agent.current_status))
        })
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deleted_runtime_does_not_keep_cache_or_transfer_values() {
        let mut cache = ResourceCache::default();
        let old = Arc::new(Mutex::new("Idle".into()));
        cache.display_values(&old, Some(7), Some((12.0, 64.0, 80)));
        drop(old);
        let replacement = Arc::new(Mutex::new("Idle".into()));
        assert_eq!(
            cache.display_values(&replacement, Some(7), None),
            (0.0, 0.0, 0)
        );
        assert!(cache.0.is_empty());
    }

    #[test]
    fn changed_pid_cannot_reuse_sample_even_with_same_owner() {
        let mut cache = ResourceCache::default();
        let owner = Arc::new(Mutex::new("Idle".into()));
        cache.display_values(&owner, Some(7), Some((12.0, 64.0, 80)));
        assert_eq!(cache.display_values(&owner, Some(8), None), (0.0, 0.0, 0));
        assert_eq!(cache.display_values(&owner, None, None), (0.0, 0.0, 0));
        assert_eq!(cache.display_values(&owner, Some(7), None), (0.0, 0.0, 0));
    }
}
