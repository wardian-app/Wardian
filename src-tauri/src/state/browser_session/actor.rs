//! Browser session lifecycle and the operations surfaces and agents perform.
//!
//! A browser session is a backend-owned runtime resource, like a PTY session.
//! Workbench surfaces attach to it as presentations; detaching a presentation —
//! closing a tab, unmounting the renderer — never disturbs the runtime. A
//! session ends only on explicit close, on its owning agent's termination, or
//! on app exit.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::process::Child;
use tokio::sync::{broadcast, Mutex, RwLock};
use tokio::time::{sleep, timeout, Instant};
use uuid::Uuid;
use wardian_core::browser::{
    BrowserCookie, CookieAction, DownloadRecord, NetworkEntry, NetworkFilter, NetworkRequestDetail,
    StorageArea, StorageEntry, StorageSnapshot, DOWNLOAD_RETENTION_DAYS, MAX_RESPONSE_BODY_BYTES,
    MAX_STORAGE_BYTES, MAX_STORAGE_VALUE_CHARS,
};
pub use wardian_core::browser::{
    BrowserDialog, BrowserSessionSummary, ConsoleEntry, LoadState, Viewport,
    DEFAULT_VIEWPORT_HEIGHT, DEFAULT_VIEWPORT_WIDTH,
};

use super::cdp::{required_str, CdpConnection, CdpError, CdpEvent, DISCONNECTED_METHOD};
use super::engine::{discover_engine, launch_engine, EngineError, EngineKind};
use super::keys;
use super::network::NetworkLedger;
use super::snapshot::{
    action_expression, parse_snapshot, snapshot_expression, PageSnapshot, RefError, SnapshotLedger,
};

/// How often a `wait` predicate is re-evaluated.
const WAIT_POLL_INTERVAL: Duration = Duration::from_millis(100);
/// Console entries retained per session for the surface's error badge.
const CONSOLE_BUFFER: usize = 200;
/// Buffered session events. Screencast frames dominate this channel.
const EVENT_CHANNEL_CAPACITY: usize = 256;
/// Quality of the screencast's JPEG frames.
///
/// A browser surface is mostly small text, which is what JPEG's chroma
/// subsampling damages first. Frames are only produced when the page changes,
/// so the idle cost of a higher setting is nothing and the cost while
/// scrolling buys back legibility that no amount of scaling can recover.
const SCREENCAST_JPEG_QUALITY: u32 = 85;
/// Keep repeated profile-removal attempts bounded and limited to this
/// session's directory.
const PROFILE_CLEANUP_ATTEMPTS: usize = 20;
const PROFILE_CLEANUP_RETRY_DELAY: Duration = Duration::from_millis(50);
/// Do not let a failed browser termination hold up profile cleanup forever.
const BROWSER_PROCESS_CLEANUP_TIMEOUT: Duration = Duration::from_secs(2);

/// What a session tells the rest of the app about itself.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BrowserSessionEvent {
    /// A screencast frame, base64-encoded JPEG.
    Frame {
        browser_id: String,
        data: String,
        width: u32,
        height: u32,
    },
    /// Navigation, title, or load-state change.
    State {
        browser_id: String,
        summary: BrowserSessionSummary,
    },
    /// A console message the page produced.
    Console {
        browser_id: String,
        entry: ConsoleEntry,
    },
    /// The session's runtime is gone. Surfaces should show the reopen path.
    Closed { browser_id: String, reason: String },
    /// Which presentation now holds the drive lease, if any holds it.
    ///
    /// The lease moves whenever a presentation attaches or leaves, so a
    /// surface that learned its standing once at attach time would keep
    /// showing a page it can no longer drive — or, worse, keep its controls
    /// disabled long after it inherited the lease. This carries the
    /// presentation id and never the token: the event reaches every listener,
    /// and the token is the credential that admits input.
    Lease {
        browser_id: String,
        presentation_id: Option<String>,
    },
}

impl BrowserSessionEvent {
    pub fn browser_id(&self) -> &str {
        match self {
            BrowserSessionEvent::Frame { browser_id, .. }
            | BrowserSessionEvent::State { browser_id, .. }
            | BrowserSessionEvent::Console { browser_id, .. }
            | BrowserSessionEvent::Closed { browser_id, .. }
            | BrowserSessionEvent::Lease { browser_id, .. } => browser_id,
        }
    }
}

/// Why a browser operation failed.
#[derive(Debug)]
pub enum BrowserError {
    /// No session matches the given target.
    NotFound { target: String },
    /// The target matched more than one session.
    Ambiguous {
        target: String,
        matches: Vec<String>,
    },
    /// The host has no usable browser, or one could not be started.
    Engine(EngineError),
    /// The protocol call failed.
    Cdp(CdpError),
    /// A snapshot ref could not be used.
    Ref(RefError),
    /// A `wait` predicate never became true.
    WaitTimeout { condition: String, timeout_ms: u64 },
    /// The caller supplied something this operation cannot accept.
    Invalid { detail: String },
    /// A browser-session filesystem or process cleanup operation failed.
    Io { detail: String },
    /// A mirrored presentation tried to drive the page.
    ReadOnlyPresentation,
}

impl std::fmt::Display for BrowserError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BrowserError::NotFound { target } => write!(
                formatter,
                "no browser session matches {target}. Run `wardian browser list` to see open sessions."
            ),
            BrowserError::Ambiguous { target, matches } => write!(
                formatter,
                "{target} matches {} sessions ({}). Use a full id.",
                matches.len(),
                matches.join(", ")
            ),
            BrowserError::Engine(error) => write!(formatter, "{error}"),
            BrowserError::Cdp(error) => write!(formatter, "{error}"),
            BrowserError::Ref(error) => write!(formatter, "{error}"),
            BrowserError::WaitTimeout {
                condition,
                timeout_ms,
            } => write!(formatter, "timed out after {timeout_ms}ms waiting for {condition}"),
            BrowserError::Invalid { detail } => write!(formatter, "{detail}"),
            BrowserError::Io { detail } => write!(formatter, "{detail}"),
            BrowserError::ReadOnlyPresentation => write!(
                formatter,
                "this presentation is mirroring the page read-only; another surface holds the drive lease"
            ),
        }
    }
}

impl std::error::Error for BrowserError {}

impl BrowserError {
    /// Stable machine-readable code for `--json` consumers.
    pub fn code(&self) -> &'static str {
        match self {
            BrowserError::NotFound { .. } => "browser_not_found",
            BrowserError::Ambiguous { .. } => "browser_ambiguous",
            BrowserError::Engine(_) => "browser_engine_unavailable",
            BrowserError::Cdp(_) => "browser_protocol_error",
            BrowserError::Ref(error) => error.code(),
            BrowserError::WaitTimeout { .. } => "browser_wait_timeout",
            BrowserError::Invalid { .. } => "browser_invalid_request",
            BrowserError::Io { .. } => "browser_io_error",
            BrowserError::ReadOnlyPresentation => "browser_read_only_presentation",
        }
    }
}

impl From<CdpError> for BrowserError {
    fn from(error: CdpError) -> Self {
        BrowserError::Cdp(error)
    }
}

impl From<RefError> for BrowserError {
    fn from(error: RefError) -> Self {
        BrowserError::Ref(error)
    }
}

/// A condition `wait` can block on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WaitCondition {
    LoadState(LoadState),
    Selector(String),
    Text(String),
    UrlContains(String),
    Function(String),
}

impl WaitCondition {
    fn describe(&self) -> String {
        match self {
            WaitCondition::LoadState(state) => format!("load state {}", state.as_str()),
            WaitCondition::Selector(selector) => format!("selector {selector:?}"),
            WaitCondition::Text(text) => format!("text {text:?}"),
            WaitCondition::UrlContains(fragment) => format!("url containing {fragment:?}"),
            WaitCondition::Function(expression) => format!("expression {expression:?}"),
        }
    }

    /// The JavaScript predicate this condition polls, when it needs one.
    fn predicate(&self) -> Option<String> {
        match self {
            WaitCondition::LoadState(LoadState::Complete) => {
                Some("document.readyState === 'complete'".to_string())
            }
            WaitCondition::LoadState(LoadState::Loading) => {
                Some("document.readyState === 'loading'".to_string())
            }
            WaitCondition::LoadState(LoadState::Idle) => {
                Some("document.readyState !== 'loading'".to_string())
            }
            WaitCondition::LoadState(LoadState::Failed) => None,
            WaitCondition::Selector(selector) => Some(format!(
                "document.querySelector({}) !== null",
                json!(selector)
            )),
            WaitCondition::Text(text) => Some(format!(
                "(document.body ? document.body.innerText : '').includes({})",
                json!(text)
            )),
            WaitCondition::UrlContains(fragment) => Some(format!(
                "window.location.href.includes({})",
                json!(fragment)
            )),
            WaitCondition::Function(expression) => Some(format!("!!({expression})")),
        }
    }
}

/// What `attach_screencast` hands back to a presentation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ScreencastAttachment {
    /// Credential for every later mutation and for detaching this attachment.
    pub token: String,
    pub can_drive: bool,
}

/// One pointer event forwarded from a surface.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PointerEvent<'a> {
    pub event_type: &'a str,
    pub x: f64,
    pub y: f64,
    pub button: &'a str,
    pub click_count: u32,
    pub modifiers: u32,
}

/// A DOM action against a snapshot ref.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ElementAction {
    Click,
    Hover,
    Fill(String),
    Press(String),
    Select(String),
    Scroll,
}

impl ElementAction {
    /// The verb an operator typed, echoed back in results and logs.
    pub fn name(&self) -> &'static str {
        match self {
            ElementAction::Click => "click",
            ElementAction::Hover => "hover",
            ElementAction::Fill(_) => "fill",
            ElementAction::Press(_) => "press",
            ElementAction::Select(_) => "select",
            ElementAction::Scroll => "scroll",
        }
    }
}

/// What `get` can read off the page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageField {
    Url,
    Title,
    Text,
    Html,
}

impl PageField {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "url" => Some(PageField::Url),
            "title" => Some(PageField::Title),
            "text" => Some(PageField::Text),
            "html" => Some(PageField::Html),
            _ => None,
        }
    }
}

#[derive(Debug, Default)]
struct SessionState {
    url: String,
    title: String,
    load_state: LoadState,
    viewport: Viewport,
    ledger: SnapshotLedger,
    /// The target's main frame, so a subframe's events can be told apart.
    ///
    /// `Page.navigatedWithinDocument` carries only a frame id, with no parent
    /// to test, so without this an iframe changing its hash would rewrite the
    /// session URL and invalidate the top-level page's refs.
    main_frame_id: Option<String>,
    console: VecDeque<ConsoleEntry>,
    console_error_count: usize,
    /// Every request the page has made, bounded and never cleared by navigation.
    network: NetworkLedger,
    /// Downloads this session has started, newest last.
    downloads: Vec<DownloadRecord>,
    /// Attachments currently streaming, in attach order.
    screencast_viewers: Vec<Attachment>,
    /// The attachment allowed to drive the page. First attach wins.
    owner_token: Option<String>,
    /// The dialog stopping the page, while one is waiting to be answered.
    dialog: Option<BrowserDialog>,
    /// Page targets this session has already accounted for.
    ///
    /// Target discovery re-announces everything that already exists the
    /// moment it is switched on, and the browser starts with a page of its
    /// own besides the one this session created. Without a record of what was
    /// there first, the session would adopt its own base page as a popup.
    known_targets: HashSet<String>,
}

/// One page target this session is attached to.
#[derive(Debug, Clone, PartialEq, Eq)]
struct AttachedTarget {
    target_id: String,
    cdp_session_id: String,
}

/// One presentation's streaming attachment.
///
/// The token, not the presentation id, is the credential: ids are derived from
/// surface and session ids and are therefore guessable by any caller, and one
/// presentation can attach several times across effect re-runs.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Attachment {
    presentation_id: String,
    token: String,
}

/// One live browser, its protocol session, and everything derived from it.
#[derive(Debug)]
pub struct BrowserSession {
    browser_id: String,
    short_ref: u32,
    owner_agent_id: Option<String>,
    workspace: Option<String>,
    engine: EngineKind,
    connection: Arc<CdpConnection>,
    /// The page this session presents, and the ones it is stacked on.
    ///
    /// A popup is presented in place of its opener, so "which protocol
    /// session do I talk to" is a runtime question rather than a fixed
    /// identity. Kept apart from `state` so reading it never contends with
    /// the page state a call is about to write.
    targets: RwLock<Vec<AttachedTarget>>,
    profile_dir: PathBuf,
    /// Where this session's downloads land.
    ///
    /// A sibling of the profile rather than a child of it: the profile is
    /// deleted on close, and a download's whole purpose is the file afterwards.
    download_dir: PathBuf,
    child: Mutex<Option<Child>>,
    state: RwLock<SessionState>,
    /// Serializes attach/detach with their CDP start/stop.
    ///
    /// The viewer list and the stream have to move together. Without this,
    /// a second attach can observe a non-empty list and skip a start that
    /// then fails and rolls back, and a detach's `stopScreencast` can land
    /// after a concurrent attach's `startScreencast`.
    screencast_transition: Mutex<()>,
    /// The broker's publication channel, so lease changes reach surfaces.
    events: broadcast::Sender<BrowserSessionEvent>,
}

impl BrowserSession {
    pub fn browser_id(&self) -> &str {
        &self.browser_id
    }

    pub fn short_ref(&self) -> String {
        format!("browser:{}", self.short_ref)
    }

    pub fn owner_agent_id(&self) -> Option<&str> {
        self.owner_agent_id.as_deref()
    }

    /// The protocol session for the page this surface is presenting.
    ///
    /// Every call goes to the presented page, not to the base one: a popup is
    /// what the operator is looking at and what an agent's next action means.
    async fn cdp_session(&self) -> String {
        let targets = self.targets.read().await;
        targets
            .last()
            .map(|target| target.cdp_session_id.clone())
            // Unreachable in practice: the base target is pushed before the
            // session exists. Falling back to an empty id makes the call fail
            // with a protocol error rather than panicking a command handler.
            .unwrap_or_default()
    }

    /// Whether a popup is presented in place of its opener.
    async fn presenting_popup(&self) -> bool {
        self.targets.read().await.len() > 1
    }

    pub async fn summary(&self) -> BrowserSessionSummary {
        // Read outside the state lock: `targets` is a separate lock and the
        // order between them has to stay one-way.
        let popup = self.presenting_popup().await;
        let state = self.state.read().await;
        BrowserSessionSummary {
            browser_id: self.browser_id.clone(),
            short_ref: self.short_ref(),
            url: state.url.clone(),
            title: state.title.clone(),
            load_state: state.load_state,
            viewport: state.viewport,
            engine: self.engine,
            owner_agent_id: self.owner_agent_id.clone(),
            workspace: self.workspace.clone(),
            console_error_count: state.console_error_count,
            network_failure_count: state.network.failure_count(),
            popup,
            dialog: state.dialog.clone(),
        }
    }

    async fn evaluate(&self, expression: &str) -> Result<Value, BrowserError> {
        let result = self
            .connection
            .call_session(
                &self.cdp_session().await,
                "Runtime.evaluate",
                json!({
                    "expression": expression,
                    "returnByValue": true,
                    "awaitPromise": true,
                    "userGesture": true,
                }),
            )
            .await?;
        if let Some(exception) = result.get("exceptionDetails") {
            let text = exception
                .get("exception")
                .and_then(|value| value.get("description"))
                .and_then(Value::as_str)
                .or_else(|| exception.get("text").and_then(Value::as_str))
                .unwrap_or("evaluation failed");
            return Err(BrowserError::Cdp(CdpError::Protocol {
                method: "Runtime.evaluate".to_string(),
                code: 0,
                message: text.to_string(),
            }));
        }
        Ok(result
            .get("result")
            .and_then(|value| value.get("value"))
            .cloned()
            .unwrap_or(Value::Null))
    }

    /// Navigates and blocks until the load settles or the deadline passes.
    pub async fn navigate(&self, url: &str) -> Result<(), BrowserError> {
        let url = normalize_url(url)?;
        {
            let mut state = self.state.write().await;
            state.load_state = LoadState::Loading;
        }
        let result = self
            .connection
            .call_session(
                &self.cdp_session().await,
                "Page.navigate",
                json!({ "url": url }),
            )
            .await?;
        if let Some(error) = result.get("errorText").and_then(Value::as_str) {
            let mut state = self.state.write().await;
            state.load_state = LoadState::Failed;
            return Err(BrowserError::Cdp(CdpError::Protocol {
                method: "Page.navigate".to_string(),
                code: 0,
                message: error.to_string(),
            }));
        }
        // Report the page being loaded rather than the one being left. The
        // commit arrives later as `Page.frameNavigated` and replaces this with
        // the post-redirect URL; until then, echoing `about:blank` back to a
        // caller who just asked for a URL is worse than being slightly early.
        {
            let mut state = self.state.write().await;
            state.url = url;
        }
        Ok(())
    }

    /// Moves through history. `delta` is negative for back, positive forward.
    pub async fn traverse_history(&self, delta: i64) -> Result<(), BrowserError> {
        let history = self
            .connection
            .call_session(
                &self.cdp_session().await,
                "Page.getNavigationHistory",
                json!({}),
            )
            .await?;
        let current = history
            .get("currentIndex")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        let entries = history
            .get("entries")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let target = current + delta;
        if target < 0 || target as usize >= entries.len() {
            return Err(BrowserError::Invalid {
                detail: if delta < 0 {
                    "there is no earlier page in this session's history".to_string()
                } else {
                    "there is no later page in this session's history".to_string()
                },
            });
        }
        let entry_id = entries[target as usize]
            .get("id")
            .and_then(Value::as_i64)
            .ok_or_else(|| BrowserError::Invalid {
                detail: "the browser returned a history entry without an id".to_string(),
            })?;
        self.connection
            .call_session(
                &self.cdp_session().await,
                "Page.navigateToHistoryEntry",
                json!({ "entryId": entry_id }),
            )
            .await?;
        Ok(())
    }

    pub async fn reload(&self) -> Result<(), BrowserError> {
        self.connection
            .call_session(&self.cdp_session().await, "Page.reload", json!({}))
            .await?;
        Ok(())
    }

    pub async fn stop_loading(&self) -> Result<(), BrowserError> {
        self.connection
            .call_session(&self.cdp_session().await, "Page.stopLoading", json!({}))
            .await?;
        Ok(())
    }

    /// Reads one field off the page, optionally scoped to a CSS selector.
    pub async fn get(
        &self,
        field: PageField,
        selector: Option<&str>,
    ) -> Result<String, BrowserError> {
        let expression = match (field, selector) {
            (PageField::Url, _) => "window.location.href".to_string(),
            (PageField::Title, _) => "document.title".to_string(),
            (PageField::Text, None) => {
                "document.body ? document.body.innerText : ''".to_string()
            }
            (PageField::Html, None) => "document.documentElement.outerHTML".to_string(),
            (PageField::Text, Some(selector)) => format!(
                "(() => {{ const node = document.querySelector({0}); if (!node) throw new Error('no element matches ' + {0}); return node.innerText; }})()",
                json!(selector)
            ),
            (PageField::Html, Some(selector)) => format!(
                "(() => {{ const node = document.querySelector({0}); if (!node) throw new Error('no element matches ' + {0}); return node.outerHTML; }})()",
                json!(selector)
            ),
        };
        let value = self.evaluate(&expression).await?;
        Ok(value.as_str().unwrap_or_default().to_string())
    }

    /// Polls a condition until it holds or the timeout elapses.
    pub async fn wait(
        &self,
        condition: &WaitCondition,
        timeout_ms: u64,
    ) -> Result<(), BrowserError> {
        if let WaitCondition::LoadState(LoadState::Failed) = condition {
            return Err(BrowserError::Invalid {
                detail: "cannot wait for the failed load state; it is reported, not awaited"
                    .to_string(),
            });
        }
        let Some(predicate) = condition.predicate() else {
            return Err(BrowserError::Invalid {
                detail: format!("{} cannot be waited on", condition.describe()),
            });
        };
        let deadline = Instant::now() + Duration::from_millis(timeout_ms);
        loop {
            // An expression that throws mid-navigation is a not-yet, not a
            // failure; only the deadline ends the wait unsuccessfully.
            if let Ok(Value::Bool(true)) = self.evaluate(&predicate).await {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(BrowserError::WaitTimeout {
                    condition: condition.describe(),
                    timeout_ms,
                });
            }
            sleep(WAIT_POLL_INTERVAL).await;
        }
    }

    /// Walks the page and mints a fresh set of refs.
    pub async fn snapshot(&self, interactive_only: bool) -> Result<PageSnapshot, BrowserError> {
        let generation = self.state.read().await.ledger.current_generation();
        let raw = self
            .evaluate(&snapshot_expression(generation, interactive_only))
            .await?;
        let snapshot = parse_snapshot(generation, interactive_only, &raw)
            .map_err(|detail| BrowserError::Invalid { detail })?;
        let mut state = self.state.write().await;
        // The page may have navigated while the walker ran. Recording against a
        // generation that has since moved would hand back refs that are already
        // stale, so the snapshot is discarded rather than published.
        if state.ledger.current_generation() != generation {
            return Err(BrowserError::Ref(RefError::Stale {
                element_ref: "snapshot".to_string(),
                snapshot_generation: generation,
                current_generation: state.ledger.current_generation(),
            }));
        }
        state.ledger.record_snapshot(&snapshot.elements);
        state.url = snapshot.url.clone();
        state.title = snapshot.title.clone();
        Ok(snapshot)
    }

    /// Performs an action against a ref minted by the current snapshot.
    ///
    /// Three checks stand between a ref and a click: the snapshot generation,
    /// that the ref resolves to exactly one element, and that the element is
    /// still what the snapshot described. Any of them failing is a refusal,
    /// never a best guess.
    pub async fn act(&self, element_ref: &str, action: &ElementAction) -> Result<(), BrowserError> {
        let (generation, expected) = {
            let state = self.state.read().await;
            let identity = state.ledger.validate(element_ref)?.clone();
            (state.ledger.current_generation(), identity)
        };
        let body = match action {
            ElementAction::Click => {
                "node.scrollIntoView({ block: 'center' }); node.click();".to_string()
            }
            ElementAction::Hover => "node.scrollIntoView({ block: 'center' }); node.dispatchEvent(new MouseEvent('mouseover', { bubbles: true })); node.dispatchEvent(new MouseEvent('mouseenter', { bubbles: true }));".to_string(),
            ElementAction::Fill(value) => format!(
                "node.focus(); node.value = {}; node.dispatchEvent(new Event('input', {{ bubbles: true }})); node.dispatchEvent(new Event('change', {{ bubbles: true }}));",
                json!(value)
            ),
            ElementAction::Press(key) => format!(
                "node.focus(); for (const type of ['keydown', 'keypress', 'keyup']) {{ node.dispatchEvent(new KeyboardEvent(type, {{ key: {0}, bubbles: true }})); }} if ({0} === 'Enter' && node.form) {{ node.form.requestSubmit ? node.form.requestSubmit() : node.form.submit(); }}",
                json!(key)
            ),
            ElementAction::Select(value) => format!(
                "node.value = {}; node.dispatchEvent(new Event('change', {{ bubbles: true }}));",
                json!(value)
            ),
            ElementAction::Scroll => "node.scrollIntoView({ block: 'center' });".to_string(),
        };
        let outcome = self
            .evaluate(&action_expression(
                element_ref,
                generation,
                &expected,
                &body,
            ))
            .await?;
        match outcome.as_str() {
            Some("ok") => Ok(()),
            Some("ambiguous") => Err(BrowserError::Ref(RefError::Ambiguous {
                element_ref: element_ref.to_string(),
            })),
            Some("changed") => Err(BrowserError::Ref(RefError::Changed {
                element_ref: element_ref.to_string(),
            })),
            _ => Err(BrowserError::Ref(RefError::Detached {
                element_ref: element_ref.to_string(),
            })),
        }
    }

    /// Scrolls the page itself rather than an element.
    pub async fn scroll_page(&self, delta_x: f64, delta_y: f64) -> Result<(), BrowserError> {
        self.evaluate(&format!(
            "window.scrollBy({{ left: {delta_x}, top: {delta_y}, behavior: 'instant' }})"
        ))
        .await?;
        Ok(())
    }

    /// Captures a PNG and writes it to `path`.
    pub async fn screenshot(&self, path: &PathBuf, full_page: bool) -> Result<(), BrowserError> {
        let result = self
            .connection
            .call_session(
                &self.cdp_session().await,
                "Page.captureScreenshot",
                json!({ "format": "png", "captureBeyondViewport": full_page }),
            )
            .await?;
        let encoded = required_str("Page.captureScreenshot", &result, "data")?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .map_err(|error| BrowserError::Invalid {
                detail: format!("the browser returned an unreadable screenshot: {error}"),
            })?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| BrowserError::Io {
                detail: format!("could not create {}: {error}", parent.display()),
            })?;
        }
        std::fs::write(path, bytes).map_err(|error| BrowserError::Io {
            detail: format!("could not write {}: {error}", path.display()),
        })
    }

    /// Overrides the rendered viewport, or clears the override when `None`.
    pub async fn set_viewport(&self, viewport: Option<Viewport>) -> Result<(), BrowserError> {
        let resolved = viewport.unwrap_or_default();
        if resolved.width == 0 || resolved.height == 0 {
            return Err(BrowserError::Invalid {
                detail: "viewport width and height must both be greater than zero".to_string(),
            });
        }
        self.connection
            .call_session(
                &self.cdp_session().await,
                "Emulation.setDeviceMetricsOverride",
                json!({
                    "width": resolved.width,
                    "height": resolved.height,
                    "deviceScaleFactor": 1,
                    "mobile": false,
                }),
            )
            .await?;
        self.state.write().await.viewport = resolved;
        Ok(())
    }

    /// Evaluates an arbitrary expression and returns its JSON value.
    pub async fn eval(&self, expression: &str) -> Result<Value, BrowserError> {
        self.evaluate(expression).await
    }

    /// Returns the retained console entries, newest last.
    ///
    /// `level` keeps only one severity; `clear` empties the buffer after
    /// reading it, so an agent can establish a clean baseline before an action.
    pub async fn console(&self, level: Option<&str>, clear: bool) -> Vec<ConsoleEntry> {
        let mut state = self.state.write().await;
        let entries: Vec<ConsoleEntry> = state
            .console
            .iter()
            .filter(|entry| level.is_none_or(|level| entry.level == level))
            .cloned()
            .collect();
        if clear {
            state.console.clear();
            state.console_error_count = 0;
        }
        entries
    }

    /// Returns the recorded requests that survive `filter`.
    pub async fn network(&self, filter: &NetworkFilter) -> Vec<NetworkEntry> {
        filter.apply(&self.state.read().await.network.entries())
    }

    /// Returns one request in full, optionally reading its body back live.
    ///
    /// The body is never stored: it is fetched through `Network.getResponseBody`
    /// only when asked, and only while the browser's own buffer still holds it.
    pub async fn network_detail(
        &self,
        request_id: &str,
        with_body: bool,
    ) -> Result<NetworkRequestDetail, BrowserError> {
        let record = {
            let state = self.state.read().await;
            state.network.detail(request_id).cloned()
        };
        let record = record.ok_or_else(|| BrowserError::Invalid {
            detail: format!(
                "no recorded request has id {request_id}. Run `network` to list what was captured."
            ),
        })?;
        let mut detail = NetworkRequestDetail {
            entry: record.entry,
            request_headers: record.request_headers,
            response_headers: record.response_headers,
            body: None,
            body_error: None,
        };
        if with_body {
            match self.response_body(request_id).await {
                Ok(body) => detail.body = Some(body),
                Err(error) => detail.body_error = Some(error.to_string()),
            }
        }
        Ok(detail)
    }

    /// Reads one response body back out of the browser, capped.
    async fn response_body(
        &self,
        request_id: &str,
    ) -> Result<wardian_core::browser::NetworkBody, BrowserError> {
        let result = self
            .connection
            .call_session(
                &self.cdp_session().await,
                "Network.getResponseBody",
                json!({ "requestId": request_id }),
            )
            .await?;
        let text = result
            .get("body")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let base64_encoded = result
            .get("base64Encoded")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let truncated = text.len() > MAX_RESPONSE_BODY_BYTES;
        let text = if truncated {
            // Cut on a character boundary so the result is still valid UTF-8.
            let mut end = MAX_RESPONSE_BODY_BYTES;
            while end > 0 && !text.is_char_boundary(end) {
                end -= 1;
            }
            text[..end].to_string()
        } else {
            text.to_string()
        };
        Ok(wardian_core::browser::NetworkBody {
            text,
            base64_encoded,
            truncated,
        })
    }

    /// Empties the network ledger. Nothing about the page changes.
    pub async fn clear_network(&self) {
        self.state.write().await.network.clear();
    }

    /// Runs one cookie verb against the session's isolated profile.
    pub async fn cookies(&self, action: &CookieAction) -> Result<Vec<BrowserCookie>, BrowserError> {
        match action {
            CookieAction::List { all } => {
                let method = if *all {
                    "Storage.getCookies"
                } else {
                    "Network.getCookies"
                };
                let result = self
                    .connection
                    .call_session(&self.cdp_session().await, method, json!({}))
                    .await?;
                Ok(cookies_from(&result))
            }
            CookieAction::Set {
                name,
                value,
                url,
                domain,
                path,
                secure,
                http_only,
                same_site,
                expires,
            } => {
                // A cookie needs somewhere to live. Neither a URL nor a domain
                // makes the browser silently drop it, so the page's own address
                // stands in — which is what a caller setting a cookie for the
                // page they are looking at means anyway.
                let mut params = json!({ "name": name, "value": value });
                match (url.as_deref(), domain.as_deref()) {
                    (Some(url), _) => params["url"] = json!(url),
                    (None, Some(_)) => {}
                    (None, None) => {
                        let current = self.state.read().await.url.clone();
                        if current.is_empty() || current.starts_with("about:") {
                            return Err(BrowserError::Invalid {
                                detail:
                                    "this page has no address to scope a cookie to; pass --url or --domain"
                                        .to_string(),
                            });
                        }
                        params["url"] = json!(current);
                    }
                }
                if let Some(domain) = domain {
                    params["domain"] = json!(domain);
                }
                if let Some(path) = path {
                    params["path"] = json!(path);
                }
                if *secure {
                    params["secure"] = json!(true);
                }
                if *http_only {
                    params["httpOnly"] = json!(true);
                }
                if let Some(same_site) = same_site {
                    params["sameSite"] = json!(normalize_same_site(same_site)?);
                }
                if let Some(expires) = expires {
                    params["expires"] = json!(expires);
                }
                self.connection
                    .call_session(&self.cdp_session().await, "Network.setCookie", params)
                    .await?;
                Ok(Vec::new())
            }
            CookieAction::Delete {
                name,
                url,
                domain,
                path,
            } => {
                let mut params = json!({ "name": name });
                match (url.as_deref(), domain.as_deref()) {
                    (Some(url), _) => params["url"] = json!(url),
                    (None, Some(_)) => {}
                    (None, None) => {
                        let current = self.state.read().await.url.clone();
                        if current.is_empty() || current.starts_with("about:") {
                            return Err(BrowserError::Invalid {
                                detail:
                                    "this page has no address to scope the deletion to; pass --url or --domain"
                                        .to_string(),
                            });
                        }
                        params["url"] = json!(current);
                    }
                }
                if let Some(domain) = domain {
                    params["domain"] = json!(domain);
                }
                if let Some(path) = path {
                    params["path"] = json!(path);
                }
                self.connection
                    .call_session(&self.cdp_session().await, "Network.deleteCookies", params)
                    .await?;
                Ok(Vec::new())
            }
            CookieAction::Clear => {
                self.connection
                    .call_session(
                        &self.cdp_session().await,
                        "Network.clearBrowserCookies",
                        json!({}),
                    )
                    .await?;
                Ok(Vec::new())
            }
        }
    }

    /// Reads a whole web-storage area at the page's own origin.
    pub async fn storage(&self, area: StorageArea) -> Result<StorageSnapshot, BrowserError> {
        let accessor = area.accessor();
        let value = self
            .storage_evaluate(&format!(
                "(() => {{ const store = window.{accessor}; const out = []; for (let index = 0; index < store.length; index += 1) {{ const key = store.key(index); out.push([key, store.getItem(key) ?? '']); }} return JSON.stringify({{ origin: window.location.origin, entries: out }}); }})()"
            ))
            .await?;
        let text = value.as_str().unwrap_or("{}");
        let parsed: Value = serde_json::from_str(text).map_err(|error| BrowserError::Invalid {
            detail: format!("the page returned an unreadable storage listing: {error}"),
        })?;
        Ok(storage_snapshot_from(area, &parsed))
    }

    /// Reads one key, returning `None` when the area does not hold it.
    pub async fn storage_get(
        &self,
        area: StorageArea,
        key: &str,
    ) -> Result<Option<String>, BrowserError> {
        let accessor = area.accessor();
        let value = self
            .storage_evaluate(&format!("window.{accessor}.getItem({})", json!(key)))
            .await?;
        Ok(value.as_str().map(str::to_string))
    }

    /// Writes, removes, or empties a web-storage area.
    pub async fn storage_mutate(
        &self,
        area: StorageArea,
        action: &wardian_core::browser::StorageAction,
    ) -> Result<(), BrowserError> {
        use wardian_core::browser::StorageAction;
        let accessor = area.accessor();
        let expression = match action {
            StorageAction::Set { key, value } => format!(
                "window.{accessor}.setItem({}, {})",
                json!(key),
                json!(value)
            ),
            StorageAction::Remove { key } => {
                format!("window.{accessor}.removeItem({})", json!(key))
            }
            StorageAction::Clear => format!("window.{accessor}.clear()"),
            StorageAction::Get { .. } => {
                return Err(BrowserError::Invalid {
                    detail: "a storage read is not a mutation".to_string(),
                })
            }
        };
        self.storage_evaluate(&expression).await?;
        Ok(())
    }

    /// Evaluates a storage expression, naming the one failure that is expected.
    ///
    /// `about:blank`, a sandboxed frame, and a `data:` URL all have opaque
    /// origins where the DOM throws `SecurityError` on any storage access. That
    /// is a caller mistake with an obvious fix, not a protocol fault, so it gets
    /// a message that names the fix instead of a raw evaluation error.
    async fn storage_evaluate(&self, expression: &str) -> Result<Value, BrowserError> {
        self.evaluate(expression).await.map_err(|error| {
            let text = error.to_string();
            if text.contains("SecurityError") || text.contains("Access is denied") {
                return BrowserError::Invalid {
                    detail: "this page's origin has no web storage; navigate to an http or https page first"
                        .to_string(),
                };
            }
            error
        })
    }

    /// Returns the downloads this session has started, newest last.
    ///
    /// Completed downloads are renamed from their GUID to their suggested
    /// filename here rather than in the event pump: the pump must stay free of
    /// filesystem work, and doing it on read also cannot race the browser's own
    /// finalization of the file.
    pub async fn downloads(&self) -> Vec<DownloadRecord> {
        let mut state = self.state.write().await;
        for record in state.downloads.iter_mut() {
            if record.state != "completed" || record.path.is_some() {
                continue;
            }
            record.path = resolve_completed_download(
                &self.download_dir,
                &record.guid,
                &record.suggested_filename,
            );
        }
        state.downloads.clone()
    }

    /// Forgets the recorded downloads. The files themselves stay on disk.
    pub async fn clear_downloads(&self) {
        self.state.write().await.downloads.clear();
    }

    /// Where this session writes downloads.
    pub fn download_dir(&self) -> &Path {
        &self.download_dir
    }

    /// Starts streaming frames and issues this attachment's lease token.
    ///
    /// The first attachment becomes the driver; later ones mirror it read-only,
    /// matching how a terminal session treats its presentations. Every attach
    /// mints a fresh token, so a stale cleanup can only ever detach its own
    /// attachment and never a newer one for the same presentation.
    pub async fn attach_screencast(
        &self,
        presentation_id: &str,
    ) -> Result<ScreencastAttachment, BrowserError> {
        let _transition = self.screencast_transition.lock().await;
        let token = Uuid::new_v4().to_string();
        let should_start = {
            let mut state = self.state.write().await;
            let was_idle = state.screencast_viewers.is_empty();
            // One presentation streams once. An earlier attachment under the
            // same id belongs to a presentation that no longer exists — a
            // reloaded webview, or an effect whose cleanup has not landed yet
            // — and leaving it registered is what strands a live surface
            // mirroring a lease that nobody will ever release.
            let superseded: Vec<String> = state
                .screencast_viewers
                .iter()
                .filter(|attachment| attachment.presentation_id == presentation_id)
                .map(|attachment| attachment.token.clone())
                .collect();
            state
                .screencast_viewers
                .retain(|attachment| attachment.presentation_id != presentation_id);
            state.screencast_viewers.push(Attachment {
                presentation_id: presentation_id.to_string(),
                token: token.clone(),
            });
            // The replacement inherits what it replaced. Anything else would
            // hand the lease to a bystander every time a surface remounts.
            let owner_is_vacant = match state.owner_token.as_deref() {
                None => true,
                Some(owner) => superseded.iter().any(|stale| stale == owner),
            };
            if owner_is_vacant {
                state.owner_token = Some(token.clone());
            }
            was_idle
        };
        if should_start {
            // Roll the attachment back if the stream never started, or a later
            // attach would see a non-empty viewer list, skip the start, and
            // mirror an owner that is producing no frames. Holding the
            // transition lock is what makes the rollback complete: no other
            // attach can observe the half-built state.
            if let Err(error) = self
                .connection
                .call_session(
                    &self.cdp_session().await,
                    "Page.startScreencast",
                    json!({ "format": "jpeg", "quality": SCREENCAST_JPEG_QUALITY, "everyNthFrame": 1 }),
                )
                .await
            {
                self.release_attachment(&token).await;
                self.announce_lease().await;
                return Err(error.into());
            }
        }
        self.announce_lease().await;
        Ok(ScreencastAttachment {
            can_drive: self.token_may_drive(&token).await,
            token,
        })
    }

    /// Publishes which presentation holds the drive lease right now.
    ///
    /// Sent on every attach and detach rather than only on a change: the
    /// event is rare next to frames, and a surface that missed one would
    /// otherwise keep a stale idea of whether it may drive the page.
    async fn announce_lease(&self) {
        let presentation_id = {
            let state = self.state.read().await;
            let owner = state.owner_token.clone();
            owner.and_then(|owner| {
                state
                    .screencast_viewers
                    .iter()
                    .find(|attachment| attachment.token == owner)
                    .map(|attachment| attachment.presentation_id.clone())
            })
        };
        let _ = self.events.send(BrowserSessionEvent::Lease {
            browser_id: self.browser_id.clone(),
            presentation_id,
        });
    }

    /// Presents a page this session's page opened, in place of its opener.
    ///
    /// A surface has one viewport, so a popup either replaces what is on it or
    /// runs where nobody can see or drive it. The second is what a popup used
    /// to do: `window.open` and every `target="_blank"` link — every OAuth
    /// flow — created a target this session ignored, leaving the operator
    /// looking at an opener that would never change.
    ///
    /// Best effort throughout. A popup that closes itself between the
    /// announcement and the attach is not an error, and failing to present it
    /// must not disturb the page that is already presented.
    async fn adopt_popup(&self, target_id: &str, events: &broadcast::Sender<BrowserSessionEvent>) {
        // Shares the screencast lock with attach/detach: the stream has to
        // stop on one target and start on the other without an attach landing
        // in between and starting it on the one being left.
        let _transition = self.screencast_transition.lock().await;
        let previous = self.cdp_session().await;
        let Ok(page) = equip_target(&self.connection, target_id).await else {
            return;
        };
        let (viewport, streaming) = {
            let state = self.state.read().await;
            (state.viewport, !state.screencast_viewers.is_empty())
        };
        // The popup opens at the browser's own window size; the pane's size is
        // the one the operator chose.
        let _ = self
            .connection
            .call_session(
                &page.target.cdp_session_id,
                "Emulation.setDeviceMetricsOverride",
                json!({
                    "width": viewport.width,
                    "height": viewport.height,
                    "deviceScaleFactor": 1,
                    "mobile": false,
                }),
            )
            .await;
        if streaming {
            let _ = self
                .connection
                .call_session(&previous, "Page.stopScreencast", json!({}))
                .await;
        }
        self.targets.write().await.push(page.target.clone());
        {
            let mut state = self.state.write().await;
            state.main_frame_id = page.main_frame_id;
            // Refs name elements in the document that minted them, and this is
            // a different document.
            state.ledger.invalidate();
        }
        if streaming {
            let _ = self
                .connection
                .call_session(
                    &page.target.cdp_session_id,
                    "Page.startScreencast",
                    json!({
                        "format": "jpeg",
                        "quality": SCREENCAST_JPEG_QUALITY,
                        "everyNthFrame": 1,
                    }),
                )
                .await;
        }
        // The stream has moved; the lock is only about that. What follows runs
        // script in the popup, and a popup that greets you with `alert` would
        // otherwise hold attach and detach for a whole protocol timeout.
        drop(_transition);
        // A popup that finished loading before this attach emits nothing
        // further, so its address is read rather than waited for.
        self.resync_presented_page(events).await;
    }

    /// Returns to the page behind a popup that has gone away.
    async fn release_popup(
        &self,
        target_id: &str,
        events: &broadcast::Sender<BrowserSessionEvent>,
    ) {
        let _transition = self.screencast_transition.lock().await;
        let restored = {
            let mut targets = self.targets.write().await;
            // The base page closing means the browser is going away, which the
            // disconnect path already owns.
            let Some(index) = targets.iter().position(|t| t.target_id == target_id) else {
                return;
            };
            if index == 0 {
                return;
            }
            let was_presented = index == targets.len() - 1;
            targets.remove(index);
            // A popup behind the presented one closed: the stack shrinks and
            // nothing on screen changes.
            if !was_presented {
                return;
            }
            targets.last().cloned()
        };
        let Some(restored) = restored else { return };
        let streaming = !self.state.read().await.screencast_viewers.is_empty();
        if streaming {
            let _ = self
                .connection
                .call_session(
                    &restored.cdp_session_id,
                    "Page.startScreencast",
                    json!({
                        "format": "jpeg",
                        "quality": SCREENCAST_JPEG_QUALITY,
                        "everyNthFrame": 1,
                    }),
                )
                .await;
        }
        {
            let mut state = self.state.write().await;
            state.ledger.invalidate();
            // A dialog belongs to the page that raised it, and that page is
            // gone.
            state.dialog = None;
        }
        let main_frame_id = self
            .connection
            .call_session(&restored.cdp_session_id, "Page.getFrameTree", json!({}))
            .await
            .ok()
            .and_then(|tree| {
                tree.get("frameTree")
                    .and_then(|frame_tree| frame_tree.get("frame"))
                    .and_then(|frame| frame.get("id"))
                    .and_then(Value::as_str)
                    .map(str::to_string)
            });
        if main_frame_id.is_some() {
            self.state.write().await.main_frame_id = main_frame_id;
        }
        // As in `adopt_popup`: the page work below must not hold the lock that
        // attach and detach need.
        drop(_transition);
        self.resync_presented_page(events).await;
    }

    /// Closes the popup on top and returns to the page behind it.
    ///
    /// A popup that does not close itself would otherwise be a one-way door:
    /// its history has no entry for the opener, so Back cannot leave it.
    pub async fn close_popup(&self, lease_token: Option<&str>) -> Result<(), BrowserError> {
        self.require_drive(lease_token).await?;
        let presented = {
            let targets = self.targets.read().await;
            if targets.len() <= 1 {
                return Err(BrowserError::Invalid {
                    detail: "this session is not presenting a popup".to_string(),
                });
            }
            targets.last().cloned()
        };
        let Some(presented) = presented else {
            return Ok(());
        };
        // `Target.targetDestroyed` does the rest, the same way it does when a
        // popup closes itself.
        self.connection
            .call(
                "Target.closeTarget",
                json!({ "targetId": presented.target_id }),
            )
            .await?;
        Ok(())
    }

    /// Answers the dialog stopping the page.
    ///
    /// `accept` is what the operator pressed; `prompt_text` is only read for a
    /// `prompt`. Answering is the only way the page resumes — until it does,
    /// nothing else about the session works.
    pub async fn answer_dialog(
        &self,
        lease_token: Option<&str>,
        accept: bool,
        prompt_text: Option<&str>,
    ) -> Result<(), BrowserError> {
        self.require_drive(lease_token).await?;
        let cdp_session_id = self.cdp_session().await;
        dispatch_dialog_answer(&self.connection, &cdp_session_id, accept, prompt_text).await?;
        self.state.write().await.dialog = None;
        Ok(())
    }

    /// Re-reads what the presented page says about itself and republishes it.
    async fn resync_presented_page(&self, events: &broadcast::Sender<BrowserSessionEvent>) {
        if let Ok(url) = self.get(PageField::Url, None).await {
            self.state.write().await.url = url;
        }
        if let Ok(title) = self.get(PageField::Title, None).await {
            self.state.write().await.title = title;
        }
        let _ = events.send(BrowserSessionEvent::State {
            browser_id: self.browser_id.clone(),
            summary: self.summary().await,
        });
    }

    /// Drops one attachment and hands the lease on if it held it.
    async fn release_attachment(&self, token: &str) -> bool {
        let mut state = self.state.write().await;
        state
            .screencast_viewers
            .retain(|attachment| attachment.token != token);
        if state.owner_token.as_deref() == Some(token) {
            state.owner_token = state
                .screencast_viewers
                .first()
                .map(|attachment| attachment.token.clone());
        }
        state.screencast_viewers.is_empty()
    }

    /// Stops streaming once the last attachment leaves. The page keeps running.
    ///
    /// Keyed on the attachment token rather than the presentation id, so a
    /// cleanup racing a re-attach cannot tear down the newer attachment.
    pub async fn detach_screencast(&self, token: &str) -> Result<(), BrowserError> {
        let _transition = self.screencast_transition.lock().await;
        let should_stop = self.release_attachment(token).await;
        self.announce_lease().await;
        if should_stop {
            // Nobody is left to answer, and a dialog holds the whole page.
            // Leaving it up would freeze a session whose surface simply
            // closed.
            let pending = self.state.read().await.dialog.clone();
            if let Some(dialog) = pending {
                let _ = self
                    .answer_dialog(None, safe_dialog_answer(&dialog.kind), None)
                    .await;
            }
        }
        if should_stop {
            self.connection
                .call_session(&self.cdp_session().await, "Page.stopScreencast", json!({}))
                .await?;
        }
        Ok(())
    }

    /// Whether an attachment token currently holds the drive lease.
    pub async fn token_may_drive(&self, token: &str) -> bool {
        self.state.read().await.owner_token.as_deref() == Some(token)
    }

    /// How many presentations are streaming, for diagnostics and tests.
    pub async fn attachment_count(&self) -> usize {
        self.state.read().await.screencast_viewers.len()
    }

    /// Refuses a mutation that does not carry the drive lease.
    ///
    /// `None` is the control-plane path: `wardian browser` reaches these
    /// operations through the control server, never through a surface, and is
    /// not a competing presentation. Every surface-originated mutation must
    /// supply its token, so an omitted one is refused rather than waved
    /// through — that is what makes the lease an enforcement boundary and not
    /// a frontend convention.
    pub(crate) async fn require_drive(&self, token: Option<&str>) -> Result<(), BrowserError> {
        let Some(token) = token else {
            return Ok(());
        };
        if self.token_may_drive(token).await {
            return Ok(());
        }
        Err(BrowserError::ReadOnlyPresentation)
    }

    /// Forwards a pointer event from a surface into the page.
    pub async fn dispatch_mouse(
        &self,
        lease_token: Option<&str>,
        event: &PointerEvent<'_>,
    ) -> Result<(), BrowserError> {
        self.require_drive(lease_token).await?;
        if !matches!(
            event.event_type,
            "mousePressed" | "mouseReleased" | "mouseMoved" | "mouseWheel"
        ) {
            return Err(BrowserError::Invalid {
                detail: format!(
                    "{} is not a pointer event this surface forwards",
                    event.event_type
                ),
            });
        }
        self.connection
            .call_session(
                &self.cdp_session().await,
                "Input.dispatchMouseEvent",
                json!({
                    "type": event.event_type,
                    "x": event.x,
                    "y": event.y,
                    "button": event.button,
                    "clickCount": event.click_count,
                    "modifiers": event.modifiers,
                }),
            )
            .await?;
        Ok(())
    }

    /// Forwards a wheel event from a surface into the page.
    pub async fn dispatch_wheel(
        &self,
        lease_token: Option<&str>,
        x: f64,
        y: f64,
        delta_x: f64,
        delta_y: f64,
        modifiers: u32,
    ) -> Result<(), BrowserError> {
        self.require_drive(lease_token).await?;
        self.connection
            .call_session(
                &self.cdp_session().await,
                "Input.dispatchMouseEvent",
                json!({
                    "type": "mouseWheel",
                    "x": x,
                    "y": y,
                    "deltaX": delta_x,
                    "deltaY": delta_y,
                    "modifiers": modifiers,
                }),
            )
            .await?;
        Ok(())
    }

    /// Forwards a key event from a surface into the page.
    pub async fn dispatch_key(
        &self,
        lease_token: Option<&str>,
        event_type: &str,
        key: &str,
        code: &str,
        text: Option<&str>,
        modifiers: u32,
    ) -> Result<(), BrowserError> {
        self.require_drive(lease_token).await?;
        if !matches!(event_type, "keyDown" | "keyUp" | "rawKeyDown" | "char") {
            return Err(BrowserError::Invalid {
                detail: format!("{event_type} is not a key event this surface forwards"),
            });
        }
        let mut params = json!({
            "type": event_type,
            "key": key,
            "code": code,
            "modifiers": modifiers,
        });
        // Blink reads the *action* of a key off its virtual key code, not off
        // `key` or `code`. Omitting it leaves Backspace, Delete, the arrows,
        // Home/End and Enter inert while printable characters keep working,
        // because those ride in on `text`.
        if let Some(virtual_key) = keys::virtual_key_code(key, code) {
            params["windowsVirtualKeyCode"] = json!(virtual_key);
            params["nativeVirtualKeyCode"] = json!(virtual_key);
        }
        // A key-up carries no text: it is the press that inserts.
        let inserts_text = matches!(event_type, "keyDown" | "char");
        if let Some(text) = text.or_else(|| inserts_text.then(|| keys::key_text(key)).flatten()) {
            params["text"] = json!(text);
        }
        self.connection
            .call_session(&self.cdp_session().await, "Input.dispatchKeyEvent", params)
            .await?;
        Ok(())
    }

    /// Inserts text as if typed, bypassing per-key synthesis.
    pub async fn insert_text(&self, text: &str) -> Result<(), BrowserError> {
        self.connection
            .call_session(
                &self.cdp_session().await,
                "Input.insertText",
                json!({ "text": text }),
            )
            .await?;
        Ok(())
    }

    /// Kills the browser process without going through `close`.
    ///
    /// Only for tests that need to simulate a crash: production teardown goes
    /// through the broker so the session is deregistered too.
    #[cfg(test)]
    pub(crate) async fn kill_child_for_test(&self) {
        if let Some(mut child) = self.child.lock().await.take() {
            let _ = child.kill().await;
        }
    }

    async fn shutdown(&self) -> Result<(), BrowserError> {
        // Skipped once the socket is gone: the page died with the browser, and
        // the call would only wait out its timeout.
        if !self.connection.is_closed() {
            let _ = self
                .connection
                .call_session(&self.cdp_session().await, "Page.close", json!({}))
                .await;
        }
        let mut cleanup_errors = Vec::new();
        if let Some(mut child) = self.child.lock().await.take() {
            if let Err(error) = terminate_browser_child(&mut child).await {
                cleanup_errors.push(error.to_string());
            }
        }
        if let Err(error) = remove_profile_dir(&self.profile_dir).await {
            cleanup_errors.push(error.to_string());
        }
        if cleanup_errors.is_empty() {
            Ok(())
        } else {
            Err(BrowserError::Io {
                detail: format!(
                    "browser session {} cleanup failed: {}",
                    self.browser_id,
                    cleanup_errors.join("; ")
                ),
            })
        }
    }
}

/// Terminates a browser child without allowing a failed kill/reap to block
/// profile cleanup forever. A successful bounded wait is enough to prove the
/// process is gone even when the initial kill reported an already-exited child.
async fn terminate_browser_child(child: &mut Child) -> Result<(), BrowserError> {
    match timeout(BROWSER_PROCESS_CLEANUP_TIMEOUT, child.kill()).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => {
            let kill_error = format!("failed to terminate browser process: {error}");
            crate::utils::logging::log_debug(&format!(
                "[Wardian] browser child kill reported an error; using bounded reap: {kill_error}"
            ));
            reap_browser_child(child, &kill_error, BROWSER_PROCESS_CLEANUP_TIMEOUT).await
        }
        Err(_) => {
            let kill_error = format!(
                "timed out after {}ms terminating browser process",
                BROWSER_PROCESS_CLEANUP_TIMEOUT.as_millis()
            );
            crate::utils::logging::log_debug(&format!(
                "[Wardian] browser child kill timed out; using bounded reap: {kill_error}"
            ));
            reap_browser_child(child, &kill_error, BROWSER_PROCESS_CLEANUP_TIMEOUT).await
        }
    }
}

/// Waits for a browser child for a bounded interval after termination failed.
async fn reap_browser_child(
    child: &mut Child,
    termination_error: &str,
    timeout_duration: Duration,
) -> Result<(), BrowserError> {
    match timeout(timeout_duration, child.wait()).await {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(error)) => Err(BrowserError::Io {
            detail: format!("{termination_error}; failed to reap browser process: {error}"),
        }),
        Err(_) => Err(BrowserError::Io {
            detail: format!(
                "{termination_error}; timed out after {}ms waiting for browser process to exit",
                timeout_duration.as_millis()
            ),
        }),
    }
}

fn attach_cleanup_diagnostic(
    attach_error: &BrowserError,
    cleanup_error: &BrowserError,
    cleanup_stage: &str,
) -> String {
    format!(
        "browser attach failed ({attach_error}); {cleanup_stage} cleanup failed: {cleanup_error}"
    )
}

/// Removes a browser profile, tolerating only bounded transient removal
/// errors after a Chromium shutdown. An exhausted retry is returned to the
/// explicit close caller instead of being silently discarded.
async fn remove_profile_dir(profile_dir: &Path) -> Result<(), BrowserError> {
    let profile_dir = profile_dir.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let mut last_error = None;
        for attempt in 0..PROFILE_CLEANUP_ATTEMPTS {
            match std::fs::remove_dir_all(&profile_dir) {
                Ok(()) => return Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                Err(error) => {
                    last_error = Some(error);
                    if attempt + 1 < PROFILE_CLEANUP_ATTEMPTS {
                        std::thread::sleep(PROFILE_CLEANUP_RETRY_DELAY);
                    }
                }
            }
        }
        Err(format!(
            "failed to remove browser profile {} after {} attempts: {}",
            profile_dir.display(),
            PROFILE_CLEANUP_ATTEMPTS,
            last_error.expect("cleanup attempts always record an error")
        ))
    })
    .await
    .map_err(|error| BrowserError::Io {
        detail: format!("browser profile cleanup task failed: {error}"),
    })?
    .map_err(|detail| BrowserError::Io { detail })
}

/// Normalizes a user-supplied address into a URL the browser will accept.
///
/// Bare hosts and `localhost:3000` are the common agent inputs and must not be
/// rejected, but a scheme that could reach the local filesystem or execute
/// script is refused outright.
/// Reads a `Network.getCookies` or `Storage.getCookies` result.
fn cookies_from(result: &Value) -> Vec<BrowserCookie> {
    result
        .get("cookies")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .map(|cookie| BrowserCookie {
                    name: string_field(cookie, "name"),
                    value: string_field(cookie, "value"),
                    domain: string_field(cookie, "domain"),
                    path: string_field(cookie, "path"),
                    secure: cookie
                        .get("secure")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                    http_only: cookie
                        .get("httpOnly")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                    same_site: cookie
                        .get("sameSite")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    // The protocol reports -1 for a session cookie rather than
                    // omitting the field, so a non-positive expiry means "none".
                    expires: cookie
                        .get("expires")
                        .and_then(Value::as_f64)
                        .filter(|expires| *expires > 0.0),
                })
                .collect()
        })
        .unwrap_or_default()
}

fn string_field(value: &Value, field: &str) -> String {
    value
        .get(field)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Maps a `--same-site` value onto the three the protocol accepts.
fn normalize_same_site(value: &str) -> Result<&'static str, BrowserError> {
    match value.to_ascii_lowercase().as_str() {
        "strict" => Ok("Strict"),
        "lax" => Ok("Lax"),
        "none" => Ok("None"),
        other => Err(BrowserError::Invalid {
            detail: format!("{other} is not a SameSite value; use strict, lax, or none"),
        }),
    }
}

/// Builds a bounded storage snapshot out of what the page reported.
///
/// Both ceilings are applied here rather than in the page: a value that would
/// blow an agent's context should not be serialized across the protocol first.
fn storage_snapshot_from(area: StorageArea, parsed: &Value) -> StorageSnapshot {
    let origin = string_field(parsed, "origin");
    let mut entries = Vec::new();
    let mut total = 0usize;
    let mut truncated = false;
    for pair in parsed
        .get("entries")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        let key = pair
            .get(0)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let raw = pair.get(1).and_then(Value::as_str).unwrap_or_default();
        let value: String = raw.chars().take(MAX_STORAGE_VALUE_CHARS).collect();
        let value_truncated = value.chars().count() < raw.chars().count();
        total += key.len() + value.len();
        if total > MAX_STORAGE_BYTES {
            truncated = true;
            break;
        }
        entries.push(StorageEntry {
            key,
            value,
            truncated: value_truncated,
        });
    }
    StorageSnapshot {
        area,
        origin,
        entries,
        truncated,
    }
}

pub fn normalize_url(input: &str) -> Result<String, BrowserError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(BrowserError::Invalid {
            detail: "a URL is required".to_string(),
        });
    }
    let lowered = trimmed.to_ascii_lowercase();
    for blocked in ["javascript:", "data:", "file:", "vbscript:"] {
        if lowered.starts_with(blocked) {
            return Err(BrowserError::Invalid {
                detail: format!("{blocked} URLs are not allowed in a browser surface"),
            });
        }
    }
    if lowered == "about:blank" {
        return Ok("about:blank".to_string());
    }
    if lowered.starts_with("http://") || lowered.starts_with("https://") {
        return Ok(trimmed.to_string());
    }
    if lowered.contains("://") {
        return Err(BrowserError::Invalid {
            detail: format!("{trimmed} uses a scheme a browser surface cannot open"),
        });
    }
    Ok(format!("http://{trimmed}"))
}

/// What a caller must supply to open a session.
#[derive(Debug, Clone, Default)]
pub struct OpenBrowserRequest {
    pub url: Option<String>,
    pub owner_agent_id: Option<String>,
    pub workspace: Option<String>,
    pub viewport: Option<Viewport>,
}

/// Owns every live browser session in the app.
#[derive(Debug)]
pub struct BrowserSessionBroker {
    /// Shared so a session's event pump can reap itself when its browser dies.
    sessions: Arc<RwLock<HashMap<String, Arc<BrowserSession>>>>,
    next_short_ref: AtomicU32,
    events: broadcast::Sender<BrowserSessionEvent>,
    profile_root: PathBuf,
    /// Sibling of `profile_root`, so a session's downloads outlive its profile.
    download_root: PathBuf,
    /// Sessions that have asked for a surface and not yet been acknowledged.
    ///
    /// Outstanding work, not a startup buffer: an entry leaves only when a
    /// frontend confirms it opened the surface, so no delivery decision
    /// depends on a message that might not arrive. Repeated delivery is
    /// harmless because the surface is `focus_resource`.
    pending_surface_opens: Mutex<Vec<BrowserSessionSummary>>,
}

/// Ceiling on unacknowledged opens, in case nothing ever acknowledges them.
const MAX_PENDING_SURFACE_OPENS: usize = 32;

impl Default for BrowserSessionBroker {
    fn default() -> Self {
        Self::new(std::env::temp_dir().join("wardian-browser"))
    }
}

impl BrowserSessionBroker {
    /// Roots every session's on-disk state under one directory.
    ///
    /// Profiles and downloads are siblings rather than nested: a profile is
    /// deleted when its session closes, and taking the agent's downloaded file
    /// with it would defeat the point of downloading.
    pub fn new(browser_root: PathBuf) -> Self {
        let (events, _) = broadcast::channel(EVENT_CHANNEL_CAPACITY);
        let download_root = browser_root.join("downloads");
        // Growth is bounded at this end rather than at close, because the files
        // are meant to survive their session.
        prune_old_downloads(&download_root);
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            next_short_ref: AtomicU32::new(1),
            events,
            profile_root: browser_root.join("profiles"),
            download_root,
            pending_surface_opens: Mutex::new(Vec::new()),
        }
    }

    /// Records a session that still needs a workbench surface.
    ///
    /// Recorded unconditionally, then emitted. The control endpoint serves
    /// before the webview mounts and a reload can retire the listener at any
    /// moment, so the event alone is never treated as delivery — the entry
    /// stays until a frontend acknowledges it.
    pub async fn queue_surface_open(&self, summary: BrowserSessionSummary) {
        let mut pending = self.pending_surface_opens.lock().await;
        if pending
            .iter()
            .any(|queued| queued.browser_id == summary.browser_id)
        {
            return;
        }
        if pending.len() >= MAX_PENDING_SURFACE_OPENS {
            pending.remove(0);
        }
        pending.push(summary);
    }

    /// Every surface open still waiting to be acknowledged.
    ///
    /// Reading does not consume: a frontend that reads and then dies before
    /// opening anything must not have taken the work with it. Sessions that
    /// have since closed are pruned, so this cannot resurrect a surface for a
    /// browser that no longer exists.
    pub async fn pending_surface_opens(&self) -> Vec<BrowserSessionSummary> {
        let sessions = self.sessions.read().await;
        let mut pending = self.pending_surface_opens.lock().await;
        pending.retain(|summary| sessions.contains_key(&summary.browser_id));
        pending.clone()
    }

    /// Marks one open as surfaced, so no later reader repeats it.
    pub async fn ack_surface_open(&self, browser_id: &str) {
        self.pending_surface_opens
            .lock()
            .await
            .retain(|summary| summary.browser_id != browser_id);
    }

    /// Subscribes to every session's events, for forwarding to the frontend.
    pub fn subscribe(&self) -> broadcast::Receiver<BrowserSessionEvent> {
        self.events.subscribe()
    }

    /// Starts a browser, attaches to a fresh page, and registers the session.
    pub async fn open(
        &self,
        request: OpenBrowserRequest,
    ) -> Result<Arc<BrowserSession>, BrowserError> {
        // Validate the URL before paying for a browser launch.
        let target_url = match request.url.as_deref() {
            Some(url) => Some(normalize_url(url)?),
            None => None,
        };
        let binary = discover_engine().map_err(BrowserError::Engine)?;
        let browser_id = Uuid::new_v4().to_string();
        let viewport = request.viewport.unwrap_or_default();
        let profile_dir = self.profile_root.join(&browser_id);
        let download_dir = self.download_root.join(&browser_id);

        let mut launched = match launch_engine(
            &binary,
            &profile_dir,
            viewport.width,
            viewport.height,
        )
        .await
        {
            Ok(launched) => launched,
            Err(error) => {
                // Nothing started, but the profile directory was created.
                if let Err(cleanup_error) = remove_profile_dir(&profile_dir).await {
                    crate::utils::logging::log_debug(&format!(
                            "[Wardian] browser profile cleanup failed after launch error: {cleanup_error}"
                        ));
                }
                return Err(BrowserError::Engine(error));
            }
        };

        // The browser is running from here on. `kill_on_drop` would terminate
        // it but never reap it, and on Windows a dying Chromium still holds
        // its profile lock — so the child is killed and awaited explicitly
        // before the directory is removed.
        let attached = attach_page(&launched.websocket_url).await;
        let (connection, page, known_targets) = match attached {
            Ok(attached) => attached,
            Err(error) => {
                if let Err(cleanup_error) = terminate_browser_child(&mut launched.child).await {
                    crate::utils::logging::log_debug(&format!(
                        "[Wardian] {}",
                        attach_cleanup_diagnostic(&error, &cleanup_error, "process")
                    ));
                }
                if let Err(cleanup_error) = remove_profile_dir(&profile_dir).await {
                    crate::utils::logging::log_debug(&format!(
                        "[Wardian] {}",
                        attach_cleanup_diagnostic(&error, &cleanup_error, "profile")
                    ));
                }
                return Err(error);
            }
        };
        let main_frame_id = page.main_frame_id.clone();

        let session = Arc::new(BrowserSession {
            browser_id: browser_id.clone(),
            short_ref: self.next_short_ref.fetch_add(1, Ordering::Relaxed),
            owner_agent_id: request.owner_agent_id.clone(),
            workspace: request.workspace.clone(),
            engine: launched.kind,
            connection,
            targets: RwLock::new(vec![page.target]),
            profile_dir: profile_dir.clone(),
            download_dir: download_dir.clone(),
            child: Mutex::new(Some(launched.child)),
            state: RwLock::new(SessionState {
                viewport,
                main_frame_id,
                known_targets,
                ..SessionState::default()
            }),
            screencast_transition: Mutex::new(()),
            events: self.events.clone(),
        });
        // The session owns the child now, so its own teardown does the
        // killing, reaping, and profile removal.
        if let Err(error) = session.set_viewport(Some(viewport)).await {
            if let Err(cleanup_error) = session.shutdown().await {
                crate::utils::logging::log_debug(&format!(
                    "[Wardian] browser session startup cleanup failed: {cleanup_error}"
                ));
            }
            return Err(error);
        }
        // Best effort, and deliberately not fatal: a browser that cannot be
        // told where to put downloads is still a usable browser, and failing
        // the open would trade every other capability for one.
        if std::fs::create_dir_all(&download_dir).is_ok() {
            let _ = session
                .connection
                .call(
                    "Browser.setDownloadBehavior",
                    json!({
                        // `allowAndName` writes each file under its GUID, which
                        // makes the path deterministic before the suggested name
                        // is known. Completion renames it to something a caller
                        // can actually use.
                        "behavior": "allowAndName",
                        "downloadPath": download_dir.to_string_lossy(),
                        "eventsEnabled": true,
                    }),
                )
                .await;
        }

        // Registered before the pump starts. The other order lets a browser
        // that dies in between be reaped before it is registered — the reap
        // finds nothing, returns, and `open` then inserts a session that is
        // already dead with no pump left to remove it.
        self.sessions
            .write()
            .await
            .insert(browser_id.clone(), Arc::clone(&session));
        self.spawn_event_pump(Arc::clone(&session));
        self.spawn_dialog_watcher(Arc::clone(&session));

        if let Some(url) = target_url {
            // A failed first load is a page outcome, not a failed open. The
            // session is already registered and its browser is running, so
            // returning Err here would strand a live browser with no handle.
            // The caller sees `load_state: failed` and can navigate again.
            if session.navigate(&url).await.is_err() {
                session.state.write().await.load_state = LoadState::Failed;
            }
        }
        let _ = self.events.send(BrowserSessionEvent::State {
            browser_id,
            summary: session.summary().await,
        });
        Ok(session)
    }

    /// Services page dialogs on a subscription of their own.
    ///
    /// A dialog stops the renderer, and the session's event pump does page
    /// work inline — a `Runtime.evaluate` for a title, an ack for a frame.
    /// Leaving dialogs to the pump deadlocks the session: the one call that
    /// releases the renderer queues behind a call the renderer cannot answer,
    /// so `alert()` froze the page for a full protocol timeout and then some.
    /// This loop only ever reads, and hands the answering to a task of its
    /// own, so nothing a page does can stop it from arriving.
    fn spawn_dialog_watcher(&self, session: Arc<BrowserSession>) {
        let mut receiver = session.connection.subscribe();
        let events = self.events.clone();
        tokio::spawn(async move {
            loop {
                let event = match receiver.recv().await {
                    Ok(event) => event,
                    // Dropping a dialog announcement would strand the page, so
                    // a lagging reader keeps going rather than giving up.
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => return,
                };
                if event.method == DISCONNECTED_METHOD {
                    return;
                }
                match event.method.as_str() {
                    "Page.javascriptDialogOpening" => {}
                    "Page.javascriptDialogClosed" => {
                        // Answered here, or dismissed by a navigation away.
                        let presented = event.session_id.as_deref()
                            == Some(session.cdp_session().await.as_str());
                        if !presented {
                            continue;
                        }
                        let had_dialog = {
                            let mut state = session.state.write().await;
                            state.dialog.take().is_some()
                        };
                        if had_dialog {
                            let _ = events.send(BrowserSessionEvent::State {
                                browser_id: session.browser_id.clone(),
                                summary: session.summary().await,
                            });
                        }
                        continue;
                    }
                    _ => continue,
                }
                let Some(cdp_session_id) = event.session_id.clone() else {
                    continue;
                };
                let dialog = BrowserDialog {
                    kind: event
                        .params
                        .get("type")
                        .and_then(Value::as_str)
                        .unwrap_or("alert")
                        .to_string(),
                    message: event
                        .params
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    default_prompt: event
                        .params
                        .get("defaultPrompt")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                };
                let session = Arc::clone(&session);
                let events = events.clone();
                // Spawned so the loop is ready for the next announcement even
                // while this one is being answered.
                tokio::spawn(async move {
                    let presented = cdp_session_id == session.cdp_session().await;
                    // A dialog on a page nobody is presenting — the opener
                    // behind a popup — cannot be shown, so it is answered
                    // rather than left to stop that page for good. So is one
                    // on a session no surface is watching: an agent driving
                    // through the CLI navigates away from `beforeunload`
                    // constantly and would otherwise wedge on the first.
                    if !presented || session.attachment_count().await == 0 {
                        let _ = dispatch_dialog_answer(
                            &session.connection,
                            &cdp_session_id,
                            safe_dialog_answer(&dialog.kind),
                            None,
                        )
                        .await;
                        return;
                    }
                    session.state.write().await.dialog = Some(dialog);
                    let _ = events.send(BrowserSessionEvent::State {
                        browser_id: session.browser_id.clone(),
                        summary: session.summary().await,
                    });
                });
            }
        });
    }

    /// Translates protocol events into session state and surface events.
    fn spawn_event_pump(&self, session: Arc<BrowserSession>) {
        let mut receiver = session.connection.subscribe();
        let events = self.events.clone();
        let sessions = Arc::clone(&self.sessions);
        tokio::spawn(async move {
            // The connection can close between `subscribe` and the first
            // `recv`, in which case the disconnect event was published to
            // nobody. Checking once here means that window cannot strand the
            // session.
            if session.connection.is_closed() {
                reap_dead_session(&sessions, &session, &events).await;
                return;
            }
            loop {
                let event = match receiver.recv().await {
                    Ok(event) => event,
                    // A lagging receiver dropped events, and this channel
                    // carries `Page.frameNavigated` as well as frames. Rather
                    // than assume only frames were lost, invalidate every
                    // outstanding ref and resynchronize: a spurious
                    // `snapshot_stale` costs one re-snapshot, while a missed
                    // navigation would let a ref act on a different document.
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        session.state.write().await.ledger.invalidate();
                        resynchronize(&session, &events).await;
                        continue;
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        reap_dead_session(&sessions, &session, &events).await;
                        return;
                    }
                };
                // Checked before the session filter: the disconnect signal is
                // connection-scoped and carries no target session.
                if event.method == DISCONNECTED_METHOD {
                    reap_dead_session(&sessions, &session, &events).await;
                    return;
                }
                match event.session_id.as_deref() {
                    // The presented page. A popup this session adopted is the
                    // presented page while it is up, so the comparison is
                    // against the top of the stack rather than the base.
                    Some(id) if id == session.cdp_session().await => {}
                    // Browser-scoped events — target discovery and download
                    // progress among them — carry no session id, and this
                    // connection serves exactly one browser, so an unaddressed
                    // event here is this session's by construction.
                    None => {}
                    // A target this session is not presenting: the page behind
                    // an open popup, or a worker. Its page events must not
                    // rewrite what the surface is showing.
                    Some(_) => continue,
                }
                handle_protocol_event(&session, &events, event).await;
            }
        });
    }

    /// Resolves `browser:N`, a full id, or an unambiguous id prefix.
    pub async fn resolve(&self, target: &str) -> Result<Arc<BrowserSession>, BrowserError> {
        let trimmed = target.trim();
        if trimmed.is_empty() {
            return Err(BrowserError::NotFound {
                target: "an empty target".to_string(),
            });
        }
        let sessions = self.sessions.read().await;
        if let Some(index) = trimmed
            .strip_prefix("browser:")
            .and_then(|rest| rest.parse::<u32>().ok())
        {
            return sessions
                .values()
                .find(|session| session.short_ref == index)
                .cloned()
                .ok_or_else(|| BrowserError::NotFound {
                    target: trimmed.to_string(),
                });
        }
        if let Some(session) = sessions.get(trimmed) {
            return Ok(Arc::clone(session));
        }
        let matches: Vec<Arc<BrowserSession>> = sessions
            .values()
            .filter(|session| session.browser_id.starts_with(trimmed))
            .cloned()
            .collect();
        match matches.len() {
            0 => Err(BrowserError::NotFound {
                target: trimmed.to_string(),
            }),
            1 => Ok(matches.into_iter().next().expect("one match")),
            _ => Err(BrowserError::Ambiguous {
                target: trimmed.to_string(),
                matches: matches.iter().map(|session| session.short_ref()).collect(),
            }),
        }
    }

    /// Every open session, ordered by short ref so listings are stable.
    pub async fn list(&self) -> Vec<BrowserSessionSummary> {
        let sessions = self.sessions.read().await;
        let mut ordered: Vec<Arc<BrowserSession>> = sessions.values().cloned().collect();
        drop(sessions);
        ordered.sort_by_key(|session| session.short_ref);
        let mut summaries = Vec::with_capacity(ordered.len());
        for session in ordered {
            summaries.push(session.summary().await);
        }
        summaries
    }

    /// Closes one session and stops its browser.
    pub async fn close(&self, target: &str) -> Result<String, BrowserError> {
        let session = self.resolve(target).await?;
        let browser_id = session.browser_id.clone();
        // A crash can win the race to remove this session. Only whoever
        // actually took it out of the map announces and tears down, so a
        // listener never sees two contradictory closures for one session.
        if take_session(&self.sessions, &browser_id).await.is_some() {
            let cleanup_result = session.shutdown().await;
            let _ = self.events.send(BrowserSessionEvent::Closed {
                browser_id: browser_id.clone(),
                reason: "closed".to_string(),
            });
            cleanup_result?;
        }
        Ok(browser_id)
    }

    /// Closes every session owned by an agent that is going away.
    pub async fn close_for_agent(&self, agent_id: &str) -> Vec<String> {
        let owned: Vec<Arc<BrowserSession>> = self
            .sessions
            .read()
            .await
            .values()
            .filter(|session| session.owner_agent_id.as_deref() == Some(agent_id))
            .cloned()
            .collect();
        let mut closed = Vec::with_capacity(owned.len());
        for session in owned {
            let browser_id = session.browser_id.clone();
            if take_session(&self.sessions, &browser_id).await.is_none() {
                continue;
            }
            if let Err(error) = session.shutdown().await {
                crate::utils::logging::log_debug(&format!(
                    "[Wardian] browser session {browser_id} cleanup failed while its agent stopped: {error}"
                ));
            }
            let _ = self.events.send(BrowserSessionEvent::Closed {
                browser_id: browser_id.clone(),
                reason: "the owning agent stopped".to_string(),
            });
            closed.push(browser_id);
        }
        closed
    }

    /// Stops every session. Called on app exit.
    pub async fn shutdown_all(&self) {
        let sessions: Vec<Arc<BrowserSession>> = self
            .sessions
            .write()
            .await
            .drain()
            .map(|(_, s)| s)
            .collect();
        for session in sessions {
            if let Err(error) = session.shutdown().await {
                crate::utils::logging::log_debug(&format!(
                    "[Wardian] browser session {} cleanup failed during app shutdown: {error}",
                    session.browser_id
                ));
            }
        }
    }
}

/// Atomically removes a session from the broker.
///
/// The single point every teardown path goes through: whoever gets the session
/// back owns announcing and shutting it down, so a crash racing an explicit
/// close produces exactly one closed event.
async fn take_session(
    sessions: &Arc<RwLock<HashMap<String, Arc<BrowserSession>>>>,
    browser_id: &str,
) -> Option<Arc<BrowserSession>> {
    sessions.write().await.remove(browser_id)
}

/// Removes a session whose browser is gone and announces it exactly once.
///
/// Without this the broker would keep listing a dead session, and every later
/// command would resolve it and fail against a closed connection instead of
/// reporting `browser_not_found`.
async fn reap_dead_session(
    sessions: &Arc<RwLock<HashMap<String, Arc<BrowserSession>>>>,
    session: &Arc<BrowserSession>,
    events: &broadcast::Sender<BrowserSessionEvent>,
) {
    let browser_id = session.browser_id.clone();
    if take_session(sessions, &browser_id).await.is_none() {
        return;
    }
    // Announce before cleaning up. Surfaces should learn immediately rather
    // than waiting behind teardown of a browser that is already gone.
    let _ = events.send(BrowserSessionEvent::Closed {
        browser_id,
        reason: "the browser process exited".to_string(),
    });
    if let Err(error) = session.shutdown().await {
        crate::utils::logging::log_debug(&format!(
            "[Wardian] browser session cleanup failed after process exit: {error}"
        ));
    }
}

/// What a freshly attached page target needs before it can be presented.
struct AttachedPage {
    target: AttachedTarget,
    main_frame_id: Option<String>,
}

/// Turns a page target into one this session can present and drive.
///
/// Every domain a session depends on is enabled here rather than only at open,
/// because a popup this session adopts has to arrive equipped the same way its
/// opener did — otherwise the console, the network ledger, and the frame
/// events all stop the moment a page opens a window.
async fn equip_target(
    connection: &Arc<CdpConnection>,
    target_id: &str,
) -> Result<AttachedPage, BrowserError> {
    let attached = connection
        .call(
            "Target.attachToTarget",
            json!({ "targetId": target_id, "flatten": true }),
        )
        .await?;
    let cdp_session_id = required_str("Target.attachToTarget", &attached, "sessionId")?;

    for method in ["Page.enable", "Runtime.enable", "Log.enable"] {
        connection
            .call_session(&cdp_session_id, method, json!({}))
            .await?;
    }
    // Recording starts with the session, not with the first `network` call. An
    // agent asks about the network *after* something went wrong, and a ledger
    // that begins recording at the moment of the question is empty exactly when
    // it matters. The buffer ceilings bound what the browser keeps for
    // `Network.getResponseBody`, which is the only thing read back live.
    connection
        .call_session(
            &cdp_session_id,
            "Network.enable",
            json!({
                "maxTotalBufferSize": 10 * 1024 * 1024,
                "maxResourceBufferSize": 5 * 1024 * 1024,
            }),
        )
        .await?;
    // Read once at attach rather than waiting for the first main-frame commit,
    // so a session that only ever routes within its document can still tell its
    // own frame's events from a subframe's.
    let main_frame_id = connection
        .call_session(&cdp_session_id, "Page.getFrameTree", json!({}))
        .await
        .ok()
        .and_then(|tree| {
            tree.get("frameTree")
                .and_then(|frame_tree| frame_tree.get("frame"))
                .and_then(|frame| frame.get("id"))
                .and_then(Value::as_str)
                .map(str::to_string)
        });
    Ok(AttachedPage {
        target: AttachedTarget {
            target_id: target_id.to_string(),
            cdp_session_id,
        },
        main_frame_id,
    })
}

/// Connects to a launched browser and attaches to a fresh page.
///
/// Free of the broker so `open` keeps ownership of the child across the whole
/// fallible region and can reap it before touching the profile directory.
async fn attach_page(
    websocket_url: &str,
) -> Result<(Arc<CdpConnection>, AttachedPage, HashSet<String>), BrowserError> {
    let connection = CdpConnection::connect(websocket_url).await?;

    // Size is deliberately omitted: the protocol only accepts it alongside
    // `newWindow`, and the viewport is established by
    // `Emulation.setDeviceMetricsOverride`, which is what the screencast
    // actually follows.
    let created = connection
        .call("Target.createTarget", json!({ "url": "about:blank" }))
        .await?;
    let target_id = required_str("Target.createTarget", &created, "targetId")?;
    let page = equip_target(&connection, &target_id).await?;

    // Everything that exists now is scenery: this session's own page, and the
    // blank one the browser opens for itself at launch. Discovery re-announces
    // all of them, so the pump needs the list that was already there to tell a
    // popup from the furniture.
    let known = connection
        .call("Target.getTargets", json!({}))
        .await
        .ok()
        .and_then(|targets| {
            targets
                .get("targetInfos")
                .and_then(Value::as_array)
                .cloned()
        })
        .unwrap_or_default()
        .iter()
        .filter_map(|info| info.get("targetId").and_then(Value::as_str))
        .map(str::to_string)
        .collect::<HashSet<String>>();
    // Discovery rather than auto-attach: auto-attach would also pause every
    // service worker until this client released it, which is a page-breaking
    // way to learn about a popup.
    connection
        .call("Target.setDiscoverTargets", json!({ "discover": true }))
        .await?;

    Ok((connection, page, known))
}

/// Answers one dialog on the protocol session that raised it.
///
/// Free of `BrowserSession` so the dialog watcher can answer a dialog raised
/// by a target the session is not presenting — the opener behind a popup —
/// which still stops that page and which nobody can see.
async fn dispatch_dialog_answer(
    connection: &Arc<CdpConnection>,
    cdp_session_id: &str,
    accept: bool,
    prompt_text: Option<&str>,
) -> Result<(), BrowserError> {
    let mut params = json!({ "accept": accept });
    if let Some(text) = prompt_text {
        params["promptText"] = json!(text);
    }
    connection
        .call_session(cdp_session_id, "Page.handleJavaScriptDialog", params)
        .await?;
    Ok(())
}

/// What a session with nobody watching should answer a dialog with.
///
/// `beforeunload` is accepted because the navigation that raised it was asked
/// for, and refusing it would silently cancel the caller's own request.
/// Everything else is dismissed: a `confirm` nobody saw has not been agreed
/// to, and a `prompt` nobody answered has no answer.
fn safe_dialog_answer(kind: &str) -> bool {
    kind == "beforeunload"
}

/// Applies one protocol event to session state and republishes what surfaces need.
async fn handle_protocol_event(
    session: &Arc<BrowserSession>,
    events: &broadcast::Sender<BrowserSessionEvent>,
    event: CdpEvent,
) {
    let browser_id = session.browser_id.clone();
    match event.method.as_str() {
        // A page opened a window. The surface has one viewport, so the popup
        // is presented in place of its opener rather than disappearing into a
        // target nobody is attached to.
        "Target.targetCreated" => {
            let Some(info) = event.params.get("targetInfo") else {
                return;
            };
            if info.get("type").and_then(Value::as_str) != Some("page") {
                return;
            }
            let Some(target_id) = info.get("targetId").and_then(Value::as_str) else {
                return;
            };
            {
                let mut state = session.state.write().await;
                // Discovery replays what already existed, and the browser's
                // own startup page is among it.
                if !state.known_targets.insert(target_id.to_string()) {
                    return;
                }
            }
            session.adopt_popup(target_id, events).await;
        }
        "Target.targetDestroyed" => {
            let Some(target_id) = event.params.get("targetId").and_then(Value::as_str) else {
                return;
            };
            session.state.write().await.known_targets.remove(target_id);
            session.release_popup(target_id, events).await;
        }
        "Page.screencastFrame" => {
            let ack_id = event.params.get("sessionId").and_then(Value::as_i64);
            if let Some(ack_id) = ack_id {
                let _ = session
                    .connection
                    .call_session(
                        &session.cdp_session().await,
                        "Page.screencastFrameAck",
                        json!({ "sessionId": ack_id }),
                    )
                    .await;
            }
            let Some(data) = event.params.get("data").and_then(Value::as_str) else {
                return;
            };
            let metadata = event.params.get("metadata");
            let width = metadata
                .and_then(|value| value.get("deviceWidth"))
                .and_then(Value::as_f64)
                .unwrap_or(f64::from(DEFAULT_VIEWPORT_WIDTH)) as u32;
            let height = metadata
                .and_then(|value| value.get("deviceHeight"))
                .and_then(Value::as_f64)
                .unwrap_or(f64::from(DEFAULT_VIEWPORT_HEIGHT)) as u32;
            let _ = events.send(BrowserSessionEvent::Frame {
                browser_id,
                data: data.to_string(),
                width,
                height,
            });
        }
        // A History API or fragment navigation changes the route without a
        // frame commit. The URL has to follow it, and refs taken against the
        // previous route must not survive it.
        "Page.navigatedWithinDocument" => {
            let frame_id = event.params.get("frameId").and_then(Value::as_str);
            let url = event
                .params
                .get("url")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            {
                let mut state = session.state.write().await;
                // An iframe routing itself is not a top-level navigation. Only
                // an unknown main frame falls through, so a session whose frame
                // tree never resolved still tracks its own route.
                if let (Some(main), Some(frame_id)) = (state.main_frame_id.as_deref(), frame_id) {
                    if main != frame_id {
                        return;
                    }
                }
                state.ledger.invalidate();
                if !url.is_empty() {
                    state.url = url;
                }
            }
            let _ = events.send(BrowserSessionEvent::State {
                browser_id,
                summary: session.summary().await,
            });
        }
        "Page.frameNavigated" => {
            // Only a main-frame commit invalidates refs; an iframe navigating
            // must not throw away the refs the agent just took.
            let is_main_frame = event
                .params
                .get("frame")
                .map(|frame| frame.get("parentId").is_none())
                .unwrap_or(false);
            if !is_main_frame {
                return;
            }
            let url = event
                .params
                .get("frame")
                .and_then(|frame| frame.get("url"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let frame_id = event
                .params
                .get("frame")
                .and_then(|frame| frame.get("id"))
                .and_then(Value::as_str)
                .map(str::to_string);
            {
                let mut state = session.state.write().await;
                // A cross-process navigation can hand the target a new main
                // frame id, so this is refreshed rather than set once.
                if frame_id.is_some() {
                    state.main_frame_id = frame_id;
                }
                state.ledger.invalidate();
                state.url = url;
                state.load_state = LoadState::Loading;
                state.console_error_count = 0;
                state.console.clear();
            }
            let _ = events.send(BrowserSessionEvent::State {
                browser_id,
                summary: session.summary().await,
            });
        }
        "Page.loadEventFired" | "Page.domContentEventFired" => {
            {
                let mut state = session.state.write().await;
                state.load_state = LoadState::Complete;
            }
            if let Ok(title) = session.get(PageField::Title, None).await {
                session.state.write().await.title = title;
            }
            let _ = events.send(BrowserSessionEvent::State {
                browser_id,
                summary: session.summary().await,
            });
        }
        "Runtime.consoleAPICalled" | "Log.entryAdded" => {
            let (level, text) = console_entry_from(&event);
            let is_error = level == "error";
            let entry = ConsoleEntry { level, text };
            {
                let mut state = session.state.write().await;
                if is_error {
                    state.console_error_count += 1;
                }
                if state.console.len() >= CONSOLE_BUFFER {
                    state.console.pop_front();
                }
                state.console.push_back(entry.clone());
            }
            let _ = events.send(BrowserSessionEvent::Console { browser_id, entry });
        }
        // Deliberately free of protocol calls. The pump is a single consumer on
        // a channel that also carries screencast frames, and a lag there does
        // not merely drop a frame — it invalidates every outstanding ref. A page
        // load emits several hundred of these, so folding one must never wait
        // on a round-trip the way the title read after `loadEventFired` does.
        method if method.starts_with("Network.") => {
            let failures_moved = {
                let mut state = session.state.write().await;
                let before = state.network.failure_count();
                state.network.apply(method, &event.params);
                state.network.failure_count() != before
            };
            // A failed request arrives with no navigation and no load event to
            // carry it, so without this the surface would keep showing a stale
            // count until the page happened to move on its own. Gated on the
            // count actually changing: a page load is several hundred of these
            // and almost none of them are news.
            if failures_moved {
                let _ = events.send(BrowserSessionEvent::State {
                    browser_id,
                    summary: session.summary().await,
                });
            }
        }
        "Browser.downloadWillBegin" => {
            let record = DownloadRecord {
                guid: string_field(&event.params, "guid"),
                url: string_field(&event.params, "url"),
                suggested_filename: string_field(&event.params, "suggestedFilename"),
                state: "in_progress".to_string(),
                received_bytes: 0,
                total_bytes: 0,
                path: None,
            };
            if record.guid.is_empty() {
                return;
            }
            let mut state = session.state.write().await;
            if state.downloads.len() >= MAX_TRACKED_DOWNLOADS {
                state.downloads.remove(0);
            }
            state.downloads.push(record);
        }
        "Browser.downloadProgress" => {
            let guid = string_field(&event.params, "guid");
            let progress_state = event
                .params
                .get("state")
                .and_then(Value::as_str)
                .unwrap_or("inProgress");
            let received = event
                .params
                .get("receivedBytes")
                .and_then(Value::as_f64)
                .unwrap_or_default()
                .max(0.0) as u64;
            let total = event
                .params
                .get("totalBytes")
                .and_then(Value::as_f64)
                .unwrap_or_default()
                .max(0.0) as u64;
            let mut state = session.state.write().await;
            if let Some(record) = state
                .downloads
                .iter_mut()
                .rev()
                .find(|record| record.guid == guid)
            {
                record.received_bytes = received;
                record.total_bytes = total;
                record.state = match progress_state {
                    "completed" => "completed",
                    "canceled" => "canceled",
                    _ => "in_progress",
                }
                .to_string();
            }
            // The file is not touched here. Renaming it belongs to `downloads`,
            // which is not on the hot path and cannot race the browser's own
            // finalization of the file.
        }
        _ => {}
    }
}

/// Ceiling on tracked downloads, so a page that downloads in a loop cannot grow
/// the record without bound.
const MAX_TRACKED_DOWNLOADS: usize = 100;

/// Renames a completed download from its GUID to its suggested filename.
///
/// `allowAndName` gives a deterministic path before the name is known, which is
/// what makes the file findable at all — but a GUID is useless to a caller, so
/// the last step is to put the name back. A rename that fails is not an error:
/// the file exists either way, and reporting the GUID path is more useful than
/// reporting nothing.
fn resolve_completed_download(
    download_dir: &Path,
    guid: &str,
    suggested_filename: &str,
) -> Option<String> {
    let source = download_dir.join(guid);
    if !source.exists() {
        return None;
    }
    // Only the file name is taken from the page, so a suggestion like
    // `../../.bashrc` cannot escape the download directory.
    let name = Path::new(suggested_filename)
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .filter(|name| !name.is_empty() && name != "." && name != "..");
    let Some(name) = name else {
        return Some(source.to_string_lossy().to_string());
    };
    let destination = unique_download_path(download_dir, &name);
    match std::fs::rename(&source, &destination) {
        Ok(()) => Some(destination.to_string_lossy().to_string()),
        Err(_) => Some(source.to_string_lossy().to_string()),
    }
}

/// Finds a free name, appending ` (2)`, ` (3)`, … before the extension.
fn unique_download_path(download_dir: &Path, name: &str) -> PathBuf {
    let candidate = download_dir.join(name);
    if !candidate.exists() {
        return candidate;
    }
    let path = Path::new(name);
    let stem = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_string())
        .unwrap_or_else(|| name.to_string());
    let extension = path
        .extension()
        .map(|extension| format!(".{}", extension.to_string_lossy()))
        .unwrap_or_default();
    for index in 2..1000 {
        let candidate = download_dir.join(format!("{stem} ({index}){extension}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    download_dir.join(format!("{stem} ({}){extension}", Uuid::new_v4()))
}

/// Removes download directories older than [`DOWNLOAD_RETENTION_DAYS`].
///
/// Downloads deliberately outlive their session, so this is the only thing
/// standing between that and unbounded disk use.
fn prune_old_downloads(download_root: &Path) {
    let Some(cutoff) = std::time::SystemTime::now()
        .checked_sub(Duration::from_secs(DOWNLOAD_RETENTION_DAYS * 24 * 60 * 60))
    else {
        return;
    };
    prune_downloads_before(download_root, cutoff);
}

/// The pruning rule, with its cutoff supplied.
///
/// Split out so the rule can be tested against a chosen instant rather than by
/// backdating a directory, which has no portable API.
fn prune_downloads_before(download_root: &Path, cutoff: std::time::SystemTime) {
    let Ok(entries) = std::fs::read_dir(download_root) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if !metadata.is_dir() {
            continue;
        }
        // A directory whose age cannot be read is left alone: deleting on a
        // missing timestamp would be a guess, and the guess destroys data.
        let Ok(modified) = metadata.modified() else {
            continue;
        };
        if modified < cutoff {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

/// Re-reads the page's own view of itself after events were dropped.
///
/// Cheaper and more truthful than guessing which events were lost.
async fn resynchronize(
    session: &Arc<BrowserSession>,
    events: &broadcast::Sender<BrowserSessionEvent>,
) {
    if let Ok(url) = session.get(PageField::Url, None).await {
        session.state.write().await.url = url;
    }
    if let Ok(title) = session.get(PageField::Title, None).await {
        session.state.write().await.title = title;
    }
    let _ = events.send(BrowserSessionEvent::State {
        browser_id: session.browser_id().to_string(),
        summary: session.summary().await,
    });
}

/// Flattens either console event shape into one level/text pair.
pub(crate) fn console_entry_from(event: &CdpEvent) -> (String, String) {
    if event.method == "Log.entryAdded" {
        let entry = event.params.get("entry");
        let level = entry
            .and_then(|value| value.get("level"))
            .and_then(Value::as_str)
            .unwrap_or("info")
            .to_string();
        let text = entry
            .and_then(|value| value.get("text"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        return (normalize_console_level(&level), text);
    }
    let level = event
        .params
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("log")
        .to_string();
    let text = event
        .params
        .get("args")
        .and_then(Value::as_array)
        .map(|args| {
            args.iter()
                .map(|arg| {
                    arg.get("value")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                        .or_else(|| arg.get("value").map(std::string::ToString::to_string))
                        .or_else(|| {
                            arg.get("description")
                                .and_then(Value::as_str)
                                .map(str::to_string)
                        })
                        .unwrap_or_default()
                })
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default();
    (normalize_console_level(&level), text)
}

/// Collapses the protocol's several severity vocabularies into three levels.
pub(crate) fn normalize_console_level(level: &str) -> String {
    match level {
        "error" | "assert" | "severe" => "error",
        "warning" | "warn" => "warning",
        _ => "info",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn browser_reap_timeout_is_reported_without_waiting_indefinitely() {
        let mut command =
            tokio::process::Command::new(if cfg!(windows) { "ping.exe" } else { "sleep" });
        if cfg!(windows) {
            command.args(["-n", "30", "127.0.0.1"]);
        } else {
            command.arg("30");
        }
        let mut child = command.spawn().expect("spawn long-running test child");

        let result = reap_browser_child(
            &mut child,
            "synthetic browser kill failure",
            Duration::from_millis(20),
        )
        .await;
        let _ = child.kill().await;
        let _ = child.wait().await;

        let error = result.expect_err("a live child must hit the bounded reap timeout");
        assert!(error.to_string().contains("timed out after 20ms"));
    }

    #[test]
    fn attach_cleanup_diagnostic_retains_original_and_cleanup_errors() {
        let attach_error = BrowserError::Invalid {
            detail: "attach failed".to_string(),
        };
        let cleanup_error = BrowserError::Io {
            detail: "kill failed".to_string(),
        };
        let diagnostic = attach_cleanup_diagnostic(&attach_error, &cleanup_error, "process");

        assert!(diagnostic.contains("attach failed"));
        assert!(diagnostic.contains("kill failed"));
    }

    #[tokio::test]
    async fn profile_cleanup_reports_an_unremovable_path() {
        let root = std::env::temp_dir().join(format!("wardian-profile-cleanup-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("create test root");
        let profile = root.join("profile");
        std::fs::write(&profile, b"not a directory").expect("create blocking file");

        let error = remove_profile_dir(&profile)
            .await
            .expect_err("a file cannot be removed as a profile directory");
        assert_eq!(error.code(), "browser_io_error");
        assert!(error
            .to_string()
            .contains("failed to remove browser profile"));
        assert!(
            profile.is_file(),
            "the test fixture must remain for cleanup"
        );

        std::fs::remove_dir_all(&root).expect("remove test root");
    }

    #[test]
    fn accepts_http_and_https_unchanged() {
        assert_eq!(
            normalize_url("https://example.com/a").unwrap(),
            "https://example.com/a"
        );
        assert_eq!(
            normalize_url("  http://localhost:3000 ").unwrap(),
            "http://localhost:3000"
        );
    }

    #[test]
    fn promotes_a_bare_host_to_http() {
        assert_eq!(
            normalize_url("localhost:5173").unwrap(),
            "http://localhost:5173"
        );
        assert_eq!(normalize_url("example.com").unwrap(), "http://example.com");
    }

    #[test]
    fn refuses_schemes_that_could_reach_the_host_or_run_script() {
        for blocked in [
            "javascript:alert(1)",
            "JavaScript:alert(1)",
            "data:text/html,<h1>x",
            "file:///etc/passwd",
            "vbscript:msgbox",
        ] {
            let error = normalize_url(blocked).expect_err("blocked");
            assert_eq!(error.code(), "browser_invalid_request", "{blocked}");
        }
    }

    #[test]
    fn refuses_an_unknown_scheme_rather_than_prefixing_it() {
        let error = normalize_url("ftp://example.com").expect_err("blocked");
        assert!(error.to_string().contains("cannot open"));
    }

    #[test]
    fn allows_about_blank_as_the_one_non_http_target() {
        assert_eq!(normalize_url("about:blank").unwrap(), "about:blank");
    }

    #[test]
    fn refuses_an_empty_url() {
        assert_eq!(
            normalize_url("   ").expect_err("empty").code(),
            "browser_invalid_request"
        );
    }

    #[test]
    fn wait_conditions_compile_to_javascript_that_quotes_their_input() {
        let predicate = WaitCondition::Selector("#a[data-x='1']".to_string())
            .predicate()
            .expect("predicate");
        assert!(predicate.contains(r##"document.querySelector("#a[data-x='1']")"##));
        let text = WaitCondition::Text("say \"hi\"".to_string())
            .predicate()
            .expect("predicate");
        assert!(text.contains(r#"\"hi\""#), "quotes must be escaped: {text}");
    }

    #[test]
    fn waiting_for_the_failed_load_state_has_no_predicate() {
        assert_eq!(
            WaitCondition::LoadState(LoadState::Failed).predicate(),
            None
        );
    }

    #[test]
    fn wait_conditions_describe_themselves_for_the_timeout_message() {
        assert_eq!(
            WaitCondition::LoadState(LoadState::Complete).describe(),
            "load state complete"
        );
        assert_eq!(
            WaitCondition::UrlContains("/dashboard".to_string()).describe(),
            r#"url containing "/dashboard""#
        );
    }

    #[test]
    fn page_fields_parse_from_their_cli_names() {
        assert_eq!(PageField::parse("url"), Some(PageField::Url));
        assert_eq!(PageField::parse("html"), Some(PageField::Html));
        assert_eq!(PageField::parse("screenshot"), None);
    }

    #[test]
    fn console_levels_collapse_to_three_buckets() {
        assert_eq!(normalize_console_level("severe"), "error");
        assert_eq!(normalize_console_level("assert"), "error");
        assert_eq!(normalize_console_level("warn"), "warning");
        assert_eq!(normalize_console_level("debug"), "info");
    }

    #[test]
    fn reads_a_console_api_event_into_a_level_and_text() {
        let event = CdpEvent {
            session_id: Some("s".to_string()),
            method: "Runtime.consoleAPICalled".to_string(),
            params: json!({
                "type": "error",
                "args": [{ "value": "boom" }, { "value": "again" }]
            }),
        };
        assert_eq!(
            console_entry_from(&event),
            ("error".to_string(), "boom again".to_string())
        );
    }

    #[test]
    fn reads_a_log_entry_event_into_a_level_and_text() {
        let event = CdpEvent {
            session_id: Some("s".to_string()),
            method: "Log.entryAdded".to_string(),
            params: json!({ "entry": { "level": "warning", "text": "deprecated" } }),
        };
        assert_eq!(
            console_entry_from(&event),
            ("warning".to_string(), "deprecated".to_string())
        );
    }

    #[test]
    fn error_codes_are_stable_for_json_consumers() {
        assert_eq!(
            BrowserError::NotFound {
                target: "browser:9".to_string()
            }
            .code(),
            "browser_not_found"
        );
        assert_eq!(
            BrowserError::WaitTimeout {
                condition: "x".to_string(),
                timeout_ms: 1
            }
            .code(),
            "browser_wait_timeout"
        );
        assert_eq!(
            BrowserError::Ref(RefError::NoSnapshot).code(),
            "snapshot_missing",
            "ref failures keep their own code rather than being flattened"
        );
    }

    #[test]
    fn a_not_found_error_points_at_the_listing_command() {
        let message = BrowserError::NotFound {
            target: "browser:9".to_string(),
        }
        .to_string();
        assert!(message.contains("wardian browser list"));
    }

    #[tokio::test]
    async fn resolving_against_an_empty_broker_reports_not_found() {
        let broker = BrowserSessionBroker::new(std::env::temp_dir().join("wardian-test-profiles"));
        assert_eq!(
            broker.resolve("browser:1").await.expect_err("empty").code(),
            "browser_not_found"
        );
        assert_eq!(
            broker.resolve("  ").await.expect_err("empty").code(),
            "browser_not_found"
        );
        assert!(broker.list().await.is_empty());
    }

    #[test]
    fn a_session_event_always_names_its_session() {
        let event = BrowserSessionEvent::Closed {
            browser_id: "abc".to_string(),
            reason: "closed".to_string(),
        };
        assert_eq!(event.browser_id(), "abc");
    }

    #[test]
    fn session_events_serialize_with_a_discriminating_kind() {
        let event = BrowserSessionEvent::Frame {
            browser_id: "abc".to_string(),
            data: "AAA".to_string(),
            width: 100,
            height: 50,
        };
        let encoded = serde_json::to_value(&event).expect("serialize");
        assert_eq!(encoded["kind"], "frame");
        assert_eq!(encoded["browser_id"], "abc");
    }

    #[test]
    fn a_session_cookie_reports_no_expiry_rather_than_the_protocols_minus_one() {
        let cookies = cookies_from(&json!({
            "cookies": [
                { "name": "sid", "value": "a", "domain": "example.com", "path": "/", "expires": -1.0 },
                {
                    "name": "keep", "value": "b", "domain": "example.com", "path": "/",
                    "expires": 1_800_000_000.0, "secure": true, "httpOnly": true, "sameSite": "Lax",
                },
            ]
        }));
        assert_eq!(cookies.len(), 2);
        assert_eq!(cookies[0].expires, None);
        assert!(!cookies[0].secure);
        assert_eq!(cookies[1].expires, Some(1_800_000_000.0));
        assert!(cookies[1].secure && cookies[1].http_only);
        assert_eq!(cookies[1].same_site.as_deref(), Some("Lax"));
    }

    #[test]
    fn a_result_with_no_cookies_is_an_empty_list_not_a_failure() {
        assert!(cookies_from(&json!({})).is_empty());
        assert!(cookies_from(&json!({ "cookies": [] })).is_empty());
    }

    #[test]
    fn same_site_is_normalized_to_the_three_values_the_protocol_accepts() {
        assert_eq!(normalize_same_site("STRICT").expect("strict"), "Strict");
        assert_eq!(normalize_same_site("lax").expect("lax"), "Lax");
        assert_eq!(normalize_same_site("None").expect("none"), "None");
        let error = normalize_same_site("maybe").expect_err("rejected");
        assert_eq!(error.code(), "browser_invalid_request");
        assert!(error.to_string().contains("strict, lax, or none"));
    }

    #[test]
    fn a_storage_listing_carries_the_origin_it_was_read_at() {
        let snapshot = storage_snapshot_from(
            StorageArea::Local,
            &json!({
                "origin": "https://example.com",
                "entries": [["token", "abc"], ["theme", "dark"]],
            }),
        );
        assert_eq!(snapshot.origin, "https://example.com");
        assert_eq!(snapshot.entries.len(), 2);
        assert_eq!(snapshot.entries[0].key, "token");
        assert!(!snapshot.truncated);
        assert!(snapshot.entries.iter().all(|entry| !entry.truncated));
    }

    #[test]
    fn an_oversized_storage_value_is_cut_and_flagged_on_its_own_entry() {
        let value = "v".repeat(MAX_STORAGE_VALUE_CHARS + 50);
        let snapshot = storage_snapshot_from(
            StorageArea::Session,
            &json!({ "origin": "https://example.com", "entries": [["big", value]] }),
        );
        assert!(snapshot.entries[0].truncated);
        assert_eq!(
            snapshot.entries[0].value.chars().count(),
            MAX_STORAGE_VALUE_CHARS
        );
        assert!(!snapshot.truncated, "one long value is not a short listing");
    }

    #[test]
    fn a_storage_area_larger_than_the_ceiling_stops_and_says_so() {
        let value = "v".repeat(MAX_STORAGE_VALUE_CHARS);
        let entries: Vec<Value> = (0..100)
            .map(|index| json!([format!("key-{index}"), value]))
            .collect();
        let snapshot = storage_snapshot_from(
            StorageArea::Local,
            &json!({ "origin": "https://example.com", "entries": entries }),
        );
        assert!(snapshot.truncated);
        assert!(snapshot.entries.len() < 100);
        let total: usize = snapshot
            .entries
            .iter()
            .map(|entry| entry.key.len() + entry.value.len())
            .sum();
        assert!(total <= MAX_STORAGE_BYTES);
    }

    #[test]
    fn a_download_name_that_is_already_taken_gets_a_numeric_suffix() {
        let dir = std::env::temp_dir().join(format!("wardian-download-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("create");
        assert_eq!(
            unique_download_path(&dir, "report.csv"),
            dir.join("report.csv")
        );

        std::fs::write(dir.join("report.csv"), b"first").expect("write");
        assert_eq!(
            unique_download_path(&dir, "report.csv"),
            dir.join("report (2).csv")
        );

        std::fs::write(dir.join("report (2).csv"), b"second").expect("write");
        assert_eq!(
            unique_download_path(&dir, "report.csv"),
            dir.join("report (3).csv")
        );

        // An extensionless name keeps the suffix at the end.
        std::fs::write(dir.join("archive"), b"third").expect("write");
        assert_eq!(
            unique_download_path(&dir, "archive"),
            dir.join("archive (2)")
        );
        std::fs::remove_dir_all(&dir).expect("clean up");
    }

    #[test]
    fn a_completed_download_is_renamed_from_its_guid_to_its_suggested_name() {
        let dir = std::env::temp_dir().join(format!("wardian-download-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("create");
        std::fs::write(dir.join("guid-1"), b"payload").expect("write");

        let resolved = resolve_completed_download(&dir, "guid-1", "report.csv").expect("resolved");
        assert_eq!(resolved, dir.join("report.csv").to_string_lossy());
        assert!(dir.join("report.csv").exists());
        assert!(!dir.join("guid-1").exists());
        std::fs::remove_dir_all(&dir).expect("clean up");
    }

    #[test]
    fn a_suggested_name_cannot_write_outside_the_download_directory() {
        let root = std::env::temp_dir().join(format!("wardian-download-{}", Uuid::new_v4()));
        let dir = root.join("session");
        std::fs::create_dir_all(&dir).expect("create");
        std::fs::write(dir.join("guid-1"), b"payload").expect("write");

        let resolved =
            resolve_completed_download(&dir, "guid-1", "../escaped.txt").expect("resolved");
        // Only the file name survives, so the escape lands inside the directory.
        assert_eq!(resolved, dir.join("escaped.txt").to_string_lossy());
        assert!(!root.join("escaped.txt").exists());
        std::fs::remove_dir_all(&root).expect("clean up");
    }

    #[test]
    fn a_suggested_name_that_is_only_a_traversal_falls_back_to_the_guid_path() {
        let dir = std::env::temp_dir().join(format!("wardian-download-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("create");
        std::fs::write(dir.join("guid-1"), b"payload").expect("write");

        let resolved = resolve_completed_download(&dir, "guid-1", "..").expect("resolved");
        assert_eq!(resolved, dir.join("guid-1").to_string_lossy());
        assert!(dir.join("guid-1").exists());
        std::fs::remove_dir_all(&dir).expect("clean up");
    }

    #[test]
    fn a_download_whose_file_is_not_there_yet_resolves_to_no_path() {
        let dir = std::env::temp_dir().join(format!("wardian-download-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("create");
        assert_eq!(
            resolve_completed_download(&dir, "guid-1", "report.csv"),
            None
        );
        std::fs::remove_dir_all(&dir).expect("clean up");
    }

    #[test]
    fn pruning_removes_expired_session_directories_and_only_directories() {
        let root = std::env::temp_dir().join(format!("wardian-downloads-{}", Uuid::new_v4()));
        let session = root.join("session-1");
        std::fs::create_dir_all(&session).expect("create");
        std::fs::write(session.join("report.csv"), b"payload").expect("write");
        std::fs::write(root.join("loose-file"), b"not a session").expect("write");

        // Everything present is older than a cutoff in the future.
        prune_downloads_before(
            &root,
            std::time::SystemTime::now() + Duration::from_secs(60),
        );

        assert!(!session.exists(), "an expired session loses its downloads");
        assert!(
            root.join("loose-file").exists(),
            "only directories are pruned"
        );
        std::fs::remove_dir_all(&root).expect("clean up");
    }

    #[test]
    fn pruning_spares_a_session_younger_than_the_cutoff() {
        let root = std::env::temp_dir().join(format!("wardian-downloads-{}", Uuid::new_v4()));
        let session = root.join("session-1");
        std::fs::create_dir_all(&session).expect("create");

        prune_downloads_before(&root, std::time::SystemTime::UNIX_EPOCH);

        assert!(session.exists());
        std::fs::remove_dir_all(&root).expect("clean up");
    }

    #[test]
    fn pruning_a_directory_that_does_not_exist_is_not_an_error() {
        let root = std::env::temp_dir().join(format!("wardian-missing-{}", Uuid::new_v4()));
        prune_old_downloads(&root);
        assert!(!root.exists());
    }

    #[test]
    fn the_broker_keeps_profiles_and_downloads_as_siblings() {
        let root = std::env::temp_dir().join(format!("wardian-browser-{}", Uuid::new_v4()));
        let broker = BrowserSessionBroker::new(root.clone());
        assert_eq!(broker.profile_root, root.join("profiles"));
        assert_eq!(broker.download_root, root.join("downloads"));
        assert!(
            !broker.download_root.starts_with(&broker.profile_root),
            "a profile is deleted on close; downloads must not be inside one"
        );
    }
}
