//! Chromium engine discovery and launch for browser sessions.
//!
//! A browser surface is backed by an out-of-process Chromium spoken to over the
//! Chrome DevTools Protocol. This module owns finding a usable binary on the
//! host and starting it with an isolated profile and a loopback debug port.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use tokio::process::{Child, Command};
use tokio::time::{sleep, Instant};
pub use wardian_core::browser::EngineKind;

/// Environment override naming an explicit Chromium binary.
pub const ENGINE_BINARY_ENV: &str = "WARDIAN_BROWSER_BINARY";

/// How long to wait for the launched browser to publish its debug port.
const ENDPOINT_TIMEOUT: Duration = Duration::from_secs(20);
const ENDPOINT_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// A Chromium binary that exists on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineBinary {
    pub kind: EngineKind,
    pub path: PathBuf,
}

/// Why no browser session could be started.
#[derive(Debug)]
pub enum EngineError {
    /// No Chromium-family browser was found on the host.
    NotFound { searched: Vec<String> },
    /// `WARDIAN_BROWSER_BINARY` was set but does not point at a file.
    OverrideMissing { path: PathBuf },
    /// The browser process could not be spawned.
    Spawn { path: PathBuf, source: String },
    /// The browser started but never published a DevTools endpoint.
    EndpointTimeout { path: PathBuf },
    /// The browser exited before it was ready.
    Exited { path: PathBuf, status: String },
}

impl std::fmt::Display for EngineError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EngineError::NotFound { searched } => write!(
                formatter,
                "no Chromium-based browser was found. Install Microsoft Edge, Google Chrome, or Chromium, or set {ENGINE_BINARY_ENV} to a browser executable. Searched: {}",
                if searched.is_empty() {
                    "nothing".to_string()
                } else {
                    searched.join(", ")
                }
            ),
            EngineError::OverrideMissing { path } => write!(
                formatter,
                "{ENGINE_BINARY_ENV} points at {}, which is not an executable file",
                path.display()
            ),
            EngineError::Spawn { path, source } => write!(
                formatter,
                "failed to start {}: {source}",
                path.display()
            ),
            EngineError::EndpointTimeout { path } => write!(
                formatter,
                "{} started but did not publish a DevTools endpoint within {} seconds",
                path.display(),
                ENDPOINT_TIMEOUT.as_secs()
            ),
            EngineError::Exited { path, status } => write!(
                formatter,
                "{} exited before becoming ready ({status})",
                path.display()
            ),
        }
    }
}

impl std::error::Error for EngineError {}

/// Candidate binaries in preference order for the current platform.
///
/// Edge leads on Windows because it is present on every machine that can run
/// Wardian at all; Chrome leads elsewhere for the same reason on those hosts.
pub fn engine_candidates() -> Vec<EngineBinary> {
    #[cfg(target_os = "windows")]
    {
        let program_files = std::env::var("ProgramFiles").unwrap_or_default();
        let program_files_x86 = std::env::var("ProgramFiles(x86)").unwrap_or_default();
        let local_app_data = std::env::var("LOCALAPPDATA").unwrap_or_default();
        let mut candidates = Vec::new();
        for (kind, relative) in [
            (EngineKind::Edge, r"Microsoft\Edge\Application\msedge.exe"),
            (EngineKind::Chrome, r"Google\Chrome\Application\chrome.exe"),
            (
                EngineKind::Brave,
                r"BraveSoftware\Brave-Browser\Application\brave.exe",
            ),
            (EngineKind::Chromium, r"Chromium\Application\chrome.exe"),
        ] {
            for root in [&program_files_x86, &program_files, &local_app_data] {
                if root.is_empty() {
                    continue;
                }
                candidates.push(EngineBinary {
                    kind,
                    path: Path::new(root).join(relative),
                });
            }
        }
        candidates
    }
    #[cfg(target_os = "macos")]
    {
        [
            (
                EngineKind::Chrome,
                "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
            ),
            (
                EngineKind::Edge,
                "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
            ),
            (
                EngineKind::Brave,
                "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser",
            ),
            (
                EngineKind::Chromium,
                "/Applications/Chromium.app/Contents/MacOS/Chromium",
            ),
        ]
        .into_iter()
        .map(|(kind, path)| EngineBinary {
            kind,
            path: PathBuf::from(path),
        })
        .collect()
    }
    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    {
        let mut candidates = Vec::new();
        for (kind, name) in [
            (EngineKind::Chrome, "google-chrome"),
            (EngineKind::Chrome, "google-chrome-stable"),
            (EngineKind::Chromium, "chromium"),
            (EngineKind::Chromium, "chromium-browser"),
            (EngineKind::Edge, "microsoft-edge"),
            (EngineKind::Brave, "brave-browser"),
        ] {
            for root in [
                "/usr/bin",
                "/usr/local/bin",
                "/snap/bin",
                "/opt/google/chrome",
            ] {
                candidates.push(EngineBinary {
                    kind,
                    path: Path::new(root).join(name),
                });
            }
        }
        candidates
    }
}

/// Resolves the browser this host should use, honoring the env override first.
pub fn discover_engine() -> Result<EngineBinary, EngineError> {
    if let Some(override_path) = std::env::var_os(ENGINE_BINARY_ENV) {
        let path = PathBuf::from(override_path);
        if path.as_os_str().is_empty() {
            // An empty override is treated as unset rather than as an error, so
            // a cleared variable cannot wedge every session.
        } else if path.is_file() {
            return Ok(EngineBinary {
                kind: EngineKind::Custom,
                path,
            });
        } else {
            return Err(EngineError::OverrideMissing { path });
        }
    }

    let candidates = engine_candidates();
    for candidate in &candidates {
        if candidate.path.is_file() {
            return Ok(candidate.clone());
        }
    }
    Err(EngineError::NotFound {
        searched: candidates
            .iter()
            .map(|candidate| candidate.path.display().to_string())
            .collect(),
    })
}

/// A running browser process and the endpoint that controls it.
#[derive(Debug)]
pub struct LaunchedEngine {
    pub kind: EngineKind,
    pub websocket_url: String,
    pub child: Child,
}

/// Command-line flags applied to every launched browser.
///
/// The profile is isolated per session, so an agent-driven browser never
/// inherits the human's cookies or signed-in state.
fn launch_flags(user_data_dir: &Path, width: u32, height: u32) -> Vec<String> {
    vec![
        "--remote-debugging-port=0".to_string(),
        format!("--user-data-dir={}", user_data_dir.display()),
        "--headless=new".to_string(),
        "--no-first-run".to_string(),
        "--no-default-browser-check".to_string(),
        "--disable-background-networking".to_string(),
        "--disable-sync".to_string(),
        "--disable-extensions".to_string(),
        "--disable-component-update".to_string(),
        "--mute-audio".to_string(),
        "--hide-scrollbars".to_string(),
        format!("--window-size={width},{height}"),
        "about:blank".to_string(),
    ]
}

/// Starts a browser with an isolated profile and waits for its debug endpoint.
pub async fn launch_engine(
    binary: &EngineBinary,
    user_data_dir: &Path,
    width: u32,
    height: u32,
) -> Result<LaunchedEngine, EngineError> {
    std::fs::create_dir_all(user_data_dir).map_err(|error| EngineError::Spawn {
        path: binary.path.clone(),
        source: format!("could not create the session profile directory: {error}"),
    })?;
    // A stale port file from a crashed predecessor would otherwise be read as
    // this launch's endpoint.
    let port_file = user_data_dir.join("DevToolsActivePort");
    let _ = std::fs::remove_file(&port_file);

    let mut command = Command::new(&binary.path);
    command
        .args(launch_flags(user_data_dir, width, height))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    #[cfg(target_os = "windows")]
    {
        // Keep the headless browser from flashing a console window.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    let mut child = crate::utils::process::spawn_owned_command(&mut command).map_err(|error| {
        EngineError::Spawn {
            path: binary.path.clone(),
            source: error.to_string(),
        }
    })?;

    let deadline = Instant::now() + ENDPOINT_TIMEOUT;
    loop {
        if let Some(url) = read_endpoint(&port_file) {
            return Ok(LaunchedEngine {
                kind: binary.kind,
                websocket_url: url,
                child,
            });
        }
        if let Ok(Some(status)) = child.try_wait() {
            return Err(EngineError::Exited {
                path: binary.path.clone(),
                status: status.to_string(),
            });
        }
        if Instant::now() >= deadline {
            let _ = child.kill().await;
            return Err(EngineError::EndpointTimeout {
                path: binary.path.clone(),
            });
        }
        sleep(ENDPOINT_POLL_INTERVAL).await;
    }
}

/// Parses Chromium's `DevToolsActivePort` file into a browser websocket URL.
///
/// The file holds the chosen port on the first line and the browser target's
/// websocket path on the second.
pub fn parse_endpoint(contents: &str) -> Option<String> {
    let mut lines = contents.lines();
    let port: u16 = lines.next()?.trim().parse().ok()?;
    let path = lines.next().unwrap_or("").trim();
    if path.is_empty() {
        return None;
    }
    let path = path.strip_prefix('/').unwrap_or(path);
    Some(format!("ws://127.0.0.1:{port}/{path}"))
}

fn read_endpoint(port_file: &Path) -> Option<String> {
    parse_endpoint(&std::fs::read_to_string(port_file).ok()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_complete_port_file() {
        let parsed = parse_endpoint("52134\n/devtools/browser/abc-123\n");
        assert_eq!(
            parsed.as_deref(),
            Some("ws://127.0.0.1:52134/devtools/browser/abc-123")
        );
    }

    #[test]
    fn rejects_a_port_file_that_is_still_being_written() {
        assert_eq!(parse_endpoint("52134\n"), None);
        assert_eq!(parse_endpoint("52134"), None);
        assert_eq!(parse_endpoint(""), None);
    }

    #[test]
    fn rejects_a_port_file_with_a_non_numeric_port() {
        assert_eq!(parse_endpoint("not-a-port\n/devtools/browser/x"), None);
    }

    #[test]
    fn candidates_are_offered_in_a_stable_preference_order() {
        let candidates = engine_candidates();
        assert!(
            !candidates.is_empty(),
            "every supported platform must offer at least one candidate"
        );
        let first = candidates.first().expect("candidate");
        if cfg!(target_os = "windows") {
            assert_eq!(first.kind, EngineKind::Edge);
        } else {
            assert_eq!(first.kind, EngineKind::Chrome);
        }
    }

    #[test]
    fn launch_flags_isolate_the_profile_and_pick_an_ephemeral_port() {
        let flags = launch_flags(Path::new("/tmp/profile"), 1280, 800);
        assert!(flags.iter().any(|flag| flag == "--remote-debugging-port=0"));
        assert!(flags
            .iter()
            .any(|flag| flag.starts_with("--user-data-dir=")));
        assert!(flags.iter().any(|flag| flag == "--window-size=1280,800"));
    }
}
