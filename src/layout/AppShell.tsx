import type { ReactNode } from "react";

export type AppShellProps = {
  titlebar: ReactNode;
  status?: ReactNode;
  conflictDialog?: ReactNode;
  leftRail: ReactNode;
  leftPane: ReactNode;
  mainContent: ReactNode;
  mainOverlays?: ReactNode;
  roster: ReactNode;
};

/**
 * Persistent desktop chrome around the active legacy/workbench content branch.
 *
 * App startup owns composition; this component owns only stable region order
 * and sizing so surface migration cannot accidentally duplicate global chrome.
 */
export function AppShell({
  titlebar,
  status,
  conflictDialog,
  leftRail,
  leftPane,
  mainContent,
  mainOverlays,
  roster,
}: AppShellProps) {
  return (
    <div
      data-testid="app-shell"
      className="flex flex-col bg-[var(--color-wardian-bg)] text-[var(--color-wardian-text)] overflow-hidden font-sans select-none"
      style={{
        width: "var(--wardian-native-window-width, 100vw)",
        height: "var(--wardian-native-window-height, 100dvh)",
      }}
    >
      {titlebar}
      {status}
      {conflictDialog}
      <div className="flex flex-1 overflow-hidden">
        {leftRail}
        {leftPane}
        <main className="flex-1 min-w-0 h-full flex flex-col overflow-hidden relative">
          {mainContent}
          {mainOverlays}
        </main>
        {roster}
      </div>
    </div>
  );
}
