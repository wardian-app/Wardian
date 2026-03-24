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
