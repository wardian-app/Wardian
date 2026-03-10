import React from "react";

interface SettingsPanelProps {
  theme: "dark" | "light" | "system";
  setTheme: (theme: "dark" | "light" | "system") => void;
  onCollapse: () => void;
}

export const SettingsPanel: React.FC<SettingsPanelProps> = ({
  theme,
  setTheme,
  onCollapse,
}) => {
  return (
    <div className="flex flex-col h-full">
      <div className="flex justify-between items-center mb-6">
        <h2 className="text-xl font-bold text-primary tracking-tight">SETTINGS</h2>
        <button onClick={onCollapse} className="text-bright-neutral hover:text-primary transition-colors">
          <svg className="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M15 19l-7-7 7-7"></path></svg>
        </button>
      </div>
      
      <div className="flex flex-col gap-6">
        <div className="bg-transparent mb-2">
          <h3 className="text-[10px] font-bold text-muted-neutral uppercase tracking-widest mb-4">Theme</h3>
          <div className="grid grid-cols-3 gap-3">
            {/* System Theme Card */}
            <button
              type="button"
              onClick={() => setTheme("system")}
              className={`flex flex-col items-center gap-2 p-3 rounded-xl border transition-all ${theme === 'system' ? 'border-[var(--color-wardian-accent)] bg-[var(--color-wardian-accent)]/5 shadow-[0_0_15px_rgba(241,211,130,0.1)]' : 'border-wardian-border bg-wardian-card-bg-muted hover:border-wardian-border-heavy'}`}
            >
              <div className="w-full aspect-[4/3] rounded-md border border-wardian-border overflow-hidden flex shadow-inner">
                <div className="flex-1 bg-gray-900 border-r border-wardian-border"></div>
                <div className="flex-1 bg-gray-100"></div>
              </div>
              <span className={`text-[11px] font-bold uppercase tracking-tight ${theme === 'system' ? 'text-[var(--color-wardian-accent)]' : 'text-muted-neutral'}`}>System</span>
            </button>

            {/* Dark Theme Card */}
            <button
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
              <span className={`text-[11px] font-bold uppercase tracking-tight ${theme === 'dark' ? 'text-[var(--color-wardian-accent)]' : 'text-muted-neutral'}`}>Dark</span>
            </button>

            {/* Light Theme Card */}
            <button
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
              <span className={`text-[11px] font-bold uppercase tracking-tight ${theme === 'light' ? 'text-[var(--color-wardian-accent)]' : 'text-muted-neutral'}`}>Light</span>
            </button>
          </div>
          <div className="mt-3 px-1">
            <p className="text-[10px] text-muted-neutral leading-relaxed">
              <span className="text-[var(--color-wardian-accent)] font-bold">NOTE:</span> For complete terminal synchronization, update the gemini CLI theme as well, and then restart the application.
            </p>
          </div>
        </div>

        <div className="text-center p-4">
          <p className="text-[10px] text-muted-neutral italic">More settings coming in Phase 3.</p>
        </div>
      </div>
    </div>
  );
};
