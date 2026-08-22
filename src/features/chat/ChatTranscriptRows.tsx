import { Check, ChevronRight } from "lucide-react";
import { useState } from "react";

import type { AgentChatEvent, AgentChatRole } from "../../types";
import { toActivityBlock, type ActivityBlockModel } from "../grid/activityBlocks";
import { parseApprovalChoices } from "../grid/approvalChoices";
import { CodePanel, renderHighlightedCode } from "../grid/chatCode";
import { ChatRowActions } from "../grid/chatCopy";
import { ChatMarkdown, type ChatMarkdownLinkHandling } from "../grid/markdown/ChatMarkdown";
import {
  changedPathsFromEvents,
  formatPresentedEntryForCopy,
  formatPresentedWorkGroupForCopy,
  type PresentedChatRow,
  type PresentedWorkEntry,
} from "../grid/workLogPresentation";
import {
  compactPath,
  compactTerminalPreview,
  diffStats,
  formatStatus,
  formatTimestamp,
  isThinkingIndicator,
  outputWithoutCommandPrefix,
  parseTodoItems,
  previewContent,
  resolvedActivityTone,
  toneDotClass,
  toolIconClass,
  toolPresentation,
  withPatchContent,
  workGroupDurationLabel,
  workGroupTitleFromEntries,
  TONE_CLASSES,
  type ToolPresentation,
} from "./chatPresentation";
import { structuredEditDiffText, structuredEditFromEvent, type StructuredEdit } from "./structuredEdit";
import type { ChatTranscriptRowModel } from "./chatTurns";
import { TurnChangeCard } from "./TurnChangeCard";

const ROLE_CLASSES: Record<AgentChatRole, string> = {
  assistant: "chat-message-assistant text-primary",
  system: "border-[var(--color-wardian-warning)] bg-[color-mix(in_srgb,var(--color-wardian-warning),transparent_92%)]",
  tool: "border-wardian-light bg-[var(--color-wardian-card-bg-muted)]",
  user: "chat-message-user text-primary",
};

/**
 * Historical surface selector kept for remote callers; message geometry is now
 * role-driven (user messages are right-aligned bubbles on every surface), so
 * both values render identically.
 */
export type ChatMessageLayout = "bubble" | "full_width";

export interface ChatTranscriptRowProps {
  row: ChatTranscriptRowModel;
  /** True while the agent is producing output; gates the in-flight tone. */
  agentIsWorking: boolean;
  isSubmitting: boolean;
  layout?: ChatMessageLayout;
  linkHandling?: ChatMarkdownLinkHandling;
  onApprovalSubmit: (response: string) => void;
  /** Omitted on surfaces with no file viewer, which renders paths inert. */
  onOpenFile?: (path: string) => void;
  /** The one approval a response can still reach; null when none is pending. */
  liveApprovalId?: string | null;
}

/** Dispatches a derived transcript row to its renderer. */
export function ChatTranscriptRow({
  row,
  agentIsWorking,
  isSubmitting,
  linkHandling,
  onApprovalSubmit,
  onOpenFile,
  liveApprovalId = null,
}: ChatTranscriptRowProps) {
  if (row.kind === "turn_change_summary") return <TurnChangeCard onOpenFile={onOpenFile} row={row} />;
  if (row.kind === "work_group") return <WorkGroupRow agentIsWorking={agentIsWorking} row={row} />;
  return row.event.kind === "message" ? (
    <MessageRow event={row.event} linkHandling={linkHandling} />
  ) : (
    <ActivityEvent
      agentIsWorking={agentIsWorking}
      approvalIsLive={row.event.id === liveApprovalId}
      entry={row.entry}
      event={row.event}
      isSubmitting={isSubmitting}
      onApprovalSubmit={onApprovalSubmit}
    />
  );
}

export function MessageRow({
  event,
  linkHandling,
}: {
  event: AgentChatEvent;
  linkHandling?: ChatMarkdownLinkHandling;
}) {
  const role = event.role ?? "assistant";
  const text = event.text?.trimEnd() || event.title || "";
  const isAssistant = role === "assistant";
  const isCardRole = role === "system" || role === "tool";
  const content = text ? (
    <ChatMarkdown linkHandling={linkHandling} source={text} />
  ) : (
    <div className="text-[13px] leading-5 text-muted-neutral">No message content</div>
  );

  if (isCardRole) {
    return (
      <article aria-label={`${role} message`} className="chat-row relative flex justify-start">
        <div
          className={`chat-message-content group/message relative max-w-[92%] rounded-[var(--density-card-radius)] border px-3 py-2.5 ${ROLE_CLASSES[role]}`}
        >
          {content}
        </div>
      </article>
    );
  }

  return (
    <article
      aria-label={`${role} message`}
      className={`chat-row group/chat-message flex w-full flex-col ${
        isAssistant ? "max-w-[76ch] items-start px-1 py-0.5" : "items-end"
      }`}
    >
      <div
        className={`chat-message-content min-w-0 max-w-full break-words ${isAssistant ? "" : "chat-message-user max-w-[92%] px-3 py-2"}`}
        data-testid={isAssistant ? undefined : "chat-user-bubble"}
      >
        {content}
      </div>
      {text ? (
        <div
          className={`chat-row-actions--inline ${
            isAssistant ? "chat-row-actions--start" : "chat-row-actions--end"
          }`}
        >
          <ChatRowActions actions={[{ label: "Copy message", value: text }]} />
        </div>
      ) : null}
    </article>
  );
}

export function ActivityEvent({
  event,
  entry,
  agentIsWorking,
  approvalIsLive,
  isSubmitting,
  onApprovalSubmit,
}: {
  event: AgentChatEvent;
  entry?: PresentedWorkEntry;
  agentIsWorking: boolean;
  approvalIsLive: boolean;
  isSubmitting: boolean;
  onApprovalSubmit: (response: string) => void;
}) {
  const baseBlock = entry?.block ?? toActivityBlock(event);
  const block: ActivityBlockModel = {
    ...baseBlock,
    tone: resolvedActivityTone(baseBlock.tone, agentIsWorking),
  };
  if (isThinkingIndicator(event)) return <ThinkingRow />;
  if (event.kind === "status") return <StatusRow event={event} block={block} />;
  if (event.kind === "terminal_output") return <TerminalFallback block={block} />;
  return (
    <ActivityRow
      approvalIsLive={approvalIsLive}
      block={block}
      entry={entry}
      event={event}
      isSubmitting={isSubmitting}
      onApprovalSubmit={onApprovalSubmit}
    />
  );
}

export function ThinkingRow() {
  return (
    <article aria-label="agent working" className="flex justify-start">
      <div className="inline-flex items-center gap-1.5 px-1 py-0.5 text-[12px] leading-5 text-muted-neutral">
        <span className="h-1.5 w-1.5 rounded-full bg-[var(--color-wardian-processing)]" aria-hidden="true" />
        <span className="sr-only">Working...</span>
        <span aria-hidden="true">
          Working
          <span data-testid="thinking-dots" className="wardian-thinking-dots">
            <span className="wardian-thinking-dot wardian-thinking-dot-1">.</span>
            <span className="wardian-thinking-dot wardian-thinking-dot-2">.</span>
            <span className="wardian-thinking-dot wardian-thinking-dot-3">.</span>
          </span>
        </span>
      </div>
    </article>
  );
}

export function StatusRow({ event, block }: { event: AgentChatEvent; block: ActivityBlockModel }) {
  const statusText = event.text?.trim() || formatStatus(event.status) || block.title;
  return (
    <div className="flex items-center gap-2 py-0.5 text-[11px] leading-4 text-muted-neutral">
      <span className={`h-1.5 w-1.5 shrink-0 rounded-full ${toneDotClass(block.tone)}`} aria-hidden="true" />
      <span className="truncate">
        {block.title}: <span className="text-primary">{statusText}</span>
      </span>
      {event.created_at ? <span className="ml-auto shrink-0">{formatTimestamp(event.created_at)}</span> : null}
    </div>
  );
}

export function ActivityRow({
  event,
  entry,
  block: rawBlock,
  approvalIsLive,
  isSubmitting,
  onApprovalSubmit,
}: {
  event: AgentChatEvent;
  entry?: PresentedWorkEntry;
  block: ActivityBlockModel;
  approvalIsLive: boolean;
  isSubmitting: boolean;
  onApprovalSubmit: (response: string) => void;
}) {
  const block = withPatchContent(event, rawBlock);
  const isApproval = block.kind === "approval" || block.tone === "warning";
  const approvalChoices = isApproval ? parseApprovalChoices(event.text ?? block.content) : [];
  const presentation = toolPresentation(event, block);
  const details = entry?.details ?? presentation.details;
  const Icon = presentation.icon;
  const output = outputWithoutCommandPrefix(block.content, event.command);
  const structuredEdit = structuredEditFromEvent(event);
  // The panel names the file and counts the lines, so the path chips below it
  // would only repeat what the reader is already looking at.
  const changedPaths = structuredEdit ? [] : entry?.changed_paths ?? changedPathsFromEvents([event]);
  const collapsedByDefault = shouldCollapseRoutineActivity(event, block, isApproval, structuredEdit, changedPaths);
  const [expanded, setExpanded] = useState(!collapsedByDefault);
  const visibleContent = expanded ? block.content : previewContent(block.content);
  const copyValue = structuredEdit
    ? structuredEditDiffText(structuredEdit)
    : entry && entry.merged_result_events.length > 0
      ? formatPresentedEntryForCopy(entry)
      : block.content;
  const copyActions = [
    { label: "Copy activity output", value: copyValue },
    ...(changedPaths.length > 0 ? [{ label: "Copy changed file paths", value: changedPaths.join("\n") }] : []),
  ];

  return (
    <article
      className={`chat-row chat-activity-row relative border-l-2 bg-[var(--color-wardian-card-bg-muted)] px-2.5 py-1.5 ${
        expanded ? "chat-activity-row-expanded" : "chat-activity-row-collapsed"
      } ${TONE_CLASSES[block.tone]}`}
      data-testid={isApproval ? "chat-activity-row-approval" : "chat-activity-row"}
      data-collapsed={expanded ? "false" : "true"}
    >
      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0 flex-1">
          <div className="flex min-w-0 items-center gap-2">
            <span className={`inline-flex h-6 w-6 shrink-0 items-center justify-center rounded border ${toolIconClass(presentation.kind)}`}>
              <Icon className="h-3.5 w-3.5" aria-hidden="true" />
            </span>
            <div className="min-w-0">
              <div className="truncate text-[12px] font-semibold leading-5 text-primary">{presentation.title}</div>
              <div className="chat-activity-detail truncate text-[11px] leading-4 text-muted-neutral">{details.join(" - ")}</div>
            </div>
          </div>
          {event.command?.trim() ? (
            <div className="mt-2 flex min-w-0 items-center gap-1.5 rounded border border-wardian-light bg-[var(--color-wardian-sidebar-primary)] px-2 py-1 font-mono text-[11px] leading-4 text-primary">
              <span className="shrink-0 text-[var(--color-wardian-accent)]">$</span>
              <span className="min-w-0 truncate" title={event.command}>
                {event.command}
              </span>
            </div>
          ) : null}
        </div>
        <div className="flex shrink-0 items-center gap-1.5">
          {collapsedByDefault ? (
            <button
              type="button"
              className="rounded border border-wardian-light px-2 py-1 text-[11px] font-semibold leading-4 text-muted-neutral hover:text-primary"
              onClick={() => setExpanded((value) => !value)}
            >
              {expanded ? "Collapse" : "Show output"}
            </button>
          ) : null}
        </div>
      </div>
      <ChatRowActions actions={copyActions} className="chat-row-actions--overlay" label="Activity actions" />
      {isApproval ? (
        <div
          className={`mt-2 rounded border px-2 py-1 text-[11px] leading-4 text-muted-neutral ${
            approvalIsLive
              ? "border-[color-mix(in_srgb,var(--color-wardian-warning),transparent_45%)] bg-[color-mix(in_srgb,var(--color-wardian-warning),transparent_92%)]"
              : "border-wardian-light bg-[var(--color-wardian-card)]"
          }`}
          data-testid="chat-approval-notice"
        >
          {!approvalIsLive
            ? "This request is no longer awaiting a response."
            : approvalChoices.length > 0
              ? "Action required. Choose a response or type below."
              : "Action required. Respond below or switch to terminal mode."}
        </div>
      ) : null}
      {changedPaths.length > 0 ? <ChangedFiles paths={changedPaths} /> : null}
      {approvalChoices.length > 0 ? (
        <div className="mt-2 flex flex-wrap gap-1.5" aria-label="Approval choices">
          {approvalChoices.map((choice) => (
            <button
              type="button"
              key={`${choice.value}-${choice.label}`}
              aria-label={
                approvalIsLive
                  ? `Send approval response ${choice.value}: ${choice.label}`
                  : `Past approval choice ${choice.value}: ${choice.label}`
              }
              className={`inline-flex max-w-full items-center gap-1.5 rounded border px-2 py-1 text-left text-[11px] font-semibold leading-4 transition-colors disabled:cursor-not-allowed disabled:opacity-60 ${
                approvalIsLive
                  ? "border-[color-mix(in_srgb,var(--color-wardian-warning),transparent_35%)] bg-[color-mix(in_srgb,var(--color-wardian-warning),transparent_88%)] text-primary hover:bg-[color-mix(in_srgb,var(--color-wardian-warning),transparent_80%)]"
                  : "border-wardian-light bg-[var(--color-wardian-card)] text-muted-neutral"
              }`}
              disabled={isSubmitting || !approvalIsLive}
              onClick={() => onApprovalSubmit(choice.value)}
            >
              <span
                className={`shrink-0 font-mono ${approvalIsLive ? "text-[var(--color-wardian-warning)]" : "text-muted-neutral"}`}
              >
                {choice.value}
              </span>
              <span className="min-w-0 truncate">{choice.label}</span>
            </button>
          ))}
        </div>
      ) : null}
      <ToolBody
        block={block}
        content={event.command ? outputWithoutCommandPrefix(visibleContent, event.command) : visibleContent}
        output={output}
        presentation={presentation}
        structuredEdit={structuredEdit}
      />
    </article>
  );
}

function shouldCollapseRoutineActivity(
  event: AgentChatEvent,
  block: ActivityBlockModel,
  isApproval: boolean,
  structuredEdit: StructuredEdit | null,
  changedPaths: string[],
): boolean {
  if (block.defaultCollapsed) return true;
  if (isApproval || event.kind === "error" || structuredEdit || changedPaths.length > 0) return false;
  if (block.tone === "error" || block.tone === "warning") return false;
  if (event.status === "running" || event.status === "processing") return false;
  return event.kind === "tool_call" || event.kind === "tool_result";
}

export function ToolBody({
  block,
  content,
  output,
  presentation,
  structuredEdit,
}: {
  block: ActivityBlockModel;
  content: string;
  output: string;
  presentation: ToolPresentation;
  structuredEdit?: StructuredEdit | null;
}) {
  const safeContent = content.trimEnd() || "No activity content";

  // Structured input beats text sniffing: providers that emit no patch text at
  // all still describe the change here, and it is exact rather than inferred.
  if (structuredEdit) {
    return <StructuredEditPanel edit={structuredEdit} />;
  }

  if (presentation.kind === "todo") {
    const items = parseTodoItems(output || safeContent);
    if (items.length > 0) {
      return (
        <ul
          className="mt-2 space-y-1 rounded border border-wardian-light bg-[var(--color-wardian-sidebar-primary)] p-2"
          data-testid="tool-todo-list"
        >
          {items.map((item, index) => (
            <li className="flex items-start gap-2 text-[12px] leading-5 text-primary" key={`${index}-${item.label.slice(0, 24)}`}>
              <span
                aria-hidden="true"
                className={`mt-1 inline-flex h-3.5 w-3.5 shrink-0 items-center justify-center rounded border ${
                  item.done
                    ? "border-[var(--color-wardian-success)] bg-[color-mix(in_srgb,var(--color-wardian-success),transparent_82%)]"
                    : "border-wardian-light bg-[var(--color-wardian-card-bg-muted)]"
                }`}
              >
                {item.done ? <Check className="h-2.5 w-2.5 text-[var(--color-wardian-success)]" aria-hidden="true" /> : null}
              </span>
              <span className="break-words">{item.label}</span>
            </li>
          ))}
        </ul>
      );
    }
  }

  if (presentation.kind === "diff") {
    const stats = diffStats((output || safeContent).trimEnd());
    // An edit tool's *result* is usually a plain acknowledgement carrying no
    // patch at all. Framing that as a diff panel headed "Patch +0 -0" invents
    // a change summary for text that has none.
    if (stats.added === 0 && stats.removed === 0 && stats.files.length === 0) {
      return <CodePanel content={safeContent} language={block.language} />;
    }
    return (
      <div className="mt-2 rounded border border-wardian-light bg-[var(--color-wardian-sidebar-primary)]" data-testid="tool-diff-panel">
        <div className="flex flex-wrap items-center gap-2 border-b border-wardian-light px-2 py-1 text-[11px] leading-4 text-muted-neutral">
          <span>{stats.files.length > 0 ? `${stats.files.length} ${stats.files.length === 1 ? "file" : "files"}` : "Patch"}</span>
          {stats.counts_unknown && stats.added === 0 && stats.removed === 0 ? (
            // Every named file is header-only, so the patch supplies no counts
            // at all. "+0 -0" would be a claim about the file's size.
            <span title="This patch reports no line counts.">Line counts not reported</span>
          ) : (
            <>
              <span className="text-[var(--color-wardian-success)]">+{stats.added}</span>
              <span className="text-[var(--color-wardian-error)]">-{stats.removed}</span>
              {stats.counts_unknown ? (
                <span title="Some files in this patch report no line counts, so these totals cover only the rest.">
                  partial
                </span>
              ) : null}
            </>
          )}
          {stats.files.slice(0, 3).map((file) => (
            <span className="max-w-[180px] truncate font-mono text-primary" key={file} title={file}>
              {compactPath(file)}
            </span>
          ))}
        </div>
        <CodePanel content={safeContent} language="diff" />
      </div>
    );
  }

  return <CodePanel content={safeContent} language={block.language} />;
}

/**
 * Renders a change recovered from provider tool input. Deliberately labelled
 * "Before/after" rather than shown as a positioned diff: these tools carry no
 * line numbers, so the panel states exactly what the provider supplied.
 */
export function StructuredEditPanel({ edit }: { edit: StructuredEdit }) {
  return (
    <div className="mt-2 rounded border border-wardian-light bg-[var(--color-wardian-sidebar-primary)]" data-testid="tool-structured-edit">
      <div className="flex flex-wrap items-center gap-2 border-b border-wardian-light px-2 py-1 text-[11px] leading-4 text-muted-neutral">
        <span>{edit.kind === "write" ? "New contents" : "Before/after"}</span>
        <span className="text-[var(--color-wardian-success)]">+{edit.added}</span>
        {/* A whole-file write states what the file now holds, never what it
            replaced, so claiming "-0" would assert the file was empty. */}
        {edit.kind === "write" ? (
          <span className="text-muted-neutral">replaced contents</span>
        ) : (
          <span className="text-[var(--color-wardian-error)]">-{edit.removed}</span>
        )}
        {edit.file_path ? (
          <span className="max-w-[220px] truncate font-mono text-primary" title={edit.file_path}>
            {compactPath(edit.file_path)}
          </span>
        ) : null}
      </div>
      <CodePanel content={structuredEditDiffText(edit)} language="diff" />
    </div>
  );
}

export function TerminalFallback({ block }: { block: ActivityBlockModel }) {
  const [expanded, setExpanded] = useState(false);
  const lineLabel = `${block.lineCount} ${block.lineCount === 1 ? "line" : "lines"}`;
  const preview = compactTerminalPreview(block.content);

  return (
    <article
      className="chat-row relative border-l-2 border-wardian-light bg-[color-mix(in_srgb,var(--color-wardian-card-bg-muted),transparent_28%)] px-3 py-2"
      data-testid="terminal-fallback-row"
    >
      <div className="flex items-center gap-3">
        <div className="min-w-0 flex-1">
          <div className="truncate text-[12px] font-semibold leading-5 text-primary">Terminal fallback</div>
          <div className="truncate text-[11px] leading-4 text-muted-neutral">Raw watch output - {lineLabel}</div>
        </div>
        <div className="flex shrink-0 items-center gap-1.5">
          <button
            type="button"
            className="rounded border border-wardian-light px-2 py-1 text-[11px] font-semibold leading-4 text-muted-neutral hover:text-primary"
            onClick={() => setExpanded((value) => !value)}
          >
            {expanded ? "Hide details" : "Show terminal"}
          </button>
        </div>
      </div>
      <ChatRowActions
        actions={[{ label: "Copy terminal output", value: block.content }]}
        className="chat-row-actions--overlay"
        label="Terminal actions"
      />
      {preview && !expanded ? (
        <div className="mt-1 truncate font-mono text-[11px] leading-4 text-muted-neutral">{preview}</div>
      ) : null}
      {expanded ? (
        <pre className="mt-2 max-h-[260px] overflow-auto whitespace-pre-wrap break-words rounded border border-wardian-light bg-[var(--color-wardian-sidebar-primary)] p-2 text-[12px] leading-5 text-primary">
          <code data-language={block.language}>
            {renderHighlightedCode(block.content || "No terminal output", block.language)}
          </code>
        </pre>
      ) : null}
    </article>
  );
}

export function WorkGroupRow({
  row,
  agentIsWorking,
}: {
  row: Extract<PresentedChatRow, { kind: "work_group" }>;
  agentIsWorking: boolean;
}) {
  const [expanded, setExpanded] = useState(false);
  const visibleEntries = expanded ? row.entries : [];
  const title = workGroupTitleFromEntries(row.entries);
  const duration = workGroupDurationLabel(row.entries);
  const latestEntry = row.entries[row.entries.length - 1];
  const copyValue = formatPresentedWorkGroupForCopy(row);

  return (
    <article
      className="chat-row chat-work-group relative border-l-2 border-wardian-light bg-[color-mix(in_srgb,var(--color-wardian-card-bg-muted),transparent_18%)] px-2.5 py-1.5"
      data-testid="chat-work-group"
      data-expanded={expanded ? "true" : "false"}
    >
      <div className="flex min-w-0 items-center gap-2">
        <span className="h-1.5 w-1.5 shrink-0 rounded-full bg-[var(--color-wardian-processing)]" aria-hidden="true" />
        <button
          type="button"
          aria-expanded={expanded}
          className="flex min-w-0 flex-1 items-center gap-1.5 text-left"
          onClick={() => setExpanded((value) => !value)}
        >
          <span className="shrink-0 text-[11px] font-semibold leading-5 text-primary">{title}</span>
          <span className="shrink-0 text-[11px] leading-5 text-muted-neutral">
            {row.entries.length} {row.entries.length === 1 ? "event" : "events"}{duration ? ` · ${duration}` : ""}
          </span>
          {!expanded && latestEntry ? (
            <span className="min-w-0 truncate text-[11px] leading-5 text-muted-neutral">
              · {latestEntry.title}{latestEntry.summary ? ` · ${latestEntry.summary}` : ""}
            </span>
          ) : null}
        </button>
        <div className="flex shrink-0 items-center gap-1.5">
          <button
            type="button"
            aria-expanded={expanded}
            className="chat-work-group-toggle inline-flex h-7 w-7 items-center justify-center rounded border border-wardian-light text-muted-neutral hover:text-primary"
            onClick={() => setExpanded((value) => !value)}
            title={expanded ? "Collapse work log" : "Show all work"}
          >
            <ChevronRight aria-hidden="true" className={`h-3.5 w-3.5 transition-transform ${expanded ? "rotate-90" : ""}`} />
            <span className="sr-only">{expanded ? "Collapse" : "Show all"}</span>
          </button>
        </div>
      </div>
      <ChatRowActions
        actions={[
          { label: "Copy work log", value: copyValue },
          ...(row.changedPaths.length > 0
            ? [{ label: "Copy changed file paths", value: row.changedPaths.join("\n") }]
            : []),
        ]}
        className="chat-row-actions--overlay chat-row-actions--with-toggle"
        label="Work log actions"
      />

      {expanded ? (
        <>
          {row.changedPaths.length > 0 ? <ChangedFiles paths={row.changedPaths} /> : null}
          <div className="mt-2 space-y-1">
            {visibleEntries.map((entry) => (
              <WorkEntry agentIsWorking={agentIsWorking} entry={entry} key={entry.id} />
            ))}
          </div>
        </>
      ) : null}
    </article>
  );
}

export function WorkEntry({ entry, agentIsWorking }: { entry: PresentedWorkEntry; agentIsWorking: boolean }) {
  const tone = resolvedActivityTone(entry.block.tone, agentIsWorking);
  return (
    <div className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-2 rounded border border-transparent py-1 text-[12px] leading-4">
      <span className={`mt-1 h-1.5 w-1.5 rounded-full ${toneDotClass(tone)}`} aria-hidden="true" />
      <div className="min-w-0">
        <div className="truncate font-medium text-primary">{entry.title}</div>
        {entry.summary ? (
          <div className="truncate font-mono text-[11px] text-muted-neutral" title={entry.summary}>
            {entry.summary}
          </div>
        ) : null}
        {entry.details.length > 0 ? (
          <div className="truncate text-[11px] text-muted-neutral">{entry.details.join(" - ")}</div>
        ) : null}
      </div>
    </div>
  );
}

export function ChangedFiles({ paths }: { paths: string[] }) {
  const shown = paths.slice(0, 6);
  const remaining = paths.length - shown.length;
  return (
    <div className="mt-2 flex flex-wrap items-center gap-1.5">
      <span className="text-[11px] font-semibold leading-4 text-muted-neutral">Changed files</span>
      {shown.map((path) => (
        <span
          className="max-w-[220px] truncate rounded border border-wardian-light bg-[var(--color-wardian-sidebar-primary)] px-1.5 py-0.5 font-mono text-[11px] leading-4 text-primary"
          key={path}
          title={path}
        >
          {compactPath(path)}
        </span>
      ))}
      {remaining > 0 ? <span className="text-[11px] leading-4 text-muted-neutral">+{remaining} more</span> : null}
    </div>
  );
}
