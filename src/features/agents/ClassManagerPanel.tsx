import React, { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { AgentClassDefinition } from "../../types";

import { ManageSkills } from "../library/ManageSkills";

interface ClassManagerPanelProps {
  agentClasses: AgentClassDefinition[];
  onClassesUpdated: () => void;
}

export const ClassManagerPanel: React.FC<ClassManagerPanelProps> = ({
  agentClasses,
  onClassesUpdated,
}) => {
  const [newClassName, setNewClassName] = useState("");
  const [newClassDesc, setNewClassDesc] = useState("");
  const [newClassInstruction, setNewClassInstruction] = useState("");
  const [isCreatingClass, setIsCreatingClass] = useState(false);

  const createAgentClass = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!newClassName.trim()) return;
    setIsCreatingClass(true);
    try {
      await invoke("create_agent_class", {
        name: newClassName,
        description: newClassDesc,
        instructionContent: newClassInstruction || null,
      });
      onClassesUpdated();
      setNewClassName("");
      setNewClassDesc("");
      setNewClassInstruction("");
    } catch (error) {
      alert(`Failed to create class: ${error}`);
    } finally {
      setIsCreatingClass(false);
    }
  };

  const deleteAgentClass = async (name: string) => {
    if (!confirm(`Delete custom class "${name}"? This will also remove its directory.`)) return;
    try {
      await invoke("delete_agent_class", { name });
      onClassesUpdated();
    } catch (error) {
      alert(`Failed to delete class: ${error}`);
    }
  };
  
  const resetAllPrompts = async () => {
    if (!confirm("Reset ALL default agent prompts to system defaults? This will overwrite your current AGENTS.md instructions for all default classes.")) return;
    try {
      await invoke("reset_all_class_prompts");
      alert("All default agent prompts have been reset to system defaults.");
      onClassesUpdated();
    } catch (error) {
      alert(`Failed to reset all prompts: ${error}`);
    }
  };

  return (
    <div className="flex flex-col h-full">
      <div className="flex justify-between items-center mb-6">
        <h2 className="text-xl font-bold text-primary tracking-tight">Classes</h2>
      </div>

      <div className="mb-6">
        <h3 className="text-xs font-bold text-muted tracking-wide mb-4">Create Class</h3>
        <form className="flex flex-col gap-3" onSubmit={createAgentClass}>
          <div>
            <label className="block text-[10px] font-bold text-muted-neutral mb-1">Name</label>
            <input
              className="w-full bg-[var(--color-wardian-input-bg)] border border-wardian-light rounded px-3 py-2 text-sm text-primary focus:outline-none focus:border-[var(--color-wardian-accent)] transition-colors"
              placeholder="e.g. DevOps"
              value={newClassName}
              onChange={(e) => setNewClassName(e.currentTarget.value)}
            />
          </div>
          <div>
            <label className="block text-[10px] font-bold text-muted-neutral mb-1">Description</label>
            <textarea
              className="w-full bg-[var(--color-wardian-input-bg)] border border-wardian-light rounded px-3 py-2 text-sm text-primary focus:outline-none focus:border-[var(--color-wardian-accent)] transition-colors h-20 resize-none"
              placeholder="Manages CI/CD pipelines and infrastructure..."
              value={newClassDesc}
              onChange={(e) => setNewClassDesc(e.currentTarget.value)}
            />
          </div>
          <div>
            <label className="block text-[10px] font-bold text-muted-neutral mb-1">Instructions</label>
            <textarea
              className="w-full bg-[var(--color-wardian-input-bg)] border border-wardian-light rounded px-3 py-2 text-xs text-primary focus:outline-none focus:border-[var(--color-wardian-accent)] transition-colors h-40 resize-none font-mono"
              placeholder={`# Role: ${newClassName || "Agent"}\n\nDefine the agent's system prompt...`}

              value={newClassInstruction}
              onChange={(e) => setNewClassInstruction(e.currentTarget.value)}
            />
          </div>
          <button
            type="submit"
            disabled={isCreatingClass || !newClassName.trim()}
            className="w-full bg-wardian-card-bg-muted border border-wardian-light/50 rounded-lg text-muted-neutral hover:text-[var(--color-wardian-accent)] hover:border-[var(--color-wardian-accent)]/30 py-2 font-bold text-xs tracking-wide transition-all flex items-center justify-center gap-2"
          >
            {isCreatingClass ? (
              <div className="animate-spin w-4 h-4 border-2 border-white/30 border-t-white rounded-full"></div>
            ) : (
              "Create"
            )}
          </button>
        </form>
      </div>

      <div className="border-t border-wardian-border pt-4 overflow-y-auto no-scrollbar flex-1 pb-8">
        <h3 className="text-xs font-bold text-muted tracking-wide mb-3">Available Classes</h3>
        <div className="space-y-2 mb-6">
          {agentClasses.map(cls => (
            <div key={cls.name} className={`p-3 bg-wardian-card-bg-muted border rounded-lg group ${cls.is_default ? 'border-wardian-border' : 'border-[var(--color-wardian-accent)]/20'}`}>
              <div className="flex items-center justify-between mb-1">
                <div className="flex items-center gap-2">
                    <span className="text-sm font-bold text-primary">{cls.name}</span>
                    {cls.is_default && (
                        <span className="text-[9px] font-bold text-muted-neutral tracking-wide bg-wardian-card-bg-muted px-2 py-0.5 rounded border border-wardian-light/30">Default</span>
                    )}
                </div>
                {!cls.is_default && (
                    <button
                      onClick={() => deleteAgentClass(cls.name)}
                      className="text-muted hover:text-red-500 transition-colors opacity-0 group-hover:opacity-100 p-0.5"
                      title="Delete class"
                    >
                      <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M19 7l-.867 12.142A2 2 0 0116.138 21H7.862a2 2 0 01-1.995-1.858L5 7m5 4v6m4-6v6m1-10V4a1 1 0 00-1-1h-4a1 1 0 00-1 1v3M4 7h16"></path></svg>
                    </button>
                )}
              </div>
              <p className="text-[11px] text-muted-neutral mb-2">{cls.description}</p>
              <ManageSkills targetType="class" targetId={cls.name} />
            </div>
          ))}
        </div>

        <div className="px-1 mt-4">
            <button
                onClick={resetAllPrompts}
                className="w-full text-[9px] font-bold text-muted hover:text-[var(--color-wardian-accent)] uppercase tracking-[0.2em] py-3 border border-dashed border-wardian-light/20 rounded-lg transition-all hover:border-[var(--color-wardian-accent)]/30 group/reset"
            >
                Reset All Default Prompts
            </button>
            <p className="text-[9px] text-muted-neutral text-center mt-2 opacity-50 italic">
                This will overwrite AGENTS.md for all default roles.
            </p>
        </div>
      </div>
    </div>
  );
};
