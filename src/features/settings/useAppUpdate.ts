import { useCallback, useEffect, useRef, useState } from 'react';
import { getVersion } from '@tauri-apps/api/app';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { check, type DownloadEvent } from '@tauri-apps/plugin-updater';
import packageJson from '../../../package.json';

export type AppUpdateStatus =
  | 'idle'
  | 'checking'
  | 'up-to-date'
  | 'available'
  | 'downloading'
  | 'installed'
  | 'disabled'
  | 'error';

export interface AppUpdateInfo {
  version: string;
  date?: string;
  body?: string;
}

export interface AppUpdateState {
  currentVersion: string;
  availableUpdate: AppUpdateInfo | null;
  status: AppUpdateStatus;
  errorMessage: string;
  updatesEnabled: boolean;
  updateEligibilityReason: string;
  updateChannel: string | null;
  downloadedBytes: number;
  contentLength: number | null;
  progressPercent: number | null;
  checkNow: (options?: { silent?: boolean }) => Promise<void>;
  downloadAndInstall: () => Promise<void>;
  relaunchApp: () => Promise<void>;
}

type TauriUpdate = NonNullable<Awaited<ReturnType<typeof check>>>;

const NON_DESKTOP_UPDATE_REASON = 'Updates are unavailable outside the Wardian desktop runtime.';
const WINDOWS_UPDATE_DOWNLOAD_EVENT = 'wardian-update-download';

interface UpdateEligibility {
  enabled: boolean;
  channel?: string | null;
  reason?: string | null;
  windows_handoff?: boolean;
}

type TauriRuntimeWindow = Window & {
  __TAURI_INTERNALS__?: {
    invoke?: unknown;
  } | null;
};

const hasTauriRuntime = () =>
  typeof window !== 'undefined'
  && typeof (window as TauriRuntimeWindow).__TAURI_INTERNALS__?.invoke === 'function';

const errorMessageFrom = (error: unknown) => {
  if (error instanceof Error) {
    return error.message;
  }
  return String(error);
};

export const useAppUpdate = (): AppUpdateState => {
  const updateRef = useRef<TauriUpdate | null>(null);
  const updatesEnabledRef = useRef<boolean | null>(null);
  const windowsUpdateHandoffRef = useRef(false);
  const [currentVersion, setCurrentVersion] = useState('');
  const [availableUpdate, setAvailableUpdate] = useState<AppUpdateInfo | null>(null);
  const [status, setStatus] = useState<AppUpdateStatus>('idle');
  const [errorMessage, setErrorMessage] = useState('');
  const [updatesEnabled, setUpdatesEnabled] = useState(false);
  const [updateEligibilityReason, setUpdateEligibilityReason] = useState('');
  const [updateChannel, setUpdateChannel] = useState<string | null>(null);
  const [downloadedBytes, setDownloadedBytes] = useState(0);
  const [contentLength, setContentLength] = useState<number | null>(null);
  const [progressPercent, setProgressPercent] = useState<number | null>(null);

  const resetDownloadProgress = useCallback(() => {
    setDownloadedBytes(0);
    setContentLength(null);
    setProgressPercent(null);
  }, []);

  const loadUpdateEligibility = useCallback(async () => {
    if (!hasTauriRuntime()) {
      updatesEnabledRef.current = false;
      windowsUpdateHandoffRef.current = false;
      setUpdatesEnabled(false);
      setUpdateChannel(null);
      setUpdateEligibilityReason(NON_DESKTOP_UPDATE_REASON);
      setStatus('disabled');
      return false;
    }

    try {
      const eligibility = await invoke<UpdateEligibility>('get_update_eligibility');
      const enabled = eligibility.enabled;
      updatesEnabledRef.current = enabled;
      windowsUpdateHandoffRef.current = eligibility.windows_handoff ?? false;
      setUpdatesEnabled(enabled);
      setUpdateChannel(eligibility.channel ?? null);
      setUpdateEligibilityReason(eligibility.reason ?? '');
      if (!enabled) {
        updateRef.current = null;
        setAvailableUpdate(null);
        setStatus('disabled');
      }
      return enabled;
    } catch (error) {
      updateRef.current = null;
      updatesEnabledRef.current = false;
      windowsUpdateHandoffRef.current = false;
      setUpdatesEnabled(false);
      setUpdateChannel(null);
      setUpdateEligibilityReason('Unable to determine update eligibility.');
      setErrorMessage(errorMessageFrom(error));
      setStatus('disabled');
      return false;
    }
  }, []);

  const checkNow = useCallback(async (options?: { silent?: boolean }) => {
    const silent = options?.silent ?? false;
    if (!silent) {
      setErrorMessage('');
    }
    setStatus('checking');
    resetDownloadProgress();

    if (!hasTauriRuntime()) {
      updateRef.current = null;
      updatesEnabledRef.current = false;
      windowsUpdateHandoffRef.current = false;
      setUpdatesEnabled(false);
      setAvailableUpdate(null);
      setUpdateEligibilityReason(NON_DESKTOP_UPDATE_REASON);
      setStatus('disabled');
      return;
    }

    if (updatesEnabledRef.current !== true) {
      const enabled = updatesEnabledRef.current === null ? await loadUpdateEligibility() : false;
      if (!enabled) {
        setStatus('disabled');
        return;
      }
    }

    try {
      const update = await check();
      updateRef.current = update;
      if (update) {
        setAvailableUpdate({
          version: update.version,
          date: update.date,
          body: update.body,
        });
        setStatus('available');
      } else {
        setAvailableUpdate(null);
        setStatus('up-to-date');
      }
    } catch (error) {
      updateRef.current = null;
      if (silent) {
        setStatus('idle');
        return;
      }
      setErrorMessage(errorMessageFrom(error));
      setStatus('error');
    }
  }, [loadUpdateEligibility, resetDownloadProgress]);

  useEffect(() => {
    let cancelled = false;

    const loadVersionAndCheck = async () => {
      try {
        const version = await getVersion();
        if (!cancelled) {
          setCurrentVersion(version);
        }
      } catch (error) {
        if (!cancelled) {
          setCurrentVersion(packageJson.version);
          if (hasTauriRuntime()) {
            setErrorMessage(errorMessageFrom(error));
            setStatus('error');
          }
        }
      }

      if (!cancelled) {
        if (hasTauriRuntime()) {
          const enabled = await loadUpdateEligibility();
          if (enabled) {
            await checkNow({ silent: true });
          }
        } else {
          updatesEnabledRef.current = false;
          windowsUpdateHandoffRef.current = false;
          setUpdatesEnabled(false);
          setUpdateEligibilityReason(NON_DESKTOP_UPDATE_REASON);
          setStatus('disabled');
        }
      }
    };

    void loadVersionAndCheck();

    return () => {
      cancelled = true;
    };
  }, [checkNow, loadUpdateEligibility]);

  const downloadAndInstall = useCallback(async () => {
    const update = updateRef.current;
    if (!update) {
      setErrorMessage('No update is available.');
      setStatus('error');
      return;
    }

    setErrorMessage('');
    setStatus('downloading');
    resetDownloadProgress();
    let nextDownloadedBytes = 0;
    let nextContentLength: number | null = null;

    const handleDownloadEvent = (event: DownloadEvent) => {
      if (event.event === 'Started') {
        nextDownloadedBytes = 0;
        nextContentLength = event.data.contentLength ?? null;
        setDownloadedBytes(0);
        setContentLength(nextContentLength);
        setProgressPercent(null);
        return;
      }

      if (event.event === 'Progress') {
        nextDownloadedBytes += event.data.chunkLength;
        setDownloadedBytes(nextDownloadedBytes);
        if (nextContentLength && nextContentLength > 0) {
          setProgressPercent(Math.min(100, Math.round((nextDownloadedBytes / nextContentLength) * 100)));
        }
      }
    };

    try {
      if (windowsUpdateHandoffRef.current) {
        const unlisten = await listen<DownloadEvent>(WINDOWS_UPDATE_DOWNLOAD_EVENT, (event) => {
          handleDownloadEvent(event.payload);
        });
        try {
          await invoke('install_update_with_windows_handoff', {
            expectedVersion: update.version,
          });
        } finally {
          unlisten();
        }
      } else {
        await update.downloadAndInstall(handleDownloadEvent);
        setStatus('installed');
        await invoke('restart_app');
        return;
      }
      setStatus('installed');
    } catch (error) {
      setErrorMessage(errorMessageFrom(error));
      setStatus('error');
    }
  }, [resetDownloadProgress]);

  const relaunchApp = useCallback(async () => {
    setErrorMessage('');
    try {
      await invoke('restart_app');
    } catch (error) {
      setErrorMessage(errorMessageFrom(error));
      setStatus('error');
    }
  }, []);

  return {
    currentVersion,
    availableUpdate,
    status,
    errorMessage,
    updatesEnabled,
    updateEligibilityReason,
    updateChannel,
    downloadedBytes,
    contentLength,
    progressPercent,
    checkNow,
    downloadAndInstall,
    relaunchApp,
  };
};
