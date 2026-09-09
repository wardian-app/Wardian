use clap::{Args, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(name = "wardian", version, about = "Wardian command-line interface")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Serve the explicit local agent messaging tool over MCP.
    Mcp {
        #[command(subcommand)]
        command: crate::mcp::McpCommand,
    },
    /// Discover command arguments as compact JSON; optionally name a command path.
    Schema {
        /// Command path, e.g. agent spawn or browser <target> snapshot.
        #[arg(value_name = "COMMAND")]
        path: Vec<String>,
    },
    /// Inspect identity and status, configure agents, or control their sessions.
    Agent(AgentArgs),
    /// Present files and inspect durable artifact reviews.
    Artifact(ArtifactArgs),
    Browser(BrowserArgs),
    /// List or read archived conversations.
    Conversation(ConversationArgs),
    /// Read Inbox events and pending decisions.
    Inbox(InboxArgs),
    /// Save, recall, and revise evidence-backed agent memory.
    Memory(MemoryArgs),
    /// Read, author, and deploy reusable Library assets.
    Library(LibraryArgs),
    /// Discover nodes, validate blueprints, and manage runs and schedules.
    Automation(AutomationArgs),
    /// List and manage durable teams and their members.
    Team(TeamArgs),
    /// List and manage monitoring watchlists.
    Watchlist(WatchlistArgs),
    Telemetry(TelemetryArgs),
    /// Inspect and edit agent communication boundaries.
    Graph(GraphArgs),
    /// Deliver a message or provider command to agents.
    Send(SendArgs),
    /// Inspect, wait for, or cancel a queued delivery.
    Delivery(DeliveryArgs),
    /// Send a user-facing update or approval request to Inbox.
    Notify(NotifyArgs),
    /// Request an accountable done, blocked, or failed reply from one peer.
    Ask(AskArgs),
    /// Complete an ask request with a structured result.
    Reply(ReplyArgs),
}

// ---------------------------------------------------------------------------
// wardian browser
// ---------------------------------------------------------------------------

/// Drive a browser surface. Sessions are addressed as `browser:N` or by id.
#[derive(Debug, Args)]
pub struct BrowserArgs {
    #[command(subcommand)]
    pub command: BrowserCommand,

    /// Emit machine-readable JSON instead of the human listing.
    #[arg(long, global = true)]
    pub json: bool,
}

#[derive(Debug, Subcommand)]
pub enum BrowserCommand {
    /// Open a browser session, and a surface for it unless detached.
    ///
    /// With no URL, the workspace is checked for a dev server that is already
    /// listening and the page opens there. `--blank` skips that.
    Open {
        /// Address to load. A bare host is treated as http.
        url: Option<String>,
        /// Owning agent. Defaults to this terminal's WARDIAN_SESSION_ID.
        #[arg(long)]
        agent: Option<String>,
        /// Workspace to attribute to and guess an address from. Defaults to the
        /// working directory.
        #[arg(long)]
        workspace: Option<String>,
        #[arg(long, requires = "height")]
        width: Option<u32>,
        #[arg(long, requires = "width")]
        height: Option<u32>,
        /// Start the runtime without opening a workbench surface.
        #[arg(long)]
        detached: bool,
        /// Open `about:blank` instead of guessing an address.
        #[arg(long, conflicts_with = "url")]
        blank: bool,
    },
    /// List open browser sessions.
    List,
    /// Everything that acts on one existing session.
    #[command(external_subcommand)]
    Target(Vec<String>),
}

/// Re-parses the tail of `wardian browser <target> ...` into a target verb.
///
/// The target comes first so `browser browser:7 click e2` reads the way an
/// operator thinks about it, which clap cannot express as a normal subcommand.
#[derive(Debug, Parser)]
#[command(name = "wardian browser <target>", no_binary_name = true, about = "Act on one browser session. Use snapshot to obtain element refs.", long_about = None)]
pub struct BrowserTargetArgs {
    #[command(subcommand)]
    pub command: BrowserTargetCommand,

    #[arg(long, global = true)]
    pub json: bool,
}

/// Verbs that operate on an already-open session.
#[derive(Debug, Subcommand)]
pub enum BrowserTargetCommand {
    /// Close the session and stop its browser.
    Close,
    /// Navigate to a URL, or `back`, `forward`, `reload`, `stop`.
    Navigate { action: String },
    /// Read `url`, `title`, `text`, or `html`, optionally under a selector.
    Get {
        field: String,
        selector: Option<String>,
    },
    /// Block until a page condition holds.
    Wait {
        #[arg(long = "load-state")]
        load_state: Option<String>,
        #[arg(long)]
        selector: Option<String>,
        #[arg(long)]
        text: Option<String>,
        #[arg(long = "url-contains")]
        url_contains: Option<String>,
        #[arg(long)]
        function: Option<String>,
        #[arg(long = "timeout-ms")]
        timeout_ms: Option<u64>,
    },
    /// Capture element refs. Refs go stale on navigation; re-snapshot then.
    Snapshot {
        /// Only interactive elements, which is what actions can target.
        #[arg(long)]
        interactive: bool,
    },
    /// Click an element from the latest snapshot.
    Click {
        element_ref: String,
        #[arg(long = "snapshot-after")]
        snapshot_after: bool,
    },
    /// Replace an input's text using its snapshot ref.
    Fill {
        element_ref: String,
        value: String,
        #[arg(long = "snapshot-after")]
        snapshot_after: bool,
    },
    /// Send a keyboard key or chord to an element.
    Press {
        element_ref: String,
        key: String,
        #[arg(long = "snapshot-after")]
        snapshot_after: bool,
    },
    /// Choose an option by value in a select element.
    Select {
        element_ref: String,
        value: String,
        #[arg(long = "snapshot-after")]
        snapshot_after: bool,
    },
    /// Move the pointer over an element.
    Hover {
        element_ref: String,
        #[arg(long = "snapshot-after")]
        snapshot_after: bool,
    },
    /// Scroll an element into view.
    Scroll {
        element_ref: String,
        #[arg(long = "snapshot-after")]
        snapshot_after: bool,
    },
    /// Write a PNG of the page to a path.
    Screenshot {
        path: String,
        #[arg(long = "full-page")]
        full_page: bool,
    },
    /// Resize the rendered viewport, or `reset` it.
    Viewport {
        width: Option<String>,
        height: Option<u32>,
    },
    /// Evaluate an expression in the page and print its JSON value.
    Eval { expression: String },
    /// Print the console messages captured since the last navigation.
    Console {
        /// Keep only this severity: error, warning, or info.
        #[arg(long)]
        level: Option<String>,
        /// Empty the buffer after printing it.
        #[arg(long)]
        clear: bool,
    },
    /// Inspect the requests the page has made. Not cleared by navigation.
    Network {
        /// A recorded request id, for the full headers of one request.
        request_id: Option<String>,
        /// Read the response body back as well. Only with a request id.
        #[arg(long, requires = "request_id")]
        body: bool,
        /// Case-insensitive substring of the URL.
        #[arg(long)]
        filter: Option<String>,
        /// Only this HTTP method.
        #[arg(long)]
        method: Option<String>,
        /// An exact code or a class, e.g. `404` or `2xx`.
        #[arg(long)]
        status: Option<String>,
        /// Comma-separated resource types, e.g. `xhr,fetch`.
        #[arg(long = "type")]
        resource_type: Option<String>,
        /// Only requests that failed or answered 4xx/5xx.
        #[arg(long)]
        failed: bool,
        /// Keep only the most recent N records.
        #[arg(long)]
        limit: Option<usize>,
        /// Empty the ledger. Nothing about the page changes.
        #[arg(long, conflicts_with_all = ["request_id", "filter", "method", "status", "resource_type", "failed", "limit"])]
        clear: bool,
    },
    /// Read or change the cookies held by this session's isolated profile.
    Cookies {
        #[command(subcommand)]
        command: Option<BrowserCookieCommand>,
        /// List every cookie in the browser context, not only the page's.
        #[arg(long)]
        all: bool,
    },
    /// Read or change web storage at the page's own origin.
    Storage {
        /// `local` or `session`.
        area: String,
        #[command(subcommand)]
        command: Option<BrowserStorageCommand>,
    },
    /// List the files this session has downloaded.
    Downloads {
        /// Forget the records. The files themselves stay on disk.
        #[arg(long)]
        clear: bool,
    },
}

/// Cookie verbs. Omitting one lists the page's cookies.
#[derive(Debug, Subcommand)]
pub enum BrowserCookieCommand {
    /// Set a cookie in the session's isolated profile.
    Set {
        name: String,
        value: String,
        /// Scope the cookie to this URL. Defaults to the current page.
        #[arg(long)]
        url: Option<String>,
        #[arg(long)]
        domain: Option<String>,
        #[arg(long)]
        path: Option<String>,
        #[arg(long)]
        secure: bool,
        #[arg(long = "http-only")]
        http_only: bool,
        /// `strict`, `lax`, or `none`.
        #[arg(long = "same-site")]
        same_site: Option<String>,
        /// Whole seconds since the epoch. Omit for a session cookie.
        #[arg(long)]
        expires: Option<i64>,
    },
    /// Delete a cookie by name and optional scope.
    Delete {
        name: String,
        #[arg(long)]
        url: Option<String>,
        #[arg(long)]
        domain: Option<String>,
        #[arg(long)]
        path: Option<String>,
    },
    /// Remove every cookie in this session's profile.
    Clear,
}

/// Storage verbs. A bare key reads it; no key at all lists the area.
#[derive(Debug, Subcommand)]
pub enum BrowserStorageCommand {
    /// Set one key at the current page origin.
    Set { key: String, value: String },
    /// Remove one key at the current page origin.
    Remove { key: String },
    /// Clear the selected storage area at this origin.
    Clear,
    /// Read one key. Also reached by writing the key with no verb.
    Get { key: String },
    /// A bare key, which clap sees as an unrecognized subcommand.
    #[command(external_subcommand)]
    Key(Vec<String>),
}

// ---------------------------------------------------------------------------
// wardian artifact
// ---------------------------------------------------------------------------

#[derive(Debug, Args)]
pub struct ArtifactArgs {
    #[command(subcommand)]
    pub command: ArtifactCommand,
}

#[derive(Debug, Subcommand)]
pub enum ArtifactCommand {
    /// Present an authorized local file as a durable artifact version.
    Present {
        path: String,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        description: Option<String>,
        #[arg(long, conflicts_with = "force_new")]
        artifact: Option<String>,
        #[arg(long = "new", conflicts_with = "artifact")]
        force_new: bool,
        #[arg(long = "address")]
        addressed_comment_ids: Vec<String>,
    },
    /// Show an artifact thread and one selected immutable version.
    Show {
        artifact_id: String,
        #[arg(long)]
        version: Option<String>,
    },
    /// Inspect reviews associated with an artifact thread.
    Review {
        #[command(subcommand)]
        command: ArtifactReviewCommand,
    },
}

#[derive(Debug, Subcommand)]
pub enum ArtifactReviewCommand {
    /// Show one review or the latest review for an artifact.
    Show {
        artifact_id: String,
        #[arg(long, conflicts_with = "latest")]
        review: Option<String>,
        #[arg(long, conflicts_with = "review")]
        latest: bool,
    },
}

// ---------------------------------------------------------------------------
// wardian library
// ---------------------------------------------------------------------------

#[derive(Debug, Args)]
pub struct LibraryArgs {
    #[command(subcommand)]
    pub command: LibraryCommand,
}

#[derive(Debug, Subcommand)]
pub enum LibraryCommand {
    /// List Library entries as a tree or agent-friendly flat rows.
    List {
        /// Optional section: skills, prompts, classes, automations, or mcps.
        section: Option<String>,
        /// Emit entries only, without tree, deployment, or orphan payloads.
        #[arg(long)]
        flat: bool,
    },
    /// Show one entry's metadata, resolved path, and optional content.
    Show {
        /// Section-qualified ref such as skills/review/planner.
        entry_ref: String,
        #[arg(long)]
        content: bool,
    },
    /// Print one entry's raw content without a JSON envelope.
    Read { entry_ref: String },
    /// Create an entry. Automation files are authored here; use wardian automation for operations.
    Create {
        entry_ref: String,
        #[arg(long, conflicts_with = "file")]
        stdin: bool,
        #[arg(long, conflicts_with = "stdin")]
        file: Option<String>,
    },
    /// Replace the content of an existing entry.
    Write {
        entry_ref: String,
        #[arg(long, conflicts_with = "file")]
        stdin: bool,
        #[arg(long, conflicts_with = "stdin")]
        file: Option<String>,
    },
    /// Rename or move an entry within its current section.
    Move { from_ref: String, to_ref: String },
    /// Delete an entry and its associated Library metadata.
    Delete { entry_ref: String },
    /// Mark an entry as starred.
    Star { entry_ref: String },
    /// Remove an entry's starred state.
    Unstar { entry_ref: String },
    /// Replace all tags on an entry.
    Tags {
        entry_ref: String,
        /// Complete tag set; repeat --set for multiple tags.
        #[arg(long = "set", required = true)]
        set: Vec<String>,
    },
    /// Show every current deployment target for one skill.
    Deployments { skill_ref: String },
    /// Reconcile a skill to the complete desired target set.
    Deploy {
        skill_ref: String,
        /// Non-empty comma-separated user, class, and agent target refs.
        #[arg(long, required_unless_present = "clear", conflicts_with = "clear")]
        targets: Option<String>,
        /// Reconcile to an empty desired set, removing every deployment.
        #[arg(long, required_unless_present = "targets", conflicts_with = "targets")]
        clear: bool,
    },
    /// List deployed skill directories whose Library source is missing.
    Orphans,
    /// Manage unresolved deployed skill directories.
    Orphan {
        #[command(subcommand)]
        command: LibraryOrphanCommand,
    },
    /// Restore the bundled instructions for a default class.
    RestoreDefault { entry_ref: String },
}

#[derive(Debug, Subcommand)]
pub enum LibraryOrphanCommand {
    /// Delete one deployment only if it is currently reported as orphaned.
    Delete {
        /// Target ref such as user:global or class:Reviewer.
        #[arg(long)]
        target: String,
        /// Deployed skill directory name.
        #[arg(long)]
        skill: String,
    },
}

// ---------------------------------------------------------------------------
// wardian conversation
// ---------------------------------------------------------------------------

#[derive(Debug, Args)]
pub struct ConversationArgs {
    #[command(subcommand)]
    pub command: ConversationCommand,
}

#[derive(Debug, Subcommand)]
pub enum ConversationCommand {
    /// List archived conversations for an agent, or all agents with --scope all.
    List {
        #[arg(long)]
        agent: Option<String>,
        #[arg(long, default_value = "current")]
        scope: String,
    },
    /// Read one archived conversation and its manifest.
    Show { conversation_id: String },
}

// ---------------------------------------------------------------------------
// wardian inbox
// ---------------------------------------------------------------------------

#[derive(Debug, Args)]
pub struct InboxArgs {
    #[command(subcommand)]
    pub command: InboxCommand,
}

#[derive(Debug, Subcommand)]
pub enum InboxCommand {
    /// Read the assembled Inbox projection, newest first.
    List {
        /// Match one or more Inbox item types, separated by commas.
        #[arg(long = "type", value_delimiter = ',')]
        types: Vec<String>,
        /// Match one or more evidence sources, separated by commas.
        #[arg(long = "source", value_delimiter = ',')]
        sources: Vec<String>,
        /// Return only items that have not been acknowledged.
        #[arg(long)]
        unread: bool,
        /// Number of items to return after filtering.
        #[arg(long, default_value_t = 200)]
        limit: usize,
        /// Number of matching items to skip.
        #[arg(long, default_value_t = 0)]
        offset: usize,
    },
}

// ---------------------------------------------------------------------------
// wardian memory
// ---------------------------------------------------------------------------

#[derive(Debug, Args)]
pub struct MemoryArgs {
    #[command(subcommand)]
    pub command: MemoryCommand,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum MemoryKindArg {
    Stable,
    Current,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum MemoryScopeArg {
    Workspace,
    Agent,
}

#[derive(Debug, Subcommand)]
pub enum MemoryCommand {
    /// Save an evidence-backed memory for an agent.
    Save {
        text: String,
        #[arg(long)]
        evidence: String,
        #[arg(long, value_enum, default_value = "stable")]
        kind: MemoryKindArg,
        #[arg(long, value_enum, default_value = "workspace")]
        scope: MemoryScopeArg,
        #[arg(long)]
        agent: Option<String>,
        #[arg(long)]
        workspace: Option<String>,
        #[arg(long)]
        source: Vec<String>,
        #[arg(long)]
        idempotency_key: Option<String>,
    },
    /// List active memories available in a workspace.
    List {
        #[arg(long)]
        agent: Option<String>,
        #[arg(long)]
        workspace: Option<String>,
    },
    /// Show the latest revision of a memory by full ID or unique prefix.
    Show { memory_id: String },
    /// Replace an active memory with a new revision by full ID or unique prefix.
    Update {
        memory_id: String,
        text: String,
        #[arg(long)]
        evidence: String,
        #[arg(long)]
        source: Vec<String>,
        #[arg(long)]
        idempotency_key: Option<String>,
    },
    /// Remove the active revision by full ID or unique prefix while retaining audit history.
    Remove { memory_id: String },
    /// Compile the active stable/current recall set.
    Recall {
        #[arg(long)]
        agent: Option<String>,
        #[arg(long)]
        workspace: Option<String>,
    },
    /// Show every revision of a logical memory by full ID or unique prefix.
    History { memory_id: String },
}

// ---------------------------------------------------------------------------
// wardian team / watchlist
// ---------------------------------------------------------------------------

#[derive(Debug, Args)]
pub struct TeamArgs {
    #[command(subcommand)]
    pub command: TeamCommand,
}

#[derive(Debug, Subcommand)]
pub enum TeamCommand {
    /// List persisted teams and their members.
    List,
    /// Show one team by name or ID.
    Show { target: String },
    /// Create a team with explicit member agents.
    Create {
        name: String,
        #[arg(long = "agent", required = true)]
        agents: Vec<String>,
    },
    /// Rename a team without changing its membership.
    Rename { target: String, new_name: String },
    /// Add agents to a team.
    Add { target: String, agents: Vec<String> },
    /// Remove agents from a team without deleting them.
    Remove { target: String, agents: Vec<String> },
    /// Move selected members into a new team.
    Split {
        target: String,
        #[arg(long)]
        name: String,
        #[arg(long = "agent", required = true)]
        agents: Vec<String>,
    },
    /// Delete the team record, preserving its agents.
    Delete { target: String },
}

#[derive(Debug, Args)]
pub struct WatchlistArgs {
    #[command(subcommand)]
    pub command: WatchlistCommand,
}

#[derive(Debug, Subcommand)]
pub enum WatchlistCommand {
    /// List persisted monitoring watchlists.
    List,
    /// Show one watchlist by name or ID.
    Show { target: String },
    /// Create an empty watchlist.
    Create { name: String },
    /// Rename a watchlist.
    Rename { target: String, new_name: String },
    /// Add a team to a watchlist.
    AddTeam { target: String, team: String },
    /// Remove a team from a watchlist, preserving the team.
    RemoveTeam { target: String, team: String },
    /// Add an agent to a watchlist.
    AddAgent { target: String, agent: String },
    /// Remove an agent from a watchlist, preserving the agent.
    RemoveAgent { target: String, agent: String },
    /// Delete the watchlist, preserving its teams and agents.
    Delete { target: String },
}

// ---------------------------------------------------------------------------
// wardian telemetry
// ---------------------------------------------------------------------------

/// Read the habitat telemetry store.
#[derive(Debug, Args)]
pub struct TelemetryArgs {
    #[command(subcommand)]
    pub command: TelemetryCommand,
}

#[derive(Debug, Subcommand)]
pub enum TelemetryCommand {
    /// Aggregate measures over a horizon, plus a ranked breakdown.
    Summary {
        /// today, day (24h), week (7d), month (30d), or all.
        #[arg(long, default_value = "week")]
        horizon: String,
        /// provider, agent, or model.
        #[arg(long, default_value = "provider")]
        dimension: String,
    },
    /// Repair facts attributed to an agent other than their source owner.
    Repair {
        /// Explicit state database to inspect or repair.
        #[arg(long, value_name = "PATH")]
        db: PathBuf,
        /// Backup destination required with --apply.
        #[arg(long, value_name = "PATH", requires = "apply")]
        backup: Option<PathBuf>,
        /// Create a verified backup and apply the idempotent repair.
        #[arg(long)]
        apply: bool,
    },
}

// ---------------------------------------------------------------------------
// wardian automation
// ---------------------------------------------------------------------------

#[derive(Debug, Args)]
pub struct AutomationArgs {
    #[command(subcommand)]
    pub command: AutomationCommand,

    #[arg(long, global = true)]
    pub pretty: bool,
}

#[derive(Debug, Subcommand)]
pub enum AutomationCommand {
    /// Print the node type registry (the contract agents author against).
    NodeTypes {
        /// Read only this node type's contract, e.g. task or branch.
        node: Option<String>,
        /// Emit the machine-readable JSON schema instead of a summary table.
        #[arg(long)]
        json: bool,
    },
    /// List automation blueprints in the Library.
    List,
    /// Validate a blueprint `.md` file and report diagnostics.
    Validate { path: String },
    /// Launch an automation blueprint and write a durable run.
    Exec {
        path: String,
        /// Execution backend: live/real/full routes through the running app; mock is reserved for engine tests.
        #[arg(long, default_value = "live")]
        executor: String,
        /// Run input as a JSON object, @file, or - for piped UTF-8 JSON.
        #[arg(long)]
        input: Option<String>,
        /// Default provider for unbound automation roles.
        #[arg(long)]
        provider: Option<String>,
        /// Workspace for live automation tasks.
        #[arg(long)]
        workspace: Option<String>,
        /// Role/class -> provider or agent-id binding, repeatable: --bind role=value
        #[arg(long)]
        bind: Vec<String>,
    },
    /// List automation runs under <home>/logs/automations.
    Runs,
    /// Show one automation run's state + event trace.
    RunShow {
        blueprint_id: String,
        run_id: String,
    },
    /// Replay an automation run's event log into its final state (no execution).
    Replay {
        blueprint_id: String,
        run_id: String,
    },
    /// Parse a blueprint `.md` and print the structured graph.
    Parse { path: String },
    /// Normalize a blueprint `.md` (print, or --write back in place).
    Normalize {
        path: String,
        #[arg(long)]
        write: bool,
    },
    /// Write the node-type JSON schema artifact for the builder.
    GenSchema {
        #[arg(
            long,
            default_value = "src/features/automations/nodeRegistry.schema.json"
        )]
        out: String,
        /// Exit non-zero if the file on disk differs (CI drift guard).
        #[arg(long)]
        check: bool,
    },
    /// Write the generated node-type reference doc.
    GenDocs {
        #[arg(long, default_value = "docs/automations/node-reference.md")]
        out: String,
        #[arg(long)]
        check: bool,
    },
    /// Manage persisted automation schedules; execution requires the running app.
    #[command(subcommand)]
    Schedule(Box<AutomationScheduleCommand>),
    /// Manage generic conversation-boundary automation invokers.
    #[command(subcommand)]
    SessionClose(Box<AutomationSessionCloseCommand>),
    /// Manage event listeners: file changes, inbound webhooks, and web polls.
    #[command(subcommand)]
    Listener(Box<AutomationListenerCommand>),
}

#[derive(Debug, Subcommand)]
pub enum AutomationListenerCommand {
    /// List persisted listeners with their arming and last-fire state.
    List,
    /// Show one listener, including its webhook URL when it has one.
    Show { id: String },
    /// Watch a filesystem path and fire when matching files change.
    Watch {
        #[arg(long)]
        blueprint: String,
        #[arg(long)]
        name: String,
        /// Absolute directory or file to watch.
        #[arg(long)]
        path: String,
        /// Watch subdirectories too.
        #[arg(long)]
        recursive: bool,
        /// Glob a change must match, repeatable: --pattern '**/*.rs'.
        #[arg(long)]
        pattern: Vec<String>,
        /// Glob to exclude, repeatable; merged over the built-in ignores.
        #[arg(long)]
        ignore: Vec<String>,
        /// Change kind to react to, repeatable: created, modified, removed.
        #[arg(long)]
        event: Vec<String>,
        /// Milliseconds of quiet that close an event burst.
        #[arg(long)]
        debounce_ms: Option<u32>,
        #[command(flatten)]
        common: ListenerCommonArgs,
    },
    /// Receive inbound HTTP deliveries at /hooks/<path>.
    Hook {
        #[arg(long)]
        blueprint: String,
        #[arg(long)]
        name: String,
        /// URL path segment; the listener answers at /hooks/<path>.
        #[arg(long)]
        path: String,
        /// Credential style: token or hmac.
        #[arg(long, default_value = "hmac")]
        auth: String,
        /// Header carrying the HMAC signature; defaults to X-Hub-Signature-256.
        #[arg(long)]
        signature_header: Option<String>,
        /// Shared secret. Omit to have one generated and printed once.
        #[arg(long)]
        secret: Option<String>,
        /// Largest accepted request body in bytes.
        #[arg(long)]
        max_body_bytes: Option<u32>,
        #[command(flatten)]
        common: ListenerCommonArgs,
    },
    /// Poll a URL and fire when the watched resource changes.
    Poll {
        #[arg(long)]
        blueprint: String,
        #[arg(long)]
        name: String,
        #[arg(long)]
        url: String,
        /// Seconds between polls; minimum 30.
        #[arg(long)]
        interval: Option<u32>,
        /// Request method: get or head.
        #[arg(long, default_value = "get")]
        method: String,
        /// What counts as a change: etag, body, json, or regex.
        #[arg(long, default_value = "etag")]
        fingerprint: String,
        /// RFC 6901 pointer for --fingerprint json, e.g. /0/tag_name.
        #[arg(long)]
        json_pointer: Option<String>,
        /// Pattern for --fingerprint regex; the first capture group is used.
        #[arg(long)]
        regex: Option<String>,
        /// Non-secret request header, repeatable: --header 'Accept: application/json'.
        #[arg(long)]
        header: Vec<String>,
        /// Largest response body read, in bytes.
        #[arg(long)]
        max_body_bytes: Option<u32>,
        #[command(flatten)]
        common: ListenerCommonArgs,
    },
    /// Enable a listener, clearing any auto-disable reason.
    Enable { id: String },
    /// Disable a listener without deleting its configuration.
    Disable { id: String },
    /// Remove a listener and its stored credentials.
    Remove { id: String },
}

/// Invocation context every listener shares with the schedule invoker.
#[derive(Debug, Args, Clone)]
pub struct ListenerCommonArgs {
    #[arg(long)]
    pub provider: Option<String>,
    /// Existing directory used as the run workspace.
    #[arg(long)]
    pub workspace: Option<String>,
    /// Run input as a JSON object, @file, or - for piped UTF-8 JSON.
    #[arg(long)]
    pub input: Option<String>,
    /// Role/class -> provider binding, repeatable: --bind role=provider.
    #[arg(long)]
    pub bind: Vec<String>,
    /// Role assignments keyed by role name: JSON object, @file, or - for stdin.
    #[arg(long, alias = "assignment")]
    pub assignments: Option<String>,
    /// Behavior when the listener fires while its own run is active.
    #[arg(long, value_parser = ["skip", "coalesce", "parallel"])]
    pub overlap: Option<String>,
    /// Create the listener enabled. Listeners are otherwise created disabled.
    #[arg(long)]
    pub enable: bool,
}

#[derive(Debug, Subcommand)]
pub enum AutomationSessionCloseCommand {
    /// List persisted conversation-boundary invokers.
    List,
    /// Add an invoker; it remains disabled unless --enable is supplied.
    Add {
        #[arg(long)]
        blueprint: String,
        #[arg(long)]
        name: String,
        #[arg(long)]
        agent: Option<String>,
        #[arg(long)]
        boundary: Vec<String>,
        #[arg(long)]
        provider: Option<String>,
        #[arg(long)]
        workspace: Option<String>,
        /// Run input as a JSON object, @file, or - for piped UTF-8 JSON.
        #[arg(long)]
        input: Option<String>,
        /// Typed assignments object, @file, or - for stdin; providers may specify model and effort.
        #[arg(long)]
        assignments: Option<String>,
        /// Enable immediately. Invokers are otherwise created disabled.
        #[arg(long)]
        enable: bool,
        /// Do not run when the closing session has no durable conversation archive.
        #[arg(long)]
        require_archive: bool,
    },
    /// Enable an invoker for future matching conversation boundaries.
    Enable { id: String },
    /// Disable an invoker without deleting its configuration.
    Disable { id: String },
    /// Remove an invoker's configuration.
    Remove { id: String },
}

#[derive(Debug, Subcommand)]
pub enum AutomationScheduleCommand {
    /// Add a schedule for a blueprint id (resolves to library/automations/<id>.md).
    Add {
        #[arg(long)]
        blueprint: String,
        #[arg(long)]
        name: String,
        #[command(flatten)]
        cadence: ScheduleDefinitionArgs,
        #[arg(long)]
        provider: Option<String>,
        /// Existing directory used as the scheduled run workspace.
        #[arg(long)]
        workspace: String,
        /// Run input as a JSON object, @file, or - for piped UTF-8 JSON.
        #[arg(long)]
        input: Option<String>,
        /// Role/class -> provider binding, repeatable: --bind role=provider.
        #[arg(long)]
        bind: Vec<String>,
        /// Role assignments keyed by role name: JSON object, @file, or - for stdin.
        #[arg(long, alias = "assignment")]
        assignments: Option<String>,
        /// Create the schedule paused instead of active.
        #[arg(long)]
        paused: bool,
    },
    /// Update selected schedule configuration without replacing its identity or history.
    Update {
        id: String,
        #[arg(long)]
        blueprint: Option<String>,
        #[arg(long)]
        name: Option<String>,
        #[command(flatten)]
        cadence: ScheduleDefinitionArgs,
        #[arg(long)]
        provider: Option<String>,
        /// Existing directory used as the scheduled run workspace.
        #[arg(long)]
        workspace: Option<String>,
        /// Run input as a JSON object, @file, or - for piped UTF-8 JSON.
        #[arg(long)]
        input: Option<String>,
        /// Role/class -> provider binding, repeatable: --bind role=provider.
        #[arg(long)]
        bind: Vec<String>,
        /// Role assignments keyed by role name: JSON object, @file, or - for stdin.
        #[arg(long, alias = "assignment")]
        assignments: Option<String>,
        /// Pause the schedule after applying configuration changes.
        #[arg(long, conflicts_with = "active")]
        paused: bool,
        /// Resume the schedule after applying configuration changes.
        #[arg(long, conflicts_with = "paused")]
        active: bool,
    },
    /// List persisted schedules.
    List,
    /// Pause future scheduled runs.
    Pause { id: String },
    /// Resume a paused schedule.
    Resume { id: String },
    /// Remove a schedule, preserving existing run history.
    Remove { id: String },
    /// Unpause and mark due now; the running app must launch the run.
    RunNow { id: String },
}

#[derive(Debug, Args, Clone)]
pub struct ScheduleDefinitionArgs {
    /// Interval cadence in minutes.
    #[arg(
        long,
        conflicts_with_all = [
            "daily",
            "weekly",
            "monthly",
            "specific_dates",
            "at",
            "repeat_every"
        ]
    )]
    pub every: Option<u32>,
    /// Daily at HH:MM local time.
    #[arg(
        long,
        conflicts_with_all = [
            "every",
            "weekly",
            "monthly",
            "specific_dates",
            "at",
            "repeat_every"
        ]
    )]
    pub daily: Option<String>,
    /// Weekly comma-separated days and time, e.g. Mon,Wed,Fri@09:30.
    #[arg(
        long,
        conflicts_with_all = ["every", "daily", "monthly", "specific_dates", "at"]
    )]
    pub weekly: Option<String>,
    /// Monthly comma-separated day numbers and time, e.g. 1,15@09:30.
    #[arg(
        long,
        conflicts_with_all = [
            "every",
            "daily",
            "weekly",
            "specific_dates",
            "at",
            "repeat_every"
        ]
    )]
    pub monthly: Option<String>,
    /// Specific comma-separated dates and time, e.g. 2026-09-01,2026-09-15@09:30.
    #[arg(
        long,
        conflicts_with_all = [
            "every",
            "daily",
            "weekly",
            "monthly",
            "at",
            "repeat_every"
        ]
    )]
    pub specific_dates: Option<String>,
    /// One-time run at RFC3339 / YYYY-MM-DDTHH:MM local time.
    #[arg(
        long,
        conflicts_with_all = [
            "every",
            "daily",
            "weekly",
            "monthly",
            "specific_dates",
            "repeat_every"
        ]
    )]
    pub at: Option<String>,
    /// Weekly recurrence interval in weeks (1-520); defaults to 1 for new weekly schedules.
    #[arg(
        long,
        conflicts_with_all = ["every", "daily", "monthly", "specific_dates", "at"]
    )]
    pub repeat_every: Option<u32>,
    /// End condition: never, on_date, or after_occurrences.
    #[arg(long)]
    pub end: Option<String>,
    /// End date used with --end on_date.
    #[arg(long)]
    pub end_date: Option<String>,
    /// Maximum occurrence count used with --end after_occurrences.
    #[arg(long)]
    pub max_occurrences: Option<u32>,
}

// ---------------------------------------------------------------------------
// wardian send
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// wardian notify
// ---------------------------------------------------------------------------

#[derive(Debug, Args)]
pub struct NotifyArgs {
    #[command(subcommand)]
    pub command: NotifyCommand,
}

#[derive(Debug, Subcommand)]
pub enum NotifyCommand {
    /// Send an important, concise update to the user's Inbox.
    Update {
        /// Update body (omit when using --stdin or --file)
        message: Option<String>,
        #[arg(long)]
        title: String,
        #[arg(long, conflicts_with = "message")]
        stdin: bool,
        #[arg(long, conflicts_with_all = ["message", "stdin"])]
        file: Option<String>,
    },
    /// Request exceptional human approval for a consequential action.
    Approval {
        /// Request context (omit when using --stdin or --file)
        message: Option<String>,
        #[arg(long)]
        title: String,
        #[arg(long)]
        action: String,
        #[arg(long)]
        risk: String,
        /// Explicit choice to present to the user; supply two to five times.
        #[arg(long = "choice", required = true)]
        choices: Vec<String>,
        /// Expiry, for example 30m or 2h. Expiry never means approval.
        #[arg(long = "expires-in", default_value = "30m")]
        expires_in: String,
        /// Wait for a user decision or expiry.
        #[arg(long)]
        wait: bool,
        /// Maximum wait duration when --wait is supplied.
        #[arg(long, default_value = "30m")]
        timeout: String,
        #[arg(long, conflicts_with = "message")]
        stdin: bool,
        #[arg(long, conflicts_with_all = ["message", "stdin"])]
        file: Option<String>,
    },
}

#[derive(Debug, Args)]
pub struct SendArgs {
    /// Message text (omit when using --stdin or --file)
    pub message: Option<String>,

    /// Target: agent name, UUID, "class:<ClassName>", or "all"
    #[arg(long)]
    pub to: String,

    /// Read message from stdin
    #[arg(long, conflicts_with = "message")]
    pub stdin: bool,

    /// Read message from a file
    #[arg(long, conflicts_with_all = ["message", "stdin"])]
    pub file: Option<String>,

    /// Reserved; thread delivery is not supported. Use --to for delivery.
    #[arg(long)]
    pub thread: Option<String>,

    /// Send the message body as a provider slash command without sender attribution
    #[arg(long = "as-command")]
    pub as_command: bool,

    /// Queue policy to use when the target is not safe for live delivery
    #[arg(long = "queue-policy", value_enum, default_value = "queue-if-busy")]
    pub queue_policy: QueuePolicyArg,

    /// Send an explicit approval action instead of a normal message
    #[arg(long, value_enum, conflicts_with = "as_command")]
    pub approval: Option<ApprovalArg>,

    /// Wait for the delivered target turn to reach this status; idle uses provider-confirmed turn completion
    #[arg(long = "wait-until")]
    pub wait_until: Option<String>,

    /// Maximum time to wait for a headless delivery or --wait-until, e.g. 30s, 10m, or 1000ms
    #[arg(long, default_value = "10m")]
    pub timeout: String,

    /// Target resolution scope for broadcast/class targets: neighbors (default) or all
    #[arg(long, value_parser = ["neighbors", "all"], default_value = "neighbors")]
    pub scope: String,

    /// Caller-owned key used to make a delivery request idempotent.
    #[arg(long = "idempotency-key")]
    pub idempotency_key: Option<String>,

    /// Absolute RFC3339 deadline after which queued delivery expires.
    #[arg(long, conflicts_with = "expires_in")]
    pub deadline: Option<String>,

    /// Relative delivery lifetime, for example 30s or 5m.
    #[arg(long = "expires-in", conflicts_with = "deadline")]
    pub expires_in: Option<String>,

    /// Reject delivery unless the target is still on this Wardian generation.
    #[arg(long = "expected-generation")]
    pub expected_generation: Option<u64>,

    /// Exceptionally steer an active turn because its premise is invalid.
    #[arg(long = "invalidate-premise")]
    pub invalidate_premise: bool,
}

// ---------------------------------------------------------------------------
// wardian delivery
// ---------------------------------------------------------------------------

#[derive(Debug, Args)]
pub struct DeliveryArgs {
    #[command(subcommand)]
    pub command: DeliveryCommand,
}

#[derive(Debug, Subcommand)]
pub enum DeliveryCommand {
    /// Show the durable delivery projection and bounded evidence history.
    Show {
        interaction_id: String,
        #[arg(long, default_value_t = 100)]
        evidence_limit: usize,
    },
    /// Request provider cancellation for submitted work.
    Cancel { interaction_id: String },
    /// Withdraw work that has not crossed the provider submission boundary.
    Withdraw { interaction_id: String },
    /// Atomically supersede queued work with a new message.
    Replace {
        interaction_id: String,
        message: Option<String>,
        #[arg(long, conflicts_with = "message")]
        stdin: bool,
        #[arg(long, conflicts_with_all = ["message", "stdin"])]
        file: Option<String>,
        #[arg(long = "idempotency-key")]
        idempotency_key: String,
        #[arg(long, conflicts_with = "expires_in")]
        deadline: Option<String>,
        #[arg(long = "expires-in", conflicts_with = "deadline")]
        expires_in: Option<String>,
    },
    /// Show native transport capabilities for a Wardian agent.
    Capabilities { target: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum QueuePolicyArg {
    QueueIfBusy,
    LiveOnly,
    MailboxOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ApprovalArg {
    Accept,
    Reject,
}

// ---------------------------------------------------------------------------
// wardian ask
// ---------------------------------------------------------------------------

#[derive(Debug, Args)]
pub struct AskArgs {
    /// Target agent name or UUID. Broadcast and class targets are not supported.
    pub target: String,

    /// Additional explicit target names or UUIDs, separated by commas. Broadcast and class targets are not supported.
    #[arg(long, value_delimiter = ',')]
    pub targets: Vec<String>,

    /// Message text (omit when using --stdin or --file)
    pub message: Option<String>,

    /// Read message from stdin
    #[arg(long, conflicts_with = "message")]
    pub stdin: bool,

    /// Read message from a file
    #[arg(long, conflicts_with_all = ["message", "stdin"])]
    pub file: Option<String>,

    /// Completion condition: reply, status:<status>, output:<substring>, event:<kind>, delivery:<state>, or a bare status
    #[arg(long, default_value = "reply")]
    pub until: Option<String>,

    /// Maximum time to wait, e.g. 30s, 10m, or 1000ms
    #[arg(long, default_value = "10m")]
    pub timeout: String,

    /// Maximum output bytes to return from the response snapshot
    #[arg(long, default_value_t = 65536)]
    pub tail: usize,

    /// Thread name for grouped conversations
    #[arg(long)]
    pub thread: Option<String>,

    /// Caller-owned key used to make a delivery request idempotent.
    #[arg(long = "idempotency-key")]
    pub idempotency_key: Option<String>,

    /// Absolute RFC3339 deadline after which queued delivery expires.
    #[arg(long, conflicts_with = "expires_in")]
    pub deadline: Option<String>,

    /// Relative delivery lifetime, for example 30s or 5m.
    #[arg(long = "expires-in", conflicts_with = "deadline")]
    pub expires_in: Option<String>,

    /// Reject delivery unless the target is still on this Wardian generation.
    #[arg(long = "expected-generation")]
    pub expected_generation: Option<u64>,

    /// Exceptionally steer an active turn because its premise is invalid.
    #[arg(long = "invalidate-premise")]
    pub invalidate_premise: bool,
}

// ---------------------------------------------------------------------------
// wardian reply
// ---------------------------------------------------------------------------

#[derive(Debug, Args)]
pub struct ReplyArgs {
    /// Structured ask request id.
    pub request_id: String,

    /// Reply status.
    #[arg(long, value_enum)]
    pub status: ReplyStatusArg,

    /// Reply body text (omit when using --stdin or --file)
    pub message: Option<String>,

    /// Read reply body from stdin
    #[arg(long, conflicts_with = "message")]
    pub stdin: bool,

    /// Read reply body from a file
    #[arg(long, conflicts_with_all = ["message", "stdin"])]
    pub file: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ReplyStatusArg {
    Done,
    Blocked,
    Failed,
}

// ---------------------------------------------------------------------------
// wardian graph
// ---------------------------------------------------------------------------

#[derive(Debug, Args)]
pub struct GraphArgs {
    #[command(subcommand)]
    pub command: GraphCommand,

    #[arg(long, global = true)]
    pub pretty: bool,
}

#[derive(Debug, Subcommand)]
pub enum GraphCommand {
    /// Whole-graph snapshot: agents, manual edges, unmapped pairs, ignored pairs.
    Show,
    /// Resolved neighbor view for one agent (defaults to self inside a session).
    Neighbors { agent: Option<String> },
    /// Per-pair communication activity with an unmapped flag.
    Activity,
    /// Create a manual edge. In a session: `link <other>` means me <-> other.
    Link { a: String, b: Option<String> },
    /// Delete a manual edge. Same identity rules as link.
    Unlink { a: String, b: Option<String> },
    /// Durably dismiss an unmapped suggestion. Same identity rules as link.
    Ignore { a: String, b: Option<String> },
    /// Remove a dismissal. Same identity rules as link.
    Unignore { a: String, b: Option<String> },
}

#[derive(Debug, Args)]
pub struct AgentArgs {
    /// Agent name or UUID; omit to inspect the managed session.
    pub target: Option<String>,

    #[command(subcommand)]
    pub command: Option<AgentCommand>,

    /// Comma-separated output fields for identity show/list/spawn/clone.
    #[arg(long, global = true, conflicts_with = "field")]
    pub fields: Option<String>,

    /// Print one identity field as a bare value.
    #[arg(long, global = true, conflicts_with_all = ["fields", "pretty", "verbose"])]
    pub field: Option<String>,

    /// Include process and visibility metadata in identity output.
    #[arg(long, global = true)]
    pub verbose: bool,

    /// Render identity output as human-readable text instead of JSON.
    #[arg(long, global = true)]
    pub pretty: bool,
}

#[derive(Debug, Subcommand)]
pub enum AgentCommand {
    /// Show one agent, or the managed session when no target is supplied.
    Show { target: Option<String> },
    /// List visible agents; narrow with scope, status, class, or workspace.
    List {
        /// auto (neighbors when WARDIAN_SESSION_ID is set, else workspace),
        /// neighbors, workspace, or all
        #[arg(long, default_value = "auto", value_parser = ["auto", "neighbors", "workspace", "all"])]
        scope: String,
        #[arg(long)]
        status: Option<String>,
        #[arg(long = "class")]
        class_name: Option<String>,
        #[arg(long)]
        workspace: Option<String>,
    },
    /// List models and compatible reasoning efforts for one installed provider.
    Models {
        #[arg(long)]
        provider: String,
        /// Bypass the short provider catalogue cache.
        #[arg(long)]
        refresh: bool,
    },
    /// Permanently remove an agent, its habitat, and its session history.
    Delete {
        /// Agent name or UUID.
        target: String,
        /// Must exactly match the agent's current name.
        #[arg(long, value_name = "AGENT_NAME")]
        confirm: String,
        /// Also terminate a running provider before removing the agent.
        #[arg(long)]
        force: bool,
    },
    /// Rename an agent without restarting its provider.
    Rename {
        /// Agent name or UUID.
        target: String,
        /// New name; use only letters, numbers, underscores, or hyphens.
        new_name: String,
    },
    /// Restart the provider while preserving the Wardian agent and its history.
    Restart { target: String },
    /// Pause an agent's provider process.
    Pause { target: String },
    /// Resume a paused agent.
    Resume { target: String },
    /// Create an agent with an installed provider and existing class.
    Spawn {
        #[arg(long)]
        provider: String,
        #[arg(long = "class")]
        class: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        workspace: Option<String>,
        /// Provider-discovered model identifier. Omit for the provider default.
        #[arg(long)]
        model: Option<String>,
        /// Provider-discovered reasoning effort. Omit for the provider default.
        #[arg(long = "reasoning-effort")]
        reasoning_effort: Option<String>,
    },
    /// Update live agent configuration without restarting the provider process.
    Update {
        /// Agent name or UUID.
        target: String,
        /// Assign an existing class and regenerate its instruction include directories.
        #[arg(long, required_unless_present_any = ["workspace", "description", "model", "reasoning_effort"])]
        class: Option<String>,
        /// Move an ordinary agent workspace to an existing path.
        #[arg(long, required_unless_present_any = ["class", "description", "model", "reasoning_effort"])]
        workspace: Option<String>,
        /// Set the optional purpose memo. Pass an empty value to clear it.
        #[arg(long, required_unless_present_any = ["class", "workspace", "model", "reasoning_effort"])]
        description: Option<String>,
        /// Provider-discovered model identifier. Pass an empty value to clear it.
        #[arg(long, required_unless_present_any = ["class", "workspace", "description", "reasoning_effort"])]
        model: Option<String>,
        /// Provider-discovered reasoning effort. Pass an empty value to clear it.
        #[arg(long = "reasoning-effort", required_unless_present_any = ["class", "workspace", "description", "model"])]
        reasoning_effort: Option<String>,
    },
    /// Show effective provider policy and launch diagnostics for one agent.
    Doctor {
        /// Agent name or UUID.
        target: String,
    },
    /// Clone an agent's configuration into a new agent.
    Clone {
        target: String,
        #[arg(long)]
        name: Option<String>,
    },
    /// Inspect or change managed workspace assignments.
    Worktree {
        #[command(subcommand)]
        command: AgentWorktreeCommand,
    },
    /// Read bounded output, optionally waiting for a marker.
    Watch {
        target: String,
        #[arg(long)]
        since: Option<String>,
        #[arg(long)]
        until: Option<String>,
        #[arg(long)]
        include: Option<String>,
        #[arg(long)]
        raw: bool,
        #[arg(long = "tail")]
        tail: Option<usize>,
        #[arg(long, default_value = "10m")]
        timeout: String,
        /// Reserved; continuous streaming is not supported. Use bounded reads.
        #[arg(long)]
        follow: bool,
    },
    /// Wait for a status; --next requires a new matching transition.
    Wait {
        target: String,
        #[arg(long)]
        until: String,
        #[arg(long, default_value = "10m")]
        timeout: String,
        #[arg(long)]
        next: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum AgentWorktreeCommand {
    /// List managed worktrees and agent assignments.
    List,
    /// Create and assign a managed worktree, starting a fresh provider session.
    Enable {
        target: String,
        #[arg(long)]
        name: Option<String>,
    },
    /// Join an existing managed worktree, starting a fresh provider session.
    Join {
        target: String,
        #[arg(long)]
        worktree: String,
    },
    /// Leave a managed worktree without deleting its project files.
    Disable { target: String },
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn parses_agent_target_shorthand() {
        let cli = Cli::try_parse_from(["wardian", "agent", "coder-a1"]).unwrap();
        assert!(matches!(cli.command, Command::Agent(_)));
    }

    #[test]
    fn telemetry_repair_is_explicit_but_retention_is_not_a_cli_command() {
        let cli =
            Cli::try_parse_from(["wardian", "telemetry", "repair", "--db", "<state-db>"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Telemetry(TelemetryArgs {
                command: TelemetryCommand::Repair {
                    apply: false,
                    backup: None,
                    ..
                }
            })
        ));
        assert!(Cli::try_parse_from([
            "wardian",
            "telemetry",
            "maintain",
            "--retain-days",
            "90",
            "--backup",
            "<backup-path>",
        ])
        .is_err());
    }

    #[test]
    fn parses_agent_update_and_requires_a_change() {
        let cli = Cli::try_parse_from([
            "wardian",
            "agent",
            "update",
            "coder-a1",
            "--class",
            "Reviewer",
            "--workspace",
            "D:/Development/Wardian",
        ])
        .unwrap();
        let Command::Agent(args) = cli.command else {
            panic!("expected Agent")
        };
        assert!(matches!(
            args.command,
            Some(AgentCommand::Update {
                ref target,
                class: Some(ref class),
                workspace: Some(ref workspace),
                description: None,
                model: None,
                reasoning_effort: None,
            }) if target == "coder-a1"
                && class == "Reviewer"
                && workspace == "D:/Development/Wardian"
        ));

        let error = Cli::try_parse_from(["wardian", "agent", "update", "coder-a1"]).unwrap_err();
        assert_eq!(
            error.kind(),
            clap::error::ErrorKind::MissingRequiredArgument
        );

        let cli = Cli::try_parse_from([
            "wardian",
            "agent",
            "update",
            "coder-a1",
            "--description",
            "Owns frontend release follow-up",
        ])
        .unwrap();
        let Command::Agent(args) = cli.command else {
            panic!("expected Agent")
        };
        assert!(matches!(
            args.command,
            Some(AgentCommand::Update {
                description: Some(ref description),
                ..
            }) if description == "Owns frontend release follow-up"
        ));
    }

    #[test]
    fn parses_agent_model_catalog_and_selection_flags() {
        let cli = Cli::try_parse_from([
            "wardian",
            "agent",
            "models",
            "--provider",
            "codex",
            "--refresh",
        ])
        .unwrap();
        let Command::Agent(args) = cli.command else {
            panic!("expected Agent")
        };
        assert!(matches!(
            args.command,
            Some(AgentCommand::Models { ref provider, refresh: true }) if provider == "codex"
        ));

        let cli = Cli::try_parse_from([
            "wardian",
            "agent",
            "spawn",
            "--provider",
            "codex",
            "--class",
            "Reviewer",
            "--model",
            "gpt-5.6-sol",
            "--reasoning-effort",
            "high",
        ])
        .unwrap();
        let Command::Agent(args) = cli.command else {
            panic!("expected Agent")
        };
        assert!(matches!(
            args.command,
            Some(AgentCommand::Spawn {
                model: Some(ref model),
                reasoning_effort: Some(ref reasoning_effort),
                ..
            }) if model == "gpt-5.6-sol" && reasoning_effort == "high"
        ));

        let cli = Cli::try_parse_from([
            "wardian",
            "agent",
            "update",
            "reviewer-a1",
            "--model",
            "gpt-5.6-sol",
        ])
        .unwrap();
        let Command::Agent(args) = cli.command else {
            panic!("expected Agent")
        };
        assert!(matches!(
            args.command,
            Some(AgentCommand::Update {
                model: Some(ref model),
                reasoning_effort: None,
                ..
            }) if model == "gpt-5.6-sol"
        ));
    }

    #[test]
    fn parses_library_list_show_and_read() {
        let cli = Cli::try_parse_from(["wardian", "library", "list", "skills", "--flat"]).unwrap();
        let Command::Library(args) = cli.command else {
            panic!("expected Library")
        };
        assert!(matches!(
            args.command,
            LibraryCommand::List {
                section: Some(ref section),
                flat: true
            } if section == "skills"
        ));

        let cli =
            Cli::try_parse_from(["wardian", "library", "show", "automations/audit.md"]).unwrap();
        let Command::Library(args) = cli.command else {
            panic!("expected Library")
        };
        assert!(matches!(
            args.command,
            LibraryCommand::Show {
                ref entry_ref,
                content: false
            } if entry_ref == "automations/audit.md"
        ));

        let cli = Cli::try_parse_from(["wardian", "library", "read", "classes/Reviewer"]).unwrap();
        let Command::Library(args) = cli.command else {
            panic!("expected Library")
        };
        assert!(matches!(
            args.command,
            LibraryCommand::Read { ref entry_ref } if entry_ref == "classes/Reviewer"
        ));
    }

    #[test]
    fn library_create_rejects_stdin_and_file_together() {
        let error = Cli::try_parse_from([
            "wardian",
            "library",
            "create",
            "prompts/triage.md",
            "--stdin",
            "--file",
            "triage.md",
        ])
        .unwrap_err();

        assert_eq!(error.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    #[test]
    fn parses_library_mutations_metadata_and_deployments() {
        let cli = Cli::try_parse_from([
            "wardian",
            "library",
            "create",
            "prompts/triage.md",
            "--stdin",
        ])
        .unwrap();
        let Command::Library(args) = cli.command else {
            panic!("expected Library")
        };
        assert!(matches!(
            args.command,
            LibraryCommand::Create {
                ref entry_ref,
                stdin: true,
                file: None
            } if entry_ref == "prompts/triage.md"
        ));

        let cli = Cli::try_parse_from([
            "wardian",
            "library",
            "write",
            "skills/planner",
            "--file",
            "SKILL.md",
        ])
        .unwrap();
        let Command::Library(args) = cli.command else {
            panic!("expected Library")
        };
        assert!(matches!(
            args.command,
            LibraryCommand::Write {
                ref entry_ref,
                stdin: false,
                ref file
            } if entry_ref == "skills/planner" && file.as_deref() == Some("SKILL.md")
        ));

        let cli = Cli::try_parse_from([
            "wardian",
            "library",
            "tags",
            "skills/planner",
            "--set",
            "review",
            "--set",
            "daily",
        ])
        .unwrap();
        let Command::Library(args) = cli.command else {
            panic!("expected Library")
        };
        assert!(matches!(
            args.command,
            LibraryCommand::Tags { ref entry_ref, ref set }
                if entry_ref == "skills/planner"
                    && set == &vec!["review".to_string(), "daily".to_string()]
        ));

        let cli = Cli::try_parse_from([
            "wardian",
            "library",
            "deploy",
            "skills/planner",
            "--targets",
            "user:global,class:Reviewer",
        ])
        .unwrap();
        let Command::Library(args) = cli.command else {
            panic!("expected Library")
        };
        assert!(matches!(
            args.command,
            LibraryCommand::Deploy { ref skill_ref, ref targets, clear: false }
                if skill_ref == "skills/planner"
                    && targets.as_deref() == Some("user:global,class:Reviewer")
        ));

        let cli = Cli::try_parse_from([
            "wardian",
            "library",
            "orphan",
            "delete",
            "--target",
            "class:Reviewer",
            "--skill",
            "planner",
        ])
        .unwrap();
        let Command::Library(args) = cli.command else {
            panic!("expected Library")
        };
        assert!(matches!(
            args.command,
            LibraryCommand::Orphan {
                command: LibraryOrphanCommand::Delete { ref target, ref skill }
            } if target == "class:Reviewer" && skill == "planner"
        ));
    }

    #[test]
    fn parses_library_deploy_clear_as_explicit_empty_set() {
        let cli =
            Cli::try_parse_from(["wardian", "library", "deploy", "skills/planner", "--clear"])
                .unwrap();
        let Command::Library(args) = cli.command else {
            panic!("expected Library")
        };
        assert!(matches!(
            args.command,
            LibraryCommand::Deploy {
                ref skill_ref,
                targets: None,
                clear: true
            } if skill_ref == "skills/planner"
        ));

        assert!(Cli::try_parse_from(["wardian", "library", "deploy", "skills/planner",]).is_err());
        assert!(Cli::try_parse_from([
            "wardian",
            "library",
            "deploy",
            "skills/planner",
            "--clear",
            "--targets",
            "user:global",
        ])
        .is_err());
    }

    #[test]
    fn library_help_describes_agent_contracts() {
        let library_help = Cli::try_parse_from(["wardian", "library", "--help"])
            .unwrap_err()
            .to_string();
        assert!(library_help.contains("List Library entries"));

        let deploy_help = Cli::try_parse_from(["wardian", "library", "deploy", "--help"])
            .unwrap_err()
            .to_string();
        assert!(deploy_help.contains("complete desired target set"));
        assert!(deploy_help.contains("--clear"));

        let create_help = Cli::try_parse_from(["wardian", "library", "create", "--help"])
            .unwrap_err()
            .to_string();
        assert!(create_help.contains("wardian automation"));
    }

    #[test]
    fn parses_automation_node_types_json() {
        let cli = Cli::try_parse_from(["wardian", "automation", "node-types", "--json"]).unwrap();
        let Command::Automation(args) = cli.command else {
            panic!("expected Automation")
        };
        assert!(matches!(
            args.command,
            AutomationCommand::NodeTypes {
                json: true,
                node: None
            }
        ));
    }

    #[test]
    fn parses_conversation_list_current_agent() {
        let cli = Cli::try_parse_from(["wardian", "conversation", "list"]).unwrap();
        let Command::Conversation(args) = cli.command else {
            panic!("expected Conversation")
        };
        assert!(matches!(
            args.command,
            ConversationCommand::List {
                agent: None,
                ref scope,
            } if scope == "current"
        ));
    }

    #[test]
    fn parses_conversation_list_agent_filter() {
        let cli =
            Cli::try_parse_from(["wardian", "conversation", "list", "--agent", "agent-1"]).unwrap();
        let Command::Conversation(args) = cli.command else {
            panic!("expected Conversation")
        };
        assert!(matches!(
            args.command,
            ConversationCommand::List {
                ref agent,
                ref scope,
            } if agent.as_deref() == Some("agent-1") && scope == "current"
        ));
    }

    #[test]
    fn parses_conversation_list_scope_all_agent() {
        let cli = Cli::try_parse_from([
            "wardian",
            "conversation",
            "list",
            "--scope",
            "all",
            "--agent",
            "agent-1",
        ])
        .unwrap();
        let Command::Conversation(args) = cli.command else {
            panic!("expected Conversation")
        };
        assert!(matches!(
            args.command,
            ConversationCommand::List {
                ref agent,
                ref scope,
            } if agent.as_deref() == Some("agent-1") && scope == "all"
        ));
    }

    #[test]
    fn parses_conversation_show() {
        let cli = Cli::try_parse_from(["wardian", "conversation", "show", "conv-1"]).unwrap();
        let Command::Conversation(args) = cli.command else {
            panic!("expected Conversation")
        };
        assert!(matches!(
            args.command,
            ConversationCommand::Show {
                ref conversation_id,
            } if conversation_id == "conv-1"
        ));
    }

    #[test]
    fn parses_listener_watch_with_patterns_and_overlap() {
        let cli = Cli::try_parse_from([
            "wardian",
            "automation",
            "listener",
            "watch",
            "--blueprint",
            "audit",
            "--name",
            "Source audit",
            "--path",
            "/work/repo",
            "--recursive",
            "--pattern",
            "**/*.rs",
            "--pattern",
            "**/*.toml",
            "--ignore",
            "**/generated/**",
            "--event",
            "modified",
            "--debounce-ms",
            "1500",
            "--overlap",
            "coalesce",
            "--enable",
        ])
        .unwrap();
        let Command::Automation(args) = cli.command else {
            panic!("expected Automation")
        };
        let AutomationCommand::Listener(command) = args.command else {
            panic!("expected Listener")
        };
        match *command {
            AutomationListenerCommand::Watch {
                blueprint,
                path,
                recursive,
                pattern,
                ignore,
                event,
                debounce_ms,
                common,
                ..
            } => {
                assert_eq!(blueprint, "audit");
                assert_eq!(path, "/work/repo");
                assert!(recursive);
                assert_eq!(pattern, vec!["**/*.rs", "**/*.toml"]);
                assert_eq!(ignore, vec!["**/generated/**"]);
                assert_eq!(event, vec!["modified"]);
                assert_eq!(debounce_ms, Some(1500));
                assert_eq!(common.overlap.as_deref(), Some("coalesce"));
                assert!(common.enable);
            }
            other => panic!("expected Watch, got {other:?}"),
        }
    }

    #[test]
    fn listeners_are_created_disabled_unless_enable_is_passed() {
        let cli = Cli::try_parse_from([
            "wardian",
            "automation",
            "listener",
            "poll",
            "--blueprint",
            "audit",
            "--name",
            "Release watch",
            "--url",
            "https://example.invalid/releases",
            "--fingerprint",
            "json",
            "--json-pointer",
            "/0/tag_name",
            "--interval",
            "600",
        ])
        .unwrap();
        let Command::Automation(args) = cli.command else {
            panic!("expected Automation")
        };
        let AutomationCommand::Listener(command) = args.command else {
            panic!("expected Listener")
        };
        match *command {
            AutomationListenerCommand::Poll {
                url,
                interval,
                fingerprint,
                json_pointer,
                common,
                ..
            } => {
                assert_eq!(url, "https://example.invalid/releases");
                assert_eq!(interval, Some(600));
                assert_eq!(fingerprint, "json");
                assert_eq!(json_pointer.as_deref(), Some("/0/tag_name"));
                assert!(
                    !common.enable,
                    "a listener must not start watching just because it was created"
                );
            }
            other => panic!("expected Poll, got {other:?}"),
        }
    }

    #[test]
    fn an_unknown_overlap_policy_is_refused_at_parse_time() {
        let result = Cli::try_parse_from([
            "wardian",
            "automation",
            "listener",
            "hook",
            "--blueprint",
            "audit",
            "--name",
            "CI",
            "--path",
            "ci",
            "--overlap",
            "queue",
        ]);
        assert!(
            result.is_err(),
            "`queue` is not one of the listener policies"
        );
    }

    #[test]
    fn parses_automation_validate_path() {
        let cli = Cli::try_parse_from(["wardian", "automation", "validate", "wf.md"]).unwrap();
        let Command::Automation(args) = cli.command else {
            panic!("expected Automation")
        };
        assert!(matches!(
            args.command,
            AutomationCommand::Validate { ref path } if path == "wf.md"
        ));
    }

    #[test]
    fn parses_inbox_list_filters_and_paging() {
        let cli = Cli::try_parse_from([
            "wardian",
            "inbox",
            "list",
            "--type",
            "action_needed,approval_request",
            "--source",
            "provider_runtime,interaction_store",
            "--unread",
            "--limit",
            "25",
            "--offset",
            "10",
        ])
        .unwrap();
        let Command::Inbox(args) = cli.command else {
            panic!("expected Inbox")
        };
        assert!(matches!(
            args.command,
            InboxCommand::List {
                ref types,
                ref sources,
                unread: true,
                limit: 25,
                offset: 10,
            } if types == &["action_needed", "approval_request"]
                && sources == &["provider_runtime", "interaction_store"]
        ));
    }

    #[test]
    fn parses_automation_exec_path_with_default_executor() {
        let cli = Cli::try_parse_from(["wardian", "automation", "exec", "wf.md"]).unwrap();
        let Command::Automation(args) = cli.command else {
            panic!("expected Automation")
        };
        assert!(matches!(
            args.command,
            AutomationCommand::Exec { ref path, ref executor, ref provider, ref workspace, .. }
                if path == "wf.md"
                    && executor == "live"
                    && provider.is_none()
                    && workspace.is_none()
        ));
    }

    #[test]
    fn parses_automation_exec_executor() {
        let cli = Cli::try_parse_from([
            "wardian",
            "automation",
            "exec",
            "wf.md",
            "--executor",
            "real",
        ])
        .unwrap();
        let Command::Automation(args) = cli.command else {
            panic!("expected Automation")
        };
        assert!(matches!(
            args.command,
            AutomationCommand::Exec { ref path, ref executor, .. }
                if path == "wf.md" && executor == "real"
        ));
    }

    #[test]
    fn parses_automation_exec_with_input_and_bind() {
        let cli = Cli::try_parse_from([
            "wardian",
            "automation",
            "exec",
            "wf.md",
            "--input",
            "{\"x\":1}",
            "--provider",
            "codex",
            "--workspace",
            ".",
            "--bind",
            "role=agent-123",
        ])
        .unwrap();
        let Command::Automation(args) = cli.command else {
            panic!("expected Automation")
        };
        assert!(matches!(
            args.command,
            AutomationCommand::Exec { ref input, ref provider, ref workspace, ref bind, .. }
                if input.as_deref() == Some("{\"x\":1}")
                    && provider.as_deref() == Some("codex")
                    && workspace.as_deref() == Some(".")
                    && bind == &vec!["role=agent-123".to_string()]
        ));
    }

    #[test]
    fn parses_automation_runs() {
        let cli = Cli::try_parse_from(["wardian", "automation", "runs"]).unwrap();
        let Command::Automation(args) = cli.command else {
            panic!("expected Automation")
        };
        assert!(matches!(args.command, AutomationCommand::Runs));
    }

    #[test]
    fn parses_schedule_add() {
        let cli = Cli::try_parse_from([
            "wardian",
            "automation",
            "schedule",
            "add",
            "--blueprint",
            "heartbeat",
            "--name",
            "HB",
            "--every",
            "60",
            "--workspace",
            ".",
        ])
        .unwrap();
        let Command::Automation(args) = cli.command else {
            panic!("expected Automation")
        };
        assert!(matches!(
            args.command,
            AutomationCommand::Schedule(ref command)
                if matches!(command.as_ref(), AutomationScheduleCommand::Add { .. })
        ));
    }

    #[test]
    fn parses_weekly_repeat_every() {
        let cli = Cli::try_parse_from([
            "wardian",
            "automation",
            "schedule",
            "add",
            "--blueprint",
            "heartbeat",
            "--name",
            "HB",
            "--weekly",
            "Sun@12:00",
            "--repeat-every",
            "2",
            "--workspace",
            ".",
        ])
        .unwrap();
        let Command::Automation(args) = cli.command else {
            panic!("expected Automation")
        };
        assert!(matches!(
            args.command,
            AutomationCommand::Schedule(ref command)
                if matches!(command.as_ref(), AutomationScheduleCommand::Add { cadence, .. }
                    if cadence.weekly.as_deref() == Some("Sun@12:00")
                        && cadence.repeat_every == Some(2))
        ));
    }

    #[test]
    fn rejects_repeat_every_with_interval_cadence() {
        let error = Cli::try_parse_from([
            "wardian",
            "automation",
            "schedule",
            "add",
            "--blueprint",
            "heartbeat",
            "--name",
            "HB",
            "--every",
            "60",
            "--repeat-every",
            "2",
            "--workspace",
            ".",
        ])
        .unwrap_err();
        assert_eq!(error.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    #[test]
    fn parses_schedule_update_with_extended_cadence_options() {
        let cli = Cli::try_parse_from([
            "wardian",
            "automation",
            "schedule",
            "update",
            "s1",
            "--monthly",
            "1,15@09:30",
            "--end",
            "after_occurrences",
            "--max-occurrences",
            "4",
            "--active",
        ])
        .unwrap();
        let Command::Automation(args) = cli.command else {
            panic!("expected Automation")
        };
        assert!(matches!(
            args.command,
            AutomationCommand::Schedule(ref command)
                if matches!(command.as_ref(), AutomationScheduleCommand::Update { id, cadence, active, .. }
                    if id == "s1" && cadence.monthly.as_deref() == Some("1,15@09:30") && *active)
        ));
    }

    #[test]
    fn parses_automation_run_show() {
        let cli = Cli::try_parse_from(["wardian", "automation", "run-show", "wf", "r1"]).unwrap();
        let Command::Automation(args) = cli.command else {
            panic!("expected Automation")
        };
        assert!(matches!(
            args.command,
            AutomationCommand::RunShow { ref blueprint_id, ref run_id }
                if blueprint_id == "wf" && run_id == "r1"
        ));
    }

    #[test]
    fn parses_automation_replay() {
        let cli = Cli::try_parse_from(["wardian", "automation", "replay", "wf", "r1"]).unwrap();
        let Command::Automation(args) = cli.command else {
            panic!("expected Automation")
        };
        assert!(matches!(
            args.command,
            AutomationCommand::Replay { ref blueprint_id, ref run_id }
                if blueprint_id == "wf" && run_id == "r1"
        ));
    }

    #[test]
    fn parses_automation_parse() {
        let cli = Cli::try_parse_from(["wardian", "automation", "parse", "wf.md"]).unwrap();
        let Command::Automation(args) = cli.command else {
            panic!("expected Automation")
        };
        assert!(matches!(
            args.command,
            AutomationCommand::Parse { ref path } if path == "wf.md"
        ));
    }

    #[test]
    fn parses_automation_normalize_write() {
        let cli = Cli::try_parse_from(["wardian", "automation", "normalize", "wf.md", "--write"])
            .unwrap();
        let Command::Automation(args) = cli.command else {
            panic!("expected Automation")
        };
        assert!(matches!(
            args.command,
            AutomationCommand::Normalize { ref path, write: true } if path == "wf.md"
        ));
    }

    #[test]
    fn parses_notify_approval_with_explicit_choices() {
        let cli = Cli::try_parse_from([
            "wardian",
            "notify",
            "approval",
            "Deployment is ready",
            "--title",
            "Deploy production",
            "--action",
            "Deploy the release",
            "--risk",
            "Changes live traffic",
            "--choice",
            "Deploy",
            "--choice",
            "Do not deploy",
            "--wait",
        ])
        .expect("parse notify approval");
        let Command::Notify(args) = cli.command else {
            panic!("expected Notify")
        };
        assert!(matches!(
            args.command,
            NotifyCommand::Approval { wait: true, ref choices, .. }
                if choices == &vec!["Deploy".to_string(), "Do not deploy".to_string()]
        ));
    }

    #[test]
    fn parses_send_as_command() {
        let cli = Cli::try_parse_from([
            "wardian",
            "send",
            "--to",
            "Wardian-Codex",
            "--as-command",
            "/goal test",
        ])
        .unwrap();
        let Command::Send(args) = cli.command else {
            panic!("expected Send command")
        };

        assert!(args.as_command);
        assert_eq!(args.to, "Wardian-Codex");
        assert_eq!(args.message.as_deref(), Some("/goal test"));
    }

    #[test]
    fn parses_send_queue_policy() {
        let cli = Cli::try_parse_from([
            "wardian",
            "send",
            "hello",
            "--to",
            "agent-1",
            "--queue-policy",
            "live-only",
        ])
        .unwrap();
        let Command::Send(args) = cli.command else {
            panic!("expected Send command")
        };

        assert_eq!(args.queue_policy, QueuePolicyArg::LiveOnly);
    }

    #[test]
    fn parses_send_approval_action() {
        let cli =
            Cli::try_parse_from(["wardian", "send", "--approval", "accept", "--to", "agent-1"])
                .unwrap();
        let Command::Send(args) = cli.command else {
            panic!("expected Send command")
        };

        assert_eq!(args.approval, Some(ApprovalArg::Accept));
        assert_eq!(args.message, None);
    }

    #[test]
    fn send_approval_conflicts_with_as_command() {
        let err = Cli::try_parse_from([
            "wardian",
            "send",
            "--approval",
            "accept",
            "--to",
            "agent-1",
            "--as-command",
            "/status",
        ])
        .unwrap_err();

        assert_eq!(err.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    #[test]
    fn parses_send_scope_all() {
        let cli = Cli::try_parse_from(["wardian", "send", "hi", "--to", "all", "--scope", "all"])
            .unwrap();
        let Command::Send(args) = cli.command else {
            panic!("expected Send command")
        };
        assert_eq!(args.scope, "all");
    }

    #[test]
    fn send_scope_defaults_to_neighbors() {
        let cli = Cli::try_parse_from(["wardian", "send", "hi", "--to", "agent-1"]).unwrap();
        let Command::Send(args) = cli.command else {
            panic!("expected Send command")
        };
        assert_eq!(args.scope, "neighbors");
    }

    #[test]
    fn parses_agent_show_explicit_target() {
        let cli = Cli::try_parse_from(["wardian", "agent", "show", "coder-a1"]).unwrap();
        let Command::Agent(args) = cli.command else {
            panic!("expected Agent command")
        };
        assert!(matches!(
            args.command,
            Some(AgentCommand::Show { target }) if target.as_deref() == Some("coder-a1")
        ));
    }

    #[test]
    fn parses_agent_list_filters() {
        let cli = Cli::try_parse_from([
            "wardian",
            "agent",
            "list",
            "--scope",
            "all",
            "--status",
            "idle",
            "--class",
            "Coder",
            "--workspace",
            "D:/Development/Wardian",
        ])
        .unwrap();
        let Command::Agent(args) = cli.command else {
            panic!("expected Agent command")
        };
        assert!(matches!(
            args.command,
            Some(AgentCommand::List {
                scope,
                status,
                class_name,
                workspace
            }) if scope == "all"
                && status.as_deref() == Some("idle")
                && class_name.as_deref() == Some("Coder")
                && workspace.as_deref() == Some("D:/Development/Wardian")
        ));
    }

    #[test]
    fn parses_agent_list_scope_defaults_to_auto() {
        let cli = Cli::try_parse_from(["wardian", "agent", "list"]).unwrap();
        let Command::Agent(args) = cli.command else {
            panic!("expected Agent command")
        };
        assert!(matches!(
            args.command,
            Some(AgentCommand::List {
                scope,
                ..
            }) if scope == "auto"
        ));
    }

    #[test]
    fn parses_output_modifiers() {
        let cli = Cli::try_parse_from([
            "wardian",
            "agent",
            "coder-a1",
            "--fields",
            "name,status",
            "--verbose",
            "--pretty",
        ])
        .unwrap();
        let Command::Agent(args) = cli.command else {
            panic!("expected Agent command")
        };
        assert_eq!(args.fields.as_deref(), Some("name,status"));
        assert_eq!(args.field, None);
        assert!(args.verbose);
        assert!(args.pretty);
    }

    #[test]
    fn parses_agent_delete_with_exact_name_confirmation() {
        let cli = Cli::try_parse_from([
            "wardian",
            "agent",
            "delete",
            "coder-a1",
            "--confirm",
            "coder-a1",
        ])
        .unwrap();
        let Command::Agent(args) = cli.command else {
            panic!("expected Agent command")
        };
        assert!(matches!(
            args.command,
            Some(AgentCommand::Delete { ref target, ref confirm, force: false })
                if target == "coder-a1" && confirm == "coder-a1"
        ));
    }

    #[test]
    fn parses_forced_agent_delete_with_exact_name_confirmation() {
        let cli = Cli::try_parse_from([
            "wardian",
            "agent",
            "delete",
            "coder-a1",
            "--confirm",
            "coder-a1",
            "--force",
        ])
        .unwrap();
        let Command::Agent(args) = cli.command else {
            panic!("expected Agent command")
        };
        assert!(matches!(
            args.command,
            Some(AgentCommand::Delete { ref target, ref confirm, force: true })
                if target == "coder-a1" && confirm == "coder-a1"
        ));
    }

    #[test]
    fn rejects_legacy_agent_kill_command() {
        assert!(
            Cli::try_parse_from(["wardian", "agent", "kill", "coder-a1", "--confirm",]).is_err()
        );
    }

    #[test]
    fn parses_agent_rename() {
        let cli = Cli::try_parse_from(["wardian", "agent", "rename", "coder-a1", "release-coder"])
            .unwrap();
        let Command::Agent(args) = cli.command else {
            panic!("expected Agent command")
        };
        assert!(matches!(
            args.command,
            Some(AgentCommand::Rename { ref target, ref new_name })
                if target == "coder-a1" && new_name == "release-coder"
        ));
    }

    #[test]
    fn parses_agent_restart() {
        let cli = Cli::try_parse_from(["wardian", "agent", "restart", "coder-a1"]).unwrap();
        let Command::Agent(args) = cli.command else {
            panic!("expected Agent command")
        };
        assert!(
            matches!(args.command, Some(AgentCommand::Restart { target }) if target == "coder-a1")
        );
    }

    #[test]
    fn parses_agent_pause() {
        let cli = Cli::try_parse_from(["wardian", "agent", "pause", "coder-a1"]).unwrap();
        let Command::Agent(args) = cli.command else {
            panic!()
        };
        assert!(matches!(args.command, Some(AgentCommand::Pause { .. })));
    }

    #[test]
    fn parses_agent_doctor_target() {
        let cli = Cli::try_parse_from(["wardian", "agent", "doctor", "ee-1"]).unwrap();
        let Command::Agent(args) = cli.command else {
            panic!("expected Agent command")
        };
        assert!(matches!(
            args.command,
            Some(AgentCommand::Doctor { target }) if target == "ee-1"
        ));
    }

    #[test]
    fn parses_agent_resume() {
        let cli = Cli::try_parse_from(["wardian", "agent", "resume", "coder-a1"]).unwrap();
        let Command::Agent(args) = cli.command else {
            panic!()
        };
        assert!(matches!(args.command, Some(AgentCommand::Resume { .. })));
    }

    #[test]
    fn parses_agent_spawn_with_class() {
        let cli = Cli::try_parse_from([
            "wardian",
            "agent",
            "spawn",
            "--provider",
            "codex",
            "--class",
            "Coder",
            "--name",
            "coder-b1",
            "--workspace",
            "D:/Projects/foo",
        ])
        .unwrap();
        let Command::Agent(args) = cli.command else {
            panic!()
        };
        assert!(matches!(
            args.command,
            Some(AgentCommand::Spawn { ref provider, ref class, ref name, ref workspace, .. })
            if provider == "codex"
                && class == "Coder"
                && name.as_deref() == Some("coder-b1")
                && workspace.as_deref() == Some("D:/Projects/foo")
        ));
    }

    #[test]
    fn parses_agent_clone() {
        let cli = Cli::try_parse_from([
            "wardian", "agent", "clone", "coder-a1", "--name", "coder-a2",
        ])
        .unwrap();
        let Command::Agent(args) = cli.command else {
            panic!()
        };
        assert!(matches!(
            args.command,
            Some(AgentCommand::Clone { ref target, ref name })
            if target == "coder-a1" && name.as_deref() == Some("coder-a2")
        ));
    }

    #[test]
    fn parses_agent_wait_until_status() {
        let cli = Cli::try_parse_from([
            "wardian",
            "agent",
            "wait",
            "reviewer-a1",
            "--until",
            "idle",
            "--timeout",
            "30s",
        ])
        .unwrap();
        let Command::Agent(args) = cli.command else {
            panic!()
        };
        assert!(matches!(
            args.command,
            Some(AgentCommand::Wait { ref target, ref until, ref timeout, next: false })
            if target == "reviewer-a1" && until == "idle" && timeout == "30s"
        ));
    }

    #[test]
    fn parses_agent_watch_options() {
        let cli = Cli::try_parse_from([
            "wardian",
            "agent",
            "watch",
            "Wardian-Codex",
            "--since",
            "agent-1:0001",
            "--until",
            "output:OK",
            "--include",
            "status,output",
            "--tail",
            "4096",
            "--timeout",
            "30s",
        ])
        .unwrap();

        let Command::Agent(args) = cli.command else {
            panic!("agent")
        };
        assert!(matches!(
            args.command,
            Some(AgentCommand::Watch { ref target, ref since, ref until, ref include, raw: false, tail, ref timeout, follow })
                if target == "Wardian-Codex"
                    && since.as_deref() == Some("agent-1:0001")
                    && until.as_deref() == Some("output:OK")
                    && include.as_deref() == Some("status,output")
                    && tail == Some(4096)
                    && timeout == "30s"
                    && !follow
        ));
    }

    #[test]
    fn parses_agent_watch_readable_and_raw_options() {
        let cli = Cli::try_parse_from([
            "wardian",
            "agent",
            "watch",
            "Wardian-Codex",
            "--include",
            "transcript,output,raw_output",
            "--raw",
        ])
        .unwrap();

        let Command::Agent(args) = cli.command else {
            panic!("agent")
        };
        assert!(matches!(
            args.command,
            Some(AgentCommand::Watch { ref include, raw: true, .. })
                if include.as_deref() == Some("transcript,output,raw_output")
        ));
    }

    #[test]
    fn parses_agent_wait_next() {
        let cli = Cli::try_parse_from([
            "wardian",
            "agent",
            "wait",
            "Wardian-Codex",
            "--until",
            "idle",
            "--next",
        ])
        .unwrap();

        let Command::Agent(args) = cli.command else {
            panic!("agent")
        };
        assert!(matches!(
            args.command,
            Some(AgentCommand::Wait { next: true, .. })
        ));
    }

    #[test]
    fn parses_agent_worktree_list() {
        let cli = Cli::try_parse_from(["wardian", "agent", "worktree", "list"]).unwrap();
        let Command::Agent(args) = cli.command else {
            panic!("expected Agent command")
        };
        assert!(matches!(
            args.command,
            Some(AgentCommand::Worktree {
                command: AgentWorktreeCommand::List
            })
        ));
    }

    #[test]
    fn parses_agent_worktree_enable_with_name() {
        let cli = Cli::try_parse_from([
            "wardian",
            "agent",
            "worktree",
            "enable",
            "coder-a1",
            "--name",
            "review fixes",
        ])
        .unwrap();
        let Command::Agent(args) = cli.command else {
            panic!("expected Agent command")
        };
        assert!(matches!(
            args.command,
            Some(AgentCommand::Worktree {
                command: AgentWorktreeCommand::Enable { ref target, ref name }
            }) if target == "coder-a1" && name.as_deref() == Some("review fixes")
        ));
    }

    #[test]
    fn parses_agent_worktree_join() {
        let cli = Cli::try_parse_from([
            "wardian",
            "agent",
            "worktree",
            "join",
            "coder-a1",
            "--worktree",
            "D:/Development/Wardian/.worktrees/review",
        ])
        .unwrap();
        let Command::Agent(args) = cli.command else {
            panic!("expected Agent command")
        };
        assert!(matches!(
            args.command,
            Some(AgentCommand::Worktree {
                command: AgentWorktreeCommand::Join { ref target, ref worktree }
            }) if target == "coder-a1" && worktree == "D:/Development/Wardian/.worktrees/review"
        ));
    }

    #[test]
    fn parses_agent_worktree_disable() {
        let cli =
            Cli::try_parse_from(["wardian", "agent", "worktree", "disable", "coder-a1"]).unwrap();
        let Command::Agent(args) = cli.command else {
            panic!("expected Agent command")
        };
        assert!(matches!(
            args.command,
            Some(AgentCommand::Worktree {
                command: AgentWorktreeCommand::Disable { ref target }
            }) if target == "coder-a1"
        ));
    }

    #[test]
    fn parses_team_list_and_show() {
        let cli = Cli::try_parse_from(["wardian", "team", "list"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Team(TeamArgs {
                command: TeamCommand::List
            })
        ));

        let cli = Cli::try_parse_from(["wardian", "team", "show", "review"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Team(TeamArgs {
                command: TeamCommand::Show { ref target }
            }) if target == "review"
        ));
    }

    #[test]
    fn parses_watchlist_list_and_show() {
        let cli = Cli::try_parse_from(["wardian", "watchlist", "list"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Watchlist(WatchlistArgs {
                command: WatchlistCommand::List
            })
        ));

        let cli = Cli::try_parse_from(["wardian", "watchlist", "show", "main"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Watchlist(WatchlistArgs {
                command: WatchlistCommand::Show { ref target }
            }) if target == "main"
        ));
    }

    #[test]
    fn parses_send_wait_until_status() {
        let cli = Cli::try_parse_from([
            "wardian",
            "send",
            "review this",
            "--to",
            "reviewer-a1",
            "--wait-until",
            "idle",
            "--timeout",
            "10m",
        ])
        .unwrap();
        let Command::Send(args) = cli.command else {
            panic!()
        };
        assert_eq!(args.message.as_deref(), Some("review this"));
        assert_eq!(args.wait_until.as_deref(), Some("idle"));
        assert_eq!(args.timeout, "10m");
    }

    #[test]
    fn parses_native_delivery_policy_on_send() {
        let cli = Cli::try_parse_from([
            "wardian",
            "send",
            "premise changed",
            "--to",
            "reviewer-a1",
            "--idempotency-key",
            "request-7",
            "--expires-in",
            "5m",
            "--expected-generation",
            "3",
            "--invalidate-premise",
        ])
        .unwrap();
        let Command::Send(args) = cli.command else {
            panic!("expected Send command")
        };
        assert_eq!(args.idempotency_key.as_deref(), Some("request-7"));
        assert_eq!(args.expires_in.as_deref(), Some("5m"));
        assert_eq!(args.expected_generation, Some(3));
        assert!(args.invalidate_premise);
    }

    #[test]
    fn parses_delivery_inspection_and_replacement() {
        let show = Cli::try_parse_from([
            "wardian",
            "delivery",
            "show",
            "interaction-1",
            "--evidence-limit",
            "25",
        ])
        .unwrap();
        assert!(matches!(
            show.command,
            Command::Delivery(DeliveryArgs {
                command: DeliveryCommand::Show {
                    ref interaction_id,
                    evidence_limit: 25
                }
            }) if interaction_id == "interaction-1"
        ));

        let replace = Cli::try_parse_from([
            "wardian",
            "delivery",
            "replace",
            "interaction-1",
            "corrected",
            "--idempotency-key",
            "replacement-1",
        ])
        .unwrap();
        assert!(matches!(
            replace.command,
            Command::Delivery(DeliveryArgs {
                command: DeliveryCommand::Replace {
                    ref interaction_id,
                    ref idempotency_key,
                    ..
                }
            }) if interaction_id == "interaction-1" && idempotency_key == "replacement-1"
        ));
    }

    #[test]
    fn parses_ask_with_inline_message_and_defaults() {
        let cli = Cli::try_parse_from(["wardian", "ask", "reviewer-a1", "review this"]).unwrap();
        let Command::Ask(args) = cli.command else {
            panic!("expected Ask command")
        };
        assert_eq!(args.target, "reviewer-a1");
        assert_eq!(args.message.as_deref(), Some("review this"));
        assert!(!args.stdin);
        assert_eq!(args.file, None);
        assert_eq!(args.until.as_deref(), Some("reply"));
        assert_eq!(args.timeout, "10m");
        assert_eq!(args.tail, 65536);
    }

    #[test]
    fn parses_ask_with_explicit_additional_targets() {
        let cli = Cli::try_parse_from([
            "wardian",
            "ask",
            "reviewer-a1",
            "review this",
            "--targets",
            "reviewer-a2,reviewer-a3",
        ])
        .unwrap();
        let Command::Ask(args) = cli.command else {
            panic!("expected Ask command")
        };
        assert_eq!(args.targets, vec!["reviewer-a2", "reviewer-a3"]);
    }

    #[test]
    fn parses_ask_with_output_condition_and_stdin() {
        let cli = Cli::try_parse_from([
            "wardian",
            "ask",
            "reviewer-a1",
            "--stdin",
            "--until",
            "output:REVIEW_DONE",
            "--tail",
            "131072",
            "--timeout",
            "30s",
        ])
        .unwrap();
        let Command::Ask(args) = cli.command else {
            panic!("expected Ask command")
        };
        assert_eq!(args.target, "reviewer-a1");
        assert!(args.stdin);
        assert_eq!(args.until.as_deref(), Some("output:REVIEW_DONE"));
        assert_eq!(args.tail, 131072);
        assert_eq!(args.timeout, "30s");
    }

    #[test]
    fn parses_reply_with_done_status_and_stdin() {
        let cli = Cli::try_parse_from([
            "wardian",
            "reply",
            "ask_0123456789abcdef",
            "--status",
            "done",
            "--stdin",
        ])
        .unwrap();
        let Command::Reply(args) = cli.command else {
            panic!("expected Reply command")
        };
        assert_eq!(args.request_id, "ask_0123456789abcdef");
        assert_eq!(args.status, ReplyStatusArg::Done);
        assert!(args.stdin);
    }

    #[test]
    fn reply_rejects_unknown_status() {
        let err = Cli::try_parse_from([
            "wardian",
            "reply",
            "ask_0123456789abcdef",
            "--status",
            "waiting",
            "--stdin",
        ])
        .unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::InvalidValue);
    }

    #[test]
    fn ask_rejects_stdin_and_file_together() {
        let err = Cli::try_parse_from([
            "wardian",
            "ask",
            "reviewer-a1",
            "--stdin",
            "--file",
            "prompt.md",
        ])
        .unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    #[test]
    fn parses_output_modifiers_after_list_subcommand() {
        let cli = Cli::try_parse_from([
            "wardian",
            "agent",
            "list",
            "--scope",
            "all",
            "--fields",
            "name,status",
            "--pretty",
        ])
        .unwrap();
        let Command::Agent(args) = cli.command else {
            panic!("expected Agent command")
        };
        assert_eq!(args.fields.as_deref(), Some("name,status"));
        assert!(args.pretty);
    }

    #[test]
    fn parses_graph_show() {
        let cli = Cli::try_parse_from(["wardian", "graph", "show"]).unwrap();
        let Command::Graph(args) = cli.command else {
            panic!("expected Graph command")
        };
        assert!(matches!(args.command, GraphCommand::Show));
        assert!(!args.pretty);
    }

    #[test]
    fn parses_graph_neighbors_with_optional_agent() {
        let cli = Cli::try_parse_from(["wardian", "graph", "neighbors"]).unwrap();
        let Command::Graph(args) = cli.command else {
            panic!("expected Graph command")
        };
        assert!(matches!(
            args.command,
            GraphCommand::Neighbors { agent: None }
        ));

        let cli = Cli::try_parse_from(["wardian", "graph", "neighbors", "coder-a1"]).unwrap();
        let Command::Graph(args) = cli.command else {
            panic!("expected Graph command")
        };
        assert!(matches!(
            args.command,
            GraphCommand::Neighbors { ref agent } if agent.as_deref() == Some("coder-a1")
        ));
    }

    #[test]
    fn parses_graph_activity_with_pretty() {
        let cli = Cli::try_parse_from(["wardian", "graph", "activity", "--pretty"]).unwrap();
        let Command::Graph(args) = cli.command else {
            panic!("expected Graph command")
        };
        assert!(matches!(args.command, GraphCommand::Activity));
        assert!(args.pretty);
    }

    #[test]
    fn parses_graph_link_one_and_two_args() {
        let cli = Cli::try_parse_from(["wardian", "graph", "link", "architect-a1"]).unwrap();
        let Command::Graph(args) = cli.command else {
            panic!("expected Graph command")
        };
        assert!(matches!(
            args.command,
            GraphCommand::Link { ref a, b: None } if a == "architect-a1"
        ));

        let cli = Cli::try_parse_from(["wardian", "graph", "link", "uuid-1", "uuid-2"]).unwrap();
        let Command::Graph(args) = cli.command else {
            panic!("expected Graph command")
        };
        assert!(matches!(
            args.command,
            GraphCommand::Link { ref a, ref b } if a == "uuid-1" && b.as_deref() == Some("uuid-2")
        ));
    }

    #[test]
    fn parses_graph_unlink_ignore_unignore() {
        let cli = Cli::try_parse_from(["wardian", "graph", "unlink", "x", "y"]).unwrap();
        let Command::Graph(args) = cli.command else {
            panic!()
        };
        assert!(matches!(args.command, GraphCommand::Unlink { .. }));

        let cli = Cli::try_parse_from(["wardian", "graph", "ignore", "x"]).unwrap();
        let Command::Graph(args) = cli.command else {
            panic!()
        };
        assert!(matches!(args.command, GraphCommand::Ignore { .. }));

        let cli = Cli::try_parse_from(["wardian", "graph", "unignore", "x"]).unwrap();
        let Command::Graph(args) = cli.command else {
            panic!()
        };
        assert!(matches!(args.command, GraphCommand::Unignore { .. }));
    }
}
