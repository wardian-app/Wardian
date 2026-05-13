import React, { useState } from 'react';
import { ListEditor } from './ListEditor';
import { AgentConfig } from '../types';
import { invoke } from '@tauri-apps/api/core';
import { writeText } from '@tauri-apps/plugin-clipboard-manager';
import { Check, Copy } from 'lucide-react';
import { providerConfigFor } from '../features/agents/configUtils';

interface AdvancedSettingsProps {
  config: Partial<AgentConfig>;
  updateField: (field: keyof AgentConfig, value: any) => void;
  showCopyFullCommand?: boolean;
}

export const AdvancedSettings: React.FC<AdvancedSettingsProps> = ({ 
  config, 
  updateField,
  showCopyFullCommand = false
}) => {
  const [showAdvanced, setShowAdvanced] = useState(false);
  const [copyCommandState, setCopyCommandState] = useState<"idle" | "copied" | "error">("idle");

  const copyFullAgentCommand = async () => {
    try {
      const command = await invoke<string>("build_agent_cli_command", {
        sessionId: config.session_id,
      });
      await writeText(command);
      setCopyCommandState("copied");
      setTimeout(() => setCopyCommandState("idle"), 2000);
    } catch (error) {
      console.error("Failed to copy full agent command", error);
      setCopyCommandState("error");
      setTimeout(() => setCopyCommandState("idle"), 3000);
    }
  };

  const provider = config.provider || "claude";
  const providerConfig = providerConfigFor(config, provider);
  const updateProviderConfigField = (field: string, value: unknown) => {
    updateField("provider_config", { ...providerConfig, [field]: value } as AgentConfig["provider_config"]);
  };

  return (
    <>
      <div className="flex flex-col gap-2">
        <button 
          type="button"
          onClick={() => setShowAdvanced(!showAdvanced)}
          className="flex items-center gap-2 text-[10px] font-bold text-muted tracking-wide hover:text-primary transition-colors"
        >
          <svg 
            className={`w-3 h-3 transform transition-transform ${showAdvanced ? 'rotate-90' : ''}`} 
            fill="none" stroke="currentColor" viewBox="0 0 24 24"
          >
            <path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M9 5l7 7-7 7" />
          </svg>
          Advanced Settings
        </button>
      </div>

      {showAdvanced && (
        <div className="flex flex-col gap-4 pt-2">
          <div>
              <label htmlFor="regular-session-resume" className="block text-[10px] font-bold text-muted-neutral mb-1">Regular Session Resume</label>
              <select
              id="regular-session-resume"
              className="w-full bg-[var(--color-wardian-input-bg)] border border-wardian-light rounded px-3 py-1.5 text-xs text-primary focus:outline-none focus:border-[var(--color-wardian-accent)] transition-colors"
              value={config.session_persistence || "default"}
              onChange={(e) => updateField("session_persistence", e.target.value as AgentConfig["session_persistence"])}
              >
                  <option value="default">Use global default</option>
                  <option value="fresh">Start fresh on resume</option>
                  <option value="resume">Resume provider session</option>
              </select>
          </div>

          {showCopyFullCommand && config.session_id && config.folder && (
            <div>
              <button
                type="button"
                onClick={copyFullAgentCommand}
                className={`w-full flex items-center justify-center gap-2 rounded border px-3 py-2 text-[10px] font-bold tracking-wide transition-all active:scale-95 ${
                  copyCommandState === "copied"
                    ? "border-wardian-success/30 bg-wardian-success/20 text-wardian-success"
                    : copyCommandState === "error"
                      ? "border-wardian-error/30 bg-wardian-error/15 text-wardian-error"
                      : "border-wardian-border bg-wardian-card-bg-muted text-muted-neutral hover:border-[var(--color-wardian-accent)]/40 hover:text-primary hover:bg-wardian-light/30"
                }`}
              >
                {copyCommandState === "copied" ? (
                  <Check className="h-3.5 w-3.5" aria-hidden="true" />
                ) : (
                  <Copy className="h-3.5 w-3.5" aria-hidden="true" />
                )}
                {copyCommandState === "copied"
                  ? "Copied!"
                  : copyCommandState === "error"
                    ? "Copy failed"
                    : "Copy Full Agent Command"}
              </button>
            </div>
          )}

          {/* Gemini CLI Properties */}
          <div className="flex flex-col gap-4">
              <h4 className="text-[10px] font-bold text-muted-neutral tracking-wide mb-1 border-b border-wardian-border pb-1">Provider Parameters</h4>

              <div className="grid grid-cols-2 gap-2 mb-1">
                  <label className="flex items-center gap-2 text-xs text-muted-neutral">
                      <input type="checkbox" checked={config.debug || false} onChange={e => updateField("debug", e.target.checked)} className="accent-[var(--color-wardian-accent)]" />
                      Debug Mode
                  </label>
                  {provider === 'gemini' && providerConfig.type === 'gemini' && (
                    <>
                      <label className="flex items-center gap-2 text-xs text-muted-neutral">
                          <input type="checkbox" checked={providerConfig.sandbox || false} onChange={e => updateProviderConfigField("sandbox", e.target.checked)} className="accent-[var(--color-wardian-accent)]" />
                          Sandbox
                      </label>
                      <label className="flex items-center gap-2 text-xs text-muted-neutral">
                          <input type="checkbox" checked={providerConfig.yolo || false} onChange={e => updateProviderConfigField("yolo", e.target.checked)} className="accent-[var(--color-wardian-accent)]" />
                          YOLO
                      </label>
                      <label className="flex items-center gap-2 text-xs text-muted-neutral">
                          <input type="checkbox" checked={providerConfig.experimental_acp || false} onChange={e => updateProviderConfigField("experimental_acp", e.target.checked)} className="accent-[var(--color-wardian-accent)]" />
                          Exp. ACP
                      </label>
                      <label className="flex items-center gap-2 text-xs text-muted-neutral">
                          <input type="checkbox" checked={providerConfig.screen_reader || false} onChange={e => updateProviderConfigField("screen_reader", e.target.checked)} className="accent-[var(--color-wardian-accent)]" />
                          Screen Reader
                      </label>
                    </>
                  )}
              </div>

              <div>
                  <label className="block text-[10px] font-bold text-muted-neutral mb-1">Model Override</label>
                  <input
                  className="w-full bg-[var(--color-wardian-input-bg)] border border-wardian-light rounded px-3 py-1.5 text-xs text-primary focus:outline-none focus:border-[var(--color-wardian-accent)] transition-colors"
                  placeholder="e.g. gemini-2.5-flash / claude-3-7-sonnet / gpt-5.4 / openai/gpt-5"
                  value={config.model || ""}
                  onChange={(e) => updateField("model", e.target.value || undefined)}
                  />
              </div>

              <ListEditor 
                label="Include Directories" 
                values={config.include_directories} 
                systemValues={config.system_include_directories}
                placeholder="e.g. C:/projects/my-app"
                onChange={(vals: string[]) => updateField("include_directories", vals)} 
                validate={(path: string) => invoke("validate_directory_path", { path })}
                onSystemValueDelete={(idx: number) => {
                  const newList = config.system_include_directories?.filter((_, i) => i !== idx);
                  updateField("system_include_directories", newList?.length ? newList : undefined);
                }}
              />

              {provider === 'claude' && providerConfig.type === 'claude' && (
                <>
                  <div>
                      <label htmlFor="claude-permission-mode" className="block text-[10px] font-bold text-muted-neutral mb-1">Permission Mode</label>
                      <select
                      id="claude-permission-mode"
                      className="w-full bg-[var(--color-wardian-input-bg)] border border-wardian-light rounded px-3 py-1.5 text-xs text-primary focus:outline-none focus:border-[var(--color-wardian-accent)] transition-colors"
                      value={providerConfig.permission_mode || ""}
                      onChange={(e) => updateProviderConfigField("permission_mode", e.target.value || undefined)}
                      >
                          <option value="">(None - Inherit Default)</option>
                          <option value="default">Default</option>
                          <option value="plan">Plan</option>
                          <option value="auto-accept">Auto Accept</option>
                      </select>
                  </div>

                  <div>
                      <label className="block text-[10px] font-bold text-muted-neutral mb-1">Max Turns</label>
                      <input
                      type="number"
                      min="0"
                      className="w-full bg-[var(--color-wardian-input-bg)] border border-wardian-light rounded px-3 py-1.5 text-xs text-primary focus:outline-none focus:border-[var(--color-wardian-accent)] transition-colors"
                      placeholder="Unlimited"
                      value={providerConfig.max_turns ?? ""}
                      onChange={(e) => updateProviderConfigField("max_turns", e.target.value ? parseInt(e.target.value) : undefined)}
                      />
                  </div>

                  <ListEditor
                    label="Allowed Tools"
                    values={providerConfig.allowed_tools}
                    placeholder="e.g. Read, Write, Bash"
                    onChange={(vals: string[]) => updateProviderConfigField("allowed_tools", vals)}
                  />

                  <ListEditor
                    label="Disallowed Tools"
                    values={providerConfig.disallowed_tools}
                    placeholder="e.g. Bash"
                    onChange={(vals: string[]) => updateProviderConfigField("disallowed_tools", vals)}
                  />

                  <div>
                      <label className="block text-[10px] font-bold text-muted-neutral mb-1">MCP Config Path</label>
                      <input
                      className="w-full bg-[var(--color-wardian-input-bg)] border border-wardian-light rounded px-3 py-1.5 text-xs text-primary focus:outline-none focus:border-[var(--color-wardian-accent)] transition-colors font-mono"
                      placeholder="e.g. ~/.claude/mcp.json"
                      value={providerConfig.mcp_config || ""}
                      onChange={(e) => updateProviderConfigField("mcp_config", e.target.value || undefined)}
                      />
                  </div>

                  <div>
                      <label className="block text-[10px] font-bold text-muted-neutral mb-1">Append System Prompt</label>
                      <textarea
                      className="w-full bg-[var(--color-wardian-input-bg)] border border-wardian-light rounded px-3 py-2 text-xs text-primary focus:outline-none focus:border-[var(--color-wardian-accent)] transition-colors h-16 resize-none"
                      placeholder="Additional instructions appended to Claude's system prompt"
                      value={providerConfig.append_system_prompt || ""}
                      onChange={(e) => updateProviderConfigField("append_system_prompt", e.target.value || undefined)}
                      />
                  </div>
                </>
              )}

              {provider === 'gemini' && providerConfig.type === 'gemini' && (
                <>
                  <div>
                      <label className="block text-[10px] font-bold text-muted-neutral mb-1">Approval Mode</label>
                      <select
                      className="w-full bg-[var(--color-wardian-input-bg)] border border-wardian-light rounded px-3 py-1.5 text-xs text-primary focus:outline-none focus:border-[var(--color-wardian-accent)] transition-colors"
                      value={providerConfig.approval_mode || ""}
                      onChange={(e) => updateProviderConfigField("approval_mode", e.target.value || undefined)}
                      >
                          <option value="">(None - Inherit Default)</option>
                          <option value="default">Default</option>
                          <option value="auto_edit">Auto Edit</option>
                          <option value="yolo">YOLO</option>
                          <option value="plan">Plan</option>
                      </select>
                  </div>

                  <ListEditor
                    label="Policies"
                    values={providerConfig.policy}
                    placeholder="e.g. read_only"
                    onChange={(vals: string[]) => updateProviderConfigField("policy", vals)}
                  />

                  <ListEditor
                    label="Allowed MCP Servers"
                    values={providerConfig.allowed_mcp_server_names}
                    placeholder="e.g. sqlite-mcp"
                    onChange={(vals: string[]) => updateProviderConfigField("allowed_mcp_server_names", vals)}
                  />

                  <ListEditor
                    label="Extensions"
                    values={providerConfig.extensions}
                    placeholder="e.g. github, search"
                    onChange={(vals: string[]) => updateProviderConfigField("extensions", vals)}
                  />

                  <div>
                      <label className="block text-[10px] font-bold text-muted-neutral mb-1">Output Format</label>
                      <select
                      className="w-full bg-[var(--color-wardian-input-bg)] border border-wardian-light rounded px-3 py-1.5 text-xs text-primary focus:outline-none focus:border-[var(--color-wardian-accent)] transition-colors"
                      value={providerConfig.output_format || ""}
                      onChange={(e) => updateProviderConfigField("output_format", (e.target.value as "text" | "json" | "stream-json") || undefined)}
                      >
                          <option value="">(None - Inherit Default)</option>
                          <option value="text">Text</option>
                          <option value="json">JSON</option>
                          <option value="stream-json">Stream JSON</option>
                      </select>
                  </div>
                </>
              )}

              {provider === 'codex' && providerConfig.type === 'codex' && (
                <>
                  <div>
                      <label htmlFor="codex-sandbox-mode" className="block text-[10px] font-bold text-muted-neutral mb-1">Sandbox Mode</label>
                      <select
                      id="codex-sandbox-mode"
                      className="w-full bg-[var(--color-wardian-input-bg)] border border-wardian-light rounded px-3 py-1.5 text-xs text-primary focus:outline-none focus:border-[var(--color-wardian-accent)] transition-colors"
                      value={providerConfig.sandbox_mode || ""}
                      onChange={(e) => updateProviderConfigField("sandbox_mode", (e.target.value as AgentConfig["codex_sandbox_mode"]) || undefined)}
                      >
                          <option value="">(None - Inherit Default)</option>
                          <option value="read-only">read-only</option>
                          <option value="workspace-write">workspace-write</option>
                          <option value="danger-full-access">danger-full-access</option>
                      </select>
                  </div>

                  <div>
                      <label className="block text-[10px] font-bold text-muted-neutral mb-1">Approval Policy</label>
                      <select
                      className="w-full bg-[var(--color-wardian-input-bg)] border border-wardian-light rounded px-3 py-1.5 text-xs text-primary focus:outline-none focus:border-[var(--color-wardian-accent)] transition-colors"
                      value={providerConfig.approval_policy || ""}
                      onChange={(e) => updateProviderConfigField("approval_policy", (e.target.value as AgentConfig["codex_approval_policy"]) || undefined)}
                      >
                          <option value="">(None - Inherit Default)</option>
                          <option value="untrusted">untrusted</option>
                          <option value="on-failure">on-failure</option>
                          <option value="on-request">on-request</option>
                          <option value="never">never</option>
                      </select>
                  </div>

                  <div>
                      <label className="block text-[10px] font-bold text-muted-neutral mb-1">Profile</label>
                      <input
                      className="w-full bg-[var(--color-wardian-input-bg)] border border-wardian-light rounded px-3 py-1.5 text-xs text-primary focus:outline-none focus:border-[var(--color-wardian-accent)] transition-colors"
                      placeholder="e.g. wardian"
                      value={providerConfig.profile || ""}
                      onChange={(e) => updateProviderConfigField("profile", e.target.value || undefined)}
                      />
                  </div>

                  <div className="grid grid-cols-2 gap-2 mb-1">
                    <label className="flex items-center gap-2 text-xs text-muted-neutral">
                        <input type="checkbox" checked={providerConfig.full_auto || false} onChange={e => updateProviderConfigField("full_auto", e.target.checked)} className="accent-[var(--color-wardian-accent)]" />
                        Full Auto
                    </label>
                    <label className="flex items-center gap-2 text-xs text-muted-neutral">
                        <input type="checkbox" checked={providerConfig.search || false} onChange={e => updateProviderConfigField("search", e.target.checked)} className="accent-[var(--color-wardian-accent)]" />
                        Search
                    </label>
                    <label className="flex items-center gap-2 text-xs text-muted-neutral">
                        <input type="checkbox" checked={providerConfig.skip_git_repo_check ?? true} onChange={e => updateProviderConfigField("skip_git_repo_check", e.target.checked)} className="accent-[var(--color-wardian-accent)]" />
                        Skip Git Check
                    </label>
                    <label className="flex items-center gap-2 text-xs text-muted-neutral">
                        <input type="checkbox" checked={providerConfig.ephemeral || false} onChange={e => updateProviderConfigField("ephemeral", e.target.checked)} className="accent-[var(--color-wardian-accent)]" />
                        Ephemeral
                    </label>
                  </div>
                </>
              )}

              {provider === 'opencode' && providerConfig.type === 'opencode' && (
                <div>
                    <label
                      htmlFor="opencode-agent"
                      className="block text-[10px] font-bold text-muted-neutral mb-1"
                    >
                      OpenCode Agent
                    </label>
                    <input
                    id="opencode-agent"
                    className="w-full bg-[var(--color-wardian-input-bg)] border border-wardian-light rounded px-3 py-1.5 text-xs text-primary focus:outline-none focus:border-[var(--color-wardian-accent)] transition-colors"
                    placeholder="e.g. build"
                    value={providerConfig.agent || ""}
                    onChange={(e) => updateProviderConfigField("agent", e.target.value || undefined)}
                    />
                </div>
              )}
          </div>

          {/* Custom Arguments */}
          <div className="flex flex-col gap-4">
              <div>
                  <label htmlFor="provider-custom-args" className="block text-[10px] font-bold text-[var(--color-wardian-accent)] mb-1">Custom Arguments</label>
                  <textarea
                  id="provider-custom-args"
                  className="w-full bg-[var(--color-wardian-input-bg)] border border-wardian-light rounded px-3 py-2 text-xs text-primary focus:outline-none focus:border-[var(--color-wardian-accent)] transition-colors h-16 resize-none font-mono"
                  placeholder='--extra-flag --some-opt "a value with spaces"'
                  value={config.custom_args || ""}
                  onChange={(e) => updateField("custom_args", e.target.value || undefined)}
                  />
              </div>
          </div>
        </div>
      )}
    </>
  );
};
