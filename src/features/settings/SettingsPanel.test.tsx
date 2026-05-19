import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
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
      terminalFontSize: 14,
      terminalFontFamily: '',
      shell_id: 'auto',
      custom_executable: '',
      custom_args: '',
      agent_session_persistence: 'resume',
      default_provider: 'auto',
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
            default_provider: 'auto',
            codex_runtime_policy: {
              sandbox_mode: 'workspace-write',
              approval_policy: 'on-request',
              full_auto: false,
            },
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
        case 'save_agent_session_persistence':
          return {
            shell_id: 'pwsh',
            custom_executable: null,
            custom_args: null,
            agent_session_persistence: (args as { persistence?: 'fresh' | 'resume' } | undefined)?.persistence ?? 'resume',
            codex_runtime_policy: {
              sandbox_mode: 'workspace-write',
              approval_policy: 'on-request',
              full_auto: false,
            },
            default_provider: 'auto',
          };
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
    expect(screen.getByLabelText('Codex sandbox')).toHaveValue('workspace-write');
    expect(screen.getByLabelText('Codex approval')).toHaveValue('on-request');
    expect(screen.getByLabelText('Autonomous full access, no prompts')).not.toBeChecked();
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
          default_provider: 'auto',
          codex_runtime_policy: {
            sandbox_mode: 'workspace-write',
            approval_policy: 'on-request',
            full_auto: false,
          },
        },
      });
    });

    expect(await screen.findByText('Shell settings updated.')).toBeInTheDocument();
  });

  it('shows the Gemini patch success message after running the patch', async () => {
    const user = userEvent.setup();
    render(<SettingsPanel />);

    await user.click(await screen.findByText('Run Patch Now'));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith('run_gemini_patch');
    });
    expect(await screen.findByText('Gemini CLI patch applied successfully.')).toBeInTheDocument();
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
        case 'save_agent_session_persistence':
          return {
            shell_id: 'auto',
            custom_executable: null,
            custom_args: null,
            agent_session_persistence: (args as { persistence?: 'fresh' | 'resume' } | undefined)?.persistence ?? 'resume',
          };
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
          default_provider: 'auto',
          codex_runtime_policy: {
            sandbox_mode: 'workspace-write',
            approval_policy: 'on-request',
            full_auto: false,
          },
        },
      });
    });

    expect(await screen.findByText('Agent runtime updated.')).toBeInTheDocument();
    expect(screen.queryByText('Shell settings updated.')).not.toBeInTheDocument();
    expect(mockInvoke).not.toHaveBeenCalledWith('save_agent_session_persistence', expect.anything());
  });

  it('saves Codex runtime defaults through shell settings payload', async () => {
    render(<SettingsPanel />);

    fireEvent.change(await screen.findByLabelText('Codex sandbox'), {
      target: { value: 'danger-full-access' },
    });
    fireEvent.change(screen.getByLabelText('Codex approval'), {
      target: { value: 'never' },
    });
    fireEvent.click(screen.getByLabelText('Autonomous full access, no prompts'));

    fireEvent.click(screen.getByText('Save Agent Runtime'));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith('save_shell_settings', {
        settings: {
          shell_id: 'pwsh',
          custom_executable: null,
          custom_args: null,
          agent_session_persistence: 'resume',
          default_provider: 'auto',
          codex_runtime_policy: {
            sandbox_mode: 'danger-full-access',
            approval_policy: 'never',
            full_auto: true,
          },
        },
      });
    });
  });

  it('defaults Codex runtime policy to autonomous full access when backend omits it', async () => {
    mockInvoke.mockImplementation(async (command, args) => {
      switch (command) {
        case 'load_shell_settings':
          return {
            shell_id: 'pwsh',
            custom_executable: null,
            custom_args: null,
            agent_session_persistence: 'resume',
            default_provider: 'auto',
          };
        case 'list_available_shells':
          return [
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

    expect(await screen.findByLabelText('Codex sandbox')).toHaveValue('danger-full-access');
    expect(screen.getByLabelText('Codex approval')).toHaveValue('never');
    expect(screen.getByLabelText('Autonomous full access, no prompts')).toBeChecked();
  });

  it('adjusts the terminal font size preference', async () => {
    render(<SettingsPanel />);

    const input = await screen.findByLabelText('Terminal font size');
    expect(input).toHaveValue(14);

    fireEvent.change(input, { target: { value: '16' } });

    expect(useSettingsStore.getState().terminalFontSize).toBe(16);
    expect(input).toHaveValue(16);
  });

  it('saves the default provider preference with agent runtime settings', async () => {
    render(<SettingsPanel />);

    const select = await screen.findByLabelText('Default provider');
    expect(select).toHaveValue('auto');
    expect(within(select).getByRole('option', { name: 'Auto' })).toBeInTheDocument();
    expect(within(select).getByRole('option', { name: 'Claude' })).toBeInTheDocument();
    expect(within(select).getByRole('option', { name: 'Codex' })).toBeInTheDocument();
    expect(within(select).getByRole('option', { name: 'Gemini' })).toBeInTheDocument();
    expect(within(select).getByRole('option', { name: 'OpenCode' })).toBeInTheDocument();

    fireEvent.change(select, { target: { value: 'gemini' } });
    fireEvent.click(screen.getByText('Save Agent Runtime'));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith('save_shell_settings', {
        settings: {
          shell_id: 'pwsh',
          custom_executable: null,
          custom_args: null,
          agent_session_persistence: 'resume',
          default_provider: 'gemini',
          codex_runtime_policy: {
            sandbox_mode: 'workspace-write',
            approval_policy: 'on-request',
            full_auto: false,
          },
        },
      });
    });
  });

  it('keeps partial terminal font size edits local until they become valid', async () => {
    const user = userEvent.setup();
    render(<SettingsPanel />);

    const input = await screen.findByLabelText('Terminal font size') as HTMLInputElement;

    await user.clear(input);
    expect(input.value).toBe('');
    expect(useSettingsStore.getState().terminalFontSize).toBe(14);

    await user.type(input, '1');
    expect(input.value).toBe('1');
    expect(useSettingsStore.getState().terminalFontSize).toBe(14);

    await user.type(input, '2');
    expect(input.value).toBe('12');
    expect(useSettingsStore.getState().terminalFontSize).toBe(12);
  });

  it('adjusts the terminal font family preference from presets and custom input', async () => {
    render(<SettingsPanel />);

    const select = await screen.findByLabelText('Terminal font family');
    expect(select).toHaveValue('');

    fireEvent.change(select, { target: { value: 'JetBrains Mono, monospace' } });

    expect(useSettingsStore.getState().terminalFontFamily).toBe('JetBrains Mono, monospace');
    expect(select).toHaveValue('JetBrains Mono, monospace');

    fireEvent.change(select, { target: { value: '__custom__' } });

    const customInput = screen.getByLabelText('Custom terminal font family');
    fireEvent.change(customInput, { target: { value: 'FiraCode Nerd Font, monospace' } });

    expect(useSettingsStore.getState().terminalFontFamily).toBe('FiraCode Nerd Font, monospace');
    expect(customInput).toHaveValue('FiraCode Nerd Font, monospace');
  });
});
