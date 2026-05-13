import React, { useState, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { FolderOpen } from "lucide-react";
import { AgentConfig, AgentClassDefinition, ProviderName } from "../../types";
import { AdvancedSettings } from "../../components/AdvancedSettings";
import { defaultProviderConfig, withProvider } from "./configUtils";

interface Props {
  agentClasses: AgentClassDefinition[];
  onSpawned: () => void;
}

export const SpawnAgentPanel: React.FC<Props> = ({ agentClasses, onSpawned }) => {
  const [newSessionName, setNewSessionName] = useState("");
  const [nameError, setNameError] = useState<string | null>(null);
  const [newAgentClass, setNewAgentClass] = useState("Generalist");

  const validateName = (name: string) => {
    if (!name.trim()) return null;
    const re = /^[a-zA-Z0-9_-]+$/;
    if (!re.test(name)) {
      return "Names must be alphanumeric, underscores, or hyphens (no spaces).";
    }
    return null;
  };

  const [newFolder, setNewFolder] = useState("");
  const [resumeSession, setResumeSession] = useState("");
  const initialProviderConfig: Partial<AgentConfig> = {
    provider: "claude",
    provider_config: defaultProviderConfig("claude"),
  };
  const [spawnAdvancedConfig, setSpawnAdvancedConfig] = useState<Partial<AgentConfig>>(initialProviderConfig);
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
    const err = validateName(newSessionName);
    if (err) {
      setNameError(err);
      return;
    }
    setIsSpawning(true);
    try {
      await invoke<AgentConfig>("spawn_agent", {
        req: {
          sessionName: newSessionName,
          agentClass: newAgentClass,
          folder: newFolder,
          resumeSession: resumeSession || null,
          isOff: false,
          configOverride: spawnAdvancedConfig,
        },
      });
      setNewSessionName("");
      setNameError(null);
      setNewAgentClass("Generalist");
      setNewFolder("");
      setResumeSession("");
      setSpawnAdvancedConfig(initialProviderConfig);
      onSpawned();
    } catch (error) {
      console.error("Failed to spawn agent:", error);
      alert(`Failed to spawn agent: ${error}`);
    } finally {
      setIsSpawning(false);
    }
  };

  const chooseWorkspaceFolder = async () => {
    try {
      const selected = await open({
        directory: true,
        multiple: false,
        title: "Choose workspace folder",
      });

      if (typeof selected === "string") {
        setNewFolder(selected);
      }
    } catch (error) {
      console.error("Failed to choose workspace folder:", error);
      alert(`Failed to choose workspace folder: ${error}`);
    }
  };

  return (
    <div className="mb-8">
      <h3 className="text-xs font-bold text-muted tracking-wide mb-4">
        Spawn Agent
      </h3>
      <form className="flex flex-col gap-4" onSubmit={spawnAgent}>
        <div>
          <label className="block text-[10px] font-bold text-muted-neutral mb-1">
            Agent Name
          </label>
          <input
            data-testid="spawn-agent-name"
            className={`w-full bg-[var(--color-wardian-input-bg)] border ${nameError ? 'border-wardian-error' : 'border-wardian-light'} rounded px-3 py-2 text-sm text-primary focus:outline-none focus:border-[var(--color-wardian-accent)] transition-colors`}
            placeholder="e.g. coder-alpha"
            value={newSessionName}
            onChange={(e) => {
              setNewSessionName(e.currentTarget.value);
              setNameError(validateName(e.currentTarget.value));
            }}
          />
          {nameError && (
            <p className="text-[10px] text-wardian-error mt-1">{nameError}</p>
          )}
        </div>
        <div>
          <label className="block text-[10px] font-bold text-muted-neutral mb-1">
            Agent Class
          </label>
          <select
            data-testid="spawn-agent-class"
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
          <label className="block text-[10px] font-bold text-muted-neutral mb-1">
            Workspace Path
          </label>
          <div className="flex items-center gap-2">
            <div className="relative min-w-0 flex-1">
              <input
                data-testid="spawn-workspace-path"
                className="w-full bg-[var(--color-wardian-input-bg)] border border-wardian-light rounded px-3 py-2 text-sm text-primary focus:outline-none focus:border-[var(--color-wardian-accent)] transition-colors pr-10"
                placeholder="C:/projects/my-app"
                value={newFolder}
                onChange={(e) => setNewFolder(e.currentTarget.value)}
              />
              {newFolder && (
                <span
                  className="absolute right-3 top-1/2 -translate-y-1/2 text-[10px]"
                  title={folderIsValid ? "Valid path" : "Invalid path"}
                >
                  {folderIsValid === true ? "✅" : folderIsValid === false ? "⚠️" : ""}
                </span>
              )}
            </div>
            <button
              type="button"
              aria-label="Choose workspace folder"
              title="Choose workspace folder"
              onClick={chooseWorkspaceFolder}
              className="flex h-10 w-10 shrink-0 items-center justify-center rounded border border-wardian-light bg-[var(--color-wardian-input-bg)] text-muted-neutral transition-colors hover:border-[var(--color-wardian-accent)] hover:text-[var(--color-wardian-accent)] focus:outline-none focus:border-[var(--color-wardian-accent)]"
            >
              <FolderOpen className="h-4 w-4" aria-hidden="true" />
            </button>
          </div>
        </div>
        <div>
          <label className="block text-[10px] font-bold text-muted-neutral mb-1">
            Provider Engine
          </label>
          <select
            data-testid="spawn-provider"
            className="w-full bg-[var(--color-wardian-input-bg)] border border-wardian-light rounded px-3 py-2 text-sm text-primary focus:outline-none focus:border-[var(--color-wardian-accent)] transition-colors"
            value={spawnAdvancedConfig.provider || "claude"}
            onChange={(e) => {
              const provider = e.currentTarget.value as ProviderName;
              setSpawnAdvancedConfig((prev) =>
                withProvider(prev, provider),
              );
            }}
          >
            <option value="claude">Claude</option>
            <option value="codex">Codex</option>
            <option value="gemini">Gemini</option>
            <option value="opencode">OpenCode</option>
          </select>
        </div>
        <div>
          <label className="block text-[10px] font-bold text-muted-neutral mb-1">
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
          data-testid="spawn-submit"
          type="submit"
          disabled={isSpawning}
          className="w-full mt-2 bg-wardian-success/80 hover:bg-wardian-success/60 disabled:bg-wardian-off/30 disabled:cursor-not-allowed text-[var(--color-wardian-bg)] py-2.5 rounded-lg font-bold text-xs tracking-wide transition-all flex items-center justify-center gap-2 shadow-lg shadow-wardian-success/10"
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
