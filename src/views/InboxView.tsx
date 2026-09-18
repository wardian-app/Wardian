import { useMemo, useState } from "react";
import { Bot, ChevronDown, ChevronUp, GitBranch, ListFilter, Terminal, Trash2 } from "lucide-react";
import { useQueueStore } from "../store/useQueueStore";
import type { QueueItem } from "../types";
import { DocsLink } from "../components/DocsLink";
import { QUEUE_EVENT_LABELS, QUEUE_EVENT_TYPES, queueItemIsVisible } from "../features/queue/queueFilters";
import { parseQueueActionChoices, type QueueActionChoice } from "../features/queue/actionChoices";
import { ProviderQuestionDetails } from "../features/queue/ProviderQuestionDetails";
import { QUEUE_TONE_CLASSES, queueItemIsAgentEvent, queueItemLabel, queueItemTone } from "../features/queue/queuePresentation";
import { isClearableLegacyCompletion, providerChoiceRecorded } from "../features/queue/queueTriage";
import { useLazyQueueItems } from "../features/queue/useLazyQueueItems";

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

function StatusBadge({ item }: { item: QueueItem }) {
  const classes = QUEUE_TONE_CLASSES[queueItemTone(item)];
  return (
    <span
      className={`text-[10px] font-bold px-2 py-0.5 rounded-full ${classes.badge}`}
    >
      {queueItemLabel(item)}
    </span>
  );
}

function queueItemAccent(item: QueueItem) {
  return QUEUE_TONE_CLASSES[queueItemTone(item)].accent;
}

function QueueItemIcon({ item }: { item: QueueItem }) {
  const Icon = queueItemIsAgentEvent(item) ? Bot : GitBranch;
  const iconClass = QUEUE_TONE_CLASSES[queueItemTone(item)].icon;

  return (
    <div
      className={`mt-0.5 flex h-8 w-8 shrink-0 items-center justify-center rounded-lg ${iconClass}`}
      aria-hidden="true"
    >
      <Icon className="h-4 w-4" />
    </div>
  );
}

interface QueueCardProps {
  item: QueueItem;
  onOpenAgent?: (sessionId: string) => void;
  onSendAgentPrompt?: (sessionId: string, prompt: string, itemId: string) => Promise<void> | void;
}

function QueueCard({ item, onOpenAgent, onSendAgentPrompt }: QueueCardProps) {
  const dismissItem = useQueueStore((s) => s.dismissItem);
  const recordProviderChoiceSent = useQueueStore((s) => s.recordProviderChoiceSent);
  const markRead = useQueueStore((s) => s.markRead);
  const resolveApprovalRequest = useQueueStore((s) => s.resolveApprovalRequest);
  const [isExpanded, setIsExpanded] = useState(false);
  const [isSending, setIsSending] = useState(false);
  const [actionError, setActionError] = useState<string | null>(null);

  const isAgent = queueItemIsAgentEvent(item);
  const isActionNeeded = item.type === "action_needed";
  const isApprovalRequest = item.type === "approval_request";
  const title = item.notification_title ?? (isAgent ? item.agent_name : item.automation_name);
  const bodyText = item.provider_question
    ? undefined
    : item.status === "failed" && item.error ? item.error : item.summary;
  const isExpandable = Boolean(bodyText && (bodyText.length > 220 || bodyText.split("\n").length > 4));
  const summaryId = `queue-item-summary-${item.id}`;
  const canOpenAgent = Boolean(item.agent_session_id && onOpenAgent);
  const actionChoices = isActionNeeded && !item.provider_question ? parseQueueActionChoices(bodyText) : [];
  const canUseActionChoices = Boolean(item.agent_session_id && onSendAgentPrompt && actionChoices.length > 0);
  const providerChoiceUncertain = Boolean(item.provider_choice_pending);
  const providerChoiceNeedsAcknowledgement = Boolean(item.provider_choice_sent && !item.read);
  const providerChoiceAlreadyRecorded = providerChoiceRecorded(item);
  const approvalChoices = isApprovalRequest && (item.automation_approval || item.notification_status === "awaiting_reply")
    ? item.approval_choices ?? []
    : [];
  const canAcknowledge = !item.automation_approval && !providerChoiceUncertain;

  const handleActionChoice = async (choice: QueueActionChoice) => {
    if (!item.agent_session_id || !onSendAgentPrompt) return;

    setActionError(null);
    setIsSending(true);
    try {
      await onSendAgentPrompt(item.agent_session_id, choice.value, item.id);
      recordProviderChoiceSent(item.id, choice.value);
      markRead(item.id);
    } catch (cause) {
      const detail = cause instanceof Error ? cause.message : String(cause);
      setActionError(`Could not send this response: ${detail}`);
    } finally {
      setIsSending(false);
    }
  };

  const handleApprovalChoice = async (choice: string) => {
    if (!item.inbox_notification_id && !item.automation_approval) return;
    setActionError(null);
    setIsSending(true);
    try {
      await resolveApprovalRequest(item, choice);
    } catch (cause) {
      const detail = cause instanceof Error ? cause.message : String(cause);
      setActionError(`Could not resolve this approval: ${detail}`);
    } finally {
      setIsSending(false);
    }
  };

  return (
    <div
      className={`group relative shrink-0 overflow-hidden rounded-lg border transition-colors cursor-pointer ${
        item.read
          ? "border-wardian-border bg-wardian-card-bg-muted"
          : "border-[var(--color-wardian-accent)]/30 bg-wardian-card-bg"
      }`}
      onClick={() => {
        if (canAcknowledge) markRead(item.id);
      }}
    >
      <div className={`absolute left-0 top-0 h-full w-1 ${queueItemAccent(item)}`} />
      {!item.read && (
        <span
          data-testid="queue-unread-dot"
          className="absolute left-2 top-2 z-10 h-2 w-2 rounded-full bg-[var(--color-wardian-accent)] shadow-[0_0_0_2px_var(--color-wardian-bg)]"
        />
      )}

      <div className="flex items-start gap-3 py-3 pl-5 pr-3">
        <QueueItemIcon item={item} />
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-2 flex-wrap">
            <span className="truncate text-sm font-semibold text-primary">{title ?? "Unknown"}</span>
            <StatusBadge item={item} />
            <span className="text-[10px] text-muted-neutral shrink-0">{relativeTime(item.timestamp)}</span>
          </div>
          {bodyText && (
            <div className="mt-2 space-y-2">
              <p
                id={summaryId}
                data-testid={summaryId}
                className={`text-[13px] leading-5 text-muted whitespace-pre-wrap break-words ${
                  isExpandable && !isExpanded
                    ? "line-clamp-4"
                    : isExpandable
                      ? "max-h-80 overflow-y-auto pr-2"
                      : ""
                }`}
              >
                {bodyText}
              </p>
              {isExpandable && (
                <button
                  type="button"
                  aria-controls={summaryId}
                  aria-expanded={isExpanded}
                  aria-label={isExpanded ? "Collapse summary" : "Show full summary"}
                  onClick={(e) => {
                    e.stopPropagation();
                    setIsExpanded((value) => !value);
                  }}
                  className="inline-flex items-center gap-1 rounded-md text-[11px] font-semibold text-muted-neutral hover:text-bright-neutral transition-colors"
                >
                  {isExpanded ? (
                    <ChevronUp className="w-3 h-3" aria-hidden="true" />
                  ) : (
                    <ChevronDown className="w-3 h-3" aria-hidden="true" />
                  )}
                  {isExpanded ? "Hide details" : "Show details"}
                </button>
              )}
            </div>
          )}
          {item.provider_question && <ProviderQuestionDetails question={item.provider_question} />}
          {isApprovalRequest && (
            <dl className="mt-3 grid gap-2 text-[12px] leading-5 text-muted">
              <div><dt className="font-semibold text-primary">Proposed action</dt><dd>{item.proposed_action}</dd></div>
              <div><dt className="font-semibold text-primary">Risk</dt><dd>{item.risk}</dd></div>
              {item.approval_decision && <div><dt className="font-semibold text-primary">Decision</dt><dd>{item.approval_decision}</dd></div>}
              {item.notification_status === "expired" && <div className="text-wardian-warning">Expired without approval.</div>}
            </dl>
          )}
          {(canOpenAgent || canUseActionChoices || approvalChoices.length > 0) && (
            <div className="mt-3 flex flex-wrap items-center gap-2" onClick={(event) => event.stopPropagation()}>
              {canOpenAgent && item.agent_session_id && (
                <button
                  type="button"
                  aria-label="Open agent terminal"
                  title="Open agent terminal"
                  onClick={() => {
                    if (canAcknowledge) markRead(item.id);
                    onOpenAgent?.(item.agent_session_id!);
                  }}
                  className="inline-flex h-7 items-center gap-1 rounded-md border border-wardian-border bg-wardian-card-bg-muted px-2 text-[11px] font-semibold text-muted-neutral hover:text-bright-neutral transition-colors"
                >
                  <Terminal className="h-3.5 w-3.5" aria-hidden="true" />
                  Open
                </button>
              )}
              {canUseActionChoices && (
                <div className="flex min-w-0 flex-wrap items-center gap-2" aria-label="Action choices">
                  {actionChoices.map((choice) => (
                    <button
                      key={`${choice.value}-${choice.label}`}
                      type="button"
                      aria-label={`Send action response ${choice.value}: ${choice.label}`}
                      title={`Send ${choice.label}`}
                      disabled={isSending || providerChoiceAlreadyRecorded}
                      onClick={() => void handleActionChoice(choice)}
                      className="inline-flex h-7 max-w-[220px] items-center gap-1.5 rounded-md border border-[color-mix(in_srgb,var(--color-wardian-warning),transparent_35%)] bg-[color-mix(in_srgb,var(--color-wardian-warning),transparent_88%)] px-2 text-[11px] font-semibold text-primary transition-colors hover:bg-[color-mix(in_srgb,var(--color-wardian-warning),transparent_80%)] disabled:cursor-not-allowed disabled:opacity-50"
                    >
                      <span className="shrink-0 font-mono text-[var(--color-wardian-warning)]">{choice.value}</span>
                      <span className="min-w-0 truncate">{choice.label}</span>
                    </button>
                  ))}
                </div>
              )}
              {approvalChoices.length > 0 && (
                <div className="flex min-w-0 flex-wrap items-center gap-2" aria-label="Approval choices">
                  {approvalChoices.map((choice) => (
                    <button
                      key={choice}
                      type="button"
                      disabled={isSending}
                      onClick={() => void handleApprovalChoice(choice)}
                      className="inline-flex h-7 max-w-[220px] cursor-pointer items-center rounded-md border border-[color-mix(in_srgb,var(--color-wardian-warning),transparent_35%)] bg-[color-mix(in_srgb,var(--color-wardian-warning),transparent_88%)] px-2 text-[11px] font-semibold text-primary transition-colors hover:bg-[color-mix(in_srgb,var(--color-wardian-warning),transparent_80%)] disabled:cursor-not-allowed disabled:opacity-50"
                    >
                      {choice}
                    </button>
                  ))}
                </div>
              )}
            </div>
          )}
          {actionError ? <p role="alert" className="mt-2 text-[11px] text-[var(--color-wardian-error)]">{actionError}</p> : null}
          {providerChoiceUncertain && <p role="alert" className="mt-2 text-[11px] text-[var(--color-wardian-error)]">Response delivery is uncertain. Check the agent before retrying.</p>}
          {providerChoiceNeedsAcknowledgement && <p role="status" className="mt-2 text-[11px] text-[var(--color-wardian-error)]">
            Response sent. Inbox status may need updating. <button type="button" onClick={(event) => { event.stopPropagation(); markRead(item.id); }} className="font-semibold underline">Retry Inbox status</button>
          </p>}
        </div>

        {!item.inbox_notification_id && !item.automation_approval && !providerChoiceUncertain && !providerChoiceNeedsAcknowledgement && <button
          type="button"
          aria-label="Clear item"
          title="Clear item"
          onClick={(e) => { e.stopPropagation(); dismissItem(item.id); }}
          className="shrink-0 p-1 rounded hover:bg-wardian-card-bg-muted text-muted-neutral hover:text-bright-neutral transition-colors"
        >
          <Trash2 className="w-3.5 h-3.5" aria-hidden="true" />
        </button>}
      </div>
    </div>
  );
}

interface QueueControlsProps {
  hasItems: boolean;
  hasReadItems: boolean;
  markAllRead: () => void;
  clearRead: () => void;
}

function QueueControls({ hasItems, hasReadItems, markAllRead, clearRead }: QueueControlsProps) {
  const preferences = useQueueStore((s) => s.preferences);
  const setEventVisible = useQueueStore((s) => s.setEventVisible);
  const [filtersOpen, setFiltersOpen] = useState(false);
  const visibleCount = QUEUE_EVENT_TYPES.filter((eventType) => preferences.visible_event_types[eventType]).length;
  const filterLabel = visibleCount === QUEUE_EVENT_TYPES.length
    ? "All events"
    : visibleCount === 0
      ? "None"
      : `${visibleCount} shown`;

  return (
    <div className="flex items-center justify-between gap-2">
      <h2 className="text-sm font-semibold text-primary tracking-wide">Inbox</h2>
      {hasItems && (
        <div className="flex items-center gap-2">
          <div className="relative">
            <button
              type="button"
              aria-label="Filter Inbox events"
              aria-expanded={filtersOpen}
              onClick={() => setFiltersOpen((open) => !open)}
              className="inline-flex h-7 items-center gap-1.5 rounded-md border border-wardian-border bg-wardian-card-bg-muted px-2 text-[11px] font-semibold text-muted-neutral transition-colors hover:text-bright-neutral"
            >
              <ListFilter className="h-3.5 w-3.5" aria-hidden="true" />
              Filter: {filterLabel}
              <ChevronDown className="h-3 w-3" aria-hidden="true" />
            </button>
            {filtersOpen && (
              <div className="absolute right-0 top-8 z-20 w-56 rounded-md border border-wardian-border bg-wardian-bg p-2 shadow-xl">
                {QUEUE_EVENT_TYPES.map((eventType) => (
                  <label
                    key={`show-${eventType}`}
                    className="flex items-center gap-2 rounded px-2 py-1.5 text-[12px] font-medium text-muted-neutral transition-colors hover:bg-wardian-card-bg-muted hover:text-primary"
                  >
                    <input
                      type="checkbox"
                      aria-label={`Show ${QUEUE_EVENT_LABELS[eventType].toLowerCase()}`}
                      checked={preferences.visible_event_types[eventType]}
                      onChange={(event) => setEventVisible(eventType, event.target.checked)}
                      className="h-3 w-3 accent-[var(--color-wardian-accent)]"
                    />
                    {QUEUE_EVENT_LABELS[eventType]}
                  </label>
                ))}
              </div>
            )}
          </div>
          <button
            type="button"
            onClick={markAllRead}
            className="rounded-md px-2 py-1 text-[11px] text-muted-neutral hover:bg-wardian-card-bg-muted hover:text-bright-neutral transition-colors"
          >
            Mark all read
          </button>
          <button
            type="button"
            onClick={clearRead}
            disabled={!hasReadItems}
            className="rounded-md px-2 py-1 text-[11px] text-muted-neutral hover:bg-wardian-card-bg-muted hover:text-bright-neutral disabled:cursor-not-allowed disabled:opacity-40 transition-colors"
          >
            Clear read
          </button>
        </div>
      )}
    </div>
  );
}

export interface InboxViewProps {
  onOpenAgent?: (sessionId: string) => void;
  onSendAgentPrompt?: (sessionId: string, prompt: string, itemId: string) => Promise<void> | void;
}

export function InboxView({ onOpenAgent, onSendAgentPrompt }: InboxViewProps) {
  const items = useQueueStore((s) => s.items);
  const inboxNotificationsTruncated = useQueueStore((s) => s.inboxNotificationsTruncated);
  const inboxNotificationsNextOffset = useQueueStore((s) => s.inboxNotificationsNextOffset);
  const loadingMoreInboxNotifications = useQueueStore((s) => s.loadingMoreInboxNotifications);
  const loadMoreInboxNotifications = useQueueStore((s) => s.loadMoreInboxNotifications);
  const preferences = useQueueStore((s) => s.preferences);
  const markAllRead = useQueueStore((s) => s.markAllRead);
  const clearRead = useQueueStore((s) => s.clearRead);
  const hasReadItems = items.some((item) => item.read && isClearableLegacyCompletion(item));
  const visibleItems = useMemo(
    () => items.filter((item) => queueItemIsVisible(item, preferences)),
    [items, preferences],
  );
  const { hasMore, loadMoreOnScroll, renderedItems } = useLazyQueueItems(visibleItems);

  return (
    <div className="queue-view flex flex-col h-full min-h-0 p-4 gap-4">
      <QueueControls hasItems={items.length > 0} hasReadItems={hasReadItems} markAllRead={markAllRead} clearRead={clearRead} />

      {inboxNotificationsTruncated && (
        <p role="status" className="text-xs text-[var(--color-wardian-warning)]">
          <span>Showing the 200 newest Inbox notifications; pages are capped at 200.</span>{' '}
          {inboxNotificationsNextOffset !== null && (
            <button type="button" className="font-semibold underline disabled:opacity-50" onClick={() => void loadMoreInboxNotifications()} disabled={loadingMoreInboxNotifications}>
              {loadingMoreInboxNotifications ? 'Loading…' : 'Load next 200'}
            </button>
          )}
        </p>
      )}

      {items.length === 0 ? (
        <div className="flex-1 flex items-center justify-center">
          <div className="max-w-sm text-center">
            <p className="text-sm font-semibold text-primary">No completions yet.</p>
            <p className="mt-2 text-xs leading-5 text-muted-neutral">
              Your Inbox collects completed work, important updates, and requests that need your input.
            </p>
            <div className="mt-3 flex justify-center gap-4">
              <DocsLink path="/guide/getting-started">First-run guide</DocsLink>
              <DocsLink path="/guide/inbox">Inbox guide</DocsLink>
            </div>
          </div>
        </div>
      ) : visibleItems.length === 0 ? (
        <div className="flex-1 flex items-center justify-center">
          <p className="text-sm font-semibold text-primary">No matching Inbox items.</p>
        </div>
      ) : (
        <div
          className="flex flex-1 min-h-0 flex-col gap-3 overflow-y-auto pr-1"
          data-testid="inbox-scroll-region"
          onScroll={loadMoreOnScroll}
        >
          {renderedItems.map((item) => (
            <QueueCard
              key={item.id}
              item={item}
              onOpenAgent={onOpenAgent}
              onSendAgentPrompt={onSendAgentPrompt}
            />
          ))}
          {hasMore && <p className="sr-only" aria-live="polite">Scroll to load older Inbox items.</p>}
        </div>
      )}
    </div>
  );
}
