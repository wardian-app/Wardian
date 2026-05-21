import React, { useState } from "react";
import { Send } from "lucide-react";
import { useRemoteStore } from "./useRemoteStore";

const REMOTE_PROMPT_MAX_LENGTH = 16_000;

const errorMessage = (error: unknown) => (error instanceof Error ? error.message : String(error));

export const RemoteCommandBar: React.FC = () => {
  const [prompt, setPrompt] = useState("");
  const [error, setError] = useState("");
  const sendPrompt = useRemoteStore((state) => state.sendPrompt);
  const broadcastPrompt = useRemoteStore((state) => state.broadcastPrompt);
  const sending = useRemoteStore((state) => state.sending);
  const selectedCount = useRemoteStore((state) => state.selectedAgentIds.size);
  const agentCount = useRemoteStore((state) => state.agents.length);

  const submitSelected = async (event: React.FormEvent) => {
    event.preventDefault();
    const trimmed = prompt.trim();
    if (!trimmed || sending || selectedCount === 0) return;
    setError("");
    try {
      await sendPrompt(trimmed);
      setPrompt("");
    } catch (err) {
      setError(`Send failed: ${errorMessage(err)}`);
    }
  };

  const submitBroadcast = async () => {
    const trimmed = prompt.trim();
    if (!trimmed || sending || agentCount === 0) return;
    setError("");
    try {
      await broadcastPrompt(trimmed);
      setPrompt("");
    } catch (err) {
      setError(`Broadcast failed: ${errorMessage(err)}`);
    }
  };

  return (
    <form onSubmit={submitSelected} className="sticky bottom-0 border-t border-wardian-border bg-wardian-bg p-3">
      <label className="sr-only" htmlFor="remote-prompt">
        Prompt
      </label>
      <textarea
        id="remote-prompt"
        aria-label="Prompt"
        value={prompt}
        onChange={(event) => setPrompt(event.target.value)}
        maxLength={REMOTE_PROMPT_MAX_LENGTH}
        className="h-24 w-full resize-none rounded-md border border-wardian-border bg-wardian-input-bg p-3 text-sm text-primary outline-none focus:border-[var(--color-wardian-accent)]"
      />
      {error && <div className="mt-2 text-xs text-wardian-error">{error}</div>}
      <div className="mt-2 grid grid-cols-2 gap-2">
        <button
          type="submit"
          disabled={sending || !prompt.trim() || selectedCount === 0}
          className="inline-flex min-h-11 items-center justify-center gap-2 rounded-md bg-[var(--color-wardian-accent)] px-3 py-2 text-sm font-semibold text-[var(--color-wardian-bg)] transition-opacity hover:opacity-90 disabled:cursor-not-allowed disabled:opacity-50"
        >
          <Send className="h-4 w-4" aria-hidden="true" />
          {sending ? "Sending..." : "Send"}
        </button>
        <button
          type="button"
          onClick={() => void submitBroadcast()}
          disabled={sending || !prompt.trim() || agentCount === 0}
          className="inline-flex min-h-11 items-center justify-center gap-2 rounded-md border border-wardian-border px-3 py-2 text-sm font-semibold text-primary transition-colors hover:border-[var(--color-wardian-accent)] disabled:cursor-not-allowed disabled:opacity-50"
        >
          <Send className="h-4 w-4" aria-hidden="true" />
          Broadcast
        </button>
      </div>
    </form>
  );
};
