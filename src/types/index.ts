export type ProviderName = "claude" | "codex" | "gemini" | "antigravity" | "opencode" | "mock";
export type UserFacingProviderName = "claude" | "codex" | "gemini" | "antigravity" | "opencode";

export type GridCardDisplayMode = "terminal" | "chat";

export interface PromptDeliveryDetail {
    uuid: string;
    name: string;
    provider: string;
    runtime_state: string;
    delivery_state: string;
    input_mode?: "message" | "command" | "approval_action";
    queue_policy?: "queue_if_busy" | "live_only" | "mailbox_only";
    message_id?: string;
    delivery_phase?: string;
    observed_state?: string;
    reason?: string;
    profile?: string;
    error?: {
        code: string;
        message: string;
    };
}

export type AgentChatEventKind =
    | "message"
    | "tool_call"
    | "tool_result"
    | "approval"
    | "status"
    | "terminal_output"
    | "error";

export type AgentChatRole = "user" | "assistant" | "system" | "tool";

export type AgentChatStatus =
    | "running"
    | "succeeded"
    | "failed"
    | "action_required"
    | "cancelled"
    | "idle"
    | "processing"
    | "unknown";

export interface AgentChatEvent {
    id: string;
    session_id: string;
    provider: string;
    kind: AgentChatEventKind;
    role: AgentChatRole | null;
    text: string | null;
    title: string | null;
    status: AgentChatStatus | null;
    turn_id: string | null;
    source: string | null;
    command: string | null;
    exit_code: number | null;
    path: string | null;
    language: string | null;
    created_at: string | null;
    sequence: number | null;
    metadata: Record<string, unknown>;
}

export interface ProviderReadiness {
    provider: UserFacingProviderName;
    display_name: string;
    available: boolean;
    executable: string | null;
    reason: string | null;
}

export interface ClaudeProviderConfig {
    type: "claude";
    permission_mode?: "default" | "plan" | "auto-accept";
    max_turns?: number;
    allowed_tools?: string[];
    disallowed_tools?: string[];
    append_system_prompt?: string;
    mcp_config?: string;
}

export interface GeminiProviderConfig {
    type: "gemini";
    sandbox?: boolean;
    yolo?: boolean;
    approval_mode?: "default" | "auto_edit" | "yolo" | "plan" | string;
    policy?: string[];
    experimental_acp?: boolean;
    allowed_mcp_server_names?: string[];
    extensions?: string[];
    screen_reader?: boolean;
    output_format?: "text" | "json" | "stream-json";
}

export interface CodexProviderConfig {
    type: "codex";
    sandbox_mode?: "read-only" | "workspace-write" | "danger-full-access";
    approval_policy?: "untrusted" | "on-failure" | "on-request" | "never";
    profile?: string;
    full_auto?: boolean;
    search?: boolean;
    skip_git_repo_check?: boolean;
    ephemeral?: boolean;
    cleared_provider_sessions?: string[];
}

export interface AntigravityProviderConfig {
    type: "antigravity";
    sandbox?: boolean;
    dangerously_skip_permissions?: boolean;
    print_timeout?: string;
}

export interface OpenCodeProviderConfig {
    type: "opencode";
    agent?: string;
    port?: number;
}

export interface MockProviderConfig {
    type: "mock";
}

export interface UnknownProviderConfig {
    type: string;
    [key: string]: unknown;
}

export type ProviderConfig =
    | ClaudeProviderConfig
    | GeminiProviderConfig
    | CodexProviderConfig
    | AntigravityProviderConfig
    | OpenCodeProviderConfig
    | MockProviderConfig;

export interface AgentConfig {
    session_id: string;
    session_name: string;
    agent_class: string;
    folder: string;
    resume_session?: string;
    is_off: boolean;
    provider?: ProviderName | string;
    debug?: boolean;
    model?: string;
    provider_config?: ProviderConfig | UnknownProviderConfig;

    // Legacy flat provider fields accepted from older persisted configs.
    sandbox?: boolean;
    yolo?: boolean;
    approval_mode?: "default" | "auto_edit" | "yolo" | "plan";
    policy?: string[];
    experimental_acp?: boolean;
    allowed_mcp_server_names?: string[];
    extensions?: string[];
    include_directories?: string[];
    system_include_directories?: string[];
    screen_reader?: boolean;
    output_format?: "text" | "json" | "stream-json";
    custom_args?: string;
    session_persistence?: "default" | "fresh" | "resume";
    conversation_logging?: "default" | "enabled" | "disabled";

    // Claude-specific fields
    permission_mode?: "default" | "plan" | "auto-accept";
    max_turns?: number;
    allowed_tools?: string[];
    disallowed_tools?: string[];
    append_system_prompt?: string;
    mcp_config?: string;

    // Codex-specific fields
    codex_sandbox_mode?: "read-only" | "workspace-write" | "danger-full-access";
    codex_approval_policy?: "untrusted" | "on-failure" | "on-request" | "never";
    codex_profile?: string;
    codex_full_auto?: boolean;
    codex_search?: boolean;
    codex_skip_git_repo_check?: boolean;
    codex_ephemeral?: boolean;
    codex_cleared_provider_sessions?: string[];

    // OpenCode-specific fields
    opencode_agent?: string;
    opencode_port?: number;

    // Git isolation
    git_worktree?: boolean;
    git_worktree_source?: string;
    git_worktree_folder?: string;

}

export interface GitFileEntry {
    path: string;
    status: string;
    is_staged: boolean;
}

export interface GitStatusResult {
    branch: string;
    upstream: string | null;
    has_upstream: boolean;
    files: GitFileEntry[];
    ahead: number;
    behind: number;
    rebase_in_progress?: boolean;
}

export interface GitLogEntry {
    hash: string;
    message: string;
    author: string;
    date: string;
    parent_hashes?: string[];
    refs?: string[];
}

export interface GitBranchSummary {
    name: string;
    current: boolean;
}

export interface GitCommitChangeEntry {
    path: string;
    status: string;
}

export interface GitStashEntry {
    selector: string;
    message: string;
}

export interface AgentWorktreeSummary {
    id: string;
    name: string;
    source_folder: string;
    worktree_folder: string;
    member_agent_ids: string[];
    can_delete: boolean;
}

export * from "./workflow";
export * from "./remote";

export interface AgentOutputPayload {
    session_id: string;
    text: string;
    stream: "stdout" | "stderr";
}

export interface AgentJsonEvent {
    session_id: string;
    data: any;
}

export interface AgentStatusUpdate {
    session_id: string;
    current_status: string;
}

export type QueueEventType = "action_needed" | "agent_completed" | "workflow_completed" | "workflow_failed";

export interface QueuePreferences {
    visible_event_types: Record<QueueEventType, boolean>;
    desktop_notifications: Record<QueueEventType, boolean>;
    sound_notifications: Record<QueueEventType, boolean>;
    sound_volume: number;
}

export interface QueueItem {
    id: string;
    type: "action_needed" | "agent_completed" | "workflow_completed";
    timestamp: number;
    read: boolean;
    evidence_id?: string;
    evidence_source?: "provider_runtime" | "interaction_store" | "live_runtime";
    // agent fields
    agent_session_id?: string;
    agent_name?: string;
    // workflow fields
    workflow_id?: string;
    workflow_run_id?: string;
    workflow_name?: string;
    status?: "completed" | "failed";
    error?: string;
    // shared
    summary?: string;
}

export interface GridLayout {
    column_tracks: number[]; // Relative weights (e.g. [1, 1] for 50/50)
    row_height: number;      // Fixed height for all rows in pixels
}

export interface TopologyEdgeDto {
  a: string;
  b: string;
  origin: string; // "manual" | "rule:<rule-id>:<instance>"
}

export interface TopologySnapshot {
  edges: TopologyEdgeDto[];
  ignored_pairs: [string, string][];
  fallback_groups: string[][];
}

export interface PairActivityEntry {
  a: string;
  b: string;
  last_message_at: string;
  active_ask: boolean;
  awaiting_reply_from: string | null;
}

export interface AgentTelemetry {
    session_id: string;
    cpu_usage: number;
    memory_mb: number;
    uptime_seconds: number;
    query_count: number;
    init_timestamp: string | null;
    current_status: string;
    log_path: string | null;
}

export interface AppTelemetry {
    cpu_usage: number;
    memory_mb: number;
}

export interface AgentClassDefinition {
    name: string;
    description: string;
    is_default: boolean;
    instruction_content?: string;
    assigned_skills?: string[];
}

export interface LibraryItemMetadata {
    id: string;
    tags: string[];
    is_starred: boolean;
    last_used?: string;
}

// --- Unified library index (Task 5 DTOs) -----------------------------------
// Mirrors wardian_core::models::library and wardian_core::library::deployments
// exactly; property names are snake_case to match Rust serde output directly.

export type LibrarySectionId = 'skills' | 'prompts' | 'workflows' | 'classes' | 'mcps';
export type LibraryEntryKind = 'skill' | 'prompt' | 'workflow' | 'class';

export interface LibraryEntry {
    kind: LibraryEntryKind;
    path: string;
    entry_ref: string;
    name: string;
    description: string;
    tags: string[];
    is_starred: boolean;
    deployment_count: number;
    error?: string | null;
}

export interface LibraryIndexFolder {
    path: string;
    name: string;
    children: (LibraryIndexFolder | LibraryEntry)[];
}

export interface LibrarySection {
    tree: LibraryIndexFolder;
    stubbed: boolean;
}

export interface DeploymentTarget {
    target_type: 'user' | 'class' | 'agent';
    target_id: string;
    linked: boolean;
}

export interface OrphanDeployment {
    target_type: string;
    target_id: string;
    skill_name: string;
}

export interface SkillDeployment {
    target_type: string;
    target_id: string;
}

export interface LibraryIndex {
    sections: Record<LibrarySectionId, LibrarySection>;
    deployments: Record<string, DeploymentTarget[]>;
    orphans: OrphanDeployment[];
}

export function isLibraryEntry(node: LibraryIndexFolder | LibraryEntry): node is LibraryEntry {
    return 'entry_ref' in node;
}

export interface DeployedSkillRef {
    name: string;
    source_path?: string | null;
}

export type CloneMode = "fresh" | "profile" | "custom";

export interface CloneFileTreeNode {
    name: string;
    path: string;
    kind: "file" | "directory";
    children: CloneFileTreeNode[];
}

export interface CloneProfileSelection {
    files: string[];
    skills: DeployedSkillRef[];
}

export interface AgentClonePreview {
    source_session_id: string;
    source_session_name: string;
    suggested_session_name: string;
    provider: string;
    agent_class: string;
    folder: string;
    files: CloneFileTreeNode;
    default_selected_files: string[];
    skills: DeployedSkillRef[];
    default_selected_skills: DeployedSkillRef[];
}

// --- Canonical workbench V1 DTOs ------------------------------------------

export type WorkbenchDocumentV1 = {
    schema_version: 1;
    revision: number;
    saved_at: string;
    root: WorkbenchNodeV1;
    groups: Record<string, WorkbenchGroupV1>;
    surfaces: Record<string, WorkbenchSurfaceV1>;
    active_group_id: string;
    recently_closed: ClosedSurfaceV1[];
    shell: WorkbenchShellV1;
};

export type WorkbenchNodeV1 =
    | { kind: "group"; group_id: string }
    | {
        kind: "split";
        node_id: string;
        direction: "horizontal" | "vertical";
        ratio: number;
        first: WorkbenchNodeV1;
        second: WorkbenchNodeV1;
    };

export type WorkbenchGroupV1 = {
    group_id: string;
    surface_ids: string[];
    active_surface_id: string | null;
};

export type WorkbenchSurfaceV1 = {
    surface_id: string;
    surface_type: string;
    resource_key?: string;
    state_schema_version: number;
    state: unknown;
};

export type ClosedSurfaceV1 = {
    surface: WorkbenchSurfaceV1;
    previous_group_id: string;
    previous_index: number;
};

export type WorkbenchShellV1 = {
    left_sidebar_collapsed: boolean;
    left_sidebar_width: number;
    right_sidebar_collapsed: boolean;
    right_sidebar_width: number;
    bottom_terminal_open: boolean;
    bottom_terminal_height: number;
};

export type WorkbenchValidationError = {
    path: string;
    message: string;
};

export type WorkbenchValidationResult =
    | {
        valid: true;
        document: WorkbenchDocumentV1;
    }
    | {
        valid: false;
        errors: WorkbenchValidationError[];
    };

export type WorkbenchCommandResult =
    | {
        accepted: true;
        document: WorkbenchDocumentV1;
    }
    | {
        accepted: false;
        document: WorkbenchDocumentV1;
        errors: WorkbenchValidationError[];
    };

// --- Workbench surface registry contracts ---------------------------------

export type SurfaceState = unknown;
export type SurfaceType = string;
export type SurfaceIcon = string;

export type SurfaceRenderPolicy =
    | "keep_alive"
    | "suspend_when_hidden"
    | "recreate_from_state";

export type SurfaceOpenPolicy = "singleton" | "focus_resource" | "allow_multiple";
export type SurfaceRuntimePolicy = "view_only" | "runtime_backed";
export type SurfaceClosePolicy = "close_view" | "confirm_if_dirty";
export type CloseDecision = "allow" | "cancel";

export type OpenSurfaceRequest = {
    surface_type: SurfaceType;
    resource_key?: string;
    state?: SurfaceState;
    group_id?: string;
    duplicate?: boolean;
};

export type SurfaceRestoreResult<TState extends SurfaceState = SurfaceState> =
    | { ok: true; state: TState }
    | { ok: false; error: string };

export type SerializedSurfaceState = {
    state_schema_version: number;
    state: unknown;
};

export type SurfaceCommandDefinition = {
    command_id: string;
    title: string;
    accessibility_label?: string;
};

export type SurfaceBadge = {
    badge_id: string;
    label: string;
};

export type SurfacePresentationMetadata = {
    title: string;
    icon: SurfaceIcon;
    commands: readonly SurfaceCommandDefinition[];
    badges: readonly SurfaceBadge[];
};

export type SurfaceDefinition<TState extends SurfaceState = SurfaceState> = {
    type: SurfaceType;
    title: (surface: WorkbenchSurfaceV1) => string;
    icon: SurfaceIcon;
    render_policy: SurfaceRenderPolicy;
    open_policy: SurfaceOpenPolicy;
    runtime_policy: SurfaceRuntimePolicy;
    close_policy: SurfaceClosePolicy;
    state_schema_version: number;
    max_state_bytes: number;
    resource_key?: (request: OpenSurfaceRequest) => string | undefined;
    resolve_existing?: (
        request: OpenSurfaceRequest,
        candidates: WorkbenchSurfaceV1[],
    ) => string | undefined;
    default_state: () => TState;
    serialize_state: (state: TState) => unknown;
    restore_state: (value: unknown, version: number) => SurfaceRestoreResult<TState>;
    can_close?: (surface: WorkbenchSurfaceV1) => Promise<CloseDecision> | CloseDecision;
    commands: SurfaceCommandDefinition[];
    badges?: (surface: WorkbenchSurfaceV1) => SurfaceBadge[];
};
