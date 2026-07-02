import React from "react";
import { ArrowLeft } from "lucide-react";
import {
  MAX_TERMINAL_FONT_SIZE,
  MIN_TERMINAL_FONT_SIZE,
  normalizeTerminalFontSize,
  useSettingsStore,
} from "../../store/useSettingsStore";
import type { AppThemeSetting } from "../../types/settings";
import { useRemoteStore } from "./useRemoteStore";

const iconButtonClass =
  "inline-flex h-9 w-9 shrink-0 items-center justify-center rounded-md border border-wardian-border text-muted-neutral transition-colors hover:border-[var(--color-wardian-accent)] hover:text-primary";

const rowClass = "border-b border-wardian-border px-4 py-4 last:border-b-0";
const selectClass =
  "mt-2 w-full rounded-md border border-wardian-border bg-wardian-input-bg px-3 py-2 text-sm text-primary outline-none focus:border-[var(--color-wardian-accent)]";

export const RemoteSettingsView: React.FC<{ onClose: () => void }> = ({ onClose }) => {
  const theme = useSettingsStore((state) => state.theme);
  const setTheme = useSettingsStore((state) => state.setTheme);
  const terminalFontSize = useSettingsStore((state) => state.terminalFontSize);
  const setTerminalFontSize = useSettingsStore((state) => state.setTerminalFontSize);
  const remoteAgentDefaultViewMode = useRemoteStore((state) => state.remoteAgentDefaultViewMode);
  const setRemoteAgentDefaultViewMode = useRemoteStore((state) => state.setRemoteAgentDefaultViewMode);

  const updateTerminalTextSize = (value: string) => {
    setTerminalFontSize(normalizeTerminalFontSize(Number(value)));
  };

  return (
    <main className="flex h-dvh flex-col overflow-hidden bg-wardian-bg text-primary" data-testid="remote-settings-view">
      <header className="shrink-0 border-b border-wardian-border bg-wardian-bg/95 px-3 py-3 backdrop-blur">
        <div className="flex items-center gap-2">
          <button type="button" aria-label="Back to remote watchlist" onClick={onClose} className={iconButtonClass}>
            <ArrowLeft className="h-4 w-4" aria-hidden="true" />
          </button>
          <div className="min-w-0">
            <h1 className="truncate text-base font-semibold text-primary">Settings</h1>
            <p className="truncate text-xs text-muted-neutral">Mobile remote preferences</p>
          </div>
        </div>
      </header>

      <section className="min-h-0 flex-1 overflow-y-auto px-3 py-3" aria-label="Remote settings">
        <div className="overflow-hidden rounded-md border border-wardian-border bg-wardian-card-bg-muted/45">
          <label className={`block ${rowClass}`}>
            <span className="text-sm font-semibold text-primary">Theme</span>
            <span className="mt-1 block text-xs leading-relaxed text-muted-neutral">
              Choose the color theme used by the installed remote PWA.
            </span>
            <select
              aria-label="Theme"
              value={theme}
              onChange={(event) => setTheme(event.target.value as AppThemeSetting)}
              className={selectClass}
            >
              <option value="system">System</option>
              <option value="light">Light</option>
              <option value="dark">Dark</option>
            </select>
          </label>

          <label className={`block ${rowClass}`}>
            <span className="text-sm font-semibold text-primary">Agent detail default</span>
            <span className="mt-1 block text-xs leading-relaxed text-muted-neutral">
              Pick which view opens first after tapping an agent.
            </span>
            <select
              aria-label="Agent detail default"
              value={remoteAgentDefaultViewMode}
              onChange={(event) => setRemoteAgentDefaultViewMode(event.target.value as "terminal" | "chat")}
              className={selectClass}
            >
              <option value="terminal">Terminal</option>
              <option value="chat">Chat</option>
            </select>
          </label>

          <label className={`block ${rowClass}`}>
            <span className="text-sm font-semibold text-primary">Terminal text size</span>
            <span className="mt-1 block text-xs leading-relaxed text-muted-neutral">
              Applies to remote terminal views on this device.
            </span>
            <select
              aria-label="Terminal text size"
              value={terminalFontSize}
              onChange={(event) => updateTerminalTextSize(event.target.value)}
              className={selectClass}
            >
              {Array.from(
                { length: MAX_TERMINAL_FONT_SIZE - MIN_TERMINAL_FONT_SIZE + 1 },
                (_, index) => MIN_TERMINAL_FONT_SIZE + index,
              ).map((size) => (
                <option key={size} value={size}>
                  {size}px
                </option>
              ))}
            </select>
          </label>
        </div>
      </section>
    </main>
  );
};
