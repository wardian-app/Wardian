import React, { useState, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { AgentConfig, AgentClassDefinition, AgentTelemetry } from "./types";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { requiresRestart } from "./configUtils";
import { AdvancedSettings } from './components/AdvancedSettings';

interface Props {
  agentId: string;
  agents: AgentConfig[];
  agentClasses: AgentClassDefinition[];
  telemetry: Record<string, AgentTelemetry>;
  onSaved: () => void;
}

export const ConfigureAgentPanel: React.FC<Props> = ({ 
  agentId, 
  agents, 
  agentClasses, 
  telemetry,
  onSaved 
}) => {
  const [config, setConfig] = useState<AgentConfig | null>(null);
  const [isSaving, setIsSaving] = useState(false);
  const [copiedId, setCopiedId] = useState(false);
  const [copiedLog, setCopiedLog] = useState(false);

  // Sync state when agentId or agents change
  useEffect(() => {
    const agent = agents.find(a => a.session_id === agentId);
    if (agent) {
      // Create a deep copy to avoid direct state mutation before save
      setConfig(JSON.parse(JSON.stringify(agent)));
    }
  }, [agentId, agents]);

  if (!config) return null;

  const updateField = (field: keyof AgentConfig, value: any) => {
    setConfig(prev => prev ? { ...prev, [field]: value } : null);
  };

  const handleSave = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!config) return;

    const originalAgent = agents.find(a => a.session_id === agentId);
    const needsRestart = originalAgent ? requiresRestart(originalAgent, config) : true;

    setIsSaving(true);
    try {
      await invoke("update_agent_config", { newConfig: config });
      if (needsRestart) {
        alert("Configuration updated! Please restart the agent for all changes (CLI parameters/arguments) to take effect.");
      }
      onSaved();
    } catch (err) {
      console.error("Failed to update config", err);
      alert(`Error updating config: ${err}`);
    } finally {
      setIsSaving(false);
    }
  };

  return (
    <div className="flex flex-col h-full">
      <form className="flex flex-col gap-4 select-text" onSubmit={handleSave}>
        
        {/* Basic Fields */}
        <div className="flex flex-col gap-4">
          <div>
            <div className="flex justify-between items-center mb-1">
              <label className="block text-[10px] font-bold text-gray-500 uppercase">Session ID</label>
              <button 
                type="button"
                onClick={async () => {
                  try {
                    await writeText(config.session_id);
                    setCopiedId(true);
                    setTimeout(() => setCopiedId(false), 2000);
                  } catch (e) {
                    console.error("Failed to copy", e);
                  }
                }}
                className={`text-[9px] font-bold uppercase tracking-wider px-2 py-1 rounded transition-all active:scale-95 cursor-pointer ${copiedId ? 'bg-emerald-500/20 text-emerald-400 border border-emerald-500/30' : 'bg-gray-700/50 text-gray-300 hover:text-white hover:bg-gray-600 border border-transparent'}`}
              >
                {copiedId ? "Copied!" : "Copy"}
              </button>
            </div>
            <input
              readOnly
              className="w-full bg-black/50 border border-gray-700/50 rounded px-3 py-2 text-xs text-gray-400 font-mono focus:outline-none select-text cursor-text"
              value={config.session_id}
            />
          </div>
          <div>
            <div className="flex justify-between items-center mb-1">
              <label className="block text-[10px] font-bold text-gray-500 uppercase">Log Path</label>
              <button 
                type="button"
                disabled={!telemetry[agentId]?.log_path}
                onClick={async () => {
                  const path = telemetry[agentId]?.log_path;
                  if (!path) return;
                  try {
                    await writeText(path);
                    setCopiedLog(true);
                    setTimeout(() => setCopiedLog(false), 2000);
                  } catch (e) {
                    console.error("Failed to copy", e);
                  }
                }}
                className={`text-[9px] font-bold uppercase tracking-wider px-2 py-1 rounded transition-all active:scale-95 cursor-pointer ${copiedLog ? 'bg-emerald-500/20 text-emerald-400 border border-emerald-500/30' : 'bg-gray-700/50 text-gray-300 hover:text-white hover:bg-gray-600 border border-transparent disabled:opacity-30 disabled:cursor-not-allowed'}`}
              >
                {copiedLog ? "Copied!" : "Copy"}
              </button>
            </div>
            <input
              readOnly
              className="w-full bg-black/50 border border-gray-700/50 rounded px-3 py-2 text-xs text-gray-400 font-mono focus:outline-none select-text cursor-text"
              value={telemetry[agentId]?.log_path || "Not available (Agent is offline or generating logs)"}
            />
          </div>
          <div>
            <label className="block text-[10px] font-bold text-gray-500 uppercase mb-1">Agent Name</label>
            <input
              className="w-full bg-gray-800/50 border border-gray-700 rounded px-3 py-2 text-sm text-white focus:outline-none focus:border-[var(--color-wardian-accent)] transition-colors"
              value={config.session_name}
              onChange={(e) => updateField("session_name", e.target.value)}
            />
          </div>
          <div>
            <label className="block text-[10px] font-bold text-gray-500 uppercase mb-1">Agent Class</label>
            <select
              className="w-full bg-gray-800/50 border border-gray-700 rounded px-3 py-2 text-sm text-white focus:outline-none focus:border-[var(--color-wardian-accent)] transition-colors"
              value={config.agent_class}
              onChange={async (e) => {
                const newClass = e.target.value;
                updateField("agent_class", newClass);
                // Proactively resolve system include directories for the new class
                try {
                  const sysDirs: string[] = await invoke("resolve_system_include_directories", { className: newClass });
                  updateField("system_include_directories", sysDirs);
                } catch (err) {
                  console.error("Failed to resolve system include dirs", err);
                }
              }}
            >
              {agentClasses.map(c => (
                <option key={c.name} value={c.name}>{c.name}</option>
              ))}
            </select>
          </div>
        </div>

        <AdvancedSettings 
          config={config} 
          updateField={updateField} 
        />

        <button
          type="submit"
          disabled={isSaving}
          className="w-full mt-2 bg-[var(--color-wardian-accent)] hover:opacity-90 disabled:opacity-50 text-white py-2.5 rounded-lg font-bold text-xs uppercase tracking-widest transition-all shadow-lg shadow-[var(--color-wardian-accent)]/20"
        >
          {isSaving ? "Saving..." : "Save Changes"}
        </button>
      </form>
    </div>
  );
};
