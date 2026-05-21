import React, { useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Check, Monitor, Search, X } from "lucide-react";
import {
  MAX_TERMINAL_FONT_SIZE,
  MIN_TERMINAL_FONT_SIZE,
  defaultTerminalFontFamily,
  normalizeTerminalFontSize,
  useSettingsStore,
} from "../../store/useSettingsStore";
import { useAppUpdate } from "./useAppUpdate";
import type { AppThemeSetting } from "../../types/settings";

interface SettingsModalProps {
  isOpen: boolean;
  onClose: () => void;
}

type SettingsCategory =
  | "General"
  | "Appearance"
  | "Terminal"
  | "Agent Runtime"
  | "Provider Utilities"
  | "Advanced";

type SettingsRowDefinition = {
  id: string;
  category: SettingsCategory;
  subgroup?: string;
  label: string;
  detail: string;
  keywords: string[];
};

const categories: SettingsCategory[] = [
  "General",
  "Appearance",
  "Terminal",
  "Agent Runtime",
  "Provider Utilities",
  "Advanced",
];

const rowDefinitions: SettingsRowDefinition[] = [
  {
    id: "version",
    category: "General",
    label: "Version",
    detail: "Current Wardian build and update status.",
    keywords: ["updates", "version", "release"],
  },
  {
    id: "theme",
    category: "Appearance",
    label: "Theme",
    detail: "Choose light, dark, or follow the operating system.",
    keywords: ["appearance", "color", "dark", "light"],
  },
  {
    id: "terminal-font-size",
    category: "Terminal",
    label: "Terminal font size",
    detail: "Applies immediately to embedded agent terminals.",
    keywords: ["terminal", "font", "size"],
  },
  {
    id: "terminal-font-family",
    category: "Terminal",
    label: "Terminal font family",
    detail: "Uses the platform terminal font unless overridden.",
    keywords: ["terminal", "font", "family", "monospace"],
  },
  {
    id: "shell",
    category: "Terminal",
    label: "Integrated terminal shell",
    detail: "Choose which shell opens in Wardian terminals.",
    keywords: ["terminal", "shell", "interpreter"],
  },
  {
    id: "custom-shell-executable",
    category: "Terminal",
    label: "Custom shell executable",
    detail: "Used only when the integrated shell is set to Custom.",
    keywords: ["terminal", "shell", "custom", "executable", "path"],
  },
  {
    id: "custom-shell-args",
    category: "Terminal",
    label: "Custom shell arguments",
    detail: "Optional arguments passed to the custom shell.",
    keywords: ["terminal", "shell", "custom", "arguments", "args"],
  },
  {
    id: "default-provider",
    category: "Agent Runtime",
    label: "Default provider",
    detail: "Auto prefers Claude when available.",
    keywords: ["agent", "provider", "runtime", "claude", "codex", "gemini", "antigravity", "opencode"],
  },
  {
    id: "session-persistence",
    category: "Agent Runtime",
    label: "Regular agent sessions",
    detail: "Controls whether off agents resume or start fresh.",
    keywords: ["agent", "session", "resume", "fresh"],
  },
  {
    id: "codex-sandbox",
    category: "Agent Runtime",
    subgroup: "Codex",
    label: "Sandbox",
    detail: "Default for Codex agents without an explicit override.",
    keywords: ["codex", "sandbox", "runtime", "permissions"],
  },
  {
    id: "codex-approval",
    category: "Agent Runtime",
    subgroup: "Codex",
    label: "Approval",
    detail: "Default approval policy for Codex launches.",
    keywords: ["codex", "approval", "runtime", "permissions"],
  },
  {
    id: "codex-full-auto",
    category: "Agent Runtime",
    subgroup: "Codex",
    label: "Autonomous mode",
    detail: "Maps to Codex full-auto execution when enabled.",
    keywords: ["codex", "full auto", "autonomous"],
  },
  {
    id: "gemini-patch",
    category: "Provider Utilities",
    label: "Auto-patch Gemini CLI",
    detail: "Helps Gemini discover Wardian skills.",
    keywords: ["gemini", "patch", "provider"],
  },
  {
    id: "settings-files",
    category: "Advanced",
    label: "Settings files",
    detail: "Global settings live under <WARDIAN_HOME>/settings/.",
    keywords: ["json", "files", "wardian_home", "advanced"],
  },
];

const terminalFontFamilyOptions = [
  { label: "Consolas", value: 'Consolas, "Courier New", monospace' },
  { label: "Menlo", value: 'Menlo, Monaco, "Courier New", monospace' },
  { label: "Droid Sans Mono", value: '"Droid Sans Mono", monospace' },
  { label: "Cascadia Mono", value: '"Cascadia Mono", "Cascadia Code", Consolas, monospace' },
  { label: "JetBrains Mono", value: "JetBrains Mono, monospace" },
];

const optionClass =
  "min-w-[220px] rounded-md border border-wardian-border bg-wardian-input-bg px-3 py-2 text-sm text-primary outline-none focus:border-[var(--color-wardian-accent)]";

const buttonClass =
  "rounded-md border border-wardian-border bg-wardian-bg px-3 py-2 text-xs font-semibold text-primary transition-colors hover:border-[var(--color-wardian-accent)] disabled:cursor-not-allowed disabled:opacity-60";

const friendlyFontFamilyName = (fontFamily: string) =>
  fontFamily
    .split(",")[0]
    ?.trim()
    .replace(/^["']|["']$/g, "") || "system monospace";

const SettingRow: React.FC<{
  label: string;
  detail: string;
  children: React.ReactNode;
  onReset?: () => void;
  resetLabel?: string;
}> = ({ label, detail, children, onReset, resetLabel }) => (
  <div className="grid min-h-[66px] grid-cols-[minmax(220px,1fr)_minmax(220px,320px)] items-center gap-6 border-b border-wardian-border px-4 py-3 last:border-b-0">
    <div className="min-w-0">
      <div className="text-sm font-medium text-primary">{label}</div>
      <div className="mt-1 text-xs leading-relaxed text-muted-neutral">{detail}</div>
    </div>
    <div className="flex items-center justify-end gap-2">
      {children}
      {onReset && (
        <button
          type="button"
          aria-label={resetLabel ?? `Reset ${label}`}
          onClick={onReset}
          className="rounded-md border border-wardian-border px-2 py-1 text-[11px] font-semibold text-muted-neutral transition-colors hover:text-primary"
        >
          Reset
        </button>
      )}
    </div>
  </div>
);

const renderRowsWithSubgroups = (
  rows: SettingsRowDefinition[],
  renderRow: (row: SettingsRowDefinition) => React.ReactNode,
) => {
  let activeSubgroup: string | undefined;
  return rows.flatMap((row) => {
    const nodes: React.ReactNode[] = [];
    if (row.subgroup && row.subgroup !== activeSubgroup) {
      activeSubgroup = row.subgroup;
      nodes.push(
        <div
          key={`${row.subgroup}-subgroup`}
          role="heading"
          aria-level={4}
          className="border-b border-wardian-border bg-wardian-bg/45 px-4 py-2 text-[11px] font-semibold uppercase tracking-wide text-muted-neutral"
        >
          {row.subgroup}
        </div>,
      );
    } else if (!row.subgroup) {
      activeSubgroup = undefined;
    }
    nodes.push(renderRow(row));
    return nodes;
  });
};

export const SettingsModal: React.FC<SettingsModalProps> = ({ isOpen, onClose }) => {
  const dialogRef = useRef<HTMLElement | null>(null);
  const closeButtonRef = useRef<HTMLButtonElement | null>(null);
  const [activeCategory, setActiveCategory] = useState<SettingsCategory>("General");
  const [query, setQuery] = useState("");
  const [terminalStatus, setTerminalStatus] = useState<"idle" | "saving" | "saved" | "error">("idle");
  const [terminalMessage, setTerminalMessage] = useState("");
  const [runtimeStatus, setRuntimeStatus] = useState<"idle" | "saving" | "saved" | "error">("idle");
  const [runtimeMessage, setRuntimeMessage] = useState("");
  const [patchStatus, setPatchStatus] = useState<"idle" | "running" | "saved" | "error">("idle");
  const [terminalFontSizeDraft, setTerminalFontSizeDraft] = useState("14");
  const appUpdate = useAppUpdate();

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
    agent_session_persistence,
    default_provider,
    codex_runtime_policy,
    available_shells,
    shell_settings_loaded,
    shells_loaded,
    setShellId,
    custom_executable,
    custom_args,
    setCustomExecutable,
    setCustomArgs,
    setAgentSessionPersistence,
    setDefaultProvider,
    setCodexSandboxMode,
    setCodexApprovalPolicy,
    setCodexFullAuto,
    loadShellSettings,
    loadAvailableShells,
    saveShellSettings,
  } = useSettingsStore();

  useEffect(() => {
    if (!isOpen) return;
    closeButtonRef.current?.focus();
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        onClose();
        return;
      }
      if (event.key === "Tab") {
        const focusable = Array.from(
          dialogRef.current?.querySelectorAll<HTMLElement>(
            'button:not([disabled]), [href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])',
          ) ?? [],
        );
        if (focusable.length === 0) {
          return;
        }
        const first = focusable[0];
        const last = focusable[focusable.length - 1];
        if (event.shiftKey && document.activeElement === first) {
          event.preventDefault();
          last.focus();
        } else if (!event.shiftKey && document.activeElement === last) {
          event.preventDefault();
          first.focus();
        }
      }
    };
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [isOpen, onClose]);

  useEffect(() => {
    if (!isOpen) return;
    if (!shell_settings_loaded) {
      void loadShellSettings();
    }
    if (!shells_loaded) {
      void loadAvailableShells();
    }
  }, [isOpen, loadAvailableShells, loadShellSettings, shell_settings_loaded, shells_loaded]);

  useEffect(() => {
    setTerminalFontSizeDraft(String(terminalFontSize));
  }, [terminalFontSize]);

  const visibleRows = useMemo(() => {
    const normalizedQuery = query.trim().toLowerCase();
    const rowsForCurrentShell = rowDefinitions.filter(
      (row) => shell_id === "custom" || (row.id !== "custom-shell-executable" && row.id !== "custom-shell-args"),
    );
    if (normalizedQuery) {
      return rowsForCurrentShell.filter((row) =>
        [row.label, row.detail, row.category, ...row.keywords]
          .join(" ")
          .toLowerCase()
          .includes(normalizedQuery),
      );
    }
    return rowsForCurrentShell.filter((row) => row.category === activeCategory);
  }, [activeCategory, query, shell_id]);

  const groupedRows = categories
    .map((category) => ({
      category,
      rows: visibleRows.filter((row) => row.category === category),
    }))
    .filter((group) => group.rows.length > 0);

  const defaultTerminalFontLabel = friendlyFontFamilyName(defaultTerminalFontFamily());
  const defaultShellLabel = available_shells[0]?.label;

  const updateTheme = async (nextTheme: AppThemeSetting) => {
    setTheme(nextTheme);
    await useSettingsStore.getState().saveAppSettings();
  };

  const handleTerminalFontSizeChange = async (value: string) => {
    setTerminalFontSizeDraft(value);
    const numeric = Number(value);
    if (Number.isFinite(numeric) && numeric >= MIN_TERMINAL_FONT_SIZE && numeric <= MAX_TERMINAL_FONT_SIZE) {
      setTerminalFontSize(numeric);
      await useSettingsStore.getState().saveAppSettings();
    }
  };

  const handleTerminalFontSizeBlur = async () => {
    const nextSize = normalizeTerminalFontSize(Number(terminalFontSizeDraft));
    setTerminalFontSize(nextSize);
    setTerminalFontSizeDraft(String(nextSize));
    await useSettingsStore.getState().saveAppSettings();
  };

  const handleTerminalFontFamilyChange = async (value: string) => {
    setTerminalFontFamily(value);
    await useSettingsStore.getState().saveAppSettings();
  };

  const handleSaveRuntime = async () => {
    setRuntimeStatus("saving");
    setRuntimeMessage("");
    try {
      await saveShellSettings();
      setRuntimeStatus("saved");
      setRuntimeMessage("Agent runtime updated.");
    } catch (error) {
      setRuntimeStatus("error");
      setRuntimeMessage(`Agent runtime update failed: ${String(error)}`);
    }
  };

  const handleSaveTerminal = async () => {
    setTerminalStatus("saving");
    setTerminalMessage("");
    try {
      await saveShellSettings();
      setTerminalStatus("saved");
      setTerminalMessage("Terminal settings updated.");
    } catch (error) {
      setTerminalStatus("error");
      setTerminalMessage(`Terminal update failed: ${String(error)}`);
    }
  };

  const handleRunPatch = async () => {
    setPatchStatus("running");
    try {
      await invoke("run_gemini_patch");
      setPatchStatus("saved");
    } catch {
      setPatchStatus("error");
    }
  };

  const handleAutoPatchGeminiChange = async (enabled: boolean) => {
    setAutoPatchGemini(enabled);
    await useSettingsStore.getState().saveAppSettings();
  };

  const updateDetail = () => {
    if (appUpdate.status === "available" && appUpdate.availableUpdate) {
      return `Update ${appUpdate.availableUpdate.version} is available.`;
    }
    if (appUpdate.status === "disabled") {
      return appUpdate.updateEligibilityReason || "Updates are unavailable in this runtime.";
    }
    if (appUpdate.status === "error") {
      return appUpdate.errorMessage || "Unable to check for updates.";
    }
    if (appUpdate.status === "checking") {
      return "Checking for updates...";
    }
    if (appUpdate.status === "installed") {
      return "Update installed. Restart to finish.";
    }
    return "Current Wardian build and update status.";
  };

  const renderRow = (row: SettingsRowDefinition) => {
    switch (row.id) {
      case "version":
        return (
          <SettingRow key={row.id} label={row.label} detail={updateDetail()}>
            <div className="flex flex-wrap items-center justify-end gap-2 text-sm text-primary">
              <div className="flex items-center gap-2">
                <Check className="h-4 w-4 text-[var(--color-wardian-accent)]" />
                <span>Wardian v{appUpdate.currentVersion || "..."}</span>
              </div>
              {appUpdate.status === "available" ? (
                <button type="button" onClick={() => void appUpdate.downloadAndInstall()} className={buttonClass}>
                  Install update
                </button>
              ) : appUpdate.status === "installed" ? (
                <button type="button" onClick={() => void appUpdate.relaunchApp()} className={buttonClass}>
                  Restart
                </button>
              ) : (
                <button
                  type="button"
                  onClick={() => void appUpdate.checkNow()}
                  disabled={!appUpdate.updatesEnabled || appUpdate.status === "checking"}
                  className={buttonClass}
                >
                  {appUpdate.status === "checking" ? "Checking..." : "Check for updates"}
                </button>
              )}
            </div>
          </SettingRow>
        );
      case "theme":
        return (
          <SettingRow
            key={row.id}
            label={row.label}
            detail={row.detail}
            onReset={() => void updateTheme("system")}
            resetLabel="Reset Theme"
          >
            <select
              aria-label="Theme"
              value={theme}
              onChange={(event) => void updateTheme(event.target.value as AppThemeSetting)}
              className={optionClass}
            >
              <option value="system">System</option>
              <option value="dark">Dark</option>
              <option value="light">Light</option>
            </select>
          </SettingRow>
        );
      case "terminal-font-size":
        return (
          <SettingRow key={row.id} label={row.label} detail={row.detail}>
            <input
              aria-label="Terminal font size"
              type="number"
              min={MIN_TERMINAL_FONT_SIZE}
              max={MAX_TERMINAL_FONT_SIZE}
              value={terminalFontSizeDraft}
              onBlur={() => void handleTerminalFontSizeBlur()}
              onChange={(event) => void handleTerminalFontSizeChange(event.target.value)}
              className={optionClass}
            />
          </SettingRow>
        );
      case "terminal-font-family":
        return (
          <SettingRow
            key={row.id}
            label={row.label}
            detail={terminalFontFamily ? row.detail : `Currently uses ${defaultTerminalFontLabel}.`}
          >
            <select
              aria-label="Terminal font family"
              value={terminalFontFamily}
              onChange={(event) => void handleTerminalFontFamilyChange(event.target.value)}
              className={optionClass}
            >
              <option value="">{`Default (${defaultTerminalFontLabel})`}</option>
              {terminalFontFamilyOptions.map((option) => (
                <option key={option.label} value={option.value}>
                  {option.label}
                </option>
              ))}
            </select>
          </SettingRow>
        );
      case "shell":
        return (
          <SettingRow
            key={row.id}
            label={row.label}
            detail={shell_id === "auto" && defaultShellLabel ? `Currently uses ${defaultShellLabel}.` : row.detail}
          >
            <select
              data-testid="shell-select"
              aria-label="Shell / Interpreter"
              value={shell_id}
              onChange={(event) => setShellId(event.target.value)}
              className={optionClass}
            >
              <option value="auto">{`Default (${defaultShellLabel ?? "detect on launch"})`}</option>
              {available_shells.map((option) => (
                <option key={option.id} value={option.id}>
                  {option.label}
                </option>
              ))}
              <option value="custom">Custom</option>
            </select>
          </SettingRow>
        );
      case "custom-shell-executable":
        return (
          <SettingRow key={row.id} label={row.label} detail={row.detail}>
            <input
              aria-label="Custom shell executable"
              value={custom_executable}
              onChange={(event) => setCustomExecutable(event.target.value)}
              placeholder="Path to shell executable"
              className={optionClass}
            />
          </SettingRow>
        );
      case "custom-shell-args":
        return (
          <SettingRow key={row.id} label={row.label} detail={row.detail}>
            <input
              aria-label="Custom shell arguments"
              value={custom_args}
              onChange={(event) => setCustomArgs(event.target.value)}
              placeholder="Optional arguments"
              className={optionClass}
            />
          </SettingRow>
        );
      case "default-provider":
        return (
          <SettingRow key={row.id} label={row.label} detail={row.detail}>
            <select
              id="default-provider-select"
              aria-label="Default provider"
              value={default_provider}
              onChange={(event) => setDefaultProvider(event.target.value as typeof default_provider)}
              className={optionClass}
            >
              <option value="auto">Auto</option>
              <option value="claude">Claude</option>
              <option value="codex">Codex</option>
              <option value="gemini">Gemini</option>
              <option value="antigravity">Antigravity</option>
              <option value="opencode">OpenCode</option>
            </select>
          </SettingRow>
        );
      case "session-persistence":
        return (
          <SettingRow key={row.id} label={row.label} detail={row.detail}>
            <select
              aria-label="Regular agent sessions"
              value={agent_session_persistence}
              onChange={(event) => setAgentSessionPersistence(event.target.value as "fresh" | "resume")}
              className={optionClass}
            >
              <option value="resume">Resume sessions</option>
              <option value="fresh">Start fresh</option>
            </select>
          </SettingRow>
        );
      case "codex-sandbox":
        return (
          <SettingRow key={row.id} label={row.label} detail={row.detail}>
            <select
              aria-label="Codex sandbox"
              value={codex_runtime_policy.sandbox_mode}
              onChange={(event) => setCodexSandboxMode(event.target.value as typeof codex_runtime_policy.sandbox_mode)}
              className={optionClass}
            >
              <option value="danger-full-access">Full access</option>
              <option value="workspace-write">Workspace write</option>
              <option value="read-only">Read only</option>
            </select>
          </SettingRow>
        );
      case "codex-approval":
        return (
          <SettingRow key={row.id} label={row.label} detail={row.detail}>
            <select
              aria-label="Codex approval"
              value={codex_runtime_policy.approval_policy}
              onChange={(event) => setCodexApprovalPolicy(event.target.value as typeof codex_runtime_policy.approval_policy)}
              className={optionClass}
            >
              <option value="never">Never</option>
              <option value="on-request">On request</option>
              <option value="on-failure">On failure</option>
              <option value="untrusted">Untrusted</option>
            </select>
          </SettingRow>
        );
      case "codex-full-auto":
        return (
          <SettingRow key={row.id} label={row.label} detail={row.detail}>
            <input
              type="checkbox"
              aria-label="Autonomous full access, no prompts"
              checked={codex_runtime_policy.full_auto}
              onChange={(event) => setCodexFullAuto(event.target.checked)}
              className="h-4 w-4"
            />
          </SettingRow>
        );
      case "gemini-patch":
        return (
          <SettingRow key={row.id} label={row.label} detail={row.detail}>
            <label className="flex items-center gap-2 text-sm text-primary">
              <input
                type="checkbox"
                checked={autoPatchGemini}
                onChange={(event) => void handleAutoPatchGeminiChange(event.target.checked)}
              />
              Enabled
            </label>
            <button
              type="button"
              onClick={() => void handleRunPatch()}
              disabled={patchStatus === "running"}
              className="rounded-md border border-wardian-border px-3 py-2 text-xs font-semibold text-primary"
            >
              {patchStatus === "running" ? "Patching..." : "Run Patch Now"}
            </button>
          </SettingRow>
        );
      case "settings-files":
        return (
          <SettingRow key={row.id} label={row.label} detail={row.detail}>
            <code className="rounded bg-wardian-card-bg-muted px-2 py-1 text-xs text-muted-neutral">
              settings/app.json
            </code>
          </SettingRow>
        );
      default:
        return null;
    }
  };

  if (!isOpen) {
    return null;
  }

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/35 p-4">
      <section
        ref={dialogRef}
        role="dialog"
        aria-modal="true"
        aria-label="Settings"
        className="relative grid h-[min(840px,calc(100vh-2rem))] w-[min(1180px,calc(100vw-2rem))] grid-cols-[260px_minmax(0,1fr)] overflow-hidden rounded-2xl border border-wardian-border bg-wardian-bg shadow-[0_32px_80px_rgba(0,0,0,0.55)]"
      >
        <aside className="border-r border-wardian-border bg-[var(--color-wardian-sidebar-secondary)]/35 p-3">
          <div className="mb-3 px-2 py-2 text-xs font-semibold text-muted-neutral">Settings</div>
          <nav className="flex flex-col gap-1" aria-label="Settings categories">
            {categories.map((category) => (
              <button
                key={category}
                type="button"
                onClick={() => {
                  setActiveCategory(category);
                  setQuery("");
                }}
                className={`rounded-md px-3 py-2 text-left text-sm transition-colors ${
                  activeCategory === category && !query
                    ? "bg-wardian-card-bg-muted text-primary"
                    : "text-muted-neutral hover:text-primary"
                }`}
              >
                {category}
              </button>
            ))}
          </nav>
        </aside>

        <div className="flex min-w-0 flex-col overflow-hidden">
          <header className="relative flex items-start gap-3 border-b border-wardian-border px-6 py-4 pr-16">
            <div className="min-w-0 flex-1">
              <h2 className="text-xl font-semibold text-primary">Settings</h2>
              <div className="mt-3 flex max-w-xl items-center gap-2 rounded-md border border-wardian-border bg-wardian-input-bg px-3 py-2">
                <Search className="h-4 w-4 text-muted-neutral" />
                <input
                  value={query}
                  onChange={(event) => setQuery(event.target.value)}
                  placeholder="Search settings"
                  className="w-full bg-transparent text-sm text-primary outline-none placeholder:text-muted-neutral"
                />
              </div>
            </div>
            <button
              ref={closeButtonRef}
              type="button"
              aria-label="Close settings"
              onClick={onClose}
              className="absolute right-6 top-4 rounded-lg border border-transparent p-2 text-muted-neutral transition-colors hover:border-wardian-border hover:text-primary"
            >
              <X className="h-4 w-4" strokeWidth={2} />
            </button>
          </header>

          <div className="min-h-0 flex-1 overflow-y-auto px-6 py-5">
            {groupedRows.length === 0 ? (
              <div className="flex h-full items-center justify-center text-sm text-muted-neutral">
                No settings match your search.
              </div>
            ) : (
              <div className="mx-auto flex max-w-[820px] flex-col gap-8">
                {groupedRows.map((group) => (
                  <section key={group.category} aria-label={group.category}>
                    <div className="mb-3 flex items-center gap-2">
                      <Monitor className="h-4 w-4 text-muted-neutral" />
                      <h3 className="text-sm font-semibold text-primary">{group.category}</h3>
                    </div>
                    <div className="overflow-hidden rounded-lg border border-wardian-border bg-wardian-card-bg-muted/45">
                      {renderRowsWithSubgroups(group.rows, renderRow)}
                    </div>
                    {group.category === "Terminal" && (
                      <div className="mt-4 flex items-center justify-end gap-3">
                        {terminalMessage && (
                          <span className={`text-xs ${terminalStatus === "error" ? "text-red-400" : "text-muted-neutral"}`}>
                            {terminalMessage}
                          </span>
                        )}
                        <button
                          type="button"
                          onClick={() => void handleSaveTerminal()}
                          disabled={terminalStatus === "saving"}
                          className={buttonClass}
                        >
                          {terminalStatus === "saving" ? "Saving..." : "Save Terminal"}
                        </button>
                      </div>
                    )}
                    {group.category === "Agent Runtime" && (
                      <div className="mt-4 flex items-center justify-end gap-3">
                        {runtimeMessage && (
                          <span className={`text-xs ${runtimeStatus === "error" ? "text-red-400" : "text-muted-neutral"}`}>
                            {runtimeMessage}
                          </span>
                        )}
                        <button
                          type="button"
                          onClick={() => void handleSaveRuntime()}
                          disabled={runtimeStatus === "saving"}
                          className={buttonClass}
                        >
                          {runtimeStatus === "saving" ? "Saving..." : "Save Agent Runtime"}
                        </button>
                      </div>
                    )}
                  </section>
                ))}
              </div>
            )}
          </div>
        </div>
      </section>
    </div>
  );
};
