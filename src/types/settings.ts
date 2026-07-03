export interface ShellOption {
  id: string;
  label: string;
  executable: string;
  default_args: string[];
}

export type CodexSandboxMode = 'read-only' | 'workspace-write' | 'danger-full-access';
export type CodexApprovalPolicy = 'untrusted' | 'on-failure' | 'on-request' | 'never';
export type DefaultProviderSetting = 'auto' | 'claude' | 'codex' | 'gemini' | 'antigravity' | 'opencode';
export type ConversationLoggingSetting = 'enabled' | 'disabled';
export type AppThemeSetting = 'dark' | 'light' | 'system';
export type WatchlistNewAgentPosition = 'top' | 'bottom';
export type ExternalEditorSetting = 'system' | 'vscode' | 'custom';
export type ExplorerFileClickAction = 'preview' | 'external';

export interface CodexRuntimePolicy {
  sandbox_mode: CodexSandboxMode;
  approval_policy: CodexApprovalPolicy;
  full_auto: boolean;
  trust_workspaces: boolean;
}

export interface CodexRuntimePolicyOverrides {
  sandbox_mode?: CodexSandboxMode;
  approval_policy?: CodexApprovalPolicy;
  full_auto?: boolean;
  trust_workspaces?: boolean;
}

export interface ShellSettings {
  shell_id: string;
  custom_executable: string | null;
  custom_args: string | null;
  agent_session_persistence: 'fresh' | 'resume';
  codex_runtime_policy?: CodexRuntimePolicy;
  default_provider?: DefaultProviderSetting;
  conversation_logging?: ConversationLoggingSetting;
}

export interface ShellSettingsOverrides {
  shell_id?: string;
  custom_executable?: string | null;
  custom_args?: string | null;
  agent_session_persistence?: 'fresh' | 'resume';
  codex_runtime_policy?: CodexRuntimePolicyOverrides;
  default_provider?: DefaultProviderSetting;
  conversation_logging?: ConversationLoggingSetting;
}

export interface AppSettings {
  theme: AppThemeSetting;
  auto_patch_gemini: boolean;
  terminal_font_size: number;
  terminal_font_family: string | null;
  grid_card_display_mode: 'terminal' | 'chat';
  watchlist_new_agent_position: WatchlistNewAgentPosition;
  titlebar_telemetry_visible: boolean;
  external_editor: ExternalEditorSetting;
  external_editor_custom_executable: string | null;
  explorer_file_click_action: ExplorerFileClickAction;
}

export interface AppSettingsOverrides {
  theme?: AppThemeSetting;
  auto_patch_gemini?: boolean;
  terminal_font_size?: number;
  terminal_font_family?: string | null;
  grid_card_display_mode?: 'terminal' | 'chat';
  watchlist_new_agent_position?: WatchlistNewAgentPosition;
  titlebar_telemetry_visible?: boolean;
  external_editor?: ExternalEditorSetting;
  external_editor_custom_executable?: string | null;
  explorer_file_click_action?: ExplorerFileClickAction;
}

export interface SettingsDocument<TSettings, TOverrides> {
  schema_version: 2;
  settings: TSettings;
  overrides: TOverrides;
  persisted?: boolean;
}
