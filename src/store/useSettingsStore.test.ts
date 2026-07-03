import { beforeEach, describe, expect, it, vi } from 'vitest';
import { invoke } from '@tauri-apps/api/core';
import {
  DEFAULT_CODEX_RUNTIME_POLICY,
  defaultTerminalFontFamily,
  defaultTerminalFontSize,
  LINUX_TERMINAL_FONT_FAMILY,
  MACOS_TERMINAL_FONT_FAMILY,
  WINDOWS_TERMINAL_FONT_FAMILY,
  useSettingsStore,
} from './useSettingsStore';

const mockedInvoke = vi.mocked(invoke);

function resetAppPreferences() {
  useSettingsStore.setState({
    theme: 'system',
    autoPatchGemini: false,
    terminalFontSize: 14,
    terminalFontFamily: '',
    gridCardDisplayMode: 'terminal',
    watchlistNewAgentPosition: 'top',
    titlebarTelemetryVisible: true,
    externalEditor: 'system',
    externalEditorCustomExecutable: '',
    explorerFileClickAction: 'preview',
    app_settings_overrides: {},
    app_settings_loaded: false,
  });
}

describe('terminal appearance defaults', () => {
  it('matches VS Code font family defaults by platform', () => {
    expect(defaultTerminalFontFamily('windows')).toBe(WINDOWS_TERMINAL_FONT_FAMILY);
    expect(defaultTerminalFontFamily('macos')).toBe(MACOS_TERMINAL_FONT_FAMILY);
    expect(defaultTerminalFontFamily('linux')).toBe(LINUX_TERMINAL_FONT_FAMILY);
  });

  it('matches VS Code terminal font size defaults by platform', () => {
    expect(defaultTerminalFontSize('macos')).toBe(12);
    expect(defaultTerminalFontSize('windows')).toBe(14);
    expect(defaultTerminalFontSize('linux')).toBe(14);
  });
});

describe('default provider settings', () => {
  beforeEach(() => {
    mockedInvoke.mockReset();
    useSettingsStore.setState({
      shell_id: 'auto',
      custom_executable: '',
      custom_args: '',
      agent_session_persistence: 'resume',
      conversation_logging: 'enabled',
      default_provider: 'auto',
      shell_settings_loaded: false,
    });
  });

  it('loads the saved default provider from shell settings', async () => {
    mockedInvoke.mockResolvedValueOnce({
      shell_id: 'auto',
      custom_executable: null,
      custom_args: null,
      agent_session_persistence: 'resume',
      conversation_logging: 'enabled',
      default_provider: 'codex',
    });

    await useSettingsStore.getState().loadShellSettings();

    expect(useSettingsStore.getState().default_provider).toBe('codex');
  });

  it('updates and saves the default provider', async () => {
    mockedInvoke.mockResolvedValueOnce({
      shell_id: 'auto',
      custom_executable: null,
      custom_args: null,
      agent_session_persistence: 'resume',
      conversation_logging: 'enabled',
      default_provider: 'gemini',
    });

    useSettingsStore.getState().setDefaultProvider('gemini');
    await useSettingsStore.getState().saveShellSettings();

    expect(mockedInvoke).toHaveBeenCalledWith('save_shell_settings', {
      settings: expect.objectContaining({
        schema_version: 2,
        overrides: expect.objectContaining({
          default_provider: 'gemini',
        }),
      }),
    });
    expect(useSettingsStore.getState().default_provider).toBe('gemini');
  });

  it('accepts antigravity as a saved default provider', async () => {
    mockedInvoke.mockResolvedValueOnce({
      shell_id: 'auto',
      custom_executable: null,
      custom_args: null,
      agent_session_persistence: 'resume',
      conversation_logging: 'enabled',
      default_provider: 'antigravity',
    });

    useSettingsStore.getState().setDefaultProvider('antigravity');
    await useSettingsStore.getState().saveShellSettings();

    expect(mockedInvoke).toHaveBeenCalledWith('save_shell_settings', {
      settings: expect.objectContaining({
        schema_version: 2,
        overrides: expect.objectContaining({
          default_provider: 'antigravity',
        }),
      }),
    });
    expect(useSettingsStore.getState().default_provider).toBe('antigravity');
  });
});

describe('conversation logging settings', () => {
  beforeEach(() => {
    mockedInvoke.mockReset();
    useSettingsStore.setState({
      shell_id: 'auto',
      custom_executable: '',
      custom_args: '',
      agent_session_persistence: 'resume',
      conversation_logging: 'enabled',
      default_provider: 'auto',
      shell_settings_loaded: false,
      shell_settings_overrides: {},
    });
  });

  it('loads disabled conversation logging from shell settings', async () => {
    mockedInvoke.mockResolvedValueOnce({
      shell_id: 'auto',
      custom_executable: null,
      custom_args: null,
      agent_session_persistence: 'resume',
      conversation_logging: 'disabled',
      default_provider: 'auto',
    });

    await useSettingsStore.getState().loadShellSettings();

    expect(useSettingsStore.getState().conversation_logging).toBe('disabled');
  });

  it('saves disabled conversation logging as a sparse shell override', async () => {
    mockedInvoke.mockResolvedValueOnce({
      schema_version: 2,
      settings: {
        shell_id: 'auto',
        custom_executable: null,
        custom_args: null,
        agent_session_persistence: 'resume',
        conversation_logging: 'disabled',
        default_provider: 'auto',
      },
      overrides: {
        conversation_logging: 'disabled',
      },
    });

    useSettingsStore.getState().setConversationLogging('disabled');
    await useSettingsStore.getState().saveShellSettings();

    expect(mockedInvoke).toHaveBeenCalledWith('save_shell_settings', {
      settings: expect.objectContaining({
        schema_version: 2,
        overrides: expect.objectContaining({
          conversation_logging: 'disabled',
        }),
      }),
    });
    expect(useSettingsStore.getState().conversation_logging).toBe('disabled');
  });
});

describe('Codex runtime defaults', () => {
  it('defaults Codex to workspace access with approval prompts', () => {
    expect(DEFAULT_CODEX_RUNTIME_POLICY).toEqual({
      sandbox_mode: 'workspace-write',
      approval_policy: 'on-request',
      full_auto: false,
      trust_workspaces: false,
    });
  });

  it('saves Codex workspace trust only when explicitly enabled', async () => {
    mockedInvoke.mockReset();
    mockedInvoke.mockResolvedValueOnce({
      schema_version: 2,
      settings: {
        shell_id: 'auto',
        custom_executable: null,
        custom_args: null,
        agent_session_persistence: 'resume',
        conversation_logging: 'enabled',
        default_provider: 'auto',
        codex_runtime_policy: {
          sandbox_mode: 'workspace-write',
          approval_policy: 'on-request',
          full_auto: false,
          trust_workspaces: true,
        },
      },
      overrides: {
        codex_runtime_policy: {
          trust_workspaces: true,
        },
      },
    });

    useSettingsStore.getState().setCodexTrustWorkspaces(true);
    await useSettingsStore.getState().saveShellSettings();

    expect(mockedInvoke).toHaveBeenCalledWith('save_shell_settings', {
      settings: expect.objectContaining({
        schema_version: 2,
        overrides: expect.objectContaining({
          codex_runtime_policy: expect.objectContaining({
            trust_workspaces: true,
          }),
        }),
      }),
    });
    expect(useSettingsStore.getState().codex_runtime_policy.trust_workspaces).toBe(true);
  });
});

describe('app settings persistence', () => {
  beforeEach(() => {
    mockedInvoke.mockReset();
    localStorage.clear();
    resetAppPreferences();
  });

  it('loads app preferences from the backend app settings file', async () => {
    mockedInvoke.mockResolvedValueOnce({
      theme: 'dark',
      auto_patch_gemini: true,
      terminal_font_size: 16,
      terminal_font_family: 'JetBrains Mono, monospace',
      grid_card_display_mode: 'chat',
      watchlist_new_agent_position: 'bottom',
      titlebar_telemetry_visible: false,
      external_editor: 'vscode',
      external_editor_custom_executable: null,
      explorer_file_click_action: 'external',
    });

    await useSettingsStore.getState().loadAppSettings();

    expect(mockedInvoke).toHaveBeenCalledWith('load_app_settings');
    expect(useSettingsStore.getState().theme).toBe('dark');
    expect(useSettingsStore.getState().autoPatchGemini).toBe(true);
    expect(useSettingsStore.getState().terminalFontSize).toBe(16);
    expect(useSettingsStore.getState().terminalFontFamily).toBe('JetBrains Mono, monospace');
    expect(useSettingsStore.getState().gridCardDisplayMode).toBe('chat');
    expect(useSettingsStore.getState().watchlistNewAgentPosition).toBe('bottom');
    expect(useSettingsStore.getState().titlebarTelemetryVisible).toBe(false);
    expect(useSettingsStore.getState().externalEditor).toBe('vscode');
    expect(useSettingsStore.getState().explorerFileClickAction).toBe('external');
    expect(useSettingsStore.getState().app_settings_loaded).toBe(true);
  });

  it('saves app preferences through the backend app settings file', async () => {
    mockedInvoke.mockResolvedValueOnce({
      theme: 'light',
      auto_patch_gemini: true,
      terminal_font_size: 12,
      terminal_font_family: null,
      grid_card_display_mode: 'chat',
      watchlist_new_agent_position: 'bottom',
      titlebar_telemetry_visible: false,
      external_editor: 'custom',
      external_editor_custom_executable: 'C:/Tools/editor.exe',
      explorer_file_click_action: 'external',
    });

    useSettingsStore.getState().setTheme('light');
    useSettingsStore.getState().setAutoPatchGemini(true);
    useSettingsStore.getState().setTerminalFontSize(12);
    useSettingsStore.getState().setTerminalFontFamily('');
    useSettingsStore.getState().setGridCardDisplayMode('chat');
    useSettingsStore.getState().setWatchlistNewAgentPosition('bottom');
    useSettingsStore.getState().setTitlebarTelemetryVisible(false);
    useSettingsStore.getState().setExternalEditor('custom');
    useSettingsStore.getState().setExternalEditorCustomExecutable('C:/Tools/editor.exe');
    useSettingsStore.getState().setExplorerFileClickAction('external');

    await useSettingsStore.getState().saveAppSettings();

    expect(mockedInvoke).toHaveBeenCalledWith('save_app_settings', {
      settings: expect.objectContaining({
        schema_version: 2,
        overrides: expect.objectContaining({
          theme: 'light',
          auto_patch_gemini: true,
          terminal_font_size: 12,
          grid_card_display_mode: 'chat',
          watchlist_new_agent_position: 'bottom',
          titlebar_telemetry_visible: false,
          external_editor: 'custom',
          external_editor_custom_executable: 'C:/Tools/editor.exe',
          explorer_file_click_action: 'external',
        }),
      }),
    });
    expect(useSettingsStore.getState().theme).toBe('light');
    expect(useSettingsStore.getState().terminalFontSize).toBe(12);
    expect(useSettingsStore.getState().gridCardDisplayMode).toBe('chat');
    expect(useSettingsStore.getState().watchlistNewAgentPosition).toBe('bottom');
    expect(useSettingsStore.getState().titlebarTelemetryVisible).toBe(false);
    expect(useSettingsStore.getState().externalEditor).toBe('custom');
    expect(useSettingsStore.getState().explorerFileClickAction).toBe('external');
  });

  it('removes the Explorer file click override when reset to preview', () => {
    useSettingsStore.getState().setExplorerFileClickAction('external');
    expect(useSettingsStore.getState().app_settings_overrides).toEqual(
      expect.objectContaining({
        explorer_file_click_action: 'external',
      }),
    );

    useSettingsStore.getState().setExplorerFileClickAction('preview');

    expect(useSettingsStore.getState().explorerFileClickAction).toBe('preview');
    expect(useSettingsStore.getState().app_settings_overrides).not.toHaveProperty('explorer_file_click_action');
  });

  it('keeps migrated local preferences when no backend app settings file exists yet', async () => {
    useSettingsStore.setState({
      theme: 'dark',
      autoPatchGemini: true,
      terminalFontSize: 16,
      terminalFontFamily: 'Cascadia Mono, monospace',
      gridCardDisplayMode: 'chat',
      watchlistNewAgentPosition: 'bottom',
      titlebarTelemetryVisible: false,
    });
    mockedInvoke.mockResolvedValueOnce({
      schema_version: 2,
      persisted: false,
      settings: {
        theme: 'system',
        auto_patch_gemini: false,
        terminal_font_size: 14,
        terminal_font_family: null,
        grid_card_display_mode: 'terminal',
        watchlist_new_agent_position: 'top',
        titlebar_telemetry_visible: true,
      },
      overrides: {},
    });

    await useSettingsStore.getState().loadAppSettings();

    expect(useSettingsStore.getState().theme).toBe('dark');
    expect(useSettingsStore.getState().autoPatchGemini).toBe(true);
    expect(useSettingsStore.getState().terminalFontSize).toBe(16);
    expect(useSettingsStore.getState().terminalFontFamily).toBe('Cascadia Mono, monospace');
    expect(useSettingsStore.getState().gridCardDisplayMode).toBe('chat');
    expect(useSettingsStore.getState().watchlistNewAgentPosition).toBe('bottom');
    expect(useSettingsStore.getState().titlebarTelemetryVisible).toBe(false);
    expect(useSettingsStore.getState().app_settings_overrides).toEqual({
      theme: 'dark',
      auto_patch_gemini: true,
      terminal_font_size: 16,
      terminal_font_family: 'Cascadia Mono, monospace',
      grid_card_display_mode: 'chat',
      watchlist_new_agent_position: 'bottom',
      titlebar_telemetry_visible: false,
    });
  });

  it('keeps migrated local preferences when missing backend settings use stable titlebar defaults', async () => {
    useSettingsStore.setState({
      theme: 'dark',
      autoPatchGemini: true,
      terminalFontSize: 16,
      terminalFontFamily: 'Cascadia Mono, monospace',
      gridCardDisplayMode: 'chat',
      watchlistNewAgentPosition: 'bottom',
      titlebarTelemetryVisible: false,
    });
    mockedInvoke.mockResolvedValueOnce({
      schema_version: 2,
      persisted: false,
      settings: {
        theme: 'system',
        auto_patch_gemini: false,
        terminal_font_size: 14,
        terminal_font_family: null,
        grid_card_display_mode: 'terminal',
        watchlist_new_agent_position: 'top',
        titlebar_telemetry_visible: false,
      },
      overrides: {},
    });

    await useSettingsStore.getState().loadAppSettings();

    expect(useSettingsStore.getState().theme).toBe('dark');
    expect(useSettingsStore.getState().autoPatchGemini).toBe(true);
    expect(useSettingsStore.getState().terminalFontSize).toBe(16);
    expect(useSettingsStore.getState().terminalFontFamily).toBe('Cascadia Mono, monospace');
    expect(useSettingsStore.getState().gridCardDisplayMode).toBe('chat');
    expect(useSettingsStore.getState().watchlistNewAgentPosition).toBe('bottom');
    expect(useSettingsStore.getState().titlebarTelemetryVisible).toBe(false);
    expect(useSettingsStore.getState().app_settings_overrides).toEqual({
      theme: 'dark',
      auto_patch_gemini: true,
      terminal_font_size: 16,
      terminal_font_family: 'Cascadia Mono, monospace',
      grid_card_display_mode: 'chat',
      watchlist_new_agent_position: 'bottom',
      titlebar_telemetry_visible: false,
    });
  });

  it('uses persisted backend defaults instead of stale local preferences', async () => {
    useSettingsStore.setState({
      theme: 'dark',
      autoPatchGemini: true,
      terminalFontSize: 16,
      terminalFontFamily: 'Cascadia Mono, monospace',
      gridCardDisplayMode: 'chat',
      watchlistNewAgentPosition: 'bottom',
      titlebarTelemetryVisible: false,
    });
    mockedInvoke.mockResolvedValueOnce({
      schema_version: 2,
      persisted: true,
      settings: {
        theme: 'system',
        auto_patch_gemini: false,
        terminal_font_size: 14,
        terminal_font_family: null,
        grid_card_display_mode: 'terminal',
        watchlist_new_agent_position: 'top',
        titlebar_telemetry_visible: true,
      },
      overrides: {},
    });

    await useSettingsStore.getState().loadAppSettings();

    expect(useSettingsStore.getState().theme).toBe('system');
    expect(useSettingsStore.getState().autoPatchGemini).toBe(false);
    expect(useSettingsStore.getState().terminalFontSize).toBe(14);
    expect(useSettingsStore.getState().terminalFontFamily).toBe('');
    expect(useSettingsStore.getState().gridCardDisplayMode).toBe('terminal');
    expect(useSettingsStore.getState().watchlistNewAgentPosition).toBe('top');
    expect(useSettingsStore.getState().titlebarTelemetryVisible).toBe(true);
    expect(useSettingsStore.getState().app_settings_overrides).toEqual({});
  });

  it('saves migrated local preferences as sparse overrides after loading a missing backend file', async () => {
    useSettingsStore.setState({
      theme: 'dark',
      autoPatchGemini: true,
      terminalFontSize: 16,
      terminalFontFamily: 'Cascadia Mono, monospace',
      gridCardDisplayMode: 'chat',
      watchlistNewAgentPosition: 'bottom',
      titlebarTelemetryVisible: false,
    });
    mockedInvoke.mockResolvedValueOnce({
      schema_version: 2,
      persisted: false,
      settings: {
        theme: 'system',
        auto_patch_gemini: false,
        terminal_font_size: 14,
        terminal_font_family: null,
        grid_card_display_mode: 'terminal',
        watchlist_new_agent_position: 'top',
        titlebar_telemetry_visible: true,
      },
      overrides: {},
    });
    mockedInvoke.mockResolvedValueOnce({
      schema_version: 2,
      persisted: true,
      settings: {
        theme: 'dark',
        auto_patch_gemini: true,
        terminal_font_size: 16,
        terminal_font_family: 'Cascadia Mono, monospace',
        grid_card_display_mode: 'chat',
        watchlist_new_agent_position: 'bottom',
        titlebar_telemetry_visible: false,
      },
      overrides: {
        theme: 'dark',
        auto_patch_gemini: true,
        terminal_font_size: 16,
        terminal_font_family: 'Cascadia Mono, monospace',
        grid_card_display_mode: 'chat',
        watchlist_new_agent_position: 'bottom',
        titlebar_telemetry_visible: false,
      },
    });

    await useSettingsStore.getState().loadAppSettings();
    await useSettingsStore.getState().saveAppSettings();

    expect(mockedInvoke).toHaveBeenLastCalledWith('save_app_settings', {
      settings: expect.objectContaining({
        schema_version: 2,
        overrides: {
          theme: 'dark',
          auto_patch_gemini: true,
          terminal_font_size: 16,
          terminal_font_family: 'Cascadia Mono, monospace',
          grid_card_display_mode: 'chat',
          watchlist_new_agent_position: 'bottom',
          titlebar_telemetry_visible: false,
        },
      }),
    });
  });

  it('keeps migrated local preferences for legacy default-shaped responses', async () => {
    useSettingsStore.setState({
      theme: 'dark',
      autoPatchGemini: true,
      terminalFontSize: 16,
      terminalFontFamily: 'Cascadia Mono, monospace',
      gridCardDisplayMode: 'chat',
      watchlistNewAgentPosition: 'bottom',
      titlebarTelemetryVisible: false,
    });
    mockedInvoke.mockResolvedValueOnce({
      theme: 'system',
      auto_patch_gemini: false,
      terminal_font_size: 14,
      terminal_font_family: null,
      grid_card_display_mode: 'terminal',
      watchlist_new_agent_position: 'top',
      titlebar_telemetry_visible: true,
    });

    await useSettingsStore.getState().loadAppSettings();

    expect(useSettingsStore.getState().theme).toBe('dark');
    expect(useSettingsStore.getState().autoPatchGemini).toBe(true);
    expect(useSettingsStore.getState().terminalFontSize).toBe(16);
    expect(useSettingsStore.getState().terminalFontFamily).toBe('Cascadia Mono, monospace');
    expect(useSettingsStore.getState().gridCardDisplayMode).toBe('chat');
    expect(useSettingsStore.getState().watchlistNewAgentPosition).toBe('bottom');
    expect(useSettingsStore.getState().titlebarTelemetryVisible).toBe(false);
  });

  it('falls back without error when backend app settings response is empty', async () => {
    const errorSpy = vi.spyOn(console, 'error').mockImplementation(() => {});
    mockedInvoke.mockResolvedValueOnce(null);

    await useSettingsStore.getState().loadAppSettings();

    expect(useSettingsStore.getState().theme).toBe('system');
    expect(useSettingsStore.getState().app_settings_loaded).toBe(true);
    expect(errorSpy).not.toHaveBeenCalled();
    errorSpy.mockRestore();
  });
});
