pub mod control;
pub mod conversation_lease;
pub mod conversations;
pub mod db;
pub mod engine;
pub mod identity;
pub mod library;
pub mod models;
pub mod paths;
pub mod schedule;
pub mod topology;
pub mod workflow;

#[cfg(test)]
mod tests {
    use once_cell::sync::Lazy;
    use std::sync::{Mutex, MutexGuard};

    static ENV_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

    pub fn env_lock() -> MutexGuard<'static, ()> {
        ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}
