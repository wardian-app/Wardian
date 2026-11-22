import { invoke } from "@tauri-apps/api/core";
import type { AgentChatPage } from "../../types";
import type { ChatPageLoader } from "./useChatPages";
import { chatPageObservation } from "./chatPageObservation";
import { boundedPreviewBackingIds } from "./chatPreviewBacking";

/** Keep one loader ownership domain and return the SDK Promise without wrapping it. */
export const loadChatPage: ChatPageLoader = ({ sessionId, cursor, revision, detailRef, revalidateIds, recentPreview, previewBackingIds }) => {
  const recent = recentPreview !== false && !cursor && !detailRef && revalidateIds === undefined;
  const backing = recent ? boundedPreviewBackingIds(previewBackingIds) : undefined;
  const args = {
    sessionId, cursor, revision, detailRef, ...(revalidateIds ? { revalidateIds } : {}),
    recentPreview: recent,
    ...(backing === undefined ? {} : { previewBackingIds: backing }),
  };
  const result = invoke<AgentChatPage>("load_agent_chat_page", args);
  chatPageObservation.notifyRequest(args, result);
  return result;
};
