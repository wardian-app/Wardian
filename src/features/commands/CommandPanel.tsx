import React, { useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { Copy, Check } from "lucide-react";
import { useLibraryStore } from "../../store/useLibraryStore";
import { AgentConfig, LibraryFolder, LibraryPrompt } from "../../types";
import { useConfirm } from "../../components/ConfirmDialog";
import { DocsLink } from "../../components/DocsLink";
import { flattenPromptForInjection, submitInputToAgents } from "../../utils/terminalInput";

interface CommandPanelProps {
  selectedAgentIds: Set<string>;
  broadcastMessage: string;
  setBroadcastMessage: (msg: string) => void;
  onBroadcast: (e: React.FormEvent) => void;
}

export const CommandPanel: React.FC<CommandPanelProps> = ({
  selectedAgentIds,
  broadcastMessage,
  setBroadcastMessage,
  onBroadcast,
}) => {
  const confirm = useConfirm();
  const { promptTree, fetchLibraryTree } = useLibraryStore();
  const [copiedPath, setCopiedPath] = useState<string | null>(null);

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
      const flattenedPrompt = flattenPromptForInjection(promptContent);
      if (selectedAgentIds.size > 0) {
        await submitInputToAgents(selectedAgentIds, flattenedPrompt);
      } else {
        if (await confirm("No agents selected. This will broadcast the prompt to all agents. Are you sure?")) {
          const agents = await invoke<AgentConfig[]>("list_agents");
          await submitInputToAgents(
            agents.map((agent) => agent.session_id),
            flattenedPrompt,
          );
        }
      }
    } catch (e) {
      console.error("Injection failed", e);
    }
  };

  const handleCopy = async (e: React.MouseEvent, content: string, path: string) => {
    e.stopPropagation();
    try {
      await writeText(content);
      setCopiedPath(path);
      setTimeout(() => setCopiedPath(null), 2000);
    } catch (e) {
      console.error("Copy failed", e);
    }
  };

  const handleBroadcastSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (selectedAgentIds.size === 0) {
      if (!await confirm("No agents selected. This will broadcast to ALL agents. Are you sure?")) {
        return;
      }
    }
    onBroadcast(e);
  };

  return (
    <div data-testid="command-panel" className="flex flex-col h-full">
      <div className="flex justify-between items-center mb-6">
        <h2 className="text-xl font-bold text-primary tracking-tight">Command</h2>
        <DocsLink path="/guide/command-panel">Command guide</DocsLink>
      </div>

      <div className="mb-8 flex-1 overflow-y-auto pr-2 no-scrollbar">
        <h3 className="text-xs font-bold text-muted tracking-wide mb-4">Quick Prompts</h3>
        <div className="flex flex-col gap-2">
          {quickPrompts.length === 0 ? (
            <div className="text-xs text-muted-neutral italic">No quick prompts in Library.</div>
          ) : (
            quickPrompts.map((prompt, idx) => (
              <div 
                data-testid={`quick-prompt-${idx}`}
                key={`starred-${prompt.path}-${idx}`}
                className="relative group/card"
              >
                <button 
                  onClick={() => handleInject(prompt.content)}
                  className="w-full flex flex-col items-start p-3 bg-wardian-card-bg-muted border border-wardian-light/50 rounded-lg text-primary hover:text-[var(--color-wardian-accent)] hover:border-[var(--color-wardian-accent)]/30 transition-all text-left group"
                >
                  <span className="text-xs font-bold truncate w-9/12">{prompt.name}</span>
                  <span className="text-[10px] text-muted-neutral mt-1 w-full line-clamp-1 whitespace-pre-wrap leading-relaxed group-hover:text-primary/70 transition-colors">
                    {prompt.content}
                  </span>
                </button>
                <button
                  onClick={(e) => handleCopy(e, prompt.content, prompt.path)}
                  title="Copy to clipboard"
                  className={`absolute top-2 right-2 p-1.5 rounded-md border transition-all active:scale-95 ${
                    copiedPath === prompt.path 
                      ? "bg-wardian-success/10 border-wardian-success/30 text-wardian-success" 
                      : "bg-wardian-card-bg border-transparent text-muted-neutral hover:text-primary hover:border-wardian-light shadow-sm opacity-0 group-hover/card:opacity-100"
                  }`}
                >
                  {copiedPath === prompt.path ? (
                    <Check className="w-3 h-3" />
                  ) : (
                    <Copy className="w-3 h-3" />
                  )}
                </button>
              </div>
            ))
          )}
        </div>
      </div>

      <div className="mt-auto pt-6 border-t border-wardian-border flex-shrink-0">
        <h3 className="text-xs font-bold text-muted tracking-wide mb-4">Broadcast</h3>
        <form onSubmit={handleBroadcastSubmit} className="flex flex-col gap-2">
          <textarea
            data-testid="broadcast-textarea"
            className="w-full bg-[var(--color-wardian-input-bg)] border border-wardian-light rounded px-3 py-2 text-primary text-xs focus:outline-none focus:border-[var(--color-wardian-accent)] h-32 resize-none"
            placeholder={selectedAgentIds.size > 0 ? `Message ${selectedAgentIds.size} selected...` : "Broadcast to all agents..."}
            value={broadcastMessage}
            onChange={(e) => setBroadcastMessage(e.currentTarget.value)}
          />
          <button
            data-testid="broadcast-submit"
            type="submit"
            className="bg-wardian-success/20 hover:bg-wardian-success/40 border border-wardian-success/30 text-wardian-success font-bold py-2 rounded text-[10px] tracking-wide transition-colors"
          >
            Execute Broadcast
          </button>
        </form>
      </div>
    </div>
  );
};
