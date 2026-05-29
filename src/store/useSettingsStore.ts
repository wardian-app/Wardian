import { invoke } from '@tauri-apps/api/core';
import { create } from 'zustand';
import { persist } from 'zustand/middleware';
import type {
  AppSettings,
  AppSettingsOverrides,
  AppThemeSetting,
  CodexApprovalPolicy,
  CodexRuntimePolicyOverrides,
  CodexRuntimePolicy,
  CodexSandboxMode,
  DefaultProviderSetting,
  ExternalEditorSetting,
  SettingsDocument,
  ShellOption,
  ShellSettings,
  ShellSettingsOverrides,
  WatchlistNewAgentPosition,
} from '../types/settings';
import type { GridCardDisplayMode } from '../types';

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

function normalizeTheme(value: string | null | undefined): AppThemeSetting {
  return value === 'dark' || value === 'light' || value === 'system' ? value : 'system';
}

export function effectiveTerminalFontFamily(value: string | null | undefined) {
  const trimmed = value?.trim() ?? '';
  return trimmed || defaultTerminalFontFamily();
}

const CODEX_SANDBOX_MODES: CodexSandboxMode[] = ['read-only', 'workspace-write', 'danger-full-access'];
const CODEX_APPROVAL_POLICIES: CodexApprovalPolicy[] = ['untrusted', 'on-failure', 'on-request', 'never'];
const DEFAULT_PROVIDER_SETTINGS: DefaultProviderSetting[] = ['auto', 'claude', 'codex', 'gemini', 'antigravity', 'opencode'];
const GRID_CARD_DISPLAY_MODES: GridCardDisplayMode[] = ['terminal', 'chat'];
const WATCHLIST_NEW_AGENT_POSITIONS: WatchlistNewAgentPosition[] = ['top', 'bottom'];
const EXTERNAL_EDITOR_SETTINGS: ExternalEditorSetting[] = ['system', 'vscode', 'custom'];

export const DEFAULT_CODEX_RUNTIME_POLICY: CodexRuntimePolicy = {
  sandbox_mode: 'workspace-write',
  approval_policy: 'on-request',
  full_auto: false,
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

export function normalizeGridCardDisplayMode(
  value: string | null | undefined,
): GridCardDisplayMode {
  return GRID_CARD_DISPLAY_MODES.includes(value as GridCardDisplayMode)
    ? value as GridCardDisplayMode
    : 'terminal';
}

export function normalizeWatchlistNewAgentPosition(
  value: string | null | undefined,
): WatchlistNewAgentPosition {
  return WATCHLIST_NEW_AGENT_POSITIONS.includes(value as WatchlistNewAgentPosition)
    ? value as WatchlistNewAgentPosition
    : 'top';
}

export function normalizeExternalEditorSetting(
  value: string | null | undefined,
): ExternalEditorSetting {
  return EXTERNAL_EDITOR_SETTINGS.includes(value as ExternalEditorSetting)
    ? value as ExternalEditorSetting
    : 'system';
}

interface SettingsState {
  theme: AppThemeSetting;
  autoPatchGemini: boolean;
  terminalFontSize: number;
  terminalFontFamily: string;
  gridCardDisplayMode: GridCardDisplayMode;
  watchlistNewAgentPosition: WatchlistNewAgentPosition;
  titlebarTelemetryVisible: boolean;
  externalEditor: ExternalEditorSetting;
  externalEditorCustomExecutable: string;
  shell_id: string;
  custom_executable: string;
  custom_args: string;
  agent_session_persistence: 'fresh' | 'resume';
  codex_runtime_policy: CodexRuntimePolicy;
  default_provider: DefaultProviderSetting;
  app_settings_overrides: AppSettingsOverrides;
  shell_settings_overrides: ShellSettingsOverrides;
  available_shells: ShellOption[];
  app_settings_loaded: boolean;
  shell_settings_loaded: boolean;
  shells_loaded: boolean;
  setTheme: (theme: AppThemeSetting) => void;
  setAutoPatchGemini: (enabled: boolean) => void;
  setTerminalFontSize: (value: number) => void;
  setTerminalFontFamily: (value: string) => void;
  setGridCardDisplayMode: (value: GridCardDisplayMode) => void;
  setWatchlistNewAgentPosition: (value: WatchlistNewAgentPosition) => void;
  setTitlebarTelemetryVisible: (value: boolean) => void;
  setExternalEditor: (value: ExternalEditorSetting) => void;
  setExternalEditorCustomExecutable: (value: string) => void;
  setShellId: (shellId: string) => void;
  setCustomExecutable: (value: string) => void;
  setCustomArgs: (value: string) => void;
  setAgentSessionPersistence: (value: 'fresh' | 'resume') => void;
  setDefaultProvider: (value: DefaultProviderSetting) => void;
  setCodexSandboxMode: (value: CodexSandboxMode) => void;
  setCodexApprovalPolicy: (value: CodexApprovalPolicy) => void;
  setCodexFullAuto: (value: boolean) => void;
  loadAppSettings: () => Promise<void>;
  saveAppSettings: () => Promise<void>;
  loadShellSettings: () => Promise<void>;
  loadAvailableShells: () => Promise<void>;
  saveShellSettings: () => Promise<void>;
  saveAgentSessionPersistence: () => Promise<void>;
}

type PersistedSettingsState = Pick<
  SettingsState,
  | 'theme'
  | 'autoPatchGemini'
  | 'terminalFontSize'
  | 'terminalFontFamily'
  | 'gridCardDisplayMode'
  | 'watchlistNewAgentPosition'
  | 'titlebarTelemetryVisible'
  | 'externalEditor'
  | 'externalEditorCustomExecutable'
>;

const DEFAULT_SHELL_SETTINGS: ShellSettings = {
  shell_id: 'auto',
  custom_executable: null,
  custom_args: null,
  agent_session_persistence: 'resume',
  codex_runtime_policy: DEFAULT_CODEX_RUNTIME_POLICY,
  default_provider: 'auto',
};

const DEFAULT_APP_SETTINGS: AppSettings = {
  theme: 'system',
  auto_patch_gemini: false,
  terminal_font_size: defaultTerminalFontSize(),
  terminal_font_family: null,
  grid_card_display_mode: 'terminal',
  watchlist_new_agent_position: 'top',
  titlebar_telemetry_visible: true,
  external_editor: 'system',
  external_editor_custom_executable: null,
};

const EMPTY_APP_SETTINGS_OVERRIDES: AppSettingsOverrides = {};
const EMPTY_SHELL_SETTINGS_OVERRIDES: ShellSettingsOverrides = {};

type AppSettingsResponse = AppSettings | SettingsDocument<AppSettings, AppSettingsOverrides> | null;
type ShellSettingsResponse = ShellSettings | SettingsDocument<ShellSettings, ShellSettingsOverrides>;

function isSettingsDocument<TSettings, TOverrides>(
  value: unknown,
): value is SettingsDocument<TSettings, TOverrides> {
  return Boolean(
    value &&
    typeof value === 'object' &&
    'settings' in value &&
    'overrides' in value,
  );
}

function appSettingsFromResponse(response: AppSettingsResponse) {
  if (!response) {
    return null;
  }
  if (isSettingsDocument<AppSettings, AppSettingsOverrides>(response)) {
    return {
      settings: response.settings,
      overrides: response.overrides ?? EMPTY_APP_SETTINGS_OVERRIDES,
      persisted: response.persisted ?? true,
    };
  }
  return {
    settings: response,
    overrides: appOverridesFromSettings(response),
    persisted: false,
  };
}

function shellSettingsFromResponse(response: ShellSettingsResponse) {
  if (isSettingsDocument<ShellSettings, ShellSettingsOverrides>(response)) {
    return {
      settings: response.settings,
      overrides: response.overrides ?? EMPTY_SHELL_SETTINGS_OVERRIDES,
    };
  }
  return {
    settings: response,
    overrides: shellOverridesFromSettings(response),
  };
}

function appOverridesFromSettings(settings: AppSettings): AppSettingsOverrides {
  return {
    ...(normalizeTheme(settings.theme) !== DEFAULT_APP_SETTINGS.theme ? { theme: normalizeTheme(settings.theme) } : {}),
    ...(Boolean(settings.auto_patch_gemini) !== DEFAULT_APP_SETTINGS.auto_patch_gemini
      ? { auto_patch_gemini: Boolean(settings.auto_patch_gemini) }
      : {}),
    ...(normalizeTerminalFontSize(settings.terminal_font_size) !== DEFAULT_APP_SETTINGS.terminal_font_size
      ? { terminal_font_size: normalizeTerminalFontSize(settings.terminal_font_size) }
      : {}),
    ...((settings.terminal_font_family?.trim() ?? '') !== ''
      ? { terminal_font_family: settings.terminal_font_family?.trim() ?? null }
      : {}),
    ...(normalizeGridCardDisplayMode(settings.grid_card_display_mode) !== DEFAULT_APP_SETTINGS.grid_card_display_mode
      ? { grid_card_display_mode: normalizeGridCardDisplayMode(settings.grid_card_display_mode) }
      : {}),
    ...(normalizeWatchlistNewAgentPosition(settings.watchlist_new_agent_position) !== DEFAULT_APP_SETTINGS.watchlist_new_agent_position
      ? { watchlist_new_agent_position: normalizeWatchlistNewAgentPosition(settings.watchlist_new_agent_position) }
      : {}),
    ...((settings.titlebar_telemetry_visible !== false) !== DEFAULT_APP_SETTINGS.titlebar_telemetry_visible
      ? { titlebar_telemetry_visible: settings.titlebar_telemetry_visible !== false }
      : {}),
    ...(normalizeExternalEditorSetting(settings.external_editor) !== DEFAULT_APP_SETTINGS.external_editor
      ? { external_editor: normalizeExternalEditorSetting(settings.external_editor) }
      : {}),
    ...((settings.external_editor_custom_executable?.trim() ?? '') !== ''
      ? { external_editor_custom_executable: settings.external_editor_custom_executable?.trim() ?? null }
      : {}),
  };
}

function appOverridesFromState(state: SettingsState): AppSettingsOverrides {
  return appOverridesFromSettings({
    theme: state.theme,
    auto_patch_gemini: state.autoPatchGemini,
    terminal_font_size: state.terminalFontSize,
    terminal_font_family: state.terminalFontFamily.trim() || null,
    grid_card_display_mode: state.gridCardDisplayMode,
    watchlist_new_agent_position: state.watchlistNewAgentPosition,
    titlebar_telemetry_visible: state.titlebarTelemetryVisible,
    external_editor: state.externalEditor,
    external_editor_custom_executable: state.externalEditorCustomExecutable.trim() || null,
  });
}

function codexOverridesFromPolicy(policy: CodexRuntimePolicy): CodexRuntimePolicyOverrides | undefined {
  const normalized = normalizeCodexRuntimePolicy(policy);
  const overrides: CodexRuntimePolicyOverrides = {
    ...(normalized.sandbox_mode !== DEFAULT_CODEX_RUNTIME_POLICY.sandbox_mode ? { sandbox_mode: normalized.sandbox_mode } : {}),
    ...(normalized.approval_policy !== DEFAULT_CODEX_RUNTIME_POLICY.approval_policy ? { approval_policy: normalized.approval_policy } : {}),
    ...(normalized.full_auto !== DEFAULT_CODEX_RUNTIME_POLICY.full_auto ? { full_auto: normalized.full_auto } : {}),
  };
  return Object.keys(overrides).length > 0 ? overrides : undefined;
}

function shellOverridesFromSettings(settings: ShellSettings): ShellSettingsOverrides {
  const codexOverrides = settings.codex_runtime_policy
    ? codexOverridesFromPolicy(normalizeCodexRuntimePolicy(settings.codex_runtime_policy))
    : undefined;
  return {
    ...(settings.shell_id !== DEFAULT_SHELL_SETTINGS.shell_id ? { shell_id: settings.shell_id } : {}),
    ...((settings.custom_executable?.trim() ?? '') !== ''
      ? { custom_executable: settings.custom_executable?.trim() ?? null }
      : {}),
    ...((settings.custom_args?.trim() ?? '') !== ''
      ? { custom_args: settings.custom_args?.trim() ?? null }
      : {}),
    ...((settings.agent_session_persistence ?? DEFAULT_SHELL_SETTINGS.agent_session_persistence) !== DEFAULT_SHELL_SETTINGS.agent_session_persistence
      ? { agent_session_persistence: settings.agent_session_persistence ?? DEFAULT_SHELL_SETTINGS.agent_session_persistence }
      : {}),
    ...(codexOverrides ? { codex_runtime_policy: codexOverrides } : {}),
    ...(normalizeDefaultProviderSetting(settings.default_provider) !== (DEFAULT_SHELL_SETTINGS.default_provider ?? 'auto')
      ? { default_provider: normalizeDefaultProviderSetting(settings.default_provider) }
      : {}),
  };
}

function normalizeAppOverrides(overrides: AppSettingsOverrides | undefined): AppSettingsOverrides {
  return {
    ...(overrides?.theme ? { theme: normalizeTheme(overrides.theme) } : {}),
    ...(typeof overrides?.auto_patch_gemini === 'boolean' ? { auto_patch_gemini: overrides.auto_patch_gemini } : {}),
    ...(typeof overrides?.terminal_font_size === 'number'
      ? { terminal_font_size: normalizeTerminalFontSize(overrides.terminal_font_size) }
      : {}),
    ...('terminal_font_family' in (overrides ?? {})
      ? { terminal_font_family: overrides?.terminal_font_family?.trim() || null }
      : {}),
    ...(overrides?.grid_card_display_mode
      ? { grid_card_display_mode: normalizeGridCardDisplayMode(overrides.grid_card_display_mode) }
      : {}),
    ...(overrides?.watchlist_new_agent_position
      ? { watchlist_new_agent_position: normalizeWatchlistNewAgentPosition(overrides.watchlist_new_agent_position) }
      : {}),
    ...(typeof overrides?.titlebar_telemetry_visible === 'boolean'
      ? { titlebar_telemetry_visible: overrides.titlebar_telemetry_visible }
      : {}),
    ...(overrides?.external_editor
      ? { external_editor: normalizeExternalEditorSetting(overrides.external_editor) }
      : {}),
    ...('external_editor_custom_executable' in (overrides ?? {})
      ? { external_editor_custom_executable: overrides?.external_editor_custom_executable?.trim() || null }
      : {}),
  };
}

function appSettingsOverridesAreEmpty(overrides: AppSettingsOverrides | undefined) {
  return Object.keys(normalizeAppOverrides(overrides)).length === 0;
}

function normalizeShellOverrides(overrides: ShellSettingsOverrides | undefined): ShellSettingsOverrides {
  const codex = overrides?.codex_runtime_policy;
  const codexOverrides: CodexRuntimePolicyOverrides = {
    ...(CODEX_SANDBOX_MODES.includes(codex?.sandbox_mode as CodexSandboxMode) ? { sandbox_mode: codex?.sandbox_mode } : {}),
    ...(CODEX_APPROVAL_POLICIES.includes(codex?.approval_policy as CodexApprovalPolicy) ? { approval_policy: codex?.approval_policy } : {}),
    ...(typeof codex?.full_auto === 'boolean' ? { full_auto: codex.full_auto } : {}),
  };
  return {
    ...(overrides?.shell_id ? { shell_id: overrides.shell_id.trim() || 'auto' } : {}),
    ...('custom_executable' in (overrides ?? {}) ? { custom_executable: overrides?.custom_executable?.trim() || null } : {}),
    ...('custom_args' in (overrides ?? {}) ? { custom_args: overrides?.custom_args?.trim() || null } : {}),
    ...(overrides?.agent_session_persistence === 'fresh' || overrides?.agent_session_persistence === 'resume'
      ? { agent_session_persistence: overrides.agent_session_persistence }
      : {}),
    ...(Object.keys(codexOverrides).length > 0 ? { codex_runtime_policy: codexOverrides } : {}),
    ...(overrides?.default_provider ? { default_provider: normalizeDefaultProviderSetting(overrides.default_provider) } : {}),
  };
}

function stateHasMigratedAppPreferences(state: SettingsState) {
  return (
    state.theme !== DEFAULT_APP_SETTINGS.theme ||
    state.autoPatchGemini !== DEFAULT_APP_SETTINGS.auto_patch_gemini ||
    normalizeTerminalFontSize(state.terminalFontSize) !== DEFAULT_APP_SETTINGS.terminal_font_size ||
    state.terminalFontFamily.trim() !== '' ||
    state.gridCardDisplayMode !== DEFAULT_APP_SETTINGS.grid_card_display_mode ||
    state.watchlistNewAgentPosition !== DEFAULT_APP_SETTINGS.watchlist_new_agent_position ||
    state.titlebarTelemetryVisible !== DEFAULT_APP_SETTINGS.titlebar_telemetry_visible ||
    state.externalEditor !== DEFAULT_APP_SETTINGS.external_editor ||
    state.externalEditorCustomExecutable.trim() !== ''
  );
}

export const useSettingsStore = create<SettingsState>()(
  persist(
    (set, get) => ({
      theme: 'system',
      autoPatchGemini: false,
      terminalFontSize: defaultTerminalFontSize(),
      terminalFontFamily: '',
      gridCardDisplayMode: 'terminal',
      watchlistNewAgentPosition: 'top',
      titlebarTelemetryVisible: true,
      externalEditor: 'system',
      externalEditorCustomExecutable: '',
      shell_id: DEFAULT_SHELL_SETTINGS.shell_id,
      custom_executable: DEFAULT_SHELL_SETTINGS.custom_executable ?? '',
      custom_args: DEFAULT_SHELL_SETTINGS.custom_args ?? '',
      agent_session_persistence: DEFAULT_SHELL_SETTINGS.agent_session_persistence,
      codex_runtime_policy: DEFAULT_CODEX_RUNTIME_POLICY,
      default_provider: DEFAULT_SHELL_SETTINGS.default_provider ?? 'auto',
      app_settings_overrides: EMPTY_APP_SETTINGS_OVERRIDES,
      shell_settings_overrides: EMPTY_SHELL_SETTINGS_OVERRIDES,
      available_shells: [],
      app_settings_loaded: false,
      shell_settings_loaded: false,
      shells_loaded: false,
      setTheme: (theme) => set((state) => ({
        theme,
        app_settings_overrides: { ...state.app_settings_overrides, theme },
      })),
      setAutoPatchGemini: (autoPatchGemini) => set((state) => ({
        autoPatchGemini,
        app_settings_overrides: { ...state.app_settings_overrides, auto_patch_gemini: autoPatchGemini },
      })),
      setTerminalFontSize: (terminalFontSize) => set((state) => {
        const normalized = normalizeTerminalFontSize(terminalFontSize);
        return {
          terminalFontSize: normalized,
          app_settings_overrides: { ...state.app_settings_overrides, terminal_font_size: normalized },
        };
      }),
      setTerminalFontFamily: (terminalFontFamily) => set((state) => {
        const trimmed = terminalFontFamily.trim();
        const { terminal_font_family: _removed, ...rest } = state.app_settings_overrides;
        return {
          terminalFontFamily,
          app_settings_overrides: trimmed ? { ...state.app_settings_overrides, terminal_font_family: trimmed } : rest,
        };
      }),
      setGridCardDisplayMode: (gridCardDisplayMode) => set((state) => {
        const normalized = normalizeGridCardDisplayMode(gridCardDisplayMode);
        return {
          gridCardDisplayMode: normalized,
          app_settings_overrides: { ...state.app_settings_overrides, grid_card_display_mode: normalized },
        };
      }),
      setWatchlistNewAgentPosition: (watchlistNewAgentPosition) => set((state) => {
        const normalized = normalizeWatchlistNewAgentPosition(watchlistNewAgentPosition);
        return {
          watchlistNewAgentPosition: normalized,
          app_settings_overrides: { ...state.app_settings_overrides, watchlist_new_agent_position: normalized },
        };
      }),
      setTitlebarTelemetryVisible: (titlebarTelemetryVisible) => set((state) => ({
        titlebarTelemetryVisible,
        app_settings_overrides: { ...state.app_settings_overrides, titlebar_telemetry_visible: titlebarTelemetryVisible },
      })),
      setExternalEditor: (externalEditor) => set((state) => {
        const normalized = normalizeExternalEditorSetting(externalEditor);
        return {
          externalEditor: normalized,
          app_settings_overrides: { ...state.app_settings_overrides, external_editor: normalized },
        };
      }),
      setExternalEditorCustomExecutable: (externalEditorCustomExecutable) => set((state) => {
        const trimmed = externalEditorCustomExecutable.trim();
        const { external_editor_custom_executable: _removed, ...rest } = state.app_settings_overrides;
        return {
          externalEditorCustomExecutable,
          app_settings_overrides: trimmed
            ? { ...state.app_settings_overrides, external_editor_custom_executable: trimmed }
            : rest,
        };
      }),
      setShellId: (shell_id) => set((state) => ({
        shell_id,
        shell_settings_overrides: { ...state.shell_settings_overrides, shell_id },
      })),
      setCustomExecutable: (custom_executable) => set((state) => ({
        custom_executable,
        shell_settings_overrides: {
          ...state.shell_settings_overrides,
          custom_executable: custom_executable.trim() || null,
        },
      })),
      setCustomArgs: (custom_args) => set((state) => ({
        custom_args,
        shell_settings_overrides: {
          ...state.shell_settings_overrides,
          custom_args: custom_args.trim() || null,
        },
      })),
      setAgentSessionPersistence: (agent_session_persistence) => set((state) => ({
        agent_session_persistence,
        shell_settings_overrides: { ...state.shell_settings_overrides, agent_session_persistence },
      })),
      setDefaultProvider: (default_provider) => set((state) => {
        const normalized = normalizeDefaultProviderSetting(default_provider);
        return {
          default_provider: normalized,
          shell_settings_overrides: { ...state.shell_settings_overrides, default_provider: normalized },
        };
      }),
      setCodexSandboxMode: (sandbox_mode) => set((state) => ({
        codex_runtime_policy: { ...state.codex_runtime_policy, sandbox_mode },
        shell_settings_overrides: {
          ...state.shell_settings_overrides,
          codex_runtime_policy: {
            ...state.shell_settings_overrides.codex_runtime_policy,
            sandbox_mode,
          },
        },
      })),
      setCodexApprovalPolicy: (approval_policy) => set((state) => ({
        codex_runtime_policy: { ...state.codex_runtime_policy, approval_policy },
        shell_settings_overrides: {
          ...state.shell_settings_overrides,
          codex_runtime_policy: {
            ...state.shell_settings_overrides.codex_runtime_policy,
            approval_policy,
          },
        },
      })),
      setCodexFullAuto: (full_auto) => set((state) => ({
        codex_runtime_policy: { ...state.codex_runtime_policy, full_auto },
        shell_settings_overrides: {
          ...state.shell_settings_overrides,
          codex_runtime_policy: {
            ...state.shell_settings_overrides.codex_runtime_policy,
            full_auto,
          },
        },
      })),
      loadAppSettings: async () => {
        try {
          const response = await invoke<AppSettingsResponse>('load_app_settings');
          const resolved = appSettingsFromResponse(response);
          if (!resolved) {
            set({ app_settings_loaded: true });
            return;
          }
          const { settings, overrides, persisted } = resolved;
          const currentState = get();
          if (!persisted && appSettingsOverridesAreEmpty(overrides) && stateHasMigratedAppPreferences(currentState)) {
            set({
              app_settings_overrides: normalizeAppOverrides(appOverridesFromState(currentState)),
              app_settings_loaded: true,
            });
            return;
          }
          set({
            theme: normalizeTheme(settings.theme),
            autoPatchGemini: Boolean(settings.auto_patch_gemini),
            terminalFontSize: normalizeTerminalFontSize(settings.terminal_font_size),
            terminalFontFamily: settings.terminal_font_family?.trim() ?? '',
            gridCardDisplayMode: normalizeGridCardDisplayMode(settings.grid_card_display_mode),
            watchlistNewAgentPosition: normalizeWatchlistNewAgentPosition(settings.watchlist_new_agent_position),
            titlebarTelemetryVisible: settings.titlebar_telemetry_visible !== false,
            externalEditor: normalizeExternalEditorSetting(settings.external_editor),
            externalEditorCustomExecutable: settings.external_editor_custom_executable?.trim() ?? '',
            app_settings_overrides: normalizeAppOverrides(overrides),
            app_settings_loaded: true,
          });
        } catch (error) {
          console.error('Failed to load app settings:', error);
          set({ app_settings_loaded: true });
        }
      },
      saveAppSettings: async () => {
        const fallbackSettings: AppSettings = {
          theme: normalizeTheme(get().theme),
          auto_patch_gemini: get().autoPatchGemini,
          terminal_font_size: normalizeTerminalFontSize(get().terminalFontSize),
          terminal_font_family: get().terminalFontFamily.trim() || null,
          grid_card_display_mode: normalizeGridCardDisplayMode(get().gridCardDisplayMode),
          watchlist_new_agent_position: normalizeWatchlistNewAgentPosition(get().watchlistNewAgentPosition),
          titlebar_telemetry_visible: get().titlebarTelemetryVisible,
          external_editor: normalizeExternalEditorSetting(get().externalEditor),
          external_editor_custom_executable: get().externalEditorCustomExecutable.trim() || null,
        };
        const settings: SettingsDocument<AppSettings, AppSettingsOverrides> = {
          schema_version: 2,
          settings: fallbackSettings,
          overrides: normalizeAppOverrides(get().app_settings_overrides),
        };
        const savedResponse = await invoke<AppSettingsResponse>('save_app_settings', { settings });
        const saved = appSettingsFromResponse(savedResponse)?.settings ?? fallbackSettings;
        const overrides = appSettingsFromResponse(savedResponse)?.overrides ?? settings.overrides;
        set({
          theme: normalizeTheme(saved.theme),
          autoPatchGemini: Boolean(saved.auto_patch_gemini),
          terminalFontSize: normalizeTerminalFontSize(saved.terminal_font_size),
          terminalFontFamily: saved.terminal_font_family?.trim() ?? '',
          gridCardDisplayMode: normalizeGridCardDisplayMode(saved.grid_card_display_mode),
          watchlistNewAgentPosition: normalizeWatchlistNewAgentPosition(saved.watchlist_new_agent_position),
          titlebarTelemetryVisible: saved.titlebar_telemetry_visible !== false,
          externalEditor: normalizeExternalEditorSetting(saved.external_editor),
          externalEditorCustomExecutable: saved.external_editor_custom_executable?.trim() ?? '',
          app_settings_overrides: normalizeAppOverrides(overrides),
          app_settings_loaded: true,
        });
      },
      loadShellSettings: async () => {
        try {
          const response = await invoke<ShellSettingsResponse>('load_shell_settings');
          const { settings, overrides } = shellSettingsFromResponse(response);
          set({
            shell_id: settings.shell_id,
            custom_executable: settings.custom_executable ?? '',
            custom_args: settings.custom_args ?? '',
            agent_session_persistence: settings.agent_session_persistence ?? DEFAULT_SHELL_SETTINGS.agent_session_persistence,
            codex_runtime_policy: normalizeCodexRuntimePolicy(settings.codex_runtime_policy),
            default_provider: normalizeDefaultProviderSetting(settings.default_provider),
            shell_settings_overrides: normalizeShellOverrides(overrides),
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
            shell_settings_overrides: EMPTY_SHELL_SETTINGS_OVERRIDES,
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
        const fallbackSettings: ShellSettings = {
          shell_id: get().shell_id,
          custom_executable: get().custom_executable.trim() || null,
          custom_args: get().custom_args.trim() || null,
          agent_session_persistence: get().agent_session_persistence,
          codex_runtime_policy: normalizeCodexRuntimePolicy(get().codex_runtime_policy),
          default_provider: normalizeDefaultProviderSetting(get().default_provider),
        };
        const settings: SettingsDocument<ShellSettings, ShellSettingsOverrides> = {
          schema_version: 2,
          settings: fallbackSettings,
          overrides: normalizeShellOverrides(get().shell_settings_overrides),
        };
        const savedResponse = await invoke<ShellSettingsResponse>('save_shell_settings', { settings });
        const { settings: saved, overrides } = shellSettingsFromResponse(savedResponse ?? fallbackSettings);
        set({
          shell_id: saved.shell_id,
          custom_executable: saved.custom_executable ?? '',
          custom_args: saved.custom_args ?? '',
          agent_session_persistence: saved.agent_session_persistence ?? DEFAULT_SHELL_SETTINGS.agent_session_persistence,
          codex_runtime_policy: normalizeCodexRuntimePolicy(saved.codex_runtime_policy ?? fallbackSettings.codex_runtime_policy),
          default_provider: normalizeDefaultProviderSetting(saved.default_provider ?? fallbackSettings.default_provider),
          shell_settings_overrides: normalizeShellOverrides(overrides),
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
      version: 2,
      migrate: (persistedState) => {
        const state = persistedState as Partial<PersistedSettingsState> & { grid_card_display_mode?: GridCardDisplayMode };
        return {
          theme: state.theme ?? 'system',
          autoPatchGemini: state.autoPatchGemini ?? false,
          terminalFontSize: normalizeTerminalFontSize(state.terminalFontSize ?? defaultTerminalFontSize()),
          terminalFontFamily: state.terminalFontFamily?.trim() ?? '',
          gridCardDisplayMode: normalizeGridCardDisplayMode(state.gridCardDisplayMode ?? state.grid_card_display_mode),
          watchlistNewAgentPosition: normalizeWatchlistNewAgentPosition(state.watchlistNewAgentPosition),
          titlebarTelemetryVisible: typeof state.titlebarTelemetryVisible === 'boolean' ? state.titlebarTelemetryVisible : true,
          externalEditor: normalizeExternalEditorSetting(state.externalEditor),
          externalEditorCustomExecutable: state.externalEditorCustomExecutable?.trim() ?? '',
        };
      },
      partialize: (state) => ({
        theme: state.theme,
        autoPatchGemini: state.autoPatchGemini,
        terminalFontSize: normalizeTerminalFontSize(state.terminalFontSize),
        terminalFontFamily: state.terminalFontFamily.trim(),
        gridCardDisplayMode: normalizeGridCardDisplayMode(state.gridCardDisplayMode),
        watchlistNewAgentPosition: normalizeWatchlistNewAgentPosition(state.watchlistNewAgentPosition),
        titlebarTelemetryVisible: state.titlebarTelemetryVisible,
        externalEditor: normalizeExternalEditorSetting(state.externalEditor),
        externalEditorCustomExecutable: state.externalEditorCustomExecutable.trim(),
      }),
    }
  )
);
