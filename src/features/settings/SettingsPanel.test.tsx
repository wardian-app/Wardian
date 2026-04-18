import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { invoke } from '@tauri-apps/api/core';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { SettingsPanel } from './SettingsPanel';
import { useSettingsStore } from '../../store/useSettingsStore';

const mockInvoke = vi.mocked(invoke);

describe('SettingsPanel', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    localStorage.clear();
    useSettingsStore.setState({
      theme: 'system',
      autoPatchGemini: false,
      shell_id: 'auto',
      custom_executable: '',
      custom_args: '',
      agent_session_persistence: 'resume',
      available_shells: [],
      shell_settings_loaded: false,
      shells_loaded: false,
    });

    mockInvoke.mockImplementation(async (command, args) => {
      switch (command) {
        case 'load_shell_settings':
          return {
            shell_id: 'pwsh',
            custom_executable: null,
            custom_args: null,
            agent_session_persistence: 'resume',
          };
        case 'list_available_shells':
          return [
            {
              id: 'pwsh',
              label: 'PowerShell 7',
              executable: 'C:/Program Files/PowerShell/7/pwsh.exe',
              default_args: ['-NoProfile', '-Command'],
            },
            {
              id: 'cmd',
              label: 'Command Prompt',
              executable: 'C:/Windows/System32/cmd.exe',
              default_args: ['/C'],
            },
          ];
        case 'save_shell_settings':
          return (args as { settings?: unknown } | undefined)?.settings;
        case 'run_gemini_patch':
          return 'ok';
        default:
          return null;
      }
    });
  });

  it('loads discovered shells and current shell settings on mount', async () => {
    render(<SettingsPanel />);

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith('load_shell_settings');
      expect(mockInvoke).toHaveBeenCalledWith('list_available_shells');
    });

    const select = await screen.findByLabelText('Shell / Interpreter');
    expect(select).toHaveValue('pwsh');
    expect(screen.getByText('C:/Program Files/PowerShell/7/pwsh.exe')).toBeInTheDocument();
  });

  it('saves custom shell settings through tauri', async () => {
    render(<SettingsPanel />);

    const select = await screen.findByLabelText('Shell / Interpreter');
    fireEvent.change(select, { target: { value: 'custom' } });

    fireEvent.change(screen.getByLabelText('Custom executable'), {
      target: { value: 'C:/Tools/custom-shell.exe' },
    });
    fireEvent.change(screen.getByLabelText('Command args'), {
      target: { value: '--command' },
    });

    fireEvent.click(screen.getByText('Save Shell'));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith('save_shell_settings', {
        settings: {
          shell_id: 'custom',
          custom_executable: 'C:/Tools/custom-shell.exe',
          custom_args: '--command',
          agent_session_persistence: 'resume',
        },
      });
    });

    expect(await screen.findByText('Shell settings updated.')).toBeInTheDocument();
  });

  it('shows the auto-selected shell when default shell is auto', async () => {
    mockInvoke.mockImplementation(async (command, args) => {
      switch (command) {
        case 'load_shell_settings':
          return {
            shell_id: 'auto',
            custom_executable: null,
            custom_args: null,
            agent_session_persistence: 'resume',
          };
        case 'list_available_shells':
          return [
            {
              id: 'cmd',
              label: 'Command Prompt',
              executable: 'C:/Windows/System32/cmd.exe',
              default_args: ['/C'],
            },
            {
              id: 'pwsh',
              label: 'PowerShell 7',
              executable: 'C:/Program Files/PowerShell/7/pwsh.exe',
              default_args: ['-NoProfile', '-Command'],
            },
          ];
        case 'save_shell_settings':
          return (args as { settings?: unknown } | undefined)?.settings;
        default:
          return null;
      }
    });

    render(<SettingsPanel />);

    expect(await screen.findByText('Auto: PowerShell 7')).toBeInTheDocument();
    expect(screen.getByText('C:/Program Files/PowerShell/7/pwsh.exe')).toBeInTheDocument();
  });

  it('saves global regular agent session persistence through tauri', async () => {
    render(<SettingsPanel />);

    const select = await screen.findByLabelText('Regular agent sessions');
    fireEvent.change(select, { target: { value: 'fresh' } });

    fireEvent.click(screen.getByText('Save Agent Runtime'));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith('save_shell_settings', {
        settings: {
          shell_id: 'pwsh',
          custom_executable: null,
          custom_args: null,
          agent_session_persistence: 'fresh',
        },
      });
    });

    expect(await screen.findByText('Agent runtime updated.')).toBeInTheDocument();
    expect(screen.queryByText('Shell settings updated.')).not.toBeInTheDocument();
  });
});
