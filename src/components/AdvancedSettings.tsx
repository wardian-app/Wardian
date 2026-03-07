import React, { useState } from 'react';
import { ListEditor } from './ListEditor';
import { AgentConfig } from '../types';
import { invoke } from '@tauri-apps/api/core';

interface AdvancedSettingsProps {
  config: Partial<AgentConfig>;
  updateField: (field: keyof AgentConfig, value: any) => void;
}

export const AdvancedSettings: React.FC<AdvancedSettingsProps> = ({ 
  config, 
  updateField
}) => {
  const [showAdvanced, setShowAdvanced] = useState(false);

  return (
    <>
      <div className="flex flex-col gap-2">
        <button 
          type="button"
          onClick={() => setShowAdvanced(!showAdvanced)}
          className="flex items-center gap-2 text-[10px] font-bold text-gray-400 uppercase tracking-widest hover:text-white transition-colors"
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
          {/* Gemini CLI Properties */}
          <div className="flex flex-col gap-4">
              <h4 className="text-[10px] font-bold text-gray-500 uppercase tracking-widest mb-1 border-b border-gray-800/50 pb-1">Gemini CLI Parameters</h4>
              
              <div className="grid grid-cols-2 gap-2 mb-1">
                  <label className="flex items-center gap-2 text-xs text-gray-300">
                      <input type="checkbox" checked={config.debug || false} onChange={e => updateField("debug", e.target.checked)} className="accent-[var(--color-wardian-accent)]" />
                      Debug Mode
                  </label>
                  <label className="flex items-center gap-2 text-xs text-gray-300">
                      <input type="checkbox" checked={config.sandbox || false} onChange={e => updateField("sandbox", e.target.checked)} className="accent-[var(--color-wardian-accent)]" />
                      Sandbox
                  </label>
                  <label className="flex items-center gap-2 text-xs text-gray-300">
                      <input type="checkbox" checked={config.yolo || false} onChange={e => updateField("yolo", e.target.checked)} className="accent-[var(--color-wardian-accent)]" />
                      YOLO
                  </label>
                  <label className="flex items-center gap-2 text-xs text-gray-300">
                      <input type="checkbox" checked={config.experimental_acp || false} onChange={e => updateField("experimental_acp", e.target.checked)} className="accent-[var(--color-wardian-accent)]" />
                      Exp. ACP
                  </label>
                  <label className="flex items-center gap-2 text-xs text-gray-300">
                      <input type="checkbox" checked={config.screen_reader || false} onChange={e => updateField("screen_reader", e.target.checked)} className="accent-[var(--color-wardian-accent)]" />
                      Screen Reader
                  </label>
              </div>

              <div>
                  <label className="block text-[10px] font-bold text-gray-500 uppercase mb-1">Model Override</label>
                  <input
                  className="w-full bg-gray-800/50 border border-gray-700 rounded px-3 py-1.5 text-xs text-white focus:outline-none focus:border-[var(--color-wardian-accent)] transition-colors"
                  placeholder="e.g. gemini-2.5-flash"
                  value={config.model || ""}
                  onChange={(e) => updateField("model", e.target.value || undefined)}
                  />
              </div>

              <div>
                  <label className="block text-[10px] font-bold text-gray-500 uppercase mb-1">Approval Mode</label>
                  <select
                  className="w-full bg-gray-800/50 border border-gray-700 rounded px-3 py-1.5 text-xs text-white focus:outline-none focus:border-[var(--color-wardian-accent)] transition-colors"
                  value={config.approval_mode || ""}
                  onChange={(e) => updateField("approval_mode", (e.target.value as any) || undefined)}
                  >
                      <option value="">(None - Inherit Default)</option>
                      <option value="default">Default</option>
                      <option value="auto_edit">Auto Edit</option>
                      <option value="yolo">YOLO</option>
                      <option value="plan">Plan</option>
                  </select>
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

              <ListEditor 
                label="Policies" 
                values={config.policy} 
                placeholder="e.g. read_only"
                onChange={(vals: string[]) => updateField("policy", vals)} 
              />

              <ListEditor 
                label="Allowed MCP Servers" 
                values={config.allowed_mcp_server_names} 
                placeholder="e.g. sqlite-mcp"
                onChange={(vals: string[]) => updateField("allowed_mcp_server_names", vals)} 
              />

              <ListEditor 
                label="Extensions" 
                values={config.extensions} 
                placeholder="e.g. github, search"
                onChange={(vals: string[]) => updateField("extensions", vals)} 
              />

              <div>
                  <label className="block text-[10px] font-bold text-gray-500 uppercase mb-1">Output Format</label>
                  <select
                  className="w-full bg-gray-800/50 border border-gray-700 rounded px-3 py-1.5 text-xs text-white focus:outline-none focus:border-[var(--color-wardian-accent)] transition-colors"
                  value={config.output_format || ""}
                  onChange={(e) => updateField("output_format", (e.target.value as "text" | "json" | "stream-json") || undefined)}
                  >
                      <option value="">(None - Inherit Default)</option>
                      <option value="text">Text</option>
                      <option value="json">JSON</option>
                      <option value="stream-json">Stream JSON</option>
                  </select>
              </div>
          </div>

          {/* Custom Arguments */}
          <div className="flex flex-col gap-4">
              <div>
                  <label className="block text-[10px] font-bold text-[var(--color-wardian-accent)] uppercase mb-1">Custom Arguments</label>
                  <textarea
                  className="w-full bg-gray-800/50 border border-gray-700 rounded px-3 py-2 text-xs text-white focus:outline-none focus:border-[var(--color-wardian-accent)] transition-colors h-16 resize-none font-mono"
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
