//! Short-lived file overlay for ordinary Codex TUI startup only.
//! The daemon retains authoritative -c arguments; background launches need no overlay.

mod journal;
mod leaves;
mod storage;
#[cfg(test)]
mod tests;

use super::CodexSharedError;
use std::path::{Path, PathBuf};
use storage::Snapshot;
use toml_edit::DocumentMut;

/// An interactive-startup overlay, held until the TUI has loaded and verified
/// configuration. Explicit restore must precede capable binding publication.
/// On failed startup, join owned readers before restoring or dropping the guard.
#[must_use = "hold until configuration is loaded, then explicitly restore"]
pub(super) struct LaunchConfigGuard {
    home: PathBuf,
    token: Option<String>,
}

impl LaunchConfigGuard {
    /// Restore matching applied leaves, preserving external value/shape edits.
    /// Repeated restore and stale-token cleanup are harmless. The caller holds
    /// its serialized private-home lifecycle gate throughout this operation.
    pub(super) fn restore(&mut self) -> Result<(), CodexSharedError> {
        if let Some(token) = &self.token {
            restore_owned(&self.home, Some(token))?;
            self.token = None;
        }
        Ok(())
    }
}

impl Drop for LaunchConfigGuard {
    fn drop(&mut self) {
        // Fallback only: explicit restoration reports errors; a failed fallback
        // retains the write-ahead journal for the next serialized recovery.
        let _ = self.restore();
    }
}

/// Recover an interrupted startup before computing arguments which read config
/// (notably writable roots). No readers of the previous startup may remain.
/// Also used before a background start if an interrupted interactive journal
/// exists; this recovers old state and does not create a background overlay.
pub(super) fn recover_launch_config(home: &Path) -> Result<(), CodexSharedError> {
    restore_owned(home, None)
}

/// Prepare a startup-only overlay in the caller's known private agent home.
/// Input is the exact generated app-server/-c sequence, without executable or
/// listen prefixes. All input validates before recovery/mutation. Recovery runs
/// again here; callers must ALSO recover before config-derived args generation.
/// Call only for interactive startup, with no prior readers and under the
/// lifecycle gate. Keep the guard until both clients read and policy verifies.
pub(super) fn prepare_launch_config(
    home: &Path,
    generated_args: &[String],
) -> Result<LaunchConfigGuard, CodexSharedError> {
    let overrides = leaves::parse(generated_args)?;
    recover_launch_config(home)?;
    let config_path = home.join("config.toml");
    let journal_path = home.join(journal::FILE);
    let before = storage::read_snapshot(&config_path)?;
    let mut document = parse_config(&before)?;
    let changes = leaves::apply(&mut document, overrides)?;
    let rendered = render(&document, &before)?;
    let journal = journal::Journal::new(home, changes)?;
    let encoded = journal.encode(home)?;
    let absent = None;
    // A complete, synced journal is visible before any config replacement.
    storage::publish(
        &journal_path,
        &absent,
        encoded.as_bytes(),
        Some((&config_path, &before)),
    )?;
    let guard = LaunchConfigGuard {
        home: home.to_owned(),
        token: Some(journal.token),
    };
    let recorded = storage::read_snapshot(&journal_path)?;
    if recorded.as_ref().map(|snapshot| snapshot.text.as_str()) != Some(encoded.as_str()) {
        return Err(failure(
            "Codex launch journal changed before config publication",
        ));
    }
    write_launch_config(home, &before, &rendered, &recorded)?;
    Ok(guard)
}

fn restore_owned(home: &Path, expected_token: Option<&str>) -> Result<(), CodexSharedError> {
    storage::validate_home(home)?;
    let journal_path = home.join(journal::FILE);
    let recorded = storage::read_snapshot(&journal_path)?;
    let Some(snapshot) = &recorded else {
        // Even with no journal, refuse unsafe config before a managed launch.
        let config = storage::read_snapshot(&home.join("config.toml"))?;
        parse_config(&config)?;
        return Ok(());
    };
    let journal = journal::Journal::decode(&snapshot.text, home)?;
    if expected_token.is_some_and(|token| token != journal.token) {
        return Ok(()); // A stale guard never mutates the next startup's files.
    }
    let before = storage::read_snapshot(&home.join("config.toml"))?;
    let mut document = parse_config(&before)?;
    leaves::restore(&mut document, &journal.changes)?;
    let rendered = render(&document, &before)?;
    write_launch_config(home, &before, &rendered, &recorded)?;
    // Crash here is safe: an intact journal plus already-restored leaf values
    // replays as a no-op. Divergent external edits become the new baseline.
    storage::remove(&journal_path, &recorded)
}

fn parse_config(snapshot: &Option<Snapshot>) -> Result<DocumentMut, CodexSharedError> {
    snapshot
        .as_ref()
        .map_or("", |snapshot| snapshot.text.as_str())
        .parse()
        .map_err(|_| failure("invalid existing Codex launch TOML"))
}

fn render(document: &DocumentMut, before: &Option<Snapshot>) -> Result<String, CodexSharedError> {
    let text = document.to_string();
    let text = if before
        .as_ref()
        .is_some_and(|snapshot| snapshot.text.contains("\r\n"))
    {
        text.replace("\r\n", "\n").replace('\n', "\r\n")
    } else {
        text
    };
    text.parse::<DocumentMut>()
        .map_err(|_| failure("invalid merged Codex launch TOML"))?;
    Ok(text)
}

// Private publication mechanism; the former permanent public writer is gone.
fn write_launch_config(
    home: &Path,
    before: &Option<Snapshot>,
    rendered: &str,
    recorded: &Option<Snapshot>,
) -> Result<(), CodexSharedError> {
    let path = home.join("config.toml");
    let journal_path = home.join(journal::FILE);
    if rendered
        == before
            .as_ref()
            .map_or("", |snapshot| snapshot.text.as_str())
    {
        storage::compare_before_publish(&journal_path, recorded)?;
        return storage::compare_before_publish(&path, before);
    }
    storage::publish(
        &path,
        before,
        rendered.as_bytes(),
        Some((&journal_path, recorded)),
    )
}

fn failure(message: &str) -> CodexSharedError {
    CodexSharedError::unsupported(message)
}

fn io_failure(error: std::io::Error) -> CodexSharedError {
    failure(&format!("Codex launch file I/O failed: {error}"))
}
