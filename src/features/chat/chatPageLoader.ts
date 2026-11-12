import { invoke } from "@tauri-apps/api/core";
import type { AgentChatPage } from "../../types";
import type { ChatPageLoader } from "./useChatPages";
import { chatPageObservation } from "./chatPageObservation";

/** Keep one loader ownership domain and return the SDK Promise without wrapping it. */
export const loadChatPage: ChatPageLoader = ({ sessionId, cursor, revision, detailRef, revalidateIds, recentPreview }) => {
  const args = {
    sessionId, cursor, revision, detailRef, ...(revalidateIds ? { revalidateIds } : {}),
    recentPreview: recentPreview !== false && !cursor && !detailRef && !revalidateIds,
  };
  const result = invoke<AgentChatPage>("load_agent_chat_page", args);
  chatPageObservation.notifyRequest(args, result);
  return result;
};
