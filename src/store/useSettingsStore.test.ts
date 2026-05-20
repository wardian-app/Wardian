import { beforeEach, describe, expect, it, vi } from 'vitest';
import { invoke } from '@tauri-apps/api/core';
import {
  defaultTerminalFontFamily,
  defaultTerminalFontSize,
  LINUX_TERMINAL_FONT_FAMILY,
  MACOS_TERMINAL_FONT_FAMILY,
  WINDOWS_TERMINAL_FONT_FAMILY,
  useSettingsStore,
} from './useSettingsStore';

const mockedInvoke = vi.mocked(invoke);

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
        default_provider: 'gemini',
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
        default_provider: 'antigravity',
      }),
    });
    expect(useSettingsStore.getState().default_provider).toBe('antigravity');
  });
});
