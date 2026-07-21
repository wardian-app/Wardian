import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { getVersion } from '@tauri-apps/api/app';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { check } from '@tauri-apps/plugin-updater';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { useAppUpdate } from './useAppUpdate';
import packageJson from '../../../package.json';
import type { DownloadEvent } from '@tauri-apps/plugin-updater';

vi.mock('@tauri-apps/api/app', () => ({
  getVersion: vi.fn(),
}));

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));

vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(),
}));

vi.mock('@tauri-apps/plugin-updater', () => ({
  check: vi.fn(),
}));

const mockGetVersion = vi.mocked(getVersion);
const mockInvoke = vi.mocked(invoke);
const mockListen = vi.mocked(listen);
const mockCheck = vi.mocked(check);
const mockedRuntimeVersion = '0.3.5';
const packageVersion = packageJson.version;
const setTauriRuntime = (available: boolean) => {
  if (available) {
    Object.defineProperty(window, '__TAURI_INTERNALS__', {
      value: { invoke: vi.fn() },
      configurable: true,
    });
  } else {
    delete (window as Window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
  }
};

const Probe = () => {
  const update = useAppUpdate();
  return (
    <div>
      <div data-testid="version">{update.currentVersion}</div>
      <div data-testid="status">{update.status}</div>
      <div data-testid="available-version">{update.availableUpdate?.version ?? ''}</div>
      <div data-testid="available-date">{update.availableUpdate?.date ?? ''}</div>
      <div data-testid="available-body">{update.availableUpdate?.body ?? ''}</div>
      <div data-testid="error">{update.errorMessage}</div>
      <div data-testid="updates-enabled">{String(update.updatesEnabled)}</div>
      <div data-testid="eligibility-reason">
        {(update as typeof update & { updateEligibilityReason?: string }).updateEligibilityReason ?? ''}
      </div>
      <div data-testid="downloaded">{update.downloadedBytes}</div>
      <div data-testid="content-length">{update.contentLength ?? ''}</div>
      <div data-testid="percent">{update.progressPercent ?? ''}</div>
      <button type="button" onClick={() => update.checkNow()}>
        check
      </button>
      <button type="button" onClick={() => update.downloadAndInstall()}>
        install
      </button>
      <button type="button" onClick={() => update.relaunchApp()}>
        restart
      </button>
    </div>
  );
};

const makeUpdate = (overrides: Partial<{
  version: string;
  date: string;
  body: string;
  downloadAndInstall: (onEvent?: (event: DownloadEvent) => void) => Promise<void>;
}> = {}) => ({
  version: overrides.version ?? '0.3.6',
  date: overrides.date,
  body: overrides.body,
  downloadAndInstall: vi.fn(overrides.downloadAndInstall ?? (async () => {})),
});

describe('useAppUpdate', () => {
  let updateEventHandler: ((event: { payload: DownloadEvent }) => void) | null = null;
  let unlistenCallCount = 0;

  beforeEach(() => {
    vi.clearAllMocks();
    updateEventHandler = null;
    unlistenCallCount = 0;
    setTauriRuntime(true);
    mockGetVersion.mockResolvedValue(mockedRuntimeVersion);
    mockInvoke.mockResolvedValue({
      enabled: true,
      channel: 'stable',
      reason: null,
      windows_handoff: false,
    });
    mockListen.mockImplementation(async (_eventName, handler) => {
      updateEventHandler = handler as (event: { payload: DownloadEvent }) => void;
      return () => {
        unlistenCallCount += 1;
      };
    });
    mockCheck.mockResolvedValue(null);
  });

  it('loads the current app version and silently checks for updates', async () => {
    render(<Probe />);

    await waitFor(() => expect(screen.getByTestId('version')).toHaveTextContent(mockedRuntimeVersion));
    await waitFor(() => expect(mockCheck).toHaveBeenCalledTimes(1));
    expect(screen.getByTestId('status')).toHaveTextContent('up-to-date');
    expect(screen.getByTestId('updates-enabled')).toHaveTextContent('true');
  });

  it('captures available update metadata', async () => {
    mockCheck.mockResolvedValue(makeUpdate({
      version: '0.3.6',
      date: '2026-05-20',
      body: 'Bug fixes',
    }) as unknown as Awaited<ReturnType<typeof check>>);

    render(<Probe />);

    await waitFor(() => {
      expect(screen.getByTestId('available-version')).toHaveTextContent('0.3.6');
      expect(screen.getByTestId('available-date')).toHaveTextContent('2026-05-20');
      expect(screen.getByTestId('available-body')).toHaveTextContent('Bug fixes');
      expect(screen.getByTestId('status')).toHaveTextContent('available');
    });
  });

  it('updates download progress and restarts through the native process after installation completes', async () => {
    const user = userEvent.setup();
    const update = makeUpdate({
      downloadAndInstall: async (onEvent) => {
        onEvent?.({ event: 'Started', data: { contentLength: 100 } });
        onEvent?.({ event: 'Progress', data: { chunkLength: 40 } });
        onEvent?.({ event: 'Progress', data: { chunkLength: 60 } });
        onEvent?.({ event: 'Finished' });
      },
    });
    mockCheck.mockResolvedValue(update as unknown as Awaited<ReturnType<typeof check>>);

    render(<Probe />);
    await screen.findByText('available');

    await user.click(screen.getByText('install'));

    await waitFor(() => expect(screen.getByTestId('status')).toHaveTextContent('installed'));
    expect(screen.getByTestId('downloaded')).toHaveTextContent('100');
    expect(screen.getByTestId('content-length')).toHaveTextContent('100');
    expect(screen.getByTestId('percent')).toHaveTextContent('100');
    expect(update.downloadAndInstall).toHaveBeenCalledTimes(1);
    expect(mockInvoke).toHaveBeenCalledWith('restart_app');
  });

  it('uses the Windows backend handoff and keeps progress in sync', async () => {
    const user = userEvent.setup();
    const update = makeUpdate();
    mockCheck.mockResolvedValue(update as unknown as Awaited<ReturnType<typeof check>>);
    mockInvoke.mockImplementation(async (command) => {
      if (command === 'get_update_eligibility') {
        return {
          enabled: true,
          channel: 'stable',
          reason: null,
          windows_handoff: true,
        };
      }

      if (command === 'install_update_with_windows_handoff') {
        updateEventHandler?.({ payload: { event: 'Started', data: { contentLength: 100 } } });
        updateEventHandler?.({ payload: { event: 'Progress', data: { chunkLength: 25 } } });
        updateEventHandler?.({ payload: { event: 'Progress', data: { chunkLength: 75 } } });
        updateEventHandler?.({ payload: { event: 'Finished' } });
        return null;
      }

      throw new Error(`Unexpected command: ${command}`);
    });

    render(<Probe />);
    await screen.findByText('available');

    await user.click(screen.getByText('install'));

    await waitFor(() => expect(screen.getByTestId('status')).toHaveTextContent('installed'));
    expect(screen.getByTestId('downloaded')).toHaveTextContent('100');
    expect(screen.getByTestId('content-length')).toHaveTextContent('100');
    expect(screen.getByTestId('percent')).toHaveTextContent('100');
    expect(mockListen).toHaveBeenCalledWith('wardian-update-download', expect.any(Function));
    expect(mockInvoke).toHaveBeenCalledWith('install_update_with_windows_handoff', {
      expectedVersion: update.version,
    });
    expect(unlistenCallCount).toBe(1);
    expect(update.downloadAndInstall).not.toHaveBeenCalled();
    expect(mockInvoke).not.toHaveBeenCalledWith('restart_app');
  });

  it('maps explicit update errors into visible state', async () => {
    const user = userEvent.setup();
    mockCheck.mockResolvedValueOnce(null);
    mockCheck.mockRejectedValueOnce(new Error('network failed'));

    render(<Probe />);
    await screen.findByText('up-to-date');

    await user.click(screen.getByText('check'));

    await waitFor(() => expect(screen.getByTestId('status')).toHaveTextContent('error'));
    expect(screen.getByTestId('error')).toHaveTextContent('network failed');
  });

  it('restarts through the native process from the explicit restart action', async () => {
    const user = userEvent.setup();
    render(<Probe />);

    await screen.findByText('up-to-date');
    expect(mockInvoke).not.toHaveBeenCalledWith('restart_app');

    await user.click(screen.getByText('restart'));

    expect(mockInvoke).toHaveBeenCalledWith('restart_app');
  });

  it('maps restart errors into visible state', async () => {
    const user = userEvent.setup();
    mockInvoke.mockImplementation(async (command) => {
      if (command === 'get_update_eligibility') {
        return {
          enabled: true,
          channel: 'stable',
          reason: null,
          windows_handoff: false,
        };
      }
      if (command === 'restart_app') {
        throw new Error('restart denied');
      }
      throw new Error(`Unexpected command: ${command}`);
    });
    render(<Probe />);

    await screen.findByText('up-to-date');
    await user.click(screen.getByText('restart'));

    await waitFor(() => expect(screen.getByTestId('status')).toHaveTextContent('error'));
    expect(screen.getByTestId('error')).toHaveTextContent('restart denied');
  });

  it('falls back to package version and disables updates outside the desktop runtime', async () => {
    setTauriRuntime(false);
    mockGetVersion.mockRejectedValue(new Error('Cannot read properties of undefined'));

    render(<Probe />);

    await waitFor(() => expect(screen.getByTestId('version')).toHaveTextContent(packageVersion));
    expect(screen.getByTestId('status')).toHaveTextContent('disabled');
    expect(screen.getByTestId('eligibility-reason')).toHaveTextContent(
      'Updates are unavailable outside the Wardian desktop runtime.',
    );
    expect(screen.getByTestId('updates-enabled')).toHaveTextContent('false');
    expect(screen.getByTestId('error')).toHaveTextContent('');
    expect(mockCheck).not.toHaveBeenCalled();
  });

  it('does not check GitHub updates from an ineligible desktop runtime', async () => {
    mockInvoke.mockResolvedValue({
      enabled: false,
      channel: null,
      reason: 'Updates are only available in official installed release builds.',
      windows_handoff: false,
    });

    render(<Probe />);

    await waitFor(() => expect(screen.getByTestId('version')).toHaveTextContent(mockedRuntimeVersion));
    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith('get_update_eligibility');
    });
    expect(screen.getByTestId('status')).toHaveTextContent('disabled');
    expect(screen.getByTestId('eligibility-reason')).toHaveTextContent(
      'Updates are only available in official installed release builds.',
    );
    expect(screen.getByTestId('updates-enabled')).toHaveTextContent('false');
    expect(mockCheck).not.toHaveBeenCalled();
  });
});
