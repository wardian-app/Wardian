//! Passive native-run diagnostics; these records confer no launch authority.

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::ffi::OsStr;
use std::io::{ErrorKind, Read};
use std::path::{Path, PathBuf};
use std::process::ExitStatus;

const MAX_RECORD_BYTES: usize = 4096;
const PREFIX: &str = "[native-provider-discovery] ";

struct NativeContext {
    home: PathBuf,
    run_id: String,
}

fn native_context(home: &Path, native_home: &Path, run_id: &str) -> Option<NativeContext> {
    if !home.is_absolute()
        || !native_home.is_absolute()
        || run_id.is_empty()
        || run_id.len() > 128
        || !run_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
        || home.canonicalize().ok()? != native_home.canonicalize().ok()?
        || home.symlink_metadata().ok()?.file_type().is_symlink()
    {
        return None;
    }
    let owner_path = home.join(".native-e2e-lock/owner.json");
    let metadata = owner_path.symlink_metadata().ok()?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return None;
    }
    let mut bytes = Vec::new();
    std::fs::File::open(owner_path)
        .ok()?
        .take((MAX_RECORD_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() > MAX_RECORD_BYTES {
        return None;
    }
    let owner: Value = serde_json::from_slice(&bytes).ok()?;
    (owner.get("runId")?.as_str()? == run_id).then(|| NativeContext {
        home: home.to_path_buf(),
        run_id: run_id.to_string(),
    })
}

fn argument_descriptor<'a>(arguments: impl IntoIterator<Item = &'a OsStr>) -> Value {
    let mut digest = Sha256::new();
    let mut count = 0;
    for argument in arguments {
        let bytes = argument.as_encoded_bytes();
        digest.update((bytes.len() as u64).to_le_bytes());
        digest.update(bytes);
        count += 1;
    }
    json!({ "count": count, "sha256": format!("{:x}", digest.finalize()) })
}

fn path_descriptor(value: &OsStr) -> Value {
    let text = value.to_string_lossy();
    json!({
        "text": (text.len() <= 512).then_some(text.as_ref()),
        "sha256": format!("{:x}", Sha256::digest(value.as_encoded_bytes())),
        "absolute": Path::new(value).is_absolute(),
    })
}

pub(super) struct DiscoveryWitness {
    context: NativeContext,
    metadata: Value,
    child_pid: Option<u32>,
    child_birth: Option<String>,
}

impl DiscoveryWitness {
    pub(super) fn begin(
        provider: &str,
        program: &str,
        base_arguments: &[String],
        arguments: &[&str],
        command: &tokio::process::Command,
    ) -> Option<Self> {
        let home = std::env::var_os("WARDIAN_HOME")?;
        let native_home = std::env::var_os("WARDIAN_E2E_NATIVE_HOME")?;
        let run_id = std::env::var("WARDIAN_E2E_RUN_ID").ok()?;
        let context = native_context(Path::new(&home), Path::new(&native_home), &run_id);
        Self::with_context(
            context,
            provider,
            program,
            base_arguments,
            arguments,
            command,
        )
    }

    fn with_context(
        context: Option<NativeContext>,
        provider: &str,
        program: &str,
        base_arguments: &[String],
        arguments: &[&str],
        command: &tokio::process::Command,
    ) -> Option<Self> {
        let context = context?;
        let kind = match (provider, arguments) {
            ("claude", ["--version"]) => "claude_version",
            ("claude", ["--help"]) => "claude_help",
            _ => "unexpected",
        };
        let binding = |name| std::env::var_os(name).map(|value| path_descriptor(&value));
        let metadata = json!({
            "call_id": uuid::Uuid::new_v4().to_string(),
            "native_run_id": context.run_id,
            "app_pid": std::process::id(),
            "provider": provider,
            "home": path_descriptor(context.home.as_os_str()),
            "program": path_descriptor(OsStr::new(program)),
            "effective_program": path_descriptor(command.as_std().get_program()),
            "base_arguments": argument_descriptor(base_arguments.iter().map(OsStr::new)),
            "effective_arguments": argument_descriptor(command.as_std().get_args()),
            "kind": kind,
            "fixed_flags": (kind != "unexpected").then_some(arguments),
            "claude_config_dir": binding("CLAUDE_CONFIG_DIR"),
            "codex_home": binding("CODEX_HOME"),
        });
        Some(Self {
            context,
            metadata,
            child_pid: None,
            child_birth: None,
        })
    }

    pub(super) fn bind_child(&mut self, child: &tokio::process::Child) {
        self.child_pid = child.id();
        self.child_birth = original_child_birth(child);
    }

    pub(super) fn spawned(&self) {
        self.emit("spawned", None, None);
    }

    fn record(
        &self,
        outcome: &str,
        exit: Option<&ExitStatus>,
        error: Option<ErrorKind>,
    ) -> Option<String> {
        let mut record = self.metadata.clone();
        record["outcome"] = json!(outcome);
        record["child_pid"] = json!(self.child_pid);
        // A string preserves all FILETIME bits when Node parses the record.
        record["child_birth_windows_filetime_100ns"] = json!(self.child_birth);
        record["original_identity_known"] =
            json!(self.child_pid.is_some() && self.child_birth.is_some());
        record["exit_code"] = json!(exit.and_then(ExitStatus::code));
        record["exit_success"] = json!(exit.map(ExitStatus::success));
        record["error_kind"] = json!(error.map(|kind| format!("{kind:?}")));
        let encoded = serde_json::to_string(&record).ok()?;
        // Leave one byte for the existing sink's trailing newline.
        (PREFIX.len() + encoded.len() < MAX_RECORD_BYTES).then(|| format!("{PREFIX}{encoded}"))
    }

    fn emit(&self, outcome: &str, exit: Option<&ExitStatus>, error: Option<ErrorKind>) {
        // The existing bounded debug sink must still address the captured home.
        // Missing diagnostics never change discovery's result or error behavior.
        if std::env::var_os("WARDIAN_HOME").as_deref() != Some(self.context.home.as_os_str()) {
            return;
        }
        if let Some(record) = self.record(outcome, exit, error) {
            crate::utils::logging::log_debug(&record);
        }
    }

    pub(super) fn finish(self, outcome: &str, exit: Option<&ExitStatus>, error: Option<ErrorKind>) {
        self.emit(outcome, exit, error);
    }
}

#[cfg(windows)]
fn original_child_birth(child: &tokio::process::Child) -> Option<String> {
    use winapi::shared::minwindef::FILETIME;
    use winapi::um::processthreadsapi::GetProcessTimes;
    let handle = child.raw_handle()?;
    let empty = || FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let (mut creation, mut exit, mut kernel, mut user) = (empty(), empty(), empty(), empty());
    // SAFETY: Child keeps its original process handle alive during this call;
    // all output pointers refer to initialized FILETIMEs. As in the existing
    // startup_resources reader, there is no PID lookup or ownership transfer.
    if unsafe {
        GetProcessTimes(
            handle.cast(),
            &mut creation,
            &mut exit,
            &mut kernel,
            &mut user,
        )
    } == 0
    {
        return None;
    }
    Some(
        ((u64::from(creation.dwHighDateTime) << 32) | u64::from(creation.dwLowDateTime))
            .to_string(),
    )
}

#[cfg(not(windows))]
fn original_child_birth(_child: &tokio::process::Child) -> Option<String> {
    // No original-handle birth implementation here: never infer one from a PID.
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owned_context(temp: &tempfile::TempDir) -> NativeContext {
        std::fs::create_dir(temp.path().join(".native-e2e-lock")).unwrap();
        std::fs::write(
            temp.path().join(".native-e2e-lock/owner.json"),
            r#"{"runId":"owned"}"#,
        )
        .unwrap();
        native_context(temp.path(), temp.path(), "owned").unwrap()
    }

    fn witness(temp: &tempfile::TempDir, arguments: &[&str]) -> DiscoveryWitness {
        let mut command = tokio::process::Command::new("claude");
        command.args(arguments);
        DiscoveryWitness::with_context(
            Some(owned_context(temp)),
            "claude",
            "claude",
            &[],
            arguments,
            &command,
        )
        .unwrap()
    }

    #[test]
    fn capture_requires_matching_explicit_home_and_run_claim() {
        let temp = tempfile::tempdir().unwrap();
        let _context = owned_context(&temp);
        let other = tempfile::tempdir().unwrap();
        assert!(native_context(temp.path(), other.path(), "owned").is_none());
        assert!(native_context(temp.path(), temp.path(), "foreign").is_none());
        assert!(DiscoveryWitness::with_context(
            None,
            "claude",
            "claude",
            &[],
            &["--help"],
            &tokio::process::Command::new("claude")
        )
        .is_none());
        std::fs::write(
            temp.path().join(".native-e2e-lock/owner.json"),
            vec![b'x'; MAX_RECORD_BYTES + 1],
        )
        .unwrap();
        assert!(native_context(temp.path(), temp.path(), "owned").is_none());
    }

    #[test]
    fn only_exact_claude_discovery_flags_are_classified_as_fixed() {
        for (flag, expected) in [("--version", "claude_version"), ("--help", "claude_help")] {
            let temp = tempfile::tempdir().unwrap();
            let observed = witness(&temp, &[flag]);
            assert_eq!(observed.metadata["kind"], expected);
            assert_eq!(observed.metadata["fixed_flags"], json!([flag]));
            assert_eq!(observed.metadata["effective_arguments"]["count"], 1);
        }
        let temp = tempfile::tempdir().unwrap();
        let mut command = tokio::process::Command::new("codex");
        command.arg("--help");
        let observed = DiscoveryWitness::with_context(
            Some(owned_context(&temp)),
            "codex",
            "codex",
            &[],
            &["--help"],
            &command,
        )
        .unwrap();
        assert_eq!(observed.metadata["kind"], "unexpected");
        assert!(observed.metadata["fixed_flags"].is_null());
    }

    #[test]
    fn unexpected_arguments_are_digest_only_and_records_remain_bounded() {
        let temp = tempfile::tempdir().unwrap();
        let secret = "private-prompt-or-credential".repeat(500);
        let observed = witness(&temp, &["--help", &secret]);
        assert_eq!(observed.metadata["kind"], "unexpected");
        assert!(observed.metadata["fixed_flags"].is_null());
        let record = observed
            .record("spawn_failed", None, Some(ErrorKind::NotFound))
            .unwrap();
        assert!(record.len() < MAX_RECORD_BYTES);
        assert!(!record.contains("private-prompt-or-credential"));
        assert!(record.contains("NotFound"));
        assert_ne!(
            argument_descriptor([OsStr::new("ab"), OsStr::new("c")]),
            argument_descriptor([OsStr::new("a"), OsStr::new("bc")])
        );
    }

    #[test]
    #[cfg(any(windows, unix))]
    fn recorded_exit_status_keeps_the_real_code_and_success_value() {
        let temp = tempfile::tempdir().unwrap();
        let observed = witness(&temp, &["--version"]);
        #[cfg(windows)]
        let exit = {
            use std::os::windows::process::ExitStatusExt;
            ExitStatus::from_raw(7)
        };
        #[cfg(unix)]
        let exit = {
            use std::os::unix::process::ExitStatusExt;
            ExitStatus::from_raw(7 << 8)
        };
        let record = observed.record("exited", Some(&exit), None).unwrap();
        let value: Value = serde_json::from_str(record.strip_prefix(PREFIX).unwrap()).unwrap();
        assert_eq!(value["exit_code"], 7);
        assert_eq!(value["exit_success"], false);
        assert_eq!(value["original_identity_known"], false);
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn original_child_handle_identity_survives_a_real_controlled_failure() {
        let temp = tempfile::tempdir().unwrap();
        let program = std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".to_string());
        let mut command = crate::utils::process::new_silent_command(&program);
        command
            .args(["/d", "/c", "exit", "7"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        let mut observed = DiscoveryWitness::with_context(
            Some(owned_context(&temp)),
            "unit-child",
            &program,
            &[],
            &["/d", "/c", "exit", "7"],
            &command,
        )
        .unwrap();
        let child = command.spawn().unwrap();
        let pid = child.id().unwrap();
        observed.bind_child(&child);
        assert_eq!(observed.child_pid, Some(pid));
        assert!(observed.child_birth.is_some());
        // This is an owned benign OS child, never an installed provider or App.
        let output =
            tokio::time::timeout(std::time::Duration::from_secs(5), child.wait_with_output())
                .await
                .unwrap()
                .unwrap();
        let record = observed
            .record("exited", Some(&output.status), None)
            .unwrap();
        let value: Value = serde_json::from_str(record.strip_prefix(PREFIX).unwrap()).unwrap();
        assert_eq!(value["original_identity_known"], true);
        assert_eq!(value["child_pid"], pid);
        assert_eq!(value["exit_code"], 7);
        assert_eq!(value["exit_success"], false);
    }

    #[test]
    fn unknown_original_identity_and_failure_outcomes_are_not_success_evidence() {
        let temp = tempfile::tempdir().unwrap();
        let observed = witness(&temp, &["--version"]);
        for outcome in ["spawn_failed", "wait_failed", "timeout"] {
            let record = observed.record(outcome, None, None).unwrap();
            let value: Value = serde_json::from_str(record.strip_prefix(PREFIX).unwrap()).unwrap();
            assert_eq!(value["outcome"], outcome);
            assert_eq!(value["original_identity_known"], false);
            assert!(value["child_pid"].is_null());
            assert!(value["child_birth_windows_filetime_100ns"].is_null());
            assert!(value["exit_success"].is_null());
        }
    }
}
