export interface AgentConfig {
    session_id: string;
    session_name: string;
    agent_class: string;
    folder: string;
    resume_session?: string;
    is_off: boolean;
    provider?: string;
    debug?: boolean;
    model?: string;
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

    // OpenCode-specific fields
    opencode_agent?: string;
    opencode_port?: number;

    // Git isolation
    git_worktree?: boolean;

}

export interface GitFileEntry {
    path: string;
    status: string;
    is_staged: boolean;
}

export interface GitStatusResult {
    branch: string;
    files: GitFileEntry[];
    ahead: number;
    behind: number;
}

export interface GitLogEntry {
    hash: string;
    message: string;
    author: string;
    date: string;
}

export * from "./workflow";

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
