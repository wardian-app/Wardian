pub mod app_settings;
pub mod cli_install;
pub mod delivery_profile;
pub mod delivery_transaction;
pub mod fs;
pub mod logging;
pub mod migration;
pub mod onboarding;
pub mod process;
pub mod pty_buffer;
pub mod pty_decode;
pub mod shell;
pub mod terminal_input;

pub use app_settings::*;
pub use cli_install::*;
pub use delivery_profile::*;
pub use delivery_transaction::*;
pub use fs::*;
pub use logging::*;
pub use onboarding::*;
pub use process::*;
pub use pty_buffer::*;
pub use pty_decode::*;
pub use shell::*;
pub use terminal_input::*;

pub fn wardian_test_env_lock() -> std::sync::MutexGuard<'static, ()> {
    use std::sync::{Mutex, OnceLock};

    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
