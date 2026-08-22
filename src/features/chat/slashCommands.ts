/**
 * Curated provider slash-command catalog for composer completion.
 *
 * These are the provider CLIs' own interactive commands, typed straight into
 * the PTY. The lists are deliberately conservative: only stable, widely
 * documented commands belong here, because a wrong entry completes into text
 * the provider rejects. Wardian does not interpret them — delivery just routes
 * "/"-prefixed input through the command channel so providers receive it on
 * the interactive surface (see `submitInputToAgent`).
 */

export interface SlashCommand {
  command: string;
  description: string;
}

const CATALOGS: Record<string, readonly SlashCommand[]> = {
  codex: [
    { command: "/init", description: "Scan the workspace and write AGENTS.md" },
    { command: "/compact", description: "Summarize the conversation to free context" },
    { command: "/model", description: "Choose the model for this session" },
    { command: "/approvals", description: "Change the approval level" },
    { command: "/review", description: "Review a change or commit" },
    { command: "/status", description: "Show session and configuration status" },
  ],
  claude: [
    { command: "/clear", description: "Clear the conversation" },
    { command: "/compact", description: "Summarize the conversation to free context" },
    { command: "/model", description: "Choose the model for this session" },
    { command: "/cost", description: "Show token usage for this session" },
    { command: "/init", description: "Seed CLAUDE.md with project guidance" },
    { command: "/review", description: "Request a code review" },
    { command: "/help", description: "List available commands" },
  ],
  opencode: [
    { command: "/init", description: "Write an AGENTS.md guide for the project" },
    { command: "/compact", description: "Summarize the conversation to free context" },
    { command: "/models", description: "List available models" },
    { command: "/sessions", description: "List sessions" },
    { command: "/undo", description: "Undo the last file change" },
    { command: "/redo", description: "Redo the last undone change" },
    { command: "/help", description: "List available commands" },
  ],
  gemini: [
    { command: "/clear", description: "Clear the conversation" },
    { command: "/compress", description: "Summarize the conversation to free context" },
    { command: "/stats", description: "Show session statistics" },
    { command: "/tools", description: "List available tools" },
    { command: "/theme", description: "Change the theme" },
    { command: "/help", description: "List available commands" },
  ],
};

const FALLBACK_CATALOG: readonly SlashCommand[] = [];

/** The provider's curated interactive commands; empty when unknown. */
export function slashCommandsForProvider(provider?: string): readonly SlashCommand[] {
  const key = provider?.trim().toLowerCase();
  if (!key) return FALLBACK_CATALOG;
  return CATALOGS[key] ?? FALLBACK_CATALOG;
}

/**
 * Completions for the current draft: only while the first token is still
 * being typed ("/", "/mo"), never after a space commits the command word.
 */
export function matchingSlashCommands(
  draft: string,
  provider?: string,
): readonly SlashCommand[] {
  const token = draft.trimStart();
  if (!token.startsWith("/") || /\s/.test(token)) return FALLBACK_CATALOG;
  const prefix = token.toLowerCase();
  return slashCommandsForProvider(provider).filter((entry) =>
    entry.command.toLowerCase().startsWith(prefix),
  );
}
