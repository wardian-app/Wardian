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

describe('Codex runtime defaults', () => {
  it('defaults Codex to workspace access with approval prompts', () => {
    expect(DEFAULT_CODEX_RUNTIME_POLICY).toEqual({
      sandbox_mode: 'workspace-write',
      approval_policy: 'on-request',
      full_auto: false,
    });
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
    });

    await useSettingsStore.getState().loadAppSettings();

    expect(mockedInvoke).toHaveBeenCalledWith('load_app_settings');
    expect(useSettingsStore.getState().theme).toBe('dark');
    expect(useSettingsStore.getState().autoPatchGemini).toBe(true);
    expect(useSettingsStore.getState().terminalFontSize).toBe(16);
    expect(useSettingsStore.getState().terminalFontFamily).toBe('JetBrains Mono, monospace');
    expect(useSettingsStore.getState().app_settings_loaded).toBe(true);
  });

  it('saves app preferences through the backend app settings file', async () => {
    mockedInvoke.mockResolvedValueOnce({
      theme: 'light',
      auto_patch_gemini: true,
      terminal_font_size: 12,
      terminal_font_family: null,
    });

    useSettingsStore.getState().setTheme('light');
    useSettingsStore.getState().setAutoPatchGemini(true);
    useSettingsStore.getState().setTerminalFontSize(12);
    useSettingsStore.getState().setTerminalFontFamily('');

    await useSettingsStore.getState().saveAppSettings();

    expect(mockedInvoke).toHaveBeenCalledWith('save_app_settings', {
      settings: expect.objectContaining({
        schema_version: 2,
        overrides: expect.objectContaining({
          theme: 'light',
          auto_patch_gemini: true,
          terminal_font_size: 12,
        }),
      }),
    });
    expect(useSettingsStore.getState().theme).toBe('light');
    expect(useSettingsStore.getState().terminalFontSize).toBe(12);
  });

  it('keeps migrated local preferences when no backend app settings file exists yet', async () => {
    useSettingsStore.setState({
      theme: 'dark',
      autoPatchGemini: true,
      terminalFontSize: 16,
      terminalFontFamily: 'Cascadia Mono, monospace',
    });
    mockedInvoke.mockResolvedValueOnce({
      theme: 'system',
      auto_patch_gemini: false,
      terminal_font_size: 14,
      terminal_font_family: null,
    });

    await useSettingsStore.getState().loadAppSettings();

    expect(useSettingsStore.getState().theme).toBe('dark');
    expect(useSettingsStore.getState().autoPatchGemini).toBe(true);
    expect(useSettingsStore.getState().terminalFontSize).toBe(16);
    expect(useSettingsStore.getState().terminalFontFamily).toBe('Cascadia Mono, monospace');
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
