import React, { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useSettingsStore } from "../../store/useSettingsStore";

interface SettingsPanelProps {}

export const SettingsPanel: React.FC<SettingsPanelProps> = () => {
  const { theme, setTheme, autoPatchGemini, setAutoPatchGemini } = useSettingsStore();
  const [patchStatus, setPatchStatus] = useState<"idle" | "running" | "success" | "error">("idle");
  const [patchMessage, setPatchMessage] = useState("");

  const handleRunPatch = async () => {
    setPatchStatus("running");
    setPatchMessage("");
    try {
      await invoke<string>("run_gemini_patch");
      setPatchStatus("success");
      setPatchMessage("Gemini CLI Patch Applied Successfully.");
      setTimeout(() => {
        setPatchStatus("idle");
        setPatchMessage("");
      }, 5000);
    } catch (e: any) {
      setPatchStatus("error");
      setPatchMessage(`Patch failed: ${e}`);
    }
  };

  return (
    <div className="flex flex-col h-full">
      <div className="flex justify-between items-center mb-6">
        <h2 className="text-xl font-bold text-primary tracking-tight">Settings</h2>
      </div>
      
      <div className="flex flex-col gap-8 flex-1 overflow-y-auto pr-2 no-scrollbar">
        <div className="bg-transparent">
          <h3 className="text-[10px] font-bold text-muted-neutral tracking-wide mb-4">Theme</h3>
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
              <span className={`text-[11px] font-bold tracking-tight ${theme === 'system' ? 'text-[var(--color-wardian-accent)]' : 'text-muted-neutral'}`}>System</span>
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
              <span className={`text-[11px] font-bold tracking-tight ${theme === 'dark' ? 'text-[var(--color-wardian-accent)]' : 'text-muted-neutral'}`}>Dark</span>
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
              <span className={`text-[11px] font-bold tracking-tight ${theme === 'light' ? 'text-[var(--color-wardian-accent)]' : 'text-muted-neutral'}`}>Light</span>
            </button>
          </div>
          <div className="mt-3 px-1">
            <p className="text-[10px] text-muted-neutral leading-relaxed">
              <span className="text-[var(--color-wardian-accent)] font-bold">NOTE:</span> For complete terminal synchronization, update the gemini CLI theme as well, and then restart the application.
            </p>
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
            
            <p className="text-[10px] text-muted-neutral leading-relaxed text-left">
              Applies a custom patch to enable skill discovery in included directories. Reruns on every launch.
            </p>

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
