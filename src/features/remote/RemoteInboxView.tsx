import React, { useMemo, useState } from "react";
import { Bot, CheckCheck, ChevronDown, ChevronUp, Filter, GitBranch, Inbox, Terminal, Trash2, X } from "lucide-react";
import type { QueueItem } from "../../types";
import { parseQueueActionChoices, type QueueActionChoice } from "../queue/actionChoices";
import { ProviderQuestionDetails } from "../queue/ProviderQuestionDetails";
import { QUEUE_TONE_CLASSES, queueItemIsAgentEvent, queueItemLabel, queueItemTone } from "../queue/queuePresentation";
import { isClearableLegacyCompletion, providerChoiceAcknowledgementUnresolved } from "../queue/queueTriage";
import { useLazyQueueItems } from "../queue/useLazyQueueItems";
import { useRemoteStore } from "./useRemoteStore";

type RemoteInboxFilter = "all" | QueueItem["type"] | "automation_failed";
const REMOTE_INBOX_FILTER_KEY = "wardian.remote.inboxFilter";

const filterOptions: readonly { value: RemoteInboxFilter; label: string }[] = [
  { value: "all", label: "All events" },
  { value: "action_needed", label: "Action needed" },
  { value: "agent_completed", label: "Agent completions" },
  { value: "agent_update", label: "Agent updates" },
  { value: "approval_request", label: "Approvals" },
  { value: "automation_completed", label: "Automation completions" },
  { value: "automation_failed", label: "Automation failures" },
];

function storedFilter(): RemoteInboxFilter {
  try {
    const value = window.localStorage.getItem(REMOTE_INBOX_FILTER_KEY);
    return filterOptions.some((option) => option.value === value) ? value as RemoteInboxFilter : "all";
  } catch {
    return "all";
  }
}

function relativeTime(ts: number): string {
  const diffMs = Date.now() - ts;
  const secs = Math.floor(diffMs / 1000);
  if (secs < 60) return "just now";
  const mins = Math.floor(secs / 60);
  if (mins < 60) return `${mins}m ago`;
  const hrs = Math.floor(mins / 60);
  if (hrs < 24) return `${hrs}h ago`;
  return `${Math.floor(hrs / 24)}d ago`;
}

function matchesFilter(item: QueueItem, filter: RemoteInboxFilter) {
  if (filter === "all") return true;
  if (filter === "automation_failed") return item.type === "automation_completed" && item.status === "failed";
  return item.type === filter;
}

interface RemoteInboxCardProps {
  item: QueueItem;
  onAction: (action: string, itemId?: string, choice?: string) => Promise<void>;
  onOpenAgent: (sessionId: string) => void;
  onSendAgentPrompt: (sessionId: string, prompt: string, inboxItemId: string) => Promise<void>;
  onRefreshInbox: () => Promise<boolean>;
}

function RemoteInboxCard({ item, onAction, onOpenAgent, onSendAgentPrompt, onRefreshInbox }: RemoteInboxCardProps) {
  const [isExpanded, setIsExpanded] = useState(false);
  const [isSending, setIsSending] = useState(false);
  const [actionError, setActionError] = useState<string | null>(null);
  const [acknowledgementError, setAcknowledgementError] = useState<string | null>(null);
  const [deliveryUncertain, setDeliveryUncertain] = useState(false);
  const providerChoiceRecoveryByItem = useRemoteStore((state) => state.providerChoiceRecoveryByItem[item.id]);
  const recordProviderChoiceRecovery = useRemoteStore((state) => state.recordProviderChoiceRecovery);
  const title = item.notification_title ?? item.agent_name ?? item.automation_name ?? "Unknown";
  const bodyText = item.provider_question
    ? undefined
    : item.status === "failed" && item.error ? item.error : item.summary;
  const Icon = queueItemIsAgentEvent(item) ? Bot : GitBranch;
  const classes = QUEUE_TONE_CLASSES[queueItemTone(item)];
  const isExpandable = Boolean(bodyText && (bodyText.length > 220 || bodyText.split("\n").length > 4));
  const summaryId = `remote-queue-item-summary-${item.id}`;
  const isApprovalRequest = item.type === "approval_request";
  const isPendingApproval = Boolean(item.automation_approval || item.notification_status === "awaiting_reply");
  const canOpenAgent = Boolean(item.agent_session_id);
  const actionChoices = item.type === "action_needed" && !item.provider_question ? parseQueueActionChoices(bodyText) : [];
  const approvalChoices = isApprovalRequest && isPendingApproval ? item.approval_choices ?? [] : [];
  const providerChoiceSent = item.provider_choice_sent ?? providerChoiceRecoveryByItem ?? null;
  const providerChoicePending = item.provider_choice_pending ?? null;
  const providerChoiceUncertain = providerChoicePending !== null || deliveryUncertain;
  const providerChoiceNeedsAcknowledgement = !providerChoiceUncertain && providerChoiceSent !== null && !item.read;
  const canDismiss = !item.inbox_notification_id
    && !item.automation_approval
    && !providerChoiceUncertain
    && !(providerChoiceSent !== null && !item.read);

  const runAction = async (action: string, choice?: string) => {
    setActionError(null);
    setIsSending(true);
    try {
      await onAction(action, item.id, choice);
    } catch (cause) {
      setActionError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setIsSending(false);
    }
  };

  const handleActionChoice = async (choice: QueueActionChoice) => {
    if (!item.agent_session_id) return;
    setActionError(null);
    setAcknowledgementError(null);
    setIsSending(true);
    try {
      await onSendAgentPrompt(item.agent_session_id, choice.value, item.id);
      // Keep a remount-surviving guard independent of the acknowledgement refresh.
      recordProviderChoiceRecovery(item.id, choice.value);
      try {
        await onAction("mark_read", item.id);
      } catch (cause) {
        setAcknowledgementError(cause instanceof Error ? cause.message : String(cause));
        // Refresh the server projection so a remount sees the durable sent marker.
        void onRefreshInbox().catch(() => undefined);
      }
    } catch (cause) {
      setDeliveryUncertain(true);
      try {
        if (await onRefreshInbox()) setDeliveryUncertain(false);
      } catch {
        // Keep the local guard if the recovery refresh is unavailable.
      }
      setActionError(`Could not send this response: ${cause instanceof Error ? cause.message : String(cause)}`);
    } finally {
      setIsSending(false);
    }
  };

  const retryAcknowledgement = async () => {
    setAcknowledgementError(null);
    setIsSending(true);
    try {
      await onAction("mark_read", item.id);
    } catch (cause) {
      setAcknowledgementError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setIsSending(false);
    }
  };

  return (
    <article
      className={`group relative overflow-hidden rounded-md border px-3 py-3 ${
        item.read ? "border-wardian-border bg-wardian-card-bg-muted" : "border-[var(--color-wardian-accent)]/30 bg-wardian-card-bg"
      }`}
      onClick={() => {
        if (!isPendingApproval && !providerChoiceUncertain) void onAction("mark_read", item.id).catch(() => undefined);
      }}
    >
      <div className={`absolute left-0 top-0 h-full w-1 ${classes.accent}`} />
      {!item.read && <span data-testid="remote-queue-unread-dot" className="absolute left-2 top-2 z-10 h-2 w-2 rounded-full bg-[var(--color-wardian-accent)] shadow-[0_0_0_2px_var(--color-wardian-bg)]" />}
      <div className="flex items-start gap-3 pl-2">
        <span className={`mt-0.5 flex h-8 w-8 shrink-0 items-center justify-center rounded-md ${classes.icon}`} aria-hidden="true">
          <Icon className="h-4 w-4" />
        </span>
        <div className="min-w-0 flex-1">
          <div className="flex min-w-0 flex-wrap items-center gap-2">
            <span className="truncate text-sm font-semibold text-primary">{title}</span>
            <span className={`rounded-full px-2 py-0.5 text-[10px] font-bold ${classes.badge}`}>{queueItemLabel(item)}</span>
            <span className="shrink-0 text-[10px] text-muted-neutral">{relativeTime(item.timestamp)}</span>
          </div>
          {bodyText && (
            <div className="mt-2 space-y-2">
              <p id={summaryId} data-testid={summaryId} className={`whitespace-pre-wrap break-words text-[13px] leading-5 text-muted ${isExpandable && !isExpanded ? "line-clamp-4" : isExpandable ? "max-h-80 overflow-y-auto pr-2" : ""}`}>
                {bodyText}
              </p>
              {isExpandable && (
                <button type="button" aria-controls={summaryId} aria-expanded={isExpanded} aria-label={isExpanded ? "Collapse summary" : "Show full summary"} onClick={(event) => { event.stopPropagation(); setIsExpanded((value) => !value); }} className="inline-flex items-center gap-1 rounded-md text-[11px] font-semibold text-muted-neutral transition-colors hover:text-bright-neutral">
                  {isExpanded ? <ChevronUp className="h-3 w-3" aria-hidden="true" /> : <ChevronDown className="h-3 w-3" aria-hidden="true" />}
                  {isExpanded ? "Hide details" : "Show details"}
                </button>
              )}
            </div>
          )}
          {item.provider_question && <ProviderQuestionDetails question={item.provider_question} />}
          {isApprovalRequest && (
            <dl className="mt-3 grid gap-2 text-[12px] leading-5 text-muted">
              {item.proposed_action && <div><dt className="font-semibold text-primary">Proposed action</dt><dd>{item.proposed_action}</dd></div>}
              {item.risk && <div><dt className="font-semibold text-primary">Risk</dt><dd>{item.risk}</dd></div>}
              {item.approval_decision && <div><dt className="font-semibold text-primary">Decision</dt><dd>{item.approval_decision}</dd></div>}
              {item.notification_status === "expired" && <div className="text-wardian-warning">Expired without approval.</div>}
            </dl>
          )}
          {(canOpenAgent || actionChoices.length > 0 || approvalChoices.length > 0) && (
            <div className="mt-3 flex flex-wrap items-center gap-2" onClick={(event) => event.stopPropagation()}>
              {canOpenAgent && item.agent_session_id && <button type="button" aria-label="Open agent terminal" title="Open agent terminal" onClick={() => { if (!isPendingApproval && !providerChoiceUncertain) void onAction("mark_read", item.id).catch(() => undefined); onOpenAgent(item.agent_session_id!); }} className="inline-flex h-7 items-center gap-1 rounded-md border border-wardian-border bg-wardian-card-bg-muted px-2 text-[11px] font-semibold text-muted-neutral transition-colors hover:text-bright-neutral"><Terminal className="h-3.5 w-3.5" aria-hidden="true" />Open agent</button>}
              {actionChoices.length > 0 && <div className="flex min-w-0 flex-wrap items-center gap-2" aria-label="Action choices">
                {actionChoices.map((choice) => <button key={`${choice.value}-${choice.label}`} type="button" aria-label={`Send action response ${choice.value}: ${choice.label}`} title={`Send ${choice.label}`} disabled={isSending || providerChoiceSent !== null || providerChoicePending !== null || deliveryUncertain} onClick={() => void handleActionChoice(choice)} className="inline-flex h-7 max-w-[220px] items-center gap-1.5 rounded-md border border-[color-mix(in_srgb,var(--color-wardian-warning),transparent_35%)] bg-[color-mix(in_srgb,var(--color-wardian-warning),transparent_88%)] px-2 text-[11px] font-semibold text-primary transition-colors hover:bg-[color-mix(in_srgb,var(--color-wardian-warning),transparent_80%)] disabled:cursor-not-allowed disabled:opacity-50"><span className="shrink-0 font-mono text-[var(--color-wardian-warning)]">{choice.value}</span><span className="min-w-0 truncate">{choice.label}</span></button>)}
              </div>}
              {approvalChoices.length > 0 && <div className="flex min-w-0 flex-wrap items-center gap-2" aria-label="Approval choices">
                {approvalChoices.map((choice) => <button key={choice} type="button" disabled={isSending} onClick={() => void runAction("resolve_approval", choice)} className="inline-flex h-7 max-w-[220px] cursor-pointer items-center rounded-md border border-[color-mix(in_srgb,var(--color-wardian-warning),transparent_35%)] bg-[color-mix(in_srgb,var(--color-wardian-warning),transparent_88%)] px-2 text-[11px] font-semibold text-primary transition-colors hover:bg-[color-mix(in_srgb,var(--color-wardian-warning),transparent_80%)] disabled:cursor-not-allowed disabled:opacity-50">{choice}</button>)}
              </div>}
            </div>
          )}
          {actionError && <p role="alert" className="mt-2 text-[11px] text-[var(--color-wardian-error)]">{actionError}</p>}
          {providerChoiceUncertain && <p role="alert" className="mt-2 text-[11px] text-[var(--color-wardian-error)]">Response delivery is uncertain. Check the agent before retrying.</p>}
          {providerChoiceNeedsAcknowledgement && <p role={acknowledgementError ? "alert" : "status"} className="mt-2 text-[11px] text-[var(--color-wardian-error)]">Response sent{acknowledgementError ? `, but Inbox status could not be updated: ${acknowledgementError}` : ". Inbox status may need updating."} <button type="button" disabled={isSending} onClick={(event) => { event.stopPropagation(); void retryAcknowledgement(); }} className="font-semibold underline disabled:cursor-not-allowed disabled:opacity-50">Retry Inbox status</button></p>}
        </div>
        {canDismiss && <button type="button" aria-label="Clear item" title="Clear item" onClick={(event) => { event.stopPropagation(); void runAction("dismiss"); }} className="shrink-0 rounded p-1 text-muted-neutral transition-colors hover:bg-wardian-card-bg-muted hover:text-bright-neutral"><Trash2 className="h-3.5 w-3.5" aria-hidden="true" /></button>}
      </div>
    </article>
  );
}

export const RemoteInboxView: React.FC = () => {
  const items = useRemoteStore((state) => state.remoteQueueItems);
  const remoteQueueError = useRemoteStore((state) => state.remoteQueueError);
  const openAgent = useRemoteStore((state) => state.openAgent);
  const refreshInbox = useRemoteStore((state) => state.refreshInbox);
  const runInboxAction = useRemoteStore((state) => state.runInboxAction);
  const sendPromptToAgent = useRemoteStore((state) => state.sendPromptToAgent);
  const [filter, setFilter] = useState<RemoteInboxFilter>(storedFilter);
  const [headerAction, setHeaderAction] = useState<"mark_all_read" | "clear_read" | null>(null);
  const [headerActionError, setHeaderActionError] = useState<string | null>(null);
  const visibleItems = useMemo(() => items.filter((item) => matchesFilter(item, filter)), [filter, items]);
  const { hasMore, loadMoreOnScroll, renderedItems } = useLazyQueueItems(visibleItems);
  const unreadCount = items.filter((item) => !item.read).length;
  const clearableReadCount = items.filter((item) => item.read
    && isClearableLegacyCompletion(item)
    && !providerChoiceAcknowledgementUnresolved(item)).length;
  const filterLabel = filterOptions.find((option) => option.value === filter)?.label ?? "All events";

  const runHeaderAction = async (action: "mark_all_read" | "clear_read") => {
    setHeaderAction(action);
    setHeaderActionError(null);
    try {
      await runInboxAction(action);
    } catch (error) {
      setHeaderActionError(error instanceof Error ? error.message : String(error));
    } finally {
      setHeaderAction(null);
    }
  };

  const updateFilter = (value: RemoteInboxFilter) => {
    setFilter(value);
    try { window.localStorage.setItem(REMOTE_INBOX_FILTER_KEY, value); } catch { /* Storage is optional. */ }
  };

  return (
    <section className="flex min-h-0 flex-1 flex-col" data-testid="remote-inbox-view">
      <header className="shrink-0 border-b border-wardian-border bg-wardian-bg/95 px-4 py-3 backdrop-blur">
        <div className="flex items-center gap-2">
          <Inbox className="h-4 w-4 text-muted-neutral" aria-hidden="true" />
          <h1 className="truncate text-base font-semibold text-primary">Inbox</h1>
          {unreadCount > 0 && <span className="rounded-full bg-[var(--color-wardian-accent)]/15 px-2 py-0.5 text-[10px] font-semibold text-primary">{unreadCount} unread</span>}
        </div>
        <div className="mt-3 flex flex-nowrap items-center gap-1 overflow-x-auto">
          <label className="sr-only" htmlFor="remote-inbox-filter">Filter Inbox events</label>
          <div className="relative inline-flex items-center">
            <Filter className="pointer-events-none absolute left-2 h-3.5 w-3.5 text-muted-neutral" aria-hidden="true" />
            <select id="remote-inbox-filter" aria-label="Filter Inbox events" value={filter} onChange={(event) => updateFilter(event.target.value as RemoteInboxFilter)} className="h-8 w-32 appearance-none rounded-md border border-wardian-border bg-wardian-card-bg-muted pl-7 pr-7 text-[11px] font-semibold text-primary">
              {filterOptions.map((option) => <option key={option.value} value={option.value}>{option.label}</option>)}
            </select>
            <X className="pointer-events-none absolute right-2 h-3 w-3 text-muted-neutral" aria-hidden="true" />
          </div>
          <button type="button" aria-label="Mark all Inbox items read" disabled={headerAction !== null || unreadCount === 0} onClick={() => void runHeaderAction("mark_all_read")} className="inline-flex h-8 shrink-0 items-center gap-1 whitespace-nowrap rounded-md px-1.5 text-[11px] font-semibold text-muted-neutral transition-colors hover:bg-wardian-card-bg-muted disabled:cursor-not-allowed disabled:opacity-40"><CheckCheck className="h-3.5 w-3.5" aria-hidden="true" />{headerAction === "mark_all_read" ? "Marking…" : "Mark all read"}</button>
          <button type="button" aria-label="Clear read Inbox items" disabled={headerAction !== null || clearableReadCount === 0} onClick={() => void runHeaderAction("clear_read")} className="h-8 shrink-0 whitespace-nowrap rounded-md px-1.5 text-[11px] font-semibold text-muted-neutral transition-colors hover:bg-wardian-card-bg-muted disabled:cursor-not-allowed disabled:opacity-40">{headerAction === "clear_read" ? "Clearing…" : "Clear read"}</button>
        </div>
        {(headerActionError || remoteQueueError) && <div role="alert" className="mt-2 flex flex-wrap items-center gap-2 text-[11px] text-[var(--color-wardian-error)]"><span>{headerActionError ?? `Inbox updated, but refresh failed: ${remoteQueueError}`}</span>{remoteQueueError && !headerActionError && <button type="button" onClick={() => void refreshInbox()} className="font-semibold underline">Retry refresh</button>}</div>}
      </header>
      <div
        className="min-h-0 flex-1 overflow-y-auto px-3 py-4"
        data-testid="remote-inbox-scroll-region"
        onScroll={loadMoreOnScroll}
      >
        {visibleItems.length === 0 ? (
          <div className="rounded-md border border-dashed border-wardian-border px-3 py-5 text-center text-xs text-muted-neutral">
            {items.length === 0 ? "No Inbox items yet." : `No ${filterLabel.toLowerCase()} in Inbox.`}
            {filter !== "all" && <button type="button" onClick={() => updateFilter("all")} className="mt-2 block w-full font-semibold text-primary hover:underline">Show all events</button>}
          </div>
        ) : (
          <div className="flex flex-col gap-3">
            {renderedItems.map((item) => <RemoteInboxCard key={item.id} item={item} onAction={runInboxAction} onOpenAgent={(sessionId) => void openAgent(sessionId)} onSendAgentPrompt={sendPromptToAgent} onRefreshInbox={refreshInbox} />)}
            {hasMore && <p className="sr-only" aria-live="polite">Scroll to load older Inbox items.</p>}
          </div>
        )}
      </div>
    </section>
  );
};
