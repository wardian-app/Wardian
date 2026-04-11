import { invoke } from "@tauri-apps/api/core";

export function flattenPromptForInjection(content: string): string {
  return content.replace(/\r?\n/g, " ").trim();
}

export async function submitInputToAgent(sessionId: string, input: string): Promise<void> {
  if (!sessionId || !input.trim()) {
    return;
  }

  await invoke("submit_prompt_to_agent", { sessionId, prompt: input });
}

export async function submitInputToAgents(
  sessionIds: Iterable<string>,
  input: string,
): Promise<void> {
  for (const sessionId of sessionIds) {
    await submitInputToAgent(sessionId, input);
  }
}
