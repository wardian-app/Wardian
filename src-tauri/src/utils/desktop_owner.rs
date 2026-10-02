//! Cross-process ownership of the interactive desktop for one Wardian home.
use fs2::FileExt;
use std::{
    fs::{File, OpenOptions},
    path::Path,
};

/// An OS lock, retained through the desktop event loop. The OS releases it
/// even on crash; stale file contents or a reused PID cannot exclude startup.
#[derive(Debug)]
pub(crate) struct DesktopOwner {
    file: File,
}

impl DesktopOwner {
    /// Claim the home before migrations, replacement recovery, or provider
    /// restore. Separate homes remain independent, as do headless CLI leases.
    pub(crate) fn acquire(home: &Path) -> Result<Self, String> {
        let runtime = home.join("runtime");
        std::fs::create_dir_all(&runtime)
            .map_err(|error| format!("Cannot prepare desktop ownership directory: {error}"))?;
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(runtime.join("desktop-owner.lock"))
            .map_err(|error| format!("Cannot open desktop ownership lock: {error}"))?;
        FileExt::try_lock_exclusive(&file).map_err(|error| {
            if error.raw_os_error() == fs2::lock_contended_error().raw_os_error() {
                "Another Wardian desktop is already using this home. Quit it before launching a replacement; use a separate WARDIAN_HOME for an independent instance.".into()
            } else {
                format!("Cannot claim desktop ownership: {error}")
            }
        })?;
        Ok(Self { file })
    }
}

impl Drop for DesktopOwner {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn child_attempt() {
        let Some(home) = std::env::var_os("WARDIAN_DESKTOP_LOCK_TEST_HOME") else {
            return;
        };
        let acquired = DesktopOwner::acquire(Path::new(&home));
        if std::env::var_os("WARDIAN_DESKTOP_LOCK_TEST_CONFLICT").is_some() {
            assert!(acquired.unwrap_err().contains("already using this home"));
        } else {
            assert!(acquired.is_ok());
        }
        // Deliberately bypass Drop to prove that OS process exit releases the
        // lock. No real Wardian home, provider, or user process is involved.
        std::process::exit(0);
    }

    fn child(home: &Path, conflict: bool) {
        let mut command = crate::utils::process::new_silent_std_command(
            std::env::current_exe().unwrap().to_str().unwrap(),
        );
        command
            .args([
                "--exact",
                "utils::desktop_owner::tests::child_attempt",
                "--nocapture",
            ])
            .env("WARDIAN_DESKTOP_LOCK_TEST_HOME", home)
            .env_remove("WARDIAN_DESKTOP_LOCK_TEST_CONFLICT");
        if conflict {
            command.env("WARDIAN_DESKTOP_LOCK_TEST_CONFLICT", "1");
        }
        assert!(command.status().unwrap().success());
    }

    #[test]
    fn excludes_other_processes_until_exit_and_isolates_homes() {
        let first = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let owner = DesktopOwner::acquire(first.path()).unwrap();
        child(first.path(), true);
        child(other.path(), false);
        drop(owner);
        child(first.path(), false);
        // The child exited without releasing its guard, and the persistent
        // lock file remains. Neither prevents a new desktop from starting.
        let _owner = DesktopOwner::acquire(first.path()).unwrap();
    }
}
