import React, { useEffect, useMemo } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useLibraryStore } from "../../store/useLibraryStore";
import { LibraryFolder, LibraryPrompt } from "../../types";

interface CommandPanelProps {
  selectedAgentIds: Set<string>;
  broadcastMessage: string;
  setBroadcastMessage: (msg: string) => void;
  onBroadcast: (e: React.FormEvent) => void;
  onCollapse: () => void;
}

export const CommandPanel: React.FC<CommandPanelProps> = ({
  selectedAgentIds,
  broadcastMessage,
  setBroadcastMessage,
  onBroadcast,
  onCollapse,
}) => {
  const { promptTree, fetchLibraryTree } = useLibraryStore();

  useEffect(() => {
    if (!promptTree) {
      fetchLibraryTree('prompts');
    }
  }, [promptTree, fetchLibraryTree]);

  const quickPrompts = useMemo(() => {
    const results: LibraryPrompt[] = [];
    if (!promptTree) return results;

    function traverse(folder: LibraryFolder) {
      for (const child of folder.children) {
        if ('content' in child) {
          if (child.metadata.is_starred) {
            results.push(child as LibraryPrompt);
          }
        } else if ('children' in child) {
          traverse(child as LibraryFolder);
        }
      }
    }

    traverse(promptTree);
    return results;
  }, [promptTree]);

  const handleInject = async (promptContent: string) => {
    try {
      const flattenedPrompt = promptContent.replace(/\r?\n/g, ' ');
      if (selectedAgentIds.size > 0) {
        for (const id of selectedAgentIds) {
          await invoke("send_input_to_agent", { sessionId: id, input: flattenedPrompt });
        }
      } else {
        if (window.confirm("No agents selected. This will broadcast the prompt to ALL agents. Are you sure?")) {
          await invoke("broadcast_input", { input: flattenedPrompt });
        }
      }
    } catch (e) {
      console.error("Injection failed", e);
    }
  };

  const handleBroadcastSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    if (selectedAgentIds.size === 0) {
      if (!window.confirm("No agents selected. This will broadcast to ALL agents. Are you sure?")) {
        return;
      }
    }
    onBroadcast(e);
  };

  return (
    <div className="flex flex-col h-full">
      <div className="flex justify-between items-center mb-6">
        <h2 className="text-xl font-bold text-primary tracking-tight">COMMAND</h2>
        <button onClick={onCollapse} className="text-bright-neutral hover:text-primary transition-colors">
          <svg className="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M15 19l-7-7 7-7"></path></svg>
        </button>
      </div>

      <div className="mb-8 flex-1 overflow-y-auto pr-2 no-scrollbar">
        <h3 className="text-xs font-bold text-muted uppercase tracking-widest mb-4">Quick Prompts</h3>
        <div className="flex flex-col gap-2">
          {quickPrompts.length === 0 ? (
            <div className="text-xs text-muted-neutral italic">No quick prompts in Library.</div>
          ) : (
            quickPrompts.map((prompt, idx) => (
              <button 
                key={`starred-${prompt.path}-${idx}`}
                onClick={() => handleInject(prompt.content)}
                className="flex flex-col items-start p-3 bg-wardian-card-bg-muted border border-wardian-light/50 rounded-lg text-primary hover:text-[var(--color-wardian-accent)] hover:border-[var(--color-wardian-accent)]/30 transition-all text-left group"
              >
                <span className="text-xs font-bold truncate w-full">{prompt.name}</span>
                <span className="text-[10px] text-muted-neutral mt-1 w-full line-clamp-1 whitespace-pre-wrap leading-relaxed group-hover:text-primary/70 transition-colors">
                  {prompt.content}
                </span>
              </button>
            ))
          )}
        </div>
      </div>

      <div className="mt-auto pt-6 border-t border-wardian-border flex-shrink-0">
        <h3 className="text-xs font-bold text-muted uppercase tracking-widest mb-4">Broadcast</h3>
        <form onSubmit={handleBroadcastSubmit} className="flex flex-col gap-2">
          <textarea
            className="w-full bg-[var(--color-wardian-input-bg)] border border-wardian-light rounded px-3 py-2 text-primary text-xs focus:outline-none focus:border-[var(--color-wardian-accent)] h-32 resize-none"
            placeholder={selectedAgentIds.size > 0 ? `Message ${selectedAgentIds.size} selected...` : "Broadcast to all agents..."}
            value={broadcastMessage}
            onChange={(e) => setBroadcastMessage(e.currentTarget.value)}
          />
          <button
            type="submit"
            className="bg-wardian-success/20 hover:bg-wardian-success/40 border border-wardian-success/30 text-wardian-success font-bold py-2 rounded text-[10px] uppercase tracking-wider transition-colors"
          >
            Execute Broadcast
          </button>
        </form>
      </div>
    </div>
  );
};
