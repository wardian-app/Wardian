import { invoke } from "@tauri-apps/api/core";
import type { PromptDeliveryDetail } from "../types";

export function flattenPromptForInjection(content: string): string {
  return content.replace(/\r?\n/g, " ").trim();
}

export async function submitInputToAgent(
  sessionId: string,
  input: string,
): Promise<PromptDeliveryDetail | undefined> {
  if (!sessionId || !input.trim()) {
    return undefined;
  }

  return invoke<PromptDeliveryDetail>("submit_prompt_to_agent", { sessionId, prompt: input });
}

export async function submitInputToAgents(
  sessionIds: Iterable<string>,
  input: string,
): Promise<PromptDeliveryDetail[]> {
  const results: PromptDeliveryDetail[] = [];
  for (const sessionId of sessionIds) {
    const result = await submitInputToAgent(sessionId, input);
    if (result) {
      results.push(result);
    }
  }
  return results;
}
