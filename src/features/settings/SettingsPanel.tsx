import React, { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  MAX_TERMINAL_FONT_SIZE,
  MIN_TERMINAL_FONT_SIZE,
  defaultTerminalFontFamily,
  normalizeTerminalFontSize,
  useSettingsStore,
} from "../../store/useSettingsStore";
import { DocsLink } from "../../components/DocsLink";
import { useAppUpdate } from "./useAppUpdate";

interface SettingsPanelProps {}

const windowsAutoShellIds = ['pwsh', 'powershell', 'cmd', 'git-bash', 'wsl', 'bash'];
const posixAutoShellIds = ['zsh', 'bash', 'sh', 'fish'];
const CUSTOM_TERMINAL_FONT_FAMILY_VALUE = '__custom__';
const terminalFontFamilyOptions = [
  { label: 'Consolas', value: 'Consolas, "Courier New", monospace' },
  { label: 'Menlo', value: 'Menlo, Monaco, "Courier New", monospace' },
  { label: 'Droid Sans Mono', value: '"Droid Sans Mono", monospace' },
  { label: 'Cascadia Mono', value: '"Cascadia Mono", "Cascadia Code", Consolas, monospace' },
  { label: 'JetBrains Mono', value: 'JetBrains Mono, monospace' },
];

const resolveAutoShell = <T extends { id: string }>(availableShells: T[]) => {
  const hasWindowsShell = availableShells.some((option) =>
    ['pwsh', 'powershell', 'cmd', 'git-bash', 'wsl'].includes(option.id),
  );
  const preferredIds = hasWindowsShell ? windowsAutoShellIds : posixAutoShellIds;
  return preferredIds
    .map((id) => availableShells.find((option) => option.id === id))
    .find((option) => option !== undefined) ?? availableShells[0];
};

const updateStatusLabel = (
  status: ReturnType<typeof useAppUpdate>['status'],
  availableVersion: string | undefined,
  progressPercent: number | null,
  errorMessage: string,
) => {
  switch (status) {
    case 'checking':
      return 'Checking for updates...';
    case 'up-to-date':
      return 'Wardian is up to date.';
    case 'available':
      return `Wardian v${availableVersion ?? 'latest'} is available.`;
    case 'downloading':
      return progressPercent === null ? 'Downloading update...' : `Downloading update... ${progressPercent}%`;
    case 'installed':
      return 'Update installed. Restart to finish.';
    case 'disabled':
      return errorMessage || 'Updates are unavailable for this build.';
    case 'error':
      return errorMessage || 'Update check failed.';
    case 'idle':
    default:
      return 'Update status idle.';
  }
};

const updateStatusColor = (status: ReturnType<typeof useAppUpdate>['status']) => {
  switch (status) {
    case 'available':
    case 'installed':
      return 'var(--color-wardian-accent)';
    case 'checking':
    case 'downloading':
      return 'var(--color-wardian-processing)';
    case 'error':
      return 'var(--color-wardian-error)';
    case 'disabled':
    case 'up-to-date':
    case 'idle':
    default:
      return 'var(--color-wardian-text-muted-neutral)';
  }
};

export const SettingsPanel: React.FC<SettingsPanelProps> = () => {
  const {
    theme,
    setTheme,
    autoPatchGemini,
    setAutoPatchGemini,
    terminalFontSize,
    setTerminalFontSize,
    terminalFontFamily,
    setTerminalFontFamily,
    shell_id,
    custom_executable,
    custom_args,
    agent_session_persistence,
    codex_runtime_policy,
    available_shells,
    shell_settings_loaded,
    shells_loaded,
    setShellId,
    setCustomExecutable,
    setCustomArgs,
    setAgentSessionPersistence,
    setCodexSandboxMode,
    setCodexApprovalPolicy,
    setCodexFullAuto,
    loadShellSettings,
    loadAvailableShells,
    saveShellSettings,
  } = useSettingsStore();
  const [patchStatus, setPatchStatus] = useState<"idle" | "running" | "success" | "error">("idle");
  const [patchMessage, setPatchMessage] = useState("");
  const [shellStatus, setShellStatus] = useState<"idle" | "saving" | "success" | "error">("idle");
  const [shellMessage, setShellMessage] = useState("");
  const [agentRuntimeStatus, setAgentRuntimeStatus] = useState<"idle" | "saving" | "success" | "error">("idle");
  const [agentRuntimeMessage, setAgentRuntimeMessage] = useState("");
  const [terminalFontSizeDraft, setTerminalFontSizeDraft] = useState(() => String(terminalFontSize));
  const [terminalFontFamilyMode, setTerminalFontFamilyMode] = useState<'preset' | 'custom'>(() =>
    terminalFontFamily === '' || terminalFontFamilyOptions.some((option) => option.value === terminalFontFamily)
      ? 'preset'
      : 'custom',
  );
  const appUpdate = useAppUpdate();

  useEffect(() => {
    if (!shell_settings_loaded) {
      loadShellSettings();
    }
    if (!shells_loaded) {
      loadAvailableShells();
    }
  }, [loadAvailableShells, loadShellSettings, shell_settings_loaded, shells_loaded]);

  useEffect(() => {
    setTerminalFontSizeDraft(String(terminalFontSize));
  }, [terminalFontSize]);

  const commitTerminalFontSizeDraft = () => {
    const nextSize = normalizeTerminalFontSize(Number(terminalFontSizeDraft));
    setTerminalFontSize(nextSize);
    setTerminalFontSizeDraft(String(nextSize));
  };

  const handleTerminalFontSizeChange = (value: string) => {
    setTerminalFontSizeDraft(value);
    const nextSize = Number(value);
    if (
      Number.isFinite(nextSize) &&
      nextSize >= MIN_TERMINAL_FONT_SIZE &&
      nextSize <= MAX_TERMINAL_FONT_SIZE
    ) {
      setTerminalFontSize(nextSize);
    }
  };

  const handleRunPatch = async () => {
    setPatchStatus("running");
    setPatchMessage("");
    try {
      await invoke<string>("run_gemini_patch");
      setPatchStatus("success");
      setPatchMessage("Gemini CLI patch applied successfully.");
      setTimeout(() => {
        setPatchStatus("idle");
        setPatchMessage("");
      }, 5000);
    } catch (e: unknown) {
      setPatchStatus("error");
      setPatchMessage(`Patch failed: ${String(e)}`);
    }
  };

  const handleSaveShell = async () => {
    setShellStatus("saving");
    setShellMessage("");
    try {
      await saveShellSettings();
      setShellStatus("success");
      setShellMessage("Shell settings updated.");
      setTimeout(() => {
        setShellStatus("idle");
        setShellMessage("");
      }, 4000);
    } catch (e: unknown) {
      setShellStatus("error");
      setShellMessage(`Shell update failed: ${String(e)}`);
    }
  };

  const handleSaveAgentRuntime = async () => {
    setAgentRuntimeStatus("saving");
    setAgentRuntimeMessage("");
    try {
      await saveShellSettings();
      setAgentRuntimeStatus("success");
      setAgentRuntimeMessage("Agent runtime updated.");
      setTimeout(() => {
        setAgentRuntimeStatus("idle");
        setAgentRuntimeMessage("");
      }, 4000);
    } catch (e: unknown) {
      setAgentRuntimeStatus("error");
      setAgentRuntimeMessage(`Agent runtime update failed: ${String(e)}`);
    }
  };

  const selectedShell = available_shells.find((option) => option.id === shell_id);
  const autoSelectedShell = resolveAutoShell(available_shells);
  const displayedShell = shell_id === 'auto' ? autoSelectedShell : selectedShell;
  const terminalFontFamilySelectValue = terminalFontFamilyMode === 'custom'
    ? CUSTOM_TERMINAL_FONT_FAMILY_VALUE
    : terminalFontFamily === '' || terminalFontFamilyOptions.some((option) => option.value === terminalFontFamily)
    ? terminalFontFamily
    : CUSTOM_TERMINAL_FONT_FAMILY_VALUE;
  const updateMessage = updateStatusLabel(
    appUpdate.status,
    appUpdate.availableUpdate?.version,
    appUpdate.progressPercent,
    appUpdate.status === 'disabled'
      ? appUpdate.updateEligibilityReason
      : appUpdate.errorMessage,
  );
  const updateBusy = appUpdate.status === 'checking' || appUpdate.status === 'downloading';
  const updatesDisabled = appUpdate.status === 'disabled';
  const canCheckForUpdates = appUpdate.updatesEnabled && !updatesDisabled;

  return (
    <div data-testid="settings-panel" className="flex flex-col h-full">
      <div className="flex justify-between items-center mb-6">
        <div>
          <h2 className="text-xl font-bold text-primary tracking-tight">Settings</h2>
          <p className="text-[11px] font-bold text-muted-neutral mt-1">
            Wardian v{appUpdate.currentVersion || '...'}
          </p>
        </div>
        <DocsLink path="/guide/getting-started">Getting Started</DocsLink>
      </div>
      
      <div className="flex flex-col gap-8 flex-1 overflow-y-auto pr-2 no-scrollbar">
        <div className="bg-transparent">
          <h3 className="text-[10px] font-bold text-muted-neutral tracking-wide mb-4">Updates</h3>
          <div className="bg-wardian-card-bg-muted border border-wardian-light/50 rounded-xl p-4 flex flex-col gap-3">
            <div className="flex flex-col gap-1">
              <p className="text-sm font-bold" style={{ color: updateStatusColor(appUpdate.status) }}>
                {updateMessage}
              </p>
              {appUpdate.status === 'downloading' && appUpdate.contentLength !== null && (
                <p className="text-[10px] text-muted-neutral">
                  {appUpdate.downloadedBytes} / {appUpdate.contentLength} bytes
                </p>
              )}
            </div>
            <div className="flex flex-wrap gap-2">
              {canCheckForUpdates && (
                <button
                  type="button"
                  onClick={() => void appUpdate.checkNow()}
                  disabled={updateBusy}
                  className={`px-4 py-2 text-xs font-bold rounded-lg border transition-all whitespace-nowrap ${
                    updateBusy
                      ? 'bg-wardian-border text-muted border-transparent cursor-not-allowed'
                      : 'bg-wardian-bg border-wardian-light text-primary hover:border-[var(--color-wardian-accent)] hover:text-[var(--color-wardian-accent)]'
                  }`}
                >
                  Check Now
                </button>
              )}
              {appUpdate.status === 'available' && (
                <button
                  type="button"
                  onClick={() => void appUpdate.downloadAndInstall()}
                  className="px-4 py-2 text-xs font-bold rounded-lg border transition-all whitespace-nowrap bg-wardian-bg border-wardian-light text-primary hover:border-[var(--color-wardian-accent)] hover:text-[var(--color-wardian-accent)]"
                >
                  Download & Install
                </button>
              )}
              {appUpdate.status === 'installed' && (
                <button
                  type="button"
                  onClick={() => void appUpdate.relaunchApp()}
                  className="px-4 py-2 text-xs font-bold rounded-lg border transition-all whitespace-nowrap bg-wardian-bg border-wardian-light text-primary hover:border-[var(--color-wardian-accent)] hover:text-[var(--color-wardian-accent)]"
                >
                  Restart
                </button>
              )}
            </div>
          </div>
        </div>

        <div className="bg-transparent">
          <h3 className="text-[10px] font-bold text-muted-neutral tracking-wide mb-4">Theme</h3>
          <div className="grid grid-cols-3 gap-3">
            <button
              data-testid="theme-system"
              type="button"
              onClick={() => setTheme("system")}
              className={`flex flex-col items-center gap-2 p-3 rounded-xl border transition-all ${theme === 'system' ? 'border-[var(--color-wardian-accent)] bg-[var(--color-wardian-accent)]/5 shadow-[0_0_15px_rgba(241,211,130,0.1)]' : 'border-wardian-border bg-wardian-card-bg-muted hover:border-wardian-border-heavy'}`}
            >
              <div className="w-full aspect-[4/3] rounded-md border border-wardian-border overflow-hidden flex shadow-inner">
                <div className="flex-1 bg-gray-900 border-r border-wardian-border"></div>
                <div className="flex-1 bg-gray-100"></div>
              </div>
              <span className={`text-[11px] font-bold tracking-tight ${theme === 'system' ? 'text-[var(--color-wardian-accent)]' : 'text-muted-neutral'}`}>System</span>
            </button>

            <button
              data-testid="theme-dark"
              type="button"
              onClick={() => setTheme("dark")}
              className={`flex flex-col items-center gap-2 p-3 rounded-xl border transition-all ${theme === 'dark' ? 'border-[var(--color-wardian-accent)] bg-[var(--color-wardian-accent)]/5 shadow-[0_0_15px_rgba(241,211,130,0.1)]' : 'border-wardian-border bg-wardian-card-bg-muted hover:border-wardian-border-heavy'}`}
            >
              <div className="w-full aspect-[4/3] rounded-md border border-wardian-border overflow-hidden flex bg-gray-900 shadow-inner p-1 gap-1">
                <div className="w-1.5 h-full rounded-sm bg-gray-800"></div>
                <div className="flex-1 flex flex-col gap-1">
                  <div className="w-full h-1.5 rounded-sm bg-gray-800"></div>
                  <div className="w-full h-full rounded-sm bg-gray-800/50"></div>
                </div>
              </div>
              <span className={`text-[11px] font-bold tracking-tight ${theme === 'dark' ? 'text-[var(--color-wardian-accent)]' : 'text-muted-neutral'}`}>Dark</span>
            </button>

            <button
              data-testid="theme-light"
              type="button"
              onClick={() => setTheme("light")}
              className={`flex flex-col items-center gap-2 p-3 rounded-xl border transition-all ${theme === 'light' ? 'border-[var(--color-wardian-accent)] bg-[var(--color-wardian-accent)]/5 shadow-[0_0_15px_rgba(241,211,130,0.1)]' : 'border-wardian-border bg-wardian-card-bg-muted hover:border-wardian-border-heavy'}`}
            >
              <div className="w-full aspect-[4/3] rounded-md border border-wardian-border overflow-hidden flex bg-[#fdfbf7] shadow-inner p-1 gap-1">
                <div className="w-1.5 h-full rounded-sm bg-gray-200"></div>
                <div className="flex-1 flex flex-col gap-1">
                  <div className="w-full h-1.5 rounded-sm bg-gray-200"></div>
                  <div className="w-full h-full rounded-sm bg-gray-100/50"></div>
                </div>
              </div>
              <span className={`text-[11px] font-bold tracking-tight ${theme === 'light' ? 'text-[var(--color-wardian-accent)]' : 'text-muted-neutral'}`}>Light</span>
            </button>
          </div>
          <div className="mt-3 px-1">
            <p className="text-[10px] text-muted-neutral leading-relaxed">
              <span className="text-[var(--color-wardian-accent)] font-bold">NOTE:</span> Gemini and OpenCode agents may require a manual theme change and restart.
            </p>
          </div>
        </div>

        <div className="border-t border-wardian-border pt-6">
          <h3 className="text-[10px] font-bold text-muted-neutral tracking-wide mb-4">Agent Runtime</h3>

          <div className="bg-wardian-card-bg-muted border border-wardian-light/50 rounded-xl p-4 flex flex-col gap-3">
            <label className="text-sm font-bold text-primary" htmlFor="agent-session-persistence">
              Regular agent sessions
            </label>
            <select
              id="agent-session-persistence"
              value={agent_session_persistence}
              onChange={(e) => setAgentSessionPersistence(e.target.value as 'fresh' | 'resume')}
              className="w-full rounded-lg border border-wardian-border bg-wardian-input-bg px-3 py-2 text-sm text-primary outline-none focus:border-[var(--color-wardian-accent)]"
            >
              <option value="resume">Resume sessions</option>
              <option value="fresh">Start fresh</option>
            </select>
            <p className="text-[10px] text-muted-neutral leading-relaxed">
              Applies when regular visible agents are resumed from Off. Workflow agent nodes use their own run mode.
            </p>

            <div className="border-t border-wardian-border pt-3 mt-1 flex flex-col gap-3">
              <div>
                <h4 className="text-[10px] font-bold text-muted-neutral tracking-wide">Codex Runtime Defaults</h4>
                <p className="text-[10px] text-muted-neutral leading-relaxed mt-1">
                  Used when Codex agents do not set explicit advanced sandbox or approval overrides.
                </p>
              </div>

              <div className="grid grid-cols-1 gap-3">
                <div className="flex flex-col gap-2">
                  <label className="text-xs font-bold text-primary" htmlFor="codex-sandbox-mode">
                    Codex sandbox
                  </label>
                  <select
                    id="codex-sandbox-mode"
                    value={codex_runtime_policy.sandbox_mode}
                    onChange={(e) => setCodexSandboxMode(e.target.value as typeof codex_runtime_policy.sandbox_mode)}
                    className="w-full rounded-lg border border-wardian-border bg-wardian-input-bg px-3 py-2 text-sm text-primary outline-none focus:border-[var(--color-wardian-accent)]"
                  >
                    <option value="danger-full-access">Full access</option>
                    <option value="workspace-write">Workspace write</option>
                    <option value="read-only">Read only</option>
                  </select>
                </div>

                <div className="flex flex-col gap-2">
                  <label className="text-xs font-bold text-primary" htmlFor="codex-approval-policy">
                    Codex approval
                  </label>
                  <select
                    id="codex-approval-policy"
                    value={codex_runtime_policy.approval_policy}
                    onChange={(e) => setCodexApprovalPolicy(e.target.value as typeof codex_runtime_policy.approval_policy)}
                    className="w-full rounded-lg border border-wardian-border bg-wardian-input-bg px-3 py-2 text-sm text-primary outline-none focus:border-[var(--color-wardian-accent)]"
                  >
                    <option value="never">Never</option>
                    <option value="on-request">On request</option>
                    <option value="on-failure">On failure</option>
                    <option value="untrusted">Untrusted</option>
                  </select>
                </div>
              </div>

              <label className="text-sm font-bold text-primary flex items-start gap-2 cursor-pointer select-none">
                <input
                  type="checkbox"
                  aria-label="Autonomous full access, no prompts"
                  checked={codex_runtime_policy.full_auto}
                  onChange={(e) => setCodexFullAuto(e.target.checked)}
                  className="w-4 h-4 mt-0.5 rounded border-wardian-border text-[var(--color-wardian-accent)] focus:ring-[var(--color-wardian-accent)] bg-wardian-input-bg"
                />
                <span className="flex flex-col gap-1">
                  <span>Autonomous full access, no prompts</span>
                  <span className="text-[10px] font-normal text-muted-neutral leading-relaxed">
                    Wardian maps this friendly default to the current Codex autonomous execution mode.
                  </span>
                </span>
              </label>
            </div>

            <div className="flex justify-end">
              <button
                type="button"
                onClick={handleSaveAgentRuntime}
                disabled={!shell_settings_loaded || agentRuntimeStatus === 'saving'}
                className={`px-4 py-2 text-xs font-bold rounded-lg border transition-all whitespace-nowrap ${
                  agentRuntimeStatus === 'saving'
                    ? 'bg-wardian-border text-muted border-transparent cursor-not-allowed'
                    : 'bg-wardian-bg border-wardian-light text-primary hover:border-[var(--color-wardian-accent)] hover:text-[var(--color-wardian-accent)]'
                }`}
              >
                {agentRuntimeStatus === 'saving' ? 'Saving...' : 'Save Agent Runtime'}
              </button>
            </div>
            {agentRuntimeMessage && (
              <div className={`p-2 mt-1 rounded border text-xs font-medium text-left ${
                agentRuntimeStatus === 'success' ? 'bg-cyan-500/10 border-cyan-500/20 text-cyan-400' :
                'bg-red-500/10 border-red-500/20 text-red-400'
              }`}>
                {agentRuntimeMessage}
              </div>
            )}
          </div>
        </div>

        <div className="border-t border-wardian-border pt-6">
          <h3 className="text-[10px] font-bold text-muted-neutral tracking-wide mb-4">Terminal</h3>

          <div className="flex flex-col gap-3">
            <div className="bg-wardian-card-bg-muted border border-wardian-light/50 rounded-xl p-4 flex flex-col gap-3">
              <h4 className="text-[10px] font-bold text-muted-neutral tracking-wide">Shell</h4>
              <label className="text-sm font-bold text-primary" htmlFor="default-shell-select">
                Shell / Interpreter
              </label>
              <select
                data-testid="shell-select"
                id="default-shell-select"
                value={shell_id}
                onChange={(e) => setShellId(e.target.value)}
                className="w-full rounded-lg border border-wardian-border bg-wardian-input-bg px-3 py-2 text-sm text-primary outline-none focus:border-[var(--color-wardian-accent)]"
              >
                <option value="auto">Auto</option>
                {available_shells.map((option) => (
                  <option key={option.id} value={option.id}>
                    {option.label}
                  </option>
                ))}
                <option value="custom">Custom</option>
              </select>

              {shell_id === 'custom' ? (
                <>
                  <label className="text-xs font-bold text-primary" htmlFor="custom-shell-executable">
                    Custom executable
                  </label>
                  <input
                    data-testid="custom-shell-executable"
                    id="custom-shell-executable"
                    type="text"
                    value={custom_executable}
                    onChange={(e) => setCustomExecutable(e.target.value)}
                    placeholder={navigator.platform.toLowerCase().includes('win') ? 'C:/Program Files/PowerShell/7/pwsh.exe' : '/usr/local/bin/fish'}
                    className="w-full rounded-lg border border-wardian-border bg-wardian-input-bg px-3 py-2 text-sm text-primary outline-none focus:border-[var(--color-wardian-accent)]"
                  />
                  <label className="text-xs font-bold text-primary" htmlFor="custom-shell-args">
                    Command args
                  </label>
                  <input
                    id="custom-shell-args"
                    type="text"
                    value={custom_args}
                    onChange={(e) => setCustomArgs(e.target.value)}
                    placeholder={navigator.platform.toLowerCase().includes('win') ? '-NoProfile -Command' : '-lc'}
                    className="w-full rounded-lg border border-wardian-border bg-wardian-input-bg px-3 py-2 text-sm text-primary outline-none focus:border-[var(--color-wardian-accent)]"
                  />
                </>
              ) : (
                <div className="rounded-lg border border-wardian-border bg-wardian-bg px-3 py-2">
                  <p className="text-[11px] font-bold text-primary">
                    {shell_id === 'auto'
                      ? (displayedShell ? `Auto: ${displayedShell.label}` : 'Auto: detecting shell...')
                      : displayedShell?.label ?? 'Loading shell details...'}
                  </p>
                  {displayedShell && (
                    <p className="text-[10px] text-muted-neutral mt-1 break-all">
                      {displayedShell.executable}
                    </p>
                  )}
                </div>
              )}

              <div className="flex items-center justify-between gap-3">
                <p className="text-[10px] text-muted-neutral">
                  {shells_loaded ? `${available_shells.length} discovered shell${available_shells.length === 1 ? '' : 's'}` : 'Detecting installed shells...'}
                </p>
                <button
                  type="button"
                  onClick={handleSaveShell}
                  disabled={!shell_settings_loaded || shellStatus === 'saving'}
                  className={`px-4 py-2 text-xs font-bold rounded-lg border transition-all whitespace-nowrap ${
                    shellStatus === 'saving'
                      ? 'bg-wardian-border text-muted border-transparent cursor-not-allowed'
                      : 'bg-wardian-bg border-wardian-light text-primary hover:border-[var(--color-wardian-accent)] hover:text-[var(--color-wardian-accent)]'
                  }`}
                >
                  {shellStatus === 'saving' ? 'Saving...' : 'Save Shell'}
                </button>
              </div>

              {shellMessage && (
                <div className={`p-2 mt-1 rounded border text-xs font-medium text-left ${
                  shellStatus === 'success' ? 'bg-cyan-500/10 border-cyan-500/20 text-cyan-400' :
                  'bg-red-500/10 border-red-500/20 text-red-400'
                }`}>
                  {shellMessage}
                </div>
              )}
            </div>

            <div className="bg-wardian-card-bg-muted border border-wardian-light/50 rounded-xl p-4 flex flex-col gap-3">
              <h4 className="text-[10px] font-bold text-muted-neutral tracking-wide">Appearance</h4>
              <label className="text-sm font-bold text-primary" htmlFor="terminal-font-size">
                Terminal font size
              </label>
              <input
                id="terminal-font-size"
                type="number"
                min={MIN_TERMINAL_FONT_SIZE}
                max={MAX_TERMINAL_FONT_SIZE}
                step={1}
                value={terminalFontSizeDraft}
                onBlur={commitTerminalFontSizeDraft}
                onChange={(e) => handleTerminalFontSizeChange(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === 'Enter') {
                    commitTerminalFontSizeDraft();
                  }
                }}
                className="w-full rounded-lg border border-wardian-border bg-wardian-input-bg px-3 py-2 text-sm text-primary outline-none focus:border-[var(--color-wardian-accent)]"
              />
              <p className="text-[10px] text-muted-neutral leading-relaxed">
                Applies immediately to embedded agent terminals.
              </p>
              <label className="text-sm font-bold text-primary" htmlFor="terminal-font-family">
                Terminal font family
              </label>
              <select
                id="terminal-font-family"
                value={terminalFontFamilySelectValue}
                onChange={(e) => {
                  if (e.target.value === CUSTOM_TERMINAL_FONT_FAMILY_VALUE) {
                    setTerminalFontFamilyMode('custom');
                  } else {
                    setTerminalFontFamilyMode('preset');
                    setTerminalFontFamily(e.target.value);
                  }
                }}
                className="w-full rounded-lg border border-wardian-border bg-wardian-input-bg px-3 py-2 text-sm text-primary outline-none focus:border-[var(--color-wardian-accent)]"
              >
                <option value="">Auto</option>
                {terminalFontFamilyOptions.map((option) => (
                  <option key={option.label} value={option.value}>
                    {option.label}
                  </option>
                ))}
                <option value={CUSTOM_TERMINAL_FONT_FAMILY_VALUE}>Custom...</option>
              </select>
              {terminalFontFamilySelectValue === CUSTOM_TERMINAL_FONT_FAMILY_VALUE && (
                <>
                  <label className="text-xs font-bold text-primary" htmlFor="custom-terminal-font-family">
                    Custom terminal font family
                  </label>
                  <input
                    id="custom-terminal-font-family"
                    type="text"
                    value={terminalFontFamily}
                    onChange={(e) => setTerminalFontFamily(e.target.value)}
                    placeholder={defaultTerminalFontFamily()}
                    className="w-full rounded-lg border border-wardian-border bg-wardian-input-bg px-3 py-2 text-sm text-primary outline-none focus:border-[var(--color-wardian-accent)]"
                  />
                </>
              )}
              <p className="text-[10px] text-muted-neutral leading-relaxed">
                Auto uses {defaultTerminalFontFamily()}.
              </p>
            </div>
          </div>
        </div>

        <div className="border-t border-wardian-border pt-6">
          <h3 className="text-[10px] font-bold text-muted-neutral tracking-wide mb-4">Advanced</h3>
          
          <div className="bg-wardian-card-bg-muted border border-wardian-light/50 rounded-xl p-4 flex flex-col gap-3">
            <label className="text-sm font-bold text-primary flex items-center gap-2 cursor-pointer select-none">
              <input 
                type="checkbox" 
                checked={autoPatchGemini}
                onChange={(e) => setAutoPatchGemini(e.target.checked)}
                className="w-4 h-4 rounded border-wardian-border text-[var(--color-wardian-accent)] focus:ring-[var(--color-wardian-accent)] bg-wardian-input-bg"
              />
              Auto-patch Gemini CLI
            </label>
            
            <div className="flex justify-center my-2">
              <button 
                onClick={handleRunPatch}
                disabled={patchStatus === "running"}
                className={`px-6 py-2 text-xs font-bold rounded-lg border transition-all whitespace-nowrap ${
                  patchStatus === "running" ? "bg-wardian-border text-muted border-transparent cursor-not-allowed" : 
                  "bg-wardian-bg border-wardian-light text-primary hover:border-[var(--color-wardian-accent)] hover:text-[var(--color-wardian-accent)]"
                }`}
              >
                {patchStatus === "running" ? "Patching..." : "Run Patch Now"}
              </button>
            </div>
            

            {patchMessage && (
              <div className={`p-2 mt-1 rounded border text-xs font-medium text-left ${
                patchStatus === "success" ? "bg-green-500/10 border-green-500/20 text-green-400" : 
                "bg-red-500/10 border-red-500/20 text-red-400"
              }`}>
                {patchMessage}
              </div>
            )}
          </div>
        </div>

        <div className="text-center p-4 mt-auto">
          <p className="text-[10px] text-muted-neutral italic">More settings coming in Phase 3.</p>
        </div>
      </div>
    </div>
  );
};
