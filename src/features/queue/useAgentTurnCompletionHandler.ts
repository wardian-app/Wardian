import { useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { AgentChatEvent } from "../../types";
import type { QueueItem } from "../../types";
import type { AgentTurnCompletion } from "../agents/useAgentResourceController";
import {
  completionPreviewFromTranscript,
  resolveAgentCompletionProjection,
} from "./completionPreview";

type Options = {
  pending: { current: Set<string> };
  onCompletion: () => void;
  applyPersisted: (item: QueueItem) => void;
  flush: (sessionId: string, agentName: string, summary: string, evidenceId: string) => void;
};

/** Projects explicit provider completion events into the Inbox queue. */
export function useAgentTurnCompletionHandler({
  pending,
  onCompletion,
  applyPersisted,
  flush,
}: Options) {
  return useCallback((completion: AgentTurnCompletion) => {
    onCompletion();
    if (completion.inbox_item) {
      applyPersisted(completion.inbox_item);
      return;
    }
    const projection = resolveAgentCompletionProjection(completion);
    if (projection.kind === "ignore" || pending.current.has(completion.session_id)) return;

    pending.current.add(completion.session_id);
    if (projection.kind === "persisted") {
      applyPersisted({
          id: `agent-completed:${projection.session_id}:${projection.evidence_id}`,
          type: "agent_completed",
          timestamp: Date.now(),
          read: false,
          agent_session_id: projection.session_id,
          agent_name: projection.agent_name,
          summary: projection.summary,
          evidence_id: projection.evidence_id,
          evidence_source: "provider_runtime",
      });
      pending.current.delete(projection.session_id);
      return;
    }
    if (projection.kind === "flush") {
      flush(
        projection.session_id,
        projection.agent_name,
        projection.summary,
        projection.evidence_id,
      );
      pending.current.delete(projection.session_id);
      return;
    }

    invoke<AgentChatEvent[]>("load_agent_chat_transcript", { sessionId: projection.session_id })
      .then(completionPreviewFromTranscript)
      .then((preview) => {
        if (preview) {
          flush(
            projection.session_id,
            projection.agent_name,
            preview.summary,
            preview.evidence_id,
          );
        }
      })
      .catch(() => undefined)
      .finally(() => pending.current.delete(projection.session_id));
  }, [applyPersisted, flush, onCompletion, pending]);
}
