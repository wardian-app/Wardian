import React, { useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { Check, Copy, Sparkles } from "lucide-react";
import { useLibraryStore } from "../../store/useLibraryStore";
import { flattenAllEntries } from "../library/libraryListUtils";
import { LibraryEntry } from "../../types";
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
  const index = useLibraryStore((s) => s.index);
  const fetchIndex = useLibraryStore((s) => s.fetchIndex);
  const [copiedPath, setCopiedPath] = useState<string | null>(null);
  const hasSelectedAgents = selectedAgentIds.size > 0;
  const selectionRequiredLabel = "Select at least one agent to send a command";

  useEffect(() => {
    if (!index) {
      void fetchIndex();
    }
  }, [index, fetchIndex]);

  // Quick prompts show only description/name from the index — the index is
  // metadata-only by design (see wardian-core::library::index), so the full
  // body is fetched on demand (inject/copy) via `read_library_item`.
  const quickPrompts = useMemo<LibraryEntry[]>(() => {
    const tree = index?.sections.prompts.tree;
    if (!tree) return [];
    return flattenAllEntries(tree)
      .map((row) => row.entry)
      .filter((entry): entry is LibraryEntry => entry != null && entry.is_starred);
  }, [index]);

  const readPromptContent = (path: string) => invoke<string>("read_library_item", { section: "prompts", path });

  const handleInject = async (path: string) => {
    if (!hasSelectedAgents) return;

    try {
      const content = await readPromptContent(path);
      const flattenedPrompt = flattenPromptForInjection(content);
      await submitInputToAgents(selectedAgentIds, flattenedPrompt);
    } catch (e) {
      console.error("Injection failed", e);
    }
  };

  const handleCopy = async (e: React.MouseEvent, path: string) => {
    e.stopPropagation();
    try {
      const content = await readPromptContent(path);
      await writeText(content);
      setCopiedPath(path);
      setTimeout(() => setCopiedPath(null), 2000);
    } catch (e) {
      console.error("Copy failed", e);
    }
  };

  const handleBroadcastSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!hasSelectedAgents) return;

    onBroadcast(e);
  };

  return (
    <div data-testid="command-panel" className="flex h-full min-h-0 flex-col">
      <div className="mb-4 flex min-h-7 items-center gap-1">
        <h2 className="min-w-0 flex-1 truncate text-sm font-bold tracking-tight text-primary">Command</h2>
      </div>

      <section className="shrink-0" aria-labelledby="broadcast-heading">
        <h3 id="broadcast-heading" className="mb-3 text-xs font-bold tracking-wide text-muted">Broadcast</h3>
        <form onSubmit={handleBroadcastSubmit} className="flex flex-col gap-2">
          <textarea
            data-testid="broadcast-textarea"
            disabled={!hasSelectedAgents}
            title={hasSelectedAgents ? undefined : selectionRequiredLabel}
            className="h-28 w-full resize-none rounded-lg border border-wardian-light bg-[var(--color-wardian-input-bg)] px-3 py-2.5 text-xs text-primary transition-colors placeholder:text-muted-neutral focus:border-[var(--color-wardian-accent)] focus:outline-none disabled:cursor-not-allowed disabled:opacity-50"
            placeholder={hasSelectedAgents ? `Message ${selectedAgentIds.size} selected...` : "Select an agent to send a command"}
            value={broadcastMessage}
            onChange={(e) => setBroadcastMessage(e.currentTarget.value)}
          />
          <button
            data-testid="broadcast-submit"
            type="submit"
            disabled={!hasSelectedAgents}
            title={hasSelectedAgents ? undefined : selectionRequiredLabel}
            className="inline-flex h-9 items-center justify-center rounded-lg border border-[var(--color-wardian-accent)]/40 bg-[var(--color-wardian-accent)] px-3 text-[11px] font-bold tracking-wide text-[var(--color-wardian-accent-contrast)] transition-colors hover:bg-[color-mix(in_srgb,var(--color-wardian-accent),white_12%)] disabled:cursor-not-allowed disabled:opacity-50 disabled:hover:bg-[var(--color-wardian-accent)]"
          >
            Execute Broadcast
          </button>
        </form>
      </section>

      <section className="mt-5 flex min-h-0 flex-1 flex-col border-t border-wardian-border pt-4" aria-labelledby="quick-prompts-heading">
        <h3 id="quick-prompts-heading" className="mb-3 shrink-0 text-xs font-bold tracking-wide text-muted">Quick Prompts</h3>
        <div className="min-h-0 flex-1 overflow-y-auto pr-1 no-scrollbar">
          <div className="flex flex-col gap-2">
            {quickPrompts.length === 0 ? (
              <div className="flex items-center gap-2 py-2 text-xs text-muted-neutral italic">
                <Sparkles className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
                No quick prompts in Library.
              </div>
            ) : (
              quickPrompts.map((prompt, idx) => (
                <div
                  data-testid={`quick-prompt-${idx}`}
                  key={`starred-${prompt.entry_ref}`}
                  className="group/card relative"
                >
                  <button
                    type="button"
                    onClick={() => void handleInject(prompt.path)}
                    disabled={!hasSelectedAgents}
                    title={hasSelectedAgents ? undefined : selectionRequiredLabel}
                    className="group flex w-full flex-col items-start rounded-lg border border-wardian-border bg-wardian-card-bg-muted px-3 py-2.5 pr-10 text-left text-primary transition-colors hover:border-[var(--color-wardian-accent)]/40 hover:bg-[var(--color-wardian-accent)]/5 disabled:cursor-not-allowed disabled:opacity-50 disabled:hover:border-wardian-border disabled:hover:bg-wardian-card-bg-muted"
                  >
                    <span className="w-full truncate text-xs font-bold group-hover:text-[var(--color-wardian-accent)]">{prompt.name}</span>
                    <span className="mt-1 w-full line-clamp-1 whitespace-pre-wrap text-[10px] leading-relaxed text-muted-neutral transition-colors group-hover:text-primary/70">
                      {prompt.description}
                    </span>
                  </button>
                  <button
                    onClick={(e) => void handleCopy(e, prompt.path)}
                    aria-label="Copy quick prompt to clipboard"
                    title="Copy to clipboard"
                    className={`absolute right-2 top-1/2 -translate-y-1/2 rounded-md border p-1.5 transition-all active:scale-95 ${
                      copiedPath === prompt.path
                        ? "bg-wardian-success/10 border-wardian-success/30 text-wardian-success"
                        : "border-transparent bg-wardian-card-bg text-muted-neutral hover:border-wardian-light hover:text-primary"
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
      </section>
    </div>
  );
};
