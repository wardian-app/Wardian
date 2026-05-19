import { invoke } from '@tauri-apps/api/core';
import { create } from 'zustand';
import { persist } from 'zustand/middleware';
import type {
  CodexApprovalPolicy,
  CodexRuntimePolicy,
  CodexSandboxMode,
  DefaultProviderSetting,
  ShellOption,
  ShellSettings,
} from '../types/settings';

export const MIN_TERMINAL_FONT_SIZE = 10;
export const MAX_TERMINAL_FONT_SIZE = 20;
export const WINDOWS_TERMINAL_FONT_FAMILY = 'Consolas, "Courier New", monospace';
export const MACOS_TERMINAL_FONT_FAMILY = 'Menlo, Monaco, "Courier New", monospace';
export const LINUX_TERMINAL_FONT_FAMILY = '"Droid Sans Mono", monospace';

type TerminalPlatform = 'windows' | 'macos' | 'linux';

function detectTerminalPlatform(): TerminalPlatform {
  const platform = navigator.platform.toLowerCase();
  const userAgent = navigator.userAgent.toLowerCase();
  if (platform.includes('mac') || userAgent.includes('mac os')) {
    return 'macos';
  }
  if (platform.includes('win') || userAgent.includes('windows')) {
    return 'windows';
  }
  return 'linux';
}

export function defaultTerminalFontSize(platform: TerminalPlatform = detectTerminalPlatform()) {
  return platform === 'macos' ? 12 : 14;
}

export function defaultTerminalFontFamily(platform: TerminalPlatform = detectTerminalPlatform()) {
  switch (platform) {
    case 'macos':
      return MACOS_TERMINAL_FONT_FAMILY;
    case 'windows':
      return WINDOWS_TERMINAL_FONT_FAMILY;
    default:
      return LINUX_TERMINAL_FONT_FAMILY;
  }
}

export function normalizeTerminalFontSize(value: number) {
  if (!Number.isFinite(value)) {
    return defaultTerminalFontSize();
  }
  return Math.min(MAX_TERMINAL_FONT_SIZE, Math.max(MIN_TERMINAL_FONT_SIZE, Math.round(value)));
}

export function effectiveTerminalFontFamily(value: string | null | undefined) {
  const trimmed = value?.trim() ?? '';
  return trimmed || defaultTerminalFontFamily();
}

const CODEX_SANDBOX_MODES: CodexSandboxMode[] = ['read-only', 'workspace-write', 'danger-full-access'];
const CODEX_APPROVAL_POLICIES: CodexApprovalPolicy[] = ['untrusted', 'on-failure', 'on-request', 'never'];
const DEFAULT_PROVIDER_SETTINGS: DefaultProviderSetting[] = ['auto', 'claude', 'codex', 'gemini', 'opencode'];

export const DEFAULT_CODEX_RUNTIME_POLICY: CodexRuntimePolicy = {
  sandbox_mode: 'danger-full-access',
  approval_policy: 'never',
  full_auto: true,
};

export function normalizeCodexRuntimePolicy(
  policy: Partial<CodexRuntimePolicy> | null | undefined,
): CodexRuntimePolicy {
  const sandboxMode = policy?.sandbox_mode;
  const approvalPolicy = policy?.approval_policy;
  return {
    sandbox_mode: CODEX_SANDBOX_MODES.includes(sandboxMode as CodexSandboxMode)
      ? sandboxMode as CodexSandboxMode
      : DEFAULT_CODEX_RUNTIME_POLICY.sandbox_mode,
    approval_policy: CODEX_APPROVAL_POLICIES.includes(approvalPolicy as CodexApprovalPolicy)
      ? approvalPolicy as CodexApprovalPolicy
      : DEFAULT_CODEX_RUNTIME_POLICY.approval_policy,
    full_auto: typeof policy?.full_auto === 'boolean'
      ? policy.full_auto
      : DEFAULT_CODEX_RUNTIME_POLICY.full_auto,
  };
}

export function normalizeDefaultProviderSetting(
  value: string | null | undefined,
): DefaultProviderSetting {
  return DEFAULT_PROVIDER_SETTINGS.includes(value as DefaultProviderSetting)
    ? value as DefaultProviderSetting
    : 'auto';
}

interface SettingsState {
  theme: 'dark' | 'light' | 'system';
  autoPatchGemini: boolean;
  terminalFontSize: number;
  terminalFontFamily: string;
  shell_id: string;
  custom_executable: string;
  custom_args: string;
  agent_session_persistence: 'fresh' | 'resume';
  codex_runtime_policy: CodexRuntimePolicy;
  default_provider: DefaultProviderSetting;
  available_shells: ShellOption[];
  shell_settings_loaded: boolean;
  shells_loaded: boolean;
  setTheme: (theme: 'dark' | 'light' | 'system') => void;
  setAutoPatchGemini: (enabled: boolean) => void;
  setTerminalFontSize: (value: number) => void;
  setTerminalFontFamily: (value: string) => void;
  setShellId: (shellId: string) => void;
  setCustomExecutable: (value: string) => void;
  setCustomArgs: (value: string) => void;
  setAgentSessionPersistence: (value: 'fresh' | 'resume') => void;
  setDefaultProvider: (value: DefaultProviderSetting) => void;
  setCodexSandboxMode: (value: CodexSandboxMode) => void;
  setCodexApprovalPolicy: (value: CodexApprovalPolicy) => void;
  setCodexFullAuto: (value: boolean) => void;
  loadShellSettings: () => Promise<void>;
  loadAvailableShells: () => Promise<void>;
  saveShellSettings: () => Promise<void>;
  saveAgentSessionPersistence: () => Promise<void>;
}

const DEFAULT_SHELL_SETTINGS: ShellSettings = {
  shell_id: 'auto',
  custom_executable: null,
  custom_args: null,
  agent_session_persistence: 'resume',
  codex_runtime_policy: DEFAULT_CODEX_RUNTIME_POLICY,
  default_provider: 'auto',
};

export const useSettingsStore = create<SettingsState>()(
  persist(
    (set, get) => ({
      theme: 'system',
      autoPatchGemini: false,
      terminalFontSize: defaultTerminalFontSize(),
      terminalFontFamily: '',
      shell_id: DEFAULT_SHELL_SETTINGS.shell_id,
      custom_executable: DEFAULT_SHELL_SETTINGS.custom_executable ?? '',
      custom_args: DEFAULT_SHELL_SETTINGS.custom_args ?? '',
      agent_session_persistence: DEFAULT_SHELL_SETTINGS.agent_session_persistence,
      codex_runtime_policy: DEFAULT_CODEX_RUNTIME_POLICY,
      default_provider: DEFAULT_SHELL_SETTINGS.default_provider ?? 'auto',
      available_shells: [],
      shell_settings_loaded: false,
      shells_loaded: false,
      setTheme: (theme) => set({ theme }),
      setAutoPatchGemini: (autoPatchGemini) => set({ autoPatchGemini }),
      setTerminalFontSize: (terminalFontSize) => set({
        terminalFontSize: normalizeTerminalFontSize(terminalFontSize),
      }),
      setTerminalFontFamily: (terminalFontFamily) => set({ terminalFontFamily }),
      setShellId: (shell_id) => set({ shell_id }),
      setCustomExecutable: (custom_executable) => set({ custom_executable }),
      setCustomArgs: (custom_args) => set({ custom_args }),
      setAgentSessionPersistence: (agent_session_persistence) => set({ agent_session_persistence }),
      setDefaultProvider: (default_provider) => set({
        default_provider: normalizeDefaultProviderSetting(default_provider),
      }),
      setCodexSandboxMode: (sandbox_mode) => set((state) => ({
        codex_runtime_policy: { ...state.codex_runtime_policy, sandbox_mode },
      })),
      setCodexApprovalPolicy: (approval_policy) => set((state) => ({
        codex_runtime_policy: { ...state.codex_runtime_policy, approval_policy },
      })),
      setCodexFullAuto: (full_auto) => set((state) => ({
        codex_runtime_policy: { ...state.codex_runtime_policy, full_auto },
      })),
      loadShellSettings: async () => {
        try {
          const settings = await invoke<ShellSettings>('load_shell_settings');
          set({
            shell_id: settings.shell_id,
            custom_executable: settings.custom_executable ?? '',
            custom_args: settings.custom_args ?? '',
            agent_session_persistence: settings.agent_session_persistence ?? DEFAULT_SHELL_SETTINGS.agent_session_persistence,
            codex_runtime_policy: normalizeCodexRuntimePolicy(settings.codex_runtime_policy),
            default_provider: normalizeDefaultProviderSetting(settings.default_provider),
            shell_settings_loaded: true,
          });
        } catch (error) {
          console.error('Failed to load shell settings:', error);
          set({
            shell_id: DEFAULT_SHELL_SETTINGS.shell_id,
            custom_executable: '',
            custom_args: '',
            agent_session_persistence: DEFAULT_SHELL_SETTINGS.agent_session_persistence,
            codex_runtime_policy: DEFAULT_CODEX_RUNTIME_POLICY,
            default_provider: DEFAULT_SHELL_SETTINGS.default_provider ?? 'auto',
            shell_settings_loaded: true,
          });
        }
      },
      loadAvailableShells: async () => {
        try {
          const available_shells = await invoke<ShellOption[]>('list_available_shells');
          set({ available_shells, shells_loaded: true });
        } catch (error) {
          console.error('Failed to load available shells:', error);
          set({ available_shells: [], shells_loaded: true });
        }
      },
      saveShellSettings: async () => {
        const settings: ShellSettings = {
          shell_id: get().shell_id,
          custom_executable: get().custom_executable.trim() || null,
          custom_args: get().custom_args.trim() || null,
          agent_session_persistence: get().agent_session_persistence,
          codex_runtime_policy: normalizeCodexRuntimePolicy(get().codex_runtime_policy),
          default_provider: normalizeDefaultProviderSetting(get().default_provider),
        };
        const saved = await invoke<ShellSettings>('save_shell_settings', { settings });
        set({
          shell_id: saved.shell_id,
          custom_executable: saved.custom_executable ?? '',
          custom_args: saved.custom_args ?? '',
          agent_session_persistence: saved.agent_session_persistence ?? DEFAULT_SHELL_SETTINGS.agent_session_persistence,
          codex_runtime_policy: normalizeCodexRuntimePolicy(saved.codex_runtime_policy ?? settings.codex_runtime_policy),
          default_provider: normalizeDefaultProviderSetting(saved.default_provider ?? settings.default_provider),
          shell_settings_loaded: true,
        });
      },
      saveAgentSessionPersistence: async () => {
        const saved = await invoke<ShellSettings>('save_agent_session_persistence', {
          persistence: get().agent_session_persistence,
        });
        set({
          shell_id: saved.shell_id,
          custom_executable: saved.custom_executable ?? '',
          custom_args: saved.custom_args ?? '',
          agent_session_persistence: saved.agent_session_persistence ?? DEFAULT_SHELL_SETTINGS.agent_session_persistence,
          codex_runtime_policy: normalizeCodexRuntimePolicy(saved.codex_runtime_policy ?? get().codex_runtime_policy),
          default_provider: normalizeDefaultProviderSetting(saved.default_provider ?? get().default_provider),
          shell_settings_loaded: true,
        });
      },
    }),
    {
      name: 'wardian-settings',
      partialize: (state) => ({
        theme: state.theme,
        autoPatchGemini: state.autoPatchGemini,
        terminalFontSize: normalizeTerminalFontSize(state.terminalFontSize),
        terminalFontFamily: state.terminalFontFamily.trim(),
      }),
    }
  )
);
