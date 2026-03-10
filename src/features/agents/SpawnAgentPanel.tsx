import React, { useState, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { AgentConfig, AgentClassDefinition } from "../../types";
import { AdvancedSettings } from "../../components/AdvancedSettings";

interface Props {
  agentClasses: AgentClassDefinition[];
  onSpawned: () => void;
}

export const SpawnAgentPanel: React.FC<Props> = ({ agentClasses, onSpawned }) => {
  const [newSessionName, setNewSessionName] = useState("");
  const [newAgentClass, setNewAgentClass] = useState("Generalist");
  const [newFolder, setNewFolder] = useState("");
  const [resumeSession, setResumeSession] = useState("");
  const [spawnAdvancedConfig, setSpawnAdvancedConfig] = useState<Partial<AgentConfig>>({});
  const [isSpawning, setIsSpawning] = useState(false);
  const [folderIsValid, setFolderIsValid] = useState<boolean | null>(null);

  useEffect(() => {
    if (newFolder) {
      invoke<boolean>("validate_directory_path", { path: newFolder })
        .then(setFolderIsValid)
        .catch(() => setFolderIsValid(false));
    } else {
      setFolderIsValid(null);
    }
  }, [newFolder]);

  const spawnAgent = async (e?: React.FormEvent) => {
    if (e) e.preventDefault();
    setIsSpawning(true);
    try {
      await invoke<AgentConfig>("spawn_agent", {
        sessionName: newSessionName,
        agentClass: newAgentClass,
        folder: newFolder,
        resumeSession: resumeSession || null,
        isOff: false,
        configOverride: spawnAdvancedConfig,
      });
      setNewSessionName("");
      setNewAgentClass("Generalist");
      setNewFolder("");
      setResumeSession("");
      setSpawnAdvancedConfig({});
      onSpawned();
    } catch (error) {
      console.error("Failed to spawn agent:", error);
      alert(`Failed to spawn agent: ${error}`);
    } finally {
      setIsSpawning(false);
    }
  };

  return (
    <div className="mb-8">
      <h3 className="text-xs font-bold text-muted uppercase tracking-widest mb-4">
        Spawn Agent
      </h3>
      <form className="flex flex-col gap-4" onSubmit={spawnAgent}>
        <div>
          <label className="block text-[10px] font-bold text-muted-neutral uppercase mb-1">
            Agent Name
          </label>
          <input
            className="w-full bg-[var(--color-wardian-input-bg)] border border-wardian-light rounded px-3 py-2 text-sm text-primary focus:outline-none focus:border-[var(--color-wardian-accent)] transition-colors"
            placeholder="e.g. Coder_Alpha"
            value={newSessionName}
            onChange={(e) => setNewSessionName(e.currentTarget.value)}
          />
        </div>
        <div>
          <label className="block text-[10px] font-bold text-muted-neutral uppercase mb-1">
            Agent Class
          </label>
          <select
            className="w-full bg-[var(--color-wardian-input-bg)] border border-wardian-light rounded px-3 py-2 text-sm text-primary focus:outline-none focus:border-[var(--color-wardian-accent)] transition-colors"
            value={newAgentClass}
            onChange={(e) => setNewAgentClass(e.currentTarget.value)}
          >
            {agentClasses.length > 0 ? (
              <>
                <optgroup label="Default Classes">
                  {agentClasses
                    .filter((c) => c.is_default)
                    .map((c) => (
                      <option key={c.name} value={c.name}>
                        {c.name}
                      </option>
                    ))}
                </optgroup>
                {agentClasses.filter((c) => !c.is_default).length > 0 && (
                  <optgroup label="Custom Classes">
                    {agentClasses
                      .filter((c) => !c.is_default)
                      .map((c) => (
                        <option key={c.name} value={c.name}>
                          {c.name}
                        </option>
                      ))}
                  </optgroup>
                )}
              </>
            ) : (
              <option value="Coder">Coder</option>
            )}
          </select>
        </div>
        <div>
          <label className="block text-[10px] font-bold text-muted-neutral uppercase mb-1">
            Workspace Path
          </label>
          <div className="relative flex items-center">
            <input
            className="w-full bg-[var(--color-wardian-input-bg)] border border-wardian-light rounded px-3 py-2 text-sm text-primary focus:outline-none focus:border-[var(--color-wardian-accent)] transition-colors pr-10"
              placeholder="C:/projects/my-app"
              value={newFolder}
              onChange={(e) => setNewFolder(e.currentTarget.value)}
            />
            {newFolder && (
              <span
                className="absolute right-3 text-[10px]"
                title={folderIsValid ? "Valid path" : "Invalid path"}
              >
                {folderIsValid === true ? "✅" : folderIsValid === false ? "⚠️" : ""}
              </span>
            )}
          </div>
        </div>
        <div>
          <label className="block text-[10px] font-bold text-muted-neutral uppercase mb-1">
            Session ID (Optional)
          </label>
          <input
            className="w-full bg-[var(--color-wardian-input-bg)] border border-wardian-light rounded px-3 py-2 text-sm text-primary focus:outline-none focus:border-[var(--color-wardian-accent)] transition-colors"
            placeholder="e.g. 1a2b3c..."
            value={resumeSession}
            onChange={(e) => setResumeSession(e.currentTarget.value)}
          />
        </div>

        <AdvancedSettings
          config={spawnAdvancedConfig}
          updateField={(field, val) =>
            setSpawnAdvancedConfig((prev) => ({ ...prev, [field]: val }))
          }
        />

        <button
          type="submit"
          disabled={isSpawning}
          className="w-full mt-2 bg-wardian-success/80 hover:bg-wardian-success/60 disabled:bg-wardian-off/30 disabled:cursor-not-allowed text-[var(--color-wardian-bg)] py-2.5 rounded-lg font-bold text-xs uppercase tracking-widest transition-all flex items-center justify-center gap-2 shadow-lg shadow-wardian-success/10"
        >
          {isSpawning ? (
            <div className="animate-spin w-4 h-4 border-2 border-[var(--color-wardian-bg)]/30 border-t-[var(--color-wardian-bg)] rounded-full"></div>
          ) : (
            "Initialize"
          )}
        </button>
      </form>
    </div>
  );
};
