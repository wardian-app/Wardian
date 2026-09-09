//! Admission checks for a fresh private daemon whose only thread loader is its TUI.
use super::*;
use std::path::{Path, PathBuf};

/// Resolve the owned home once, then use Codex's ordinary Windows spelling for
/// CODEX_HOME, default-socket byte validation and the proxy's socket argument.
pub(super) fn canonical_home(home: &Path) -> Result<PathBuf, CodexSharedError> {
    let canonical = std::fs::canonicalize(home)
        .map_err(|_| CodexSharedError::unsupported("cannot canonicalize owned Codex home"))?;
    #[cfg(windows)]
    {
        windows_home_spelling(&canonical)
    }
    #[cfg(not(windows))]
    {
        Ok(canonical)
    }
}

#[cfg(windows)]
fn windows_home_spelling(canonical: &Path) -> Result<PathBuf, CodexSharedError> {
    let path = canonical
        .to_str()
        .ok_or_else(|| CodexSharedError::unsupported("canonical Codex home is not valid UTF-8"))?;
    Ok(PathBuf::from(
        crate::utils::fs::strip_windows_verbatim_prefix(path),
    ))
}

/// An observed socket entry belonging to this fresh private owner. Never
/// remove by path alone after exit: another incarnation may have replaced it.
pub(super) struct OwnedSocket {
    path: PathBuf,
    identity: (u64, u64),
}

impl OwnedSocket {
    pub(super) fn capture(path: &Path) -> Result<Self, CodexSharedError> {
        Ok(Self {
            path: path.to_owned(),
            identity: socket_identity(path).map_err(|_| {
                CodexSharedError::unsupported("cannot capture owned Codex socket identity")
            })?,
        })
    }

    /// Caller has joined the daemon and proxy under the generation gate.
    pub(super) fn remove_after_exit(&self) -> Result<(), CodexSharedError> {
        match socket_identity(&self.path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Ok(identity) if identity == self.identity => {
                std::fs::remove_file(&self.path).map_err(|_| {
                    CodexSharedError::unsupported("cannot remove exited owner's Codex socket")
                })
            }
            _ => Err(CodexSharedError::unsupported(
                "Codex socket identity changed; replacement retained",
            )),
        }
    }
}

#[cfg(unix)]
fn socket_identity(path: &Path) -> std::io::Result<(u64, u64)> {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.file_type().is_socket() {
        return Err(std::io::Error::other("default socket is not a socket"));
    }
    Ok((metadata.dev(), metadata.ino()))
}

#[cfg(windows)]
fn socket_identity(path: &Path) -> std::io::Result<(u64, u64)> {
    use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
    use winapi::um::fileapi::{GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION};
    // Windows AF_UNIX endpoints are reparse points. Inspect the entry itself
    // with zero data access; never follow it or open it as a stream transport.
    let file = std::fs::OpenOptions::new()
        .access_mode(0)
        .custom_flags(0x0020_0000 | 0x0200_0000)
        .open(path)?;
    let mut information = std::mem::MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::uninit();
    // SAFETY: owned file handle and the Win32 BY_HANDLE_FILE_INFORMATION ABI.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle().cast(), information.as_mut_ptr()) }
        == 0
    {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: successful GetFileInformationByHandle initialized the structure.
    let information = unsafe { information.assume_init() };
    Ok((
        u64::from(information.dwVolumeSerialNumber),
        (u64::from(information.nFileIndexHigh) << 32) | u64::from(information.nFileIndexLow),
    ))
}

#[cfg(not(any(unix, windows)))]
fn socket_identity(_path: &Path) -> std::io::Result<(u64, u64)> {
    Err(std::io::Error::other(
        "local socket identity unsupported on this platform",
    ))
}

/// Codex uses a pathname AF_UNIX socket, including on Windows. Count encoded
/// OS bytes and reserve the terminating NUL; character counts are insufficient.
pub(super) fn default_socket(home: &Path) -> Result<PathBuf, CodexSharedError> {
    let socket = home
        .join("app-server-control")
        .join("app-server-control.sock");
    #[cfg(unix)]
    let capacity = {
        // Only inspect the platform ABI's fixed pathname capacity.
        let address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
        address.sun_path.len()
    };
    #[cfg(windows)]
    let capacity = 108;
    #[cfg(not(any(unix, windows)))]
    let capacity = 104;
    validate_socket_bytes(socket.as_os_str().as_encoded_bytes(), capacity)?;
    Ok(socket)
}

fn validate_socket_bytes(bytes: &[u8], capacity: usize) -> Result<(), CodexSharedError> {
    if bytes.contains(&0) || bytes.len() >= capacity {
        return Err(CodexSharedError::unsupported(format!(
            "canonical default Codex socket needs {} OS bytes; platform requires fewer than {capacity}; managed home relocation is not configured",
            bytes.len()
        )));
    }
    Ok(())
}

pub(super) fn require_local_version(version: &str) -> Result<(), CodexSharedError> {
    let validated = super::super::version::supported_version(&format!("wardian/{version}"))?;
    if validated == "0.154.0-alpha.6" {
        return Ok(());
    }
    let numbers: Vec<u64> = validated
        .split('+')
        .next()
        .unwrap_or_default()
        .split('.')
        .filter_map(|part| part.parse().ok())
        .collect();
    if numbers.len() == 3 && (numbers[0], numbers[1], numbers[2]) >= (0, 154, 0) {
        Ok(())
    } else {
        Err(CodexSharedError::unsupported(
            "local Codex daemon requires exact 0.154.0-alpha.6 or stable >=0.154.0 plus actual initialize/direct-input checks",
        ))
    }
}

pub(super) fn child_alive(child: &mut Child) -> Result<(), CodexSharedError> {
    match child.try_wait() {
        Ok(None) => Ok(()),
        _ => Err(CodexSharedError::unsupported(
            "captured Codex daemon is no longer alive",
        )),
    }
}

fn loaded_thread(
    result: &Value,
    expected: Option<&str>,
) -> Result<Option<String>, CodexSharedError> {
    // Limit two is sufficient: zero is pending, one may qualify, two or any
    // continuation is ambiguous. Never accept the first page of a larger set.
    let ids = result["data"]
        .as_array()
        .ok_or_else(|| CodexSharedError::unsupported("invalid loaded-thread response"))?;
    if !result.get("nextCursor").is_none_or(Value::is_null) || ids.len() > 1 {
        return Err(CodexSharedError::unsupported(
            "private Codex owner has ambiguous loaded threads",
        ));
    }
    let Some(id) = ids.first() else {
        return Ok(None);
    };
    let id = id
        .as_str()
        .filter(|id| !id.is_empty())
        .ok_or_else(|| CodexSharedError::unsupported("invalid loaded thread identity"))?;
    if expected.is_some_and(|expected| expected != id) {
        return Err(CodexSharedError::unsupported(
            "ordinary TUI loaded an unexpected resume identity",
        ));
    }
    Ok(Some(id.to_owned()))
}

async fn loaded(client: &CodexSharedClient) -> Result<Value, CodexSharedError> {
    client
        .request_with_timeout(
            "thread/loaded/list",
            json!({"limit":2}),
            Duration::from_secs(5),
        )
        .await
}

pub(super) async fn require_empty(client: &CodexSharedClient) -> Result<(), CodexSharedError> {
    if loaded_thread(&loaded(client).await?, None)?.is_some() {
        return Err(CodexSharedError::unsupported(
            "fresh Codex owner already has a loaded thread",
        ));
    }
    Ok(())
}

pub(super) async fn require_observed_thread(
    client: &CodexSharedClient,
    id: &str,
) -> Result<(), CodexSharedError> {
    if loaded_thread(&loaded(client).await?, Some(id))?.as_deref() != Some(id) {
        return Err(CodexSharedError::unsupported(
            "observed TUI thread disappeared during attachment",
        ));
    }
    Ok(())
}

pub(super) async fn wait_for_tui_thread(
    client: &CodexSharedClient,
    expected: Option<&str>,
    alive: &mut impl FnMut() -> Result<(), CodexSharedError>,
) -> Result<String, CodexSharedError> {
    loop {
        alive()?;
        let result = loaded(client).await?;
        alive()?;
        if let Some(id) = loaded_thread(&result, expected)? {
            return Ok(id);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

pub(super) async fn inject_initial_context(
    client: &CodexSharedClient,
    id: &str,
    context: &str,
) -> Result<(), CodexSharedError> {
    // This observed ID is not yet a client binding. Acknowledged developer
    // context materializes a fresh rollout without submitting a model turn.
    let result = client
        .request_with_timeout(
            "thread/inject_items",
            json!({
                "threadId":id,"items":[{"type":"message","role":"developer",
                "content":[{"type":"input_text","text":context}]}]
            }),
            STARTUP_TIMEOUT,
        )
        .await?;
    if result != json!({}) {
        return Err(CodexSharedError::unsupported(
            "unexpected startup context acknowledgement",
        ));
    }
    Ok(())
}

/// Check effective thread policy, not just the arguments sent to the daemon.
pub(super) struct ExpectedPolicy(Vec<(String, String)>);

impl ExpectedPolicy {
    pub(super) fn from_server_args(args: &[String]) -> Result<Self, CodexSharedError> {
        let mut fields = Vec::new();
        for pair in args.windows(2).filter(|pair| pair[0] == "-c") {
            let document: toml_edit::DocumentMut = pair[1]
                .parse()
                .map_err(|_| CodexSharedError::unsupported("invalid generated server policy"))?;
            for (key, response_key) in [
                ("model", "model"),
                ("approval_policy", "approvalPolicy"),
                ("approvals_reviewer", "approvalsReviewer"),
                ("sandbox_mode", "sandbox"),
                ("model_reasoning_effort", "reasoningEffort"),
            ] {
                if let Some(value) = document.get(key).and_then(toml_edit::Item::as_str) {
                    fields.push((response_key.to_owned(), value.to_owned()));
                }
            }
        }
        Ok(Self(fields))
    }

    /// Validate the transient selection without changing the saved agent settings.
    pub(super) fn expect_launch_model(
        &mut self,
        model: Option<&str>,
    ) -> Result<(), CodexSharedError> {
        let Some(model) = model else { return Ok(()) };
        if let Some((_, configured)) = self.0.iter().find(|(key, _)| key == "model") {
            if configured != model {
                return Err(CodexSharedError::unsupported(
                    "resolved Codex launch model changed configured model",
                ));
            }
        } else {
            self.0.push(("model".into(), model.into()));
        }
        Ok(())
    }

    /// Stock Codex clears an unset persisted effort on a cold ID-only resume,
    /// even when the daemon received an explicit effort override. Carry the
    /// agent's configured preferences across that per-thread boundary as well.
    pub(super) fn background_resume_params(&self, thread_id: &str) -> Value {
        let mut params = json!({"threadId": thread_id});
        for (key, value) in &self.0 {
            match key.as_str() {
                "model" => params["model"] = json!(value),
                "reasoningEffort" => {
                    params["config"] = json!({"model_reasoning_effort": value});
                }
                _ => {}
            }
        }
        params
    }

    pub(super) fn validate(&self, response: &Value) -> Result<(), CodexSharedError> {
        for (key, expected) in &self.0 {
            let expected = if key == "sandbox" {
                match expected.as_str() {
                    "danger-full-access" => "dangerFullAccess",
                    "workspace-write" => "workspaceWrite",
                    "read-only" => "readOnly",
                    value => value,
                }
            } else {
                expected.as_str()
            };
            let actual = if key == "sandbox" {
                &response[key]["type"]
            } else {
                &response[key]
            };
            if actual.as_str() != Some(expected) {
                return Err(CodexSharedError::unsupported(format!(
                    "local Codex thread changed configured {key}; attachment rejected"
                )));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn canonical_home_spelling_normalizes_drive_and_unc_without_loss() {
        for (canonical, ordinary) in [
            (r"\\?\C:\owned\codex", r"C:\owned\codex"),
            (r"\\?\UNC\server\share\codex", r"\\server\share\codex"),
            (r"C:\owned\codex", r"C:\owned\codex"),
            (r"\\server\share\codex", r"\\server\share\codex"),
            (r"\\?\C:\owned\é", r"C:\owned\é"),
        ] {
            assert_eq!(
                windows_home_spelling(Path::new(canonical)).unwrap(),
                PathBuf::from(ordinary)
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn default_socket_limit_counts_provider_spelling_without_verbatim_overhead() {
        for ordinary in [r"C:\", r"\\server\share\"] {
            let suffix_bytes = Path::new(ordinary)
                .join("app-server-control")
                .join("app-server-control.sock")
                .as_os_str()
                .as_encoded_bytes()
                .len();
            // Adding a directory adds one separator to the existing suffix.
            let home = PathBuf::from(format!("{ordinary}{}", "a".repeat(106 - suffix_bytes)));
            let text = home.to_str().unwrap();
            let verbatim = if let Some(unc) = text.strip_prefix(r"\\") {
                format!(r"\\?\UNC\{unc}")
            } else {
                format!(r"\\?\{text}")
            };
            assert!(default_socket(Path::new(&verbatim)).is_err());
            let normalized = windows_home_spelling(Path::new(&verbatim)).unwrap();
            let socket = default_socket(&normalized).unwrap();
            assert_eq!(socket.as_os_str().as_encoded_bytes().len(), 107);
            assert_eq!(normalized, home);
            assert!(default_socket(&PathBuf::from(format!("{text}a"))).is_err());
        }
    }

    #[cfg(windows)]
    #[test]
    fn canonical_home_spelling_rejects_unpaired_utf16() {
        use std::os::windows::ffi::OsStringExt;
        let mut units: Vec<u16> = r"\\?\C:\owned\".encode_utf16().collect();
        units.push(0xd800);
        let path = PathBuf::from(std::ffi::OsString::from_wide(&units));
        assert!(windows_home_spelling(&path).is_err());
    }

    #[test]
    fn local_version_rejects_older_shared_owner_versions_and_adjacent_alphas() {
        for version in ["0.154.0-alpha.6", "0.154.0", "0.155.0+build.1", "1.0.0"] {
            assert!(require_local_version(version).is_ok(), "{version}");
        }
        for version in [
            "0.153.4",
            "0.153.99",
            "0.154.0-alpha.7",
            "0.154.0-alpha.6+build",
            "00.154.0",
        ] {
            assert!(require_local_version(version).is_err(), "{version}");
        }
    }

    #[test]
    fn socket_capacity_reserves_nul_and_counts_multibyte_encoding() {
        assert!(validate_socket_bytes(&[b'a'; 107], 108).is_ok());
        assert!(validate_socket_bytes(&[b'a'; 108], 108).is_err());
        assert!(validate_socket_bytes("é".repeat(54).as_bytes(), 108).is_err());
        assert!(validate_socket_bytes(b"a\0b", 108).is_err());
        assert!(validate_socket_bytes(&[b'a'; 104], 104).is_err());
    }

    #[test]
    fn exclusive_membership_rejects_wrong_identity_ambiguity_and_pagination() {
        assert_eq!(
            loaded_thread(&json!({"data":[],"nextCursor":null}), None).unwrap(),
            None
        );
        assert_eq!(
            loaded_thread(&json!({"data":["a"],"nextCursor":null}), Some("a"))
                .unwrap()
                .as_deref(),
            Some("a")
        );
        for result in [
            json!({"data":["b"]}),
            json!({"data":["a","b"]}),
            json!({"data":["a"],"nextCursor":"more"}),
            json!({"data":[null]}),
            json!({}),
        ] {
            assert!(loaded_thread(&result, Some("a")).is_err(), "{result}");
        }
    }

    #[test]
    fn effective_policy_mismatch_never_qualifies_for_binding() {
        let args: Vec<String> = [
            "app-server",
            "-c",
            "model=\"selected\"",
            "-c",
            "approval_policy=\"on-request\"",
            "-c",
            "sandbox_mode=\"workspace-write\"",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        let policy = ExpectedPolicy::from_server_args(&args).unwrap();
        let response = json!({"model":"selected", "approvalPolicy":"on-request", "sandbox":{"type":"workspaceWrite"}});
        assert!(policy.validate(&response).is_ok());
        for (key, value) in [
            ("model", json!("default")),
            ("approvalPolicy", json!("never")),
            ("sandbox", json!({"type":"dangerFullAccess"})),
        ] {
            let mut changed = response.clone();
            changed[key] = value;
            assert!(policy.validate(&changed).is_err());
        }
        assert!(policy.validate(&json!({})).is_err());
    }

    #[test]
    fn selected_luna_and_low_effort_must_both_match() {
        let args: Vec<String> = [
            "app-server",
            "-c",
            "model=\"gpt-5.6-luna\"",
            "-c",
            "model_reasoning_effort=\"low\"",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        let policy = ExpectedPolicy::from_server_args(&args).unwrap();
        assert_eq!(
            policy.background_resume_params("fresh-seed"),
            json!({"threadId":"fresh-seed", "model":"gpt-5.6-luna",
                "config":{"model_reasoning_effort":"low"}})
        );
        assert!(policy
            .validate(&json!({"model":"gpt-5.6-luna", "reasoningEffort":"low"}))
            .is_ok());
        for response in [
            json!({"model":"gpt-5.6-luna", "reasoningEffort":"high"}),
            json!({"model":"unexpected-model", "reasoningEffort":"low"}),
            json!({"model":"gpt-5.6-luna", "reasoningEffort":null}),
            json!({"model":"gpt-5.6-luna"}),
        ] {
            assert!(policy.validate(&response).is_err());
        }
        assert_eq!(
            ExpectedPolicy::from_server_args(&[])
                .unwrap()
                .background_resume_params("inherit-defaults"),
            json!({"threadId":"inherit-defaults"})
        );
        let mut inherited = ExpectedPolicy::from_server_args(&[
            "-c".into(),
            "model_reasoning_effort=\"low\"".into(),
        ])
        .unwrap();
        inherited
            .expect_launch_model(Some("historical-model"))
            .unwrap();
        assert!(inherited
            .validate(&json!({"model":"historical-model","reasoningEffort":"low"}))
            .is_ok());
        assert!(inherited
            .validate(&json!({"model":"changed-model","reasoningEffort":"low"}))
            .is_err());
        assert!(inherited
            .expect_launch_model(Some("changed-model"))
            .is_err());
    }

    #[cfg(unix)]
    #[test]
    fn joined_socket_cleanup_preserves_a_replacement_entry() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("control.sock");
        let first = std::os::unix::net::UnixListener::bind(&path).unwrap();
        let owned = OwnedSocket::capture(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        let second = std::os::unix::net::UnixListener::bind(&path).unwrap();
        assert!(owned.remove_after_exit().is_err());
        assert!(path.exists());
        drop((first, second));
    }
}
