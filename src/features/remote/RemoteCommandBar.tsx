import React, { useState } from "react";
import { Send } from "lucide-react";
import { useRemoteStore } from "./useRemoteStore";

const REMOTE_PROMPT_MAX_LENGTH = 16_000;

const errorMessage = (error: unknown) => (error instanceof Error ? error.message : String(error));

export const RemoteCommandBar: React.FC = () => {
  const [prompt, setPrompt] = useState("");
  const [error, setError] = useState("");
  const broadcastPrompt = useRemoteStore((state) => state.broadcastPrompt);
  const sending = useRemoteStore((state) => state.sending);
  const agentCount = useRemoteStore((state) => state.agents.length);

  const submitBroadcast = async (event?: React.FormEvent) => {
    event?.preventDefault();
    const trimmed = prompt.trim();
    if (!trimmed || sending || agentCount === 0) return;
    setError("");
    const agentLabel = agentCount === 1 ? "agent" : "agents";
    if (!window.confirm(`Broadcast to ${agentCount} ${agentLabel}?`)) return;
    try {
      await broadcastPrompt(trimmed);
      setPrompt("");
    } catch (err) {
      setError(`Broadcast failed: ${errorMessage(err)}`);
    }
  };

  return (
    <form onSubmit={submitBroadcast} className="rounded-md border border-wardian-border bg-wardian-card p-3">
      <label className="sr-only" htmlFor="remote-prompt">
        Broadcast prompt
      </label>
      <textarea
        id="remote-prompt"
        aria-label="Broadcast prompt"
        value={prompt}
        onChange={(event) => setPrompt(event.target.value)}
        maxLength={REMOTE_PROMPT_MAX_LENGTH}
        className="h-24 w-full resize-none rounded-md border border-wardian-border bg-wardian-input-bg p-3 text-sm text-primary outline-none focus:border-[var(--color-wardian-accent)]"
      />
      {error && <div className="mt-2 text-xs text-wardian-error">{error}</div>}
      <div className="mt-2">
        <button
          type="submit"
          disabled={sending || !prompt.trim() || agentCount === 0}
          className="inline-flex min-h-11 w-full items-center justify-center gap-2 rounded-md bg-[var(--color-wardian-accent)] px-3 py-2 text-sm font-semibold text-[var(--color-wardian-bg)] transition-opacity hover:opacity-90 disabled:cursor-not-allowed disabled:opacity-50"
        >
          <Send className="h-4 w-4" aria-hidden="true" />
          {sending ? "Broadcasting..." : "Broadcast"}
        </button>
      </div>
    </form>
  );
};
