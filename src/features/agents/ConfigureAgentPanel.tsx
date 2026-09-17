import React, { useState, useEffect, useMemo } from "react";
import { invoke } from "@tauri-apps/api/core";
import { AgentConfig, AgentClassDefinition, AgentModelSelectionUpdateResult, AgentTelemetry, ProviderReadiness, UserFacingProviderName } from "../../types";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { normalizeAgentConfig, reasoningEffortForConfig, requiresRestart, toPersistedAgentConfig, withProvider, withReasoningEffort } from "./configUtils";
import { ProviderModelSelector } from "./ProviderModelSelector";
import { AdvancedSettings } from '../../components/AdvancedSettings';
import { useLibraryStore } from "../../store/useLibraryStore";
import { buildProviderOptions, buildUngatedProviderOptions, isUserFacingProviderName } from "./providerOptions";

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
  onSaved,
}) => {
  const [config, setConfig] = useState<AgentConfig | null>(null);
  const [isSaving, setIsSaving] = useState(false);
  const [copiedId, setCopiedId] = useState(false);
  const [copiedLog, setCopiedLog] = useState(false);
  const [providerReadiness, setProviderReadiness] = useState<ProviderReadiness[] | null>(null);
  const [providerNote, setProviderNote] = useState<string | null>(null);
  const openLibraryAt = useLibraryStore((s) => s.openLibraryAt);

  // Sync state when agentId or agents change
  useEffect(() => {
    const agent = agents.find(a => a.session_id === agentId);
    if (agent) {
      // Create a deep copy to avoid direct state mutation before save
      setConfig(normalizeAgentConfig(JSON.parse(JSON.stringify(agent))));
    }
  }, [agentId, agents]);

  useEffect(() => {
    let cancelled = false;
    invoke<ProviderReadiness[]>("list_provider_readiness")
      .then((readiness) => {
        if (!cancelled) setProviderReadiness(readiness);
      })
      .catch((error) => {
        console.error("Failed to load provider readiness:", error);
        if (!cancelled) setProviderNote("Unable to check provider readiness.");
      });

    return () => {
      cancelled = true;
    };
  }, []);

  const providerOptions = useMemo(
    () => (providerReadiness ? buildProviderOptions(providerReadiness) : buildUngatedProviderOptions()),
    [providerReadiness],
  );

  if (!config) return null;

  const descriptionLength = config.description?.length ?? 0;

  const updateField = (field: keyof AgentConfig, value: AgentConfig[keyof AgentConfig]) => {
    setConfig(prev => prev ? { ...prev, [field]: value } : null);
  };

  const currentProvider = isUserFacingProviderName(config.provider) ? config.provider : "";
  const selectedProviderAvailable = currentProvider
    ? providerReadiness
      ? providerOptions.some((option) => option.value === currentProvider && option.available)
      : true
    : true;

  const handleSave = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!config) return;
    if (!selectedProviderAvailable) return;

    const originalAgent = agents.find(a => a.session_id === agentId);
    const persistedConfig = toPersistedAgentConfig(config);
    const needsRestart = originalAgent
      ? requiresRestart(normalizeAgentConfig(originalAgent), persistedConfig)
      : true;

    setIsSaving(true);
    try {
      const result = await invoke<AgentModelSelectionUpdateResult>("update_agent_config", { newConfig: persistedConfig });
      if (result?.live_application === "unknown") {
        alert(`Configuration saved, but the live model change is unconfirmed: ${result.live_error ?? "the provider runtime changed before acknowledgement"}`);
      } else if (result?.live_application === "failed") {
        alert(`Configuration saved, but the live model change failed: ${result.live_error ?? "the provider rejected the change"}`);
      } else if (result?.live_application === "deferred") {
        const deferredReasons = [result.model?.reason, result.reasoning_effort?.reason]
          .filter((reason): reason is string => Boolean(reason && reason !== "unchanged"));
        const reason = deferredReasons[0] ?? "no_live_change";
        const savedModel = result.model?.desired_value ?? result.config.model ?? "provider default";
        const savedEffort = result.reasoning_effort?.desired_value
          ?? reasoningEffortForConfig(result.config)
          ?? "provider default";
        alert(
          `Configuration saved. Live settings deferred (${reason}). `
          + `Saved model: ${savedModel}; saved reasoning effort: ${savedEffort}.`,
        );
      } else if (result?.restart_required ?? needsRestart) {
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
            <label className="block text-[10px] font-bold text-muted-neutral mb-1">Agent Name</label>
            <input
              className="w-full bg-[var(--color-wardian-input-bg)] border border-wardian-light rounded px-3 py-2 text-sm text-primary focus:outline-none focus:border-[var(--color-wardian-accent)] transition-colors"
              value={config.session_name}
              onChange={(e) => updateField("session_name", e.target.value)}
            />
          </div>
          <div>
            <div className="mb-1 flex items-center justify-between gap-2">
              <label className="block text-[10px] font-bold text-muted-neutral" htmlFor="agent-description">
                Description <span className="font-normal">(optional)</span>
              </label>
              <span className="text-[9px] tabular-nums text-muted-neutral">{descriptionLength}/280</span>
            </div>
            <textarea
              id="agent-description"
              className="min-h-20 w-full resize-y rounded border border-wardian-light bg-[var(--color-wardian-input-bg)] px-3 py-2 text-sm text-primary transition-colors focus:border-[var(--color-wardian-accent)] focus:outline-none"
              maxLength={280}
              placeholder="What is this agent responsible for?"
              value={config.description ?? ""}
              onChange={(e) => updateField("description", e.target.value)}
            />
          </div>
          <div>
            <label className="block text-[10px] font-bold text-muted-neutral mb-1">Agent Class</label>
            <select
              className="w-full bg-[var(--color-wardian-input-bg)] border border-wardian-light rounded px-3 py-2 text-sm text-primary focus:outline-none focus:border-[var(--color-wardian-accent)] transition-colors"
              value={config.agent_class}
              onChange={async (e) => {
                const newClass = e.target.value;
                updateField("agent_class", newClass);
                // Proactively resolve system include directories for the new class
                try {
                  const sysDirs: string[] = await invoke("resolve_system_include_directories", { className: newClass, sessionId: config.session_id });
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
          <div>
            <label className="block text-[10px] font-bold text-muted-neutral mb-1">Workspace Path</label>
            <input
              readOnly
              className="w-full bg-[var(--color-wardian-input-bg)] border border-wardian-border rounded px-3 py-2 text-xs text-muted-neutral font-mono focus:outline-none select-text cursor-text"
              value={config.folder}
            />
          </div>
          <div>
            <label className="block text-[10px] font-bold text-muted-neutral mb-1">Provider</label>
            <select
              aria-label="Provider"
              className="w-full bg-[var(--color-wardian-input-bg)] border border-wardian-light rounded px-3 py-2 text-sm text-primary focus:outline-none focus:border-[var(--color-wardian-accent)] transition-colors"
              value={currentProvider}
              onChange={(e) => {
                const provider = e.target.value as UserFacingProviderName;
                setConfig(prev => prev ? { ...prev, ...withProvider(prev, provider, { preserveCustomArgs: true }) } : null);
              }}
            >
              {providerOptions.map((option) => (
                <option key={option.value} value={option.value} disabled={!option.available}>
                  {option.label}
                </option>
              ))}
            </select>
            {!selectedProviderAvailable && (
              <p className="mt-1 text-[10px] text-wardian-warning">
                Choose an installed provider before saving this agent.
              </p>
            )}
            {providerNote && (
              <p className="mt-1 text-[10px] text-wardian-warning">{providerNote}</p>
            )}
          </div>
          <ProviderModelSelector
            idPrefix="configure-agent"
            provider={config.provider}
            selection={{
              model: config.model,
              reasoning_effort: reasoningEffortForConfig(config),
            }}
            onSelectionChange={(selection) => {
              setConfig((current) => current
                ? withReasoningEffort({ ...current, model: selection.model }, selection.reasoning_effort) as AgentConfig
                : null);
            }}
          />
          <div>
            <div className="flex justify-between items-center mb-1">
              <label className="block text-[10px] font-bold text-muted-neutral">Agent ID</label>
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
                className={`text-[9px] font-bold tracking-wider px-2 py-1 rounded transition-all active:scale-95 cursor-pointer ${copiedId ? 'bg-wardian-success/20 text-wardian-success border border-wardian-success/30' : 'bg-wardian-card-bg-muted text-muted-neutral hover:text-primary hover:bg-wardian-light border border-transparent'}`}
              >
                {copiedId ? "Copied!" : "Copy"}
              </button>
            </div>
            <input
              readOnly
              className="w-full bg-[var(--color-wardian-input-bg)] border border-wardian-border rounded px-3 py-2 text-xs text-muted-neutral font-mono focus:outline-none select-text cursor-text"
              value={config.session_id}
            />
          </div>
          <div>
            <div className="flex justify-between items-center mb-1">
              <label className="block text-[10px] font-bold text-muted-neutral">Log Path</label>
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
                className={`text-[9px] font-bold tracking-wider px-2 py-1 rounded transition-all active:scale-95 cursor-pointer ${copiedLog ? 'bg-wardian-success/20 text-wardian-success border border-wardian-success/30' : 'bg-wardian-card-bg-muted text-muted-neutral hover:text-primary hover:bg-wardian-light border border-transparent disabled:opacity-30 disabled:cursor-not-allowed'}`}
              >
                {copiedLog ? "Copied!" : "Copy"}
              </button>
            </div>
            <input
              readOnly
              className="w-full bg-[var(--color-wardian-input-bg)] border border-wardian-border rounded px-3 py-2 text-xs text-muted-neutral font-mono focus:outline-none select-text cursor-text"
              value={telemetry[agentId]?.log_path || "Not available (Agent is offline or generating logs)"}
            />
          </div>
        </div>

        <div className="flex flex-col gap-2 mt-2">
          <h3 className="text-xs font-bold text-muted tracking-wide">Skills</h3>
          <button
            type="button"
            data-testid="configure-agent-manage-skills"
            onClick={() => openLibraryAt("skills")}
            className="self-start rounded border border-wardian-light/50 bg-wardian-card-bg-muted px-3 py-1.5 text-xs text-primary transition-colors hover:border-[var(--color-wardian-accent)] hover:text-[var(--color-wardian-accent)]"
          >
            Manage skills in Library
          </button>
        </div>

        <AdvancedSettings 
          config={config} 
          updateField={updateField} 
          showCopyFullCommand
        />

        <button
          type="submit"
          disabled={isSaving || !selectedProviderAvailable}
          className="w-full mt-2 bg-[var(--color-wardian-accent)] hover:opacity-90 disabled:opacity-50 text-[var(--color-wardian-bg)] py-2.5 rounded-lg font-bold text-xs tracking-wide transition-all shadow-lg shadow-[var(--color-wardian-accent)]/20"
        >
          {isSaving ? "Saving..." : "Save Changes"}
        </button>
      </form>
    </div>
  );
};
