pub mod cli_install;
pub mod fs;
pub mod logging;
pub mod migration;
pub mod process;
pub mod shell;
pub mod terminal_input;

pub use cli_install::*;
pub use fs::*;
pub use logging::*;
pub use process::*;
pub use shell::*;
pub use terminal_input::*;

#[cfg(test)]
pub fn wardian_test_env_lock() -> std::sync::MutexGuard<'static, ()> {
    use std::sync::{Mutex, OnceLock};

    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
