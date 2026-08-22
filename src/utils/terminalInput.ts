import { invoke } from "@tauri-apps/api/core";
import { Image } from "@tauri-apps/api/image";
import { writeImage } from "@tauri-apps/plugin-clipboard-manager";
import type { PromptDeliveryDetail } from "../types";

export type ChatAttachment = {
  name: string;
  path: string;
  image?: Image;
};

const IMAGE_ATTACHMENT_EXTENSION = /\.(bmp|gif|jpe?g|png|webp)$/i;

export function isImageChatAttachment(attachment: ChatAttachment): boolean {
  return Boolean(attachment.image) || IMAGE_ATTACHMENT_EXTENSION.test(attachment.name);
}

export function providerImagePasteKey(provider?: string, platform?: string): string {
  const resolvedPlatform = platform ?? (typeof navigator === "undefined" ? "" : navigator.platform);
  // Claude Code reserves Alt+V for clipboard images on native Windows and WSL.
  // The other supported provider TUIs handle the standard Ctrl+V control character.
  return provider?.trim().toLowerCase() === "claude" && /win/i.test(resolvedPlatform) ? "\u001bv" : "\u0016";
}

export function promptWithChatAttachments(prompt: string, attachments: readonly ChatAttachment[]): string {
  const body = prompt.trim() || "Please inspect the attached files.";
  if (attachments.length === 0) return body;

  return `${body}\n\nAttached files:\n${attachments.map((attachment) => `- ${attachment.path || attachment.name}`).join("\n")}`;
}

export async function stageChatImageAttachments(
  sessionId: string,
  provider: string | undefined,
  attachments: readonly ChatAttachment[],
  platform?: string,
): Promise<void> {
  for (const attachment of attachments) {
    if (!isImageChatAttachment(attachment)) continue;

    const image = attachment.image ?? (await Image.fromPath(attachment.path));
    await writeImage(image);
    await invoke("inject_session_input", {
      sessionId,
      text: providerImagePasteKey(provider, platform),
    });
  }
}

export function flattenPromptForInjection(content: string): string {
  return content.replace(/\r?\n/g, " ").trim();
}

export async function submitInputToAgent(
  sessionId: string,
  input: string,
  inputMode: "message" | "command" = isSlashCommand(input) ? "command" : "message",
): Promise<PromptDeliveryDetail | undefined> {
  if (!sessionId || !input.trim()) {
    return undefined;
  }

  return invoke<PromptDeliveryDetail>("submit_prompt_to_agent", {
    sessionId,
    prompt: input,
    ...(inputMode === "command" ? { inputMode: "command" as const } : {}),
  });
}

/**
 * Provider CLIs treat a leading "/" as an interactive command rather than
 * prompt text. Routing these through the command channel keeps them off the
 * headless path when an agent is off or errored, matching the remote PWA.
 */
export function isSlashCommand(input: string): boolean {
  return input.trimStart().startsWith("/");
}

export async function submitInboxProviderChoice(
  sessionId: string,
  input: string,
  inboxItemId: string,
): Promise<void> {
  if (!sessionId || !input.trim() || !inboxItemId) return;

  await invoke("submit_inbox_provider_choice", {
    sessionId,
    prompt: input,
    inboxItemId,
  });
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
