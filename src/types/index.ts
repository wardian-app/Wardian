export type ProviderName = "claude" | "codex" | "gemini" | "antigravity" | "opencode" | "mock";
export type UserFacingProviderName = "claude" | "codex" | "gemini" | "antigravity" | "opencode";

export type GridCardDisplayMode = "terminal" | "chat";

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
    files: GitFileEntry[];
    ahead: number;
    behind: number;
}

export interface GitLogEntry {
    hash: string;
    parents: string[];
    refs: string[];
    message: string;
    author: string;
    author_email: string;
    date: string;
}

export interface AgentWorktreeSummary {
    id: string;
    name: string;
    source_folder: string;
    worktree_folder: string;
    member_agent_ids: string[];
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

export interface QueueItem {
    id: string;
    type: "agent_completed" | "workflow_completed";
    timestamp: number;
    read: boolean;
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

export interface LibrarySkill {
    type: 'Skill';
    path: string;
    name: string;
    description: string;
    content: string;
    metadata: LibraryItemMetadata;
}

export interface LibraryPrompt {
    type: 'Prompt';
    path: string;
    name: string;
    content: string;
    metadata: LibraryItemMetadata;
}

export interface LibraryFolder {
    type: 'Folder';
    path: string;
    name: string;
    children: (LibraryFolder | LibraryPrompt | LibrarySkill)[];
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
