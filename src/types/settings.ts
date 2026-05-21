export interface ShellOption {
  id: string;
  label: string;
  executable: string;
  default_args: string[];
}

export type CodexSandboxMode = 'read-only' | 'workspace-write' | 'danger-full-access';
export type CodexApprovalPolicy = 'untrusted' | 'on-failure' | 'on-request' | 'never';
export type DefaultProviderSetting = 'auto' | 'claude' | 'codex' | 'gemini' | 'antigravity' | 'opencode';
export type AppThemeSetting = 'dark' | 'light' | 'system';

export interface CodexRuntimePolicy {
  sandbox_mode: CodexSandboxMode;
  approval_policy: CodexApprovalPolicy;
  full_auto: boolean;
}

export interface ShellSettings {
  shell_id: string;
  custom_executable: string | null;
  custom_args: string | null;
  agent_session_persistence: 'fresh' | 'resume';
  codex_runtime_policy?: CodexRuntimePolicy;
  default_provider?: DefaultProviderSetting;
}

export interface AppSettings {
  theme: AppThemeSetting;
  auto_patch_gemini: boolean;
  terminal_font_size: number;
  terminal_font_family: string | null;
}
