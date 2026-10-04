import { useCallback } from "react";
import type { QueueItem } from "../../types";
import type { AgentTurnCompletion } from "../agents/useAgentResourceController";

type Options = {
  onCompletion: () => void;
  applyPersisted: (item: QueueItem) => void;
};

/**
 * Handles the two forms of `agent-turn-completed`.
 *
 * Without `inbox_item` the event marks a provider turn boundary. With one, it
 * projects a completion card the backend has already persisted. The frontend
 * never reads a transcript or invents completion identity.
 */
export function useAgentTurnCompletionHandler({ onCompletion, applyPersisted }: Options) {
  return useCallback((completion: AgentTurnCompletion) => {
    if (completion.inbox_item) {
      applyPersisted(completion.inbox_item);
      return;
    }
    onCompletion();
  }, [applyPersisted, onCompletion]);
}
