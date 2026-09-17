//! Persistence and matching for generic automation session-close invokers.

use crate::models::AutomationAssignments;
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::OpenOptions;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AutomationSessionCloseInvoker {
    pub id: String,
    pub blueprint_id: String,
    pub name: String,
    #[serde(default)]
    pub enabled: bool,
    /// Skip this invocation when the lifecycle boundary has no durable
    /// conversation archive. This remains generic automation behavior.
    #[serde(default)]
    pub require_archive: bool,
    #[serde(default)]
    pub source_agent_id: Option<String>,
    #[serde(default)]
    pub boundary_reasons: Vec<String>,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub workspace: Option<String>,
    #[serde(default)]
    pub input: serde_json::Value,
    #[serde(default)]
    pub bindings: HashMap<String, String>,
    #[serde(default)]
    pub assignments: AutomationAssignments,
}

pub fn load_invokers() -> Vec<AutomationSessionCloseInvoker> {
    let Some(path) = crate::paths::session_close_invokers_path() else {
        return Vec::new();
    };
    std::fs::read_to_string(path)
        .ok()
        .and_then(|body| serde_json::from_str(&body).ok())
        .unwrap_or_default()
}

pub fn save_invokers(invokers: &[AutomationSessionCloseInvoker]) -> std::io::Result<()> {
    mutate_invokers(|stored| {
        *stored = invokers.to_vec();
        Ok(())
    })
}

/// Serialize the complete read-modify-write operation across app and CLI
/// processes. Atomic replacement alone prevents torn JSON, not lost updates.
pub fn mutate_invokers<T>(
    mutate: impl FnOnce(&mut Vec<AutomationSessionCloseInvoker>) -> std::io::Result<T>,
) -> std::io::Result<T> {
    let path = crate::paths::session_close_invokers_path().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::NotFound, "Wardian home is unavailable")
    })?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let lock_path = path.with_extension("lock");
    let lock = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(lock_path)?;
    FileExt::lock_exclusive(&lock)?;
    let mut invokers = read_for_mutation(&path)?;
    let result = mutate(&mut invokers)?;
    crate::atomic_file::write_json_atomic(&path, &invokers)?;
    Ok(result)
}

/// Read the current invoker set for a read-modify-write, failing closed on a
/// file that exists but cannot be parsed or decoded.
///
/// A missing file is an empty set because that is the fresh-install state.
/// Treating damaged bytes as an empty set would let the next mutation
/// overwrite every configured session-close invoker.
fn read_for_mutation(
    path: &std::path::Path,
) -> std::io::Result<Vec<AutomationSessionCloseInvoker>> {
    match std::fs::read_to_string(path) {
        Ok(body) => serde_json::from_str(&body).map_err(|error| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "refusing to overwrite malformed {}: {error}",
                    path.display()
                ),
            )
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error),
    }
}

pub fn matching_invokers(
    agent_id: &str,
    boundary_reason: &str,
) -> Vec<AutomationSessionCloseInvoker> {
    load_invokers()
        .into_iter()
        .filter(|invoker| {
            invoker.enabled
                && invoker
                    .source_agent_id
                    .as_deref()
                    .is_none_or(|source| source == agent_id)
                && (invoker.boundary_reasons.is_empty()
                    || invoker
                        .boundary_reasons
                        .iter()
                        .any(|reason| reason == boundary_reason))
        })
        .collect()
}

pub fn archive_requirement_satisfied(
    invoker: &AutomationSessionCloseInvoker,
    archive_available: bool,
) -> bool {
    !invoker.require_archive || archive_available
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestHome {
        _guard: std::sync::MutexGuard<'static, ()>,
        _home: tempfile::TempDir,
        previous: Option<std::ffi::OsString>,
    }

    impl TestHome {
        fn new() -> Self {
            let guard = crate::tests::env_lock();
            let home = tempfile::tempdir().expect("temp home");
            let previous = std::env::var_os("WARDIAN_HOME");
            std::env::set_var("WARDIAN_HOME", home.path());
            Self {
                _guard: guard,
                _home: home,
                previous,
            }
        }
    }

    impl Drop for TestHome {
        fn drop(&mut self) {
            match self.previous.take() {
                Some(value) => std::env::set_var("WARDIAN_HOME", value),
                None => std::env::remove_var("WARDIAN_HOME"),
            }
        }
    }

    fn invoker(id: &str) -> AutomationSessionCloseInvoker {
        AutomationSessionCloseInvoker {
            id: id.into(),
            blueprint_id: "memory-consolidation".into(),
            name: id.into(),
            enabled: false,
            require_archive: false,
            source_agent_id: None,
            boundary_reasons: vec![],
            provider: None,
            workspace: None,
            input: serde_json::json!({}),
            bindings: HashMap::new(),
            assignments: AutomationAssignments::new(),
        }
    }

    #[test]
    fn matching_is_disabled_by_default_and_honors_filters() {
        let invoker = AutomationSessionCloseInvoker {
            id: "one".into(),
            blueprint_id: "memory-consolidation".into(),
            name: "Memory".into(),
            enabled: true,
            require_archive: true,
            source_agent_id: Some("agent-a".into()),
            boundary_reasons: vec!["clear".into()],
            provider: Some("codex".into()),
            workspace: None,
            input: serde_json::json!({}),
            bindings: HashMap::new(),
            assignments: AutomationAssignments::new(),
        };
        assert!(invoker.enabled);
        assert_eq!(invoker.source_agent_id.as_deref(), Some("agent-a"));
        assert!(invoker.boundary_reasons.contains(&"clear".to_string()));
        assert!(!archive_requirement_satisfied(&invoker, false));
        assert!(archive_requirement_satisfied(&invoker, true));
    }

    #[test]
    fn concurrent_mutations_preserve_every_invoker() {
        let _home = TestHome::new();
        let threads = (0..8)
            .map(|index| {
                std::thread::spawn(move || {
                    mutate_invokers(|invokers| {
                        invokers.push(invoker(&format!("invoker-{index}")));
                        Ok(())
                    })
                    .expect("mutate invokers");
                })
            })
            .collect::<Vec<_>>();
        for thread in threads {
            thread.join().expect("join invoker writer");
        }
        let stored = load_invokers();
        assert_eq!(stored.len(), 8);
        for index in 0..8 {
            assert!(stored
                .iter()
                .any(|item| item.id == format!("invoker-{index}")));
        }
    }

    #[test]
    fn a_missing_config_is_still_an_empty_set() {
        let _home = TestHome::new();
        mutate_invokers(|stored| {
            stored.push(invoker("fresh"));
            Ok(())
        })
        .expect("a fresh install writes its first invoker");

        assert_eq!(load_invokers(), vec![invoker("fresh")]);
    }

    #[test]
    fn a_malformed_config_is_never_overwritten() {
        let _home = TestHome::new();
        let path = crate::paths::session_close_invokers_path().expect("path");
        std::fs::create_dir_all(path.parent().expect("parent")).expect("library dir");
        let original = b"{ not json";
        std::fs::write(&path, original).expect("seed damage");

        let error = mutate_invokers(|stored| {
            stored.clear();
            Ok(())
        })
        .expect_err("a malformed config must fail closed");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("refusing to overwrite"));
        assert_eq!(std::fs::read(&path).expect("read back"), original);
    }

    #[test]
    fn undecodable_config_bytes_are_never_overwritten() {
        let _home = TestHome::new();
        let path = crate::paths::session_close_invokers_path().expect("path");
        std::fs::create_dir_all(path.parent().expect("parent")).expect("library dir");
        let original = [0xff, 0xfe, 0x00];
        std::fs::write(&path, original).expect("seed undecodable bytes");

        let error = mutate_invokers(|stored| {
            stored.clear();
            Ok(())
        })
        .expect_err("undecodable config bytes must fail closed");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert_eq!(std::fs::read(&path).expect("read back"), original);
    }
}
