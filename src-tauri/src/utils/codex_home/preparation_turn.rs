//! Local scheduling complements, and never replaces, the cross-process gate.
use super::{canonical, storage, validate_agent_id};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use tokio::sync::Mutex as AsyncMutex;

type Key = (PathBuf, String);
type Turns = HashMap<Key, Weak<AsyncMutex<()>>>;
static TURNS: OnceLock<Mutex<Turns>> = OnceLock::new();

/// Share one FIFO turn for owner preparation and background observation.
/// Dead entries are pruned on lookup; no global lock survives the lookup.
/// Callers must still acquire and validate the ordinary OS preparation gate.
pub(crate) fn preparation_turn(home: &Path, agent_id: &str) -> Result<Arc<AsyncMutex<()>>, String> {
    validate_agent_id(agent_id)?;
    if !home.is_absolute() {
        return Err("Codex preparation requires an absolute Wardian home".into());
    }
    let home = canonical(home)?;
    storage::plain_directory(&home)?;
    #[cfg(windows)]
    let home = PathBuf::from(home.as_os_str().to_ascii_lowercase());
    let key = (home, agent_id.to_owned());
    let mut turns = TURNS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    turns.retain(|_, turn| turn.strong_count() != 0);
    if let Some(turn) = turns.get(&key).and_then(Weak::upgrade) {
        return Ok(turn);
    }
    let turn = Arc::new(AsyncMutex::new(()));
    turns.insert(key, Arc::downgrade(&turn));
    Ok(turn)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::Future;
    use std::task::Poll;

    #[tokio::test]
    async fn startup_observation_queued_owner_has_priority_and_other_agents_are_independent() {
        let temp = tempfile::tempdir().unwrap();
        let turn = preparation_turn(temp.path(), "agent").unwrap();
        let observer = turn.clone().try_lock_owned().unwrap();
        let owner = turn.clone().lock_owned();
        tokio::pin!(owner);
        assert!(
            std::future::poll_fn(|cx| Poll::Ready(owner.as_mut().poll(cx)))
                .await
                .is_pending()
        );
        let other = preparation_turn(temp.path(), "other")
            .unwrap()
            .try_lock_owned()
            .unwrap();
        drop(observer);
        assert!(preparation_turn(temp.path(), "agent")
            .unwrap()
            .try_lock_owned()
            .is_err());
        let owner = owner.await;
        assert!(turn.clone().try_lock_owned().is_err());
        drop(owner);
        assert!(turn.try_lock_owned().is_ok());
        drop(other);
    }

    #[test]
    fn startup_observation_turn_reuses_canonical_home_and_does_not_retain_idle_mutexes() {
        let temp = tempfile::tempdir().unwrap();
        let turn = preparation_turn(temp.path(), "agent").unwrap();
        let alias = preparation_turn(&temp.path().join("."), "agent").unwrap();
        assert!(Arc::ptr_eq(&turn, &alias));
        let weak = Arc::downgrade(&turn);
        drop(turn);
        drop(alias);
        assert!(weak.upgrade().is_none());
        let _other = preparation_turn(temp.path(), "other").unwrap();
        assert!(!TURNS
            .get()
            .unwrap()
            .lock()
            .unwrap()
            .values()
            .any(|turn| Weak::ptr_eq(turn, &weak)));
    }
}
