import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { readImage } from "@tauri-apps/plugin-clipboard-manager";
import { FileText, Hand, Image as ImageIcon, Loader2, Plus, SendHorizontal, Square, X } from "lucide-react";
import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import type { DragEvent, KeyboardEvent } from "react";
import type { AgentChatEvent, AgentChatPage, AgentConfig, AgentModelSelectionUpdateResult, AgentTelemetry } from "../../types";
import { useSettingsStore } from "../../store/useSettingsStore";
import { reasoningEffortForConfig } from "../agents/configUtils";
import { ProviderModelSelector, type ModelSelection } from "../agents/ProviderModelSelector";
import {
  promptWithChatAttachments,
  stageChatImageAttachments,
  submitInputToAgent,
  type ChatAttachment,
} from "../../utils/terminalInput";
import { ChatTranscriptRow } from "../chat/ChatTranscriptRows";
import { useChatPages, type ChatPageLoader } from "../chat/useChatPages";
import { chatReadProgress, submittedChatEvent } from "../chat/chatReadState";
import { captureChatScrollAnchor, restoreChatScrollAnchor, type ChatScrollAnchor } from "../chat/chatScrollAnchor";
import { matchingSlashCommands } from "../chat/slashCommands";
import {
  isProcessingAgentStatus,
  liveApprovalEventId,
  shouldShowChatEvent,
  sortTranscriptEvents,
} from "../chat/chatPresentation";
import { chatTranscriptRowKey, withTurnChangeSummaries, type ChatTranscriptRowModel } from "../chat/chatTurns";
import { useAppShellWorkbenchNavigation } from "../../layout/AppShell";
import { openFileWithSettings } from "../files/fileOpenRouting";
import { type ChatMarkdownLinkHandling } from "./markdown/ChatMarkdown";
import { derivePresentedChatRows } from "./workLogPresentation";
import {
  fileNameFromPath,
  getDroppedFilePaths,
  hasWardianFileDropData,
  isNativeFileDropInsideBounds,
} from "../../utils/fileDrop";

interface AgentChatViewBaseProps {
  sessionId: string;
  agent?: Pick<AgentConfig, "session_name" | "agent_class" | "provider" | "model" | "provider_config">;
  provider?: AgentConfig["provider"];
  isMaximized?: boolean;
  theme?: "dark" | "light" | "system";
  status?: string | null;
  telemetry?: Pick<AgentTelemetry, "current_status"> | null;
  className?: string;
  workspacePath?: string | null;
  refreshIntervalMs?: number;
  /** Blocks chat mutations for explicitly read-only presentations; history still pages normally. */
  readOnly?: boolean;
  /** Discover provider models only after an explicit picker activation on history-first surfaces. */
  deferModelDiscovery?: boolean;
  autoFocusComposer?: boolean;
  onComposerAutoFocused?: () => void;
  onAgentConfigUpdated?: (agent: AgentConfig) => void;
}

type AgentChatDraftControlProps =
  | { draft?: undefined; onDraftChange?: undefined }
  | { draft: string; onDraftChange: (value: string) => void };

type AgentChatViewProps = AgentChatViewBaseProps & AgentChatDraftControlProps;

type LoadState = "loading" | "waiting" | "ready" | "error";
const CHAT_REFRESH_INTERVAL_MS = 3000;


type AwaitingResponseMarker = { id: string; response_ids: Set<string> };

const CHAT_SCROLL_BOTTOM_THRESHOLD_PX = 48;
const loadChatPage: ChatPageLoader = ({ sessionId, cursor, revision, detailRef }) =>
  invoke<AgentChatPage>("load_agent_chat_page", { sessionId, cursor, revision, detailRef });

export function AgentChatView({
  sessionId,
  agent,
  provider,
  isMaximized = false,
  theme = "system",
  status,
  telemetry,
  className = "",
  workspacePath,
  refreshIntervalMs = CHAT_REFRESH_INTERVAL_MS,
  readOnly = false,
  deferModelDiscovery = false,
  autoFocusComposer = false,
  draft,
  onComposerAutoFocused,
  onAgentConfigUpdated,
  onDraftChange,
}: AgentChatViewProps) {
  const [awaitingResponse, setAwaitingResponse] = useState<AwaitingResponseMarker | null>(null);
  const [reloadKey, setReloadKey] = useState(0);
  const chat = useChatPages(sessionId, loadChatPage, refreshIntervalMs, reloadKey);
  const { events, page, loadingOlder, loadOlder, loadDetail, reset: resetChat, isCurrentScope: isCurrentChatScope } = chat;
  const error = chat.error;
  const loadState: LoadState = events.length === 0 && chat.waiting ? "waiting"
    : chat.loading && events.length === 0 ? "loading" : error !== null && events.length === 0 ? "error" : "ready";
  const progressText = page ? chatReadProgress(page.progress) : null;
  const [internalDraft, setInternalDraft] = useState("");
  const [isSubmitting, setIsSubmitting] = useState(false);
  const [isInterrupting, setIsInterrupting] = useState(false);
  const [interruptRequested, setInterruptRequested] = useState(false);
  const [submitError, setSubmitError] = useState<string | null>(null);
  const [fileOpenError, setFileOpenError] = useState<string | null>(null);
  const [attachments, setAttachments] = useState<ChatAttachment[]>([]);
  const submissionSerial = useRef(0);
  const pendingSubmissionScope = useRef<ReturnType<typeof chat.submissionScope> | null>(null);
  const workbenchNavigation = useAppShellWorkbenchNavigation();
  const externalEditor = useSettingsStore((state) => state.externalEditor);
  const externalEditorCustomExecutable = useSettingsStore((state) => state.externalEditorCustomExecutable);
  const fileOpenActions = useSettingsStore((state) => state.fileOpenActions);
  const transcriptScrollRef = useRef<HTMLDivElement>(null);
  const stickToLatestRef = useRef(true);
  const prependScrollSnapshotRef = useRef<ChatScrollAnchor | null>(null);
  const [settledPrependSnapshot, setSettledPrependSnapshot] = useState<ChatScrollAnchor | null>(null);
  const scrollSessionRef = useRef(sessionId);
  const activeDraft = draft ?? internalDraft;
  const setActiveDraft = onDraftChange ?? setInternalDraft;

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | null = null;

    listen<{ session_id?: string }>("agent-terminal-cleared", (event) => {
      if (disposed || event.payload?.session_id !== sessionId) return;
      stickToLatestRef.current = true;
      prependScrollSnapshotRef.current = null;
      resetChat();
      submissionSerial.current += 1; pendingSubmissionScope.current = null; setIsSubmitting(false);
      setAwaitingResponse(null);
      setSubmitError(null);
      setAttachments([]);
    })
      .then((dispose) => {
        if (disposed) {
          dispose();
          return;
        }
        unlisten = dispose;
      })
      .catch((reason) => {
        console.warn("agent-terminal-cleared chat listener error:", reason);
      });

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [sessionId, resetChat]);

  useEffect(() => {
    if (pendingSubmissionScope.current && !isCurrentChatScope(pendingSubmissionScope.current)) {
      submissionSerial.current += 1; pendingSubmissionScope.current = null; setIsSubmitting(false);
    }
  }, [page?.conversation_id, page?.source_epoch, isCurrentChatScope]);

  useEffect(() => {
    setAwaitingResponse((marker) => clearAwaitingResponseWhenAnswered(events, marker));
  }, [events]);

  const mergedEvents = events;
  const activeStatus = status ?? telemetry?.current_status ?? null;
  const showThinking = isProcessingAgentStatus(activeStatus) || awaitingResponse !== null;
  const isExecutionActive = showThinking && !interruptRequested;
  const displayEvents = useMemo(
    () =>
      appendThinkingIndicator(
        mergedEvents,
        sessionId,
        agent?.provider ?? provider ?? providerFromEvents(mergedEvents),
        showThinking,
      ),
    [agent?.provider, mergedEvents, provider, sessionId, showThinking],
  );
  const chatRows = useMemo<ChatTranscriptRowModel[]>(
    () => withTurnChangeSummaries(derivePresentedChatRows(displayEvents.filter(shouldShowChatEvent))),
    [displayEvents],
  );
  const visibleChatRows = chatRows;
  const latestVisibleRowKey = visibleChatRows.length > 0 ? chatTranscriptRowKey(visibleChatRows[visibleChatRows.length - 1]) : "";
  const hasActionRequired = mergedEvents.some((event) => event.status === "action_required");
  const liveApprovalId = useMemo(() => liveApprovalEventId(sortTranscriptEvents(mergedEvents)), [mergedEvents]);
  const disabledReason = readOnly ? "Read only" : inputDisabledReason(isSubmitting);
  const openChangedFile = useMemo(() => {
    const workspace = workspacePath?.trim();
    if (!workbenchNavigation || !workspace) return undefined;
    return async (path: string) => {
      const absolute = /^([A-Za-z]:[\\/]|\/|\\\\)/.test(path)
        ? path
        : `${workspace.replace(/[\\/]+$/g, "")}/${path.replace(/^[\\/]+/g, "")}`;
      try {
        await openFileWithSettings(absolute, {
          navigation: workbenchNavigation,
          file_open_actions: fileOpenActions,
          external_editor: externalEditor,
          external_editor_custom_executable: externalEditorCustomExecutable,
        });
        setFileOpenError(null);
      } catch (reason) {
        const message = `Failed to open changed file: ${String(reason)}`;
        console.warn(message);
        setFileOpenError(message);
      }
    };
  }, [externalEditor, externalEditorCustomExecutable, fileOpenActions, workbenchNavigation, workspacePath]);
  const markdownLinkHandling = useMemo<ChatMarkdownLinkHandling>(() => ({
    getBasePath: () => workspacePath?.trim() || null,
    getExternalEditor: () => ({
      external_editor: externalEditor,
      external_editor_custom_executable: externalEditorCustomExecutable.trim() || null,
    }),
    openFile: async (path, editor) => {
      await openFileWithSettings(path, {
        navigation: workbenchNavigation,
        file_open_actions: fileOpenActions,
        external_editor: editor.external_editor,
        external_editor_custom_executable: editor.external_editor_custom_executable,
      });
    },
    onOpenError: (message) => {
      console.warn(message);
      setFileOpenError(message);
    },
  }), [externalEditor, externalEditorCustomExecutable, fileOpenActions, workbenchNavigation, workspacePath]);

  useEffect(() => {
    stickToLatestRef.current = true;
    submissionSerial.current += 1; pendingSubmissionScope.current = null;
    prependScrollSnapshotRef.current = null;
    setAwaitingResponse(null);
    setInterruptRequested(false);
    setIsInterrupting(false);
    setIsSubmitting(false);
    setAttachments([]);
  }, [sessionId]);

  useLayoutEffect(() => {
    if (scrollSessionRef.current !== sessionId) {
      scrollSessionRef.current = sessionId;
      prependScrollSnapshotRef.current = null;
      stickToLatestRef.current = true;
    }
    const scrollRegion = transcriptScrollRef.current;
    if (!scrollRegion || loadState !== "ready") return;

    const prependSnapshot = prependScrollSnapshotRef.current;
    if (prependSnapshot) {
      if (settledPrependSnapshot !== prependSnapshot) return;
      restoreChatScrollAnchor(scrollRegion, prependSnapshot);
      prependScrollSnapshotRef.current = null;
      stickToLatestRef.current = isNearTranscriptBottom(scrollRegion);
      return;
    }

    if (stickToLatestRef.current) {
      scrollRegion.scrollTop = scrollRegion.scrollHeight;
      stickToLatestRef.current = true;
    }
  }, [latestVisibleRowKey, loadState, visibleChatRows.length, sessionId, settledPrependSnapshot]);

  const submitPrompt = async (
    promptValue: string,
    clearDraft: boolean,
    selectedAttachments: readonly ChatAttachment[] = [],
  ) => {
    const prompt = promptValue.trim();
    if ((!prompt && selectedAttachments.length === 0) || disabledReason) return;

    const providerName = agent?.provider ?? provider ?? providerFromEvents(events);
    const submittedPrompt = promptWithChatAttachments(prompt, selectedAttachments);

    stickToLatestRef.current = true;
    if (chat.browsingOlder) chat.jumpToLatest();
    setInterruptRequested(false);
    setIsSubmitting(true);
    setSubmitError(null);
    const submissionScope = chat.submissionScope();
    const request = ++submissionSerial.current;
    pendingSubmissionScope.current = submissionScope;
    try {
      await stageChatImageAttachments(sessionId, providerName, selectedAttachments);
      if (!chat.isCurrentScope(submissionScope)) return;
      const acknowledgement = await submitInputToAgent(sessionId, submittedPrompt);
      if (!chat.isCurrentScope(submissionScope)) return;
      if (clearDraft) setActiveDraft("");
      if (selectedAttachments.length > 0) setAttachments([]);
      chat.addSubmitted(submittedChatEvent(sessionId, providerName, submittedPrompt, acknowledgement));
      setAwaitingResponse({
        id: `awaiting-response-${sessionId}-${Date.now()}`,
        response_ids: new Set(responseEvents(events).map((event) => event.id)),
      });
      setReloadKey((key) => key + 1);
    } catch (reason) {
      if (chat.isCurrentScope(submissionScope)) setSubmitError(errorMessage(reason));
    } finally {
      if (request === submissionSerial.current) { pendingSubmissionScope.current = null; setIsSubmitting(false); }
    }
  };

  const handleSubmit = () => {
    void submitPrompt(activeDraft, true, attachments);
  };

  const handleApprovalSubmit = (response: string) => {
    void submitPrompt(response, false);
  };

  const handleInterrupt = async () => {
    if (readOnly || isInterrupting || !showThinking) return;
    setInterruptRequested(true);
    setIsInterrupting(true);
    setSubmitError(null);
    try {
      await invoke("send_input_to_agent", { sessionId, input: "\u0003" });
      setAwaitingResponse(null);
      setReloadKey((key) => key + 1);
    } catch (reason) {
      setInterruptRequested(false);
      setSubmitError(errorMessage(reason));
    } finally {
      setIsInterrupting(false);
    }
  };

  const handleTranscriptScroll = () => {
    const scrollRegion = transcriptScrollRef.current;
    if (!scrollRegion || prependScrollSnapshotRef.current) return;
    stickToLatestRef.current = isNearTranscriptBottom(scrollRegion);
    if (scrollRegion.scrollTop <= 160 && page?.next_before && !loadingOlder) void handleLoadOlderRows();
  };

  const handleLoadOlderRows = async () => {
    if (!page?.next_before || loadingOlder || prependScrollSnapshotRef.current) return;
    const scrollRegion = transcriptScrollRef.current;
    const snapshot = scrollRegion ? captureChatScrollAnchor(scrollRegion) : null;
    if (snapshot) {
      prependScrollSnapshotRef.current = snapshot;
      stickToLatestRef.current = false;
    }
    try {
      await loadOlder();
    } finally {
      // Settle after the page updates so restoration sees the committed rows,
      // including requests that return no rows or skip an active recent read.
      if (snapshot && prependScrollSnapshotRef.current === snapshot) setSettledPrependSnapshot(snapshot);
    }
  };

  return (
    <section
      aria-label={`Chat transcript for ${agent?.session_name ?? sessionId}`}
      className={`agent-chat-view chat-surface flex h-full min-h-0 flex-col bg-wardian-bg text-primary ${isMaximized ? "text-[14px]" : "text-[13px]"} ${className}`}
      data-theme-mode={theme}
      data-testid="agent-chat-view"
    >
      {fileOpenError ? (
        <div
          role="alert"
          className="mx-3 mt-2 rounded-md border border-wardian-error/40 bg-wardian-error/10 px-3 py-2 text-xs leading-relaxed text-wardian-error"
        >
          {fileOpenError}
        </div>
      ) : null}
      <div
        className="chat-transcript-scroll min-h-0 flex-1 overflow-auto px-2.5 py-2.5"
        data-testid="agent-chat-scroll-region"
        onScroll={handleTranscriptScroll}
        ref={transcriptScrollRef}
      >
        {loadState === "loading" ? <LoadingState /> : null}
        {chat.waiting ? <WaitingState compact={events.length > 0} /> : null}
        {error !== null ? <ErrorState error={error} onRetry={chat.errorDirection === "older" ? handleLoadOlderRows : chat.retry} compact={chatRows.length > 0} /> : null}
        {progressText ? <p role="status" className="mb-2 text-xs text-muted-neutral">{progressText}</p> : null}
        {loadState === "ready" && chatRows.length === 0 && !progressText ? <EmptyState /> : null}
        {loadState === "ready" && chatRows.length > 0 ? (
          <ol className="chat-transcript-list space-y-1.5" data-testid="agent-chat-transcript">
            {chat.browsingOlder ? (
              <li>
                <button
                  type="button"
                  className="w-full rounded border border-wardian-light bg-[var(--color-wardian-card-bg-muted)] px-2.5 py-1.5 text-[11px] font-semibold leading-5 text-muted-neutral hover:text-primary"
                  onClick={() => {
                    prependScrollSnapshotRef.current = null; stickToLatestRef.current = true; chat.jumpToLatest();
                  }}
                >Jump to latest</button>
              </li>
            ) : null}
            {page?.next_before ? (
              <li>
                <button
                  type="button"
                  className="w-full rounded border border-wardian-light bg-[var(--color-wardian-card-bg-muted)] px-2.5 py-1.5 text-[11px] font-semibold leading-5 text-muted-neutral hover:text-primary"
                  onClick={handleLoadOlderRows}
                  disabled={loadingOlder}
                >
                  {loadingOlder ? "Loading older transcript..." : "Load older transcript"}
                </button>
              </li>
            ) : null}
            {visibleChatRows.map((row) => (
              <li key={chatTranscriptRowKey(row)} data-chat-row-key={chatTranscriptRowKey(row)}>
                <ChatTranscriptRow
                  agentIsWorking={showThinking}
                  isSubmitting={isSubmitting || readOnly}
                  linkHandling={markdownLinkHandling}
                  onApprovalSubmit={handleApprovalSubmit}
                  liveApprovalId={liveApprovalId}
                  onOpenFile={openChangedFile}
                  onLoadDetail={loadDetail}
                  row={row}
                />
              </li>
            ))}
          </ol>
        ) : null}
      </div>
      <ChatComposer
        agent={agent}
        autoFocus={autoFocusComposer}
        disabledReason={disabledReason}
        draft={activeDraft}
        hasActionRequired={hasActionRequired}
        isExecuting={isExecutionActive}
        isInterrupting={isInterrupting}
        isSubmitting={isSubmitting}
        readOnly={readOnly}
        deferModelDiscovery={deferModelDiscovery}
        attachments={attachments}
        onAutoFocused={onComposerAutoFocused}
        onAgentConfigUpdated={onAgentConfigUpdated}
        onAttachmentsChange={setAttachments}
        onChange={setActiveDraft}
        onInterrupt={handleInterrupt}
        onSubmit={handleSubmit}
        sessionId={sessionId}
        submitError={submitError}
      />
    </section>
  );
}

function isNearTranscriptBottom(scrollRegion: HTMLElement): boolean {
  return scrollRegion.scrollHeight - scrollRegion.scrollTop - scrollRegion.clientHeight <= CHAT_SCROLL_BOTTOM_THRESHOLD_PX;
}


function ChatComposer({
  agent,
  attachments,
  autoFocus,
  disabledReason,
  draft,
  hasActionRequired,
  isExecuting,
  isInterrupting,
  isSubmitting,
  readOnly,
  deferModelDiscovery,
  onAutoFocused,
  onAgentConfigUpdated,
  onAttachmentsChange,
  onChange,
  onInterrupt,
  onSubmit,
  sessionId,
  submitError,
}: {
  agent?: Pick<AgentConfig, "session_name" | "agent_class" | "provider" | "model" | "provider_config">;
  attachments: readonly ChatAttachment[];
  autoFocus: boolean;
  disabledReason: string | null;
  draft: string;
  hasActionRequired: boolean;
  isExecuting: boolean;
  isInterrupting: boolean;
  isSubmitting: boolean;
  readOnly: boolean;
  deferModelDiscovery: boolean;
  onAutoFocused?: () => void;
  onAgentConfigUpdated?: (agent: AgentConfig) => void;
  onAttachmentsChange: (attachments: ChatAttachment[]) => void;
  onChange: (value: string) => void;
  onInterrupt: () => void;
  onSubmit: () => void;
  sessionId: string;
  submitError: string | null;
}) {
  const composerRef = useRef<HTMLFormElement>(null);
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const autoFocusConsumedRef = useRef(false);
  const [attachmentError, setAttachmentError] = useState<string | null>(null);
  const [slashDismissed, setSlashDismissed] = useState(false);
  const [slashIndex, setSlashIndex] = useState(0);
  const [isFileDragOver, setIsFileDragOver] = useState(false);
  const fileDragCounterRef = useRef(0);
  const lastFileDropRef = useRef<{ key: string; at: number } | null>(null);
  const placeholder = disabledReason ?? (hasActionRequired ? "Respond to action required..." : "Message agent...");
  const canSubmit = (draft.trim().length > 0 || attachments.length > 0) && !disabledReason;
  const isInterruptAction = isExecuting && !canSubmit;
  const slashMatches = useMemo(
    () => matchingSlashCommands(draft, agent?.provider),
    [agent?.provider, draft],
  );
  const showSlashMenu =
    !disabledReason && !slashDismissed && slashMatches.length > 0 && slashIndex < slashMatches.length;

  useEffect(() => {
    setSlashIndex(0);
  }, [slashMatches]);

  const applySlashCommand = (command: string) => {
    onChange(`${command} `);
    setSlashDismissed(false);
    textareaRef.current?.focus();
  };

  const handleComposerKeyDown = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    if (showSlashMenu) {
      if (event.key === "ArrowDown") {
        event.preventDefault();
        setSlashIndex((index) => (index + 1) % slashMatches.length);
        return;
      }
      if (event.key === "ArrowUp") {
        event.preventDefault();
        setSlashIndex((index) => (index - 1 + slashMatches.length) % slashMatches.length);
        return;
      }
      if (event.key === "Escape") {
        event.preventDefault();
        setSlashDismissed(true);
        return;
      }
      if (event.key === "Enter" || event.key === "Tab") {
        // A visible completion list owns Enter so a partially typed command
        // completes instead of submitting mid-word.
        event.preventDefault();
        applySlashCommand(slashMatches[slashIndex].command);
        return;
      }
    }
    if (shouldSubmitComposerKey(event)) {
      event.preventDefault();
      event.stopPropagation();
      if (canSubmit) onSubmit();
    }
  };

  const chooseAttachments = async () => {
    try {
      const selected = await open({
        directory: false,
        multiple: true,
        title: "Attach files to agent",
      });
      const paths = typeof selected === "string" ? [selected] : selected ?? [];
      if (paths.length === 0) return;

      const knownPaths = new Set(attachments.map((attachment) => attachment.path.toLocaleLowerCase()));
      const added = paths
        .filter((path) => !knownPaths.has(path.toLocaleLowerCase()))
        .map((path) => ({ name: fileNameFromPath(path), path }));
      if (added.length > 0) {
        setAttachmentError(null);
        onAttachmentsChange([...attachments, ...added]);
      }
    } catch (error) {
      console.warn("Failed to choose chat attachments:", error);
    }
  };

  const addAttachmentPaths = (paths: readonly string[]) => {
    const knownPaths = new Set(attachments.map((attachment) => attachment.path.toLocaleLowerCase()));
    const added = paths
      .filter((path) => path.trim() && !knownPaths.has(path.toLocaleLowerCase()))
      .map((path) => ({ name: fileNameFromPath(path), path }));
    if (added.length > 0) {
      setAttachmentError(null);
      onAttachmentsChange([...attachments, ...added]);
    }
  };

  const shouldAcceptFileDrop = (paths: readonly string[]) => {
    const key = paths.map((path) => path.toLocaleLowerCase()).join("\u0000");
    const now = Date.now();
    const previous = lastFileDropRef.current;
    if (previous?.key === key && now - previous.at < 1_000) return false;
    lastFileDropRef.current = { key, at: now };
    return true;
  };

  const resetFileDragState = () => {
    fileDragCounterRef.current = 0;
    setIsFileDragOver(false);
  };

  const handleFileDragEnter = (event: DragEvent<HTMLFormElement>) => {
    if (!hasWardianFileDropData(event.dataTransfer)) return;
    event.preventDefault();
    fileDragCounterRef.current += 1;
    setIsFileDragOver(true);
  };

  const handleFileDragLeave = (event: DragEvent<HTMLFormElement>) => {
    if (!hasWardianFileDropData(event.dataTransfer)) return;
    fileDragCounterRef.current -= 1;
    if (fileDragCounterRef.current <= 0) resetFileDragState();
  };

  const captureClipboardImage = async () => {
    try {
      const image = await readImage();
      const usedNames = new Set(attachments.map((attachment) => attachment.name));
      let index = 1;
      let name = `pasted-image-${index}.png`;
      while (usedNames.has(name)) {
        index += 1;
        name = `pasted-image-${index}.png`;
      }
      setAttachmentError(null);
      onAttachmentsChange([...attachments, { name, path: "", image }]);
    } catch (error) {
      console.warn("Failed to capture clipboard image:", error);
      setAttachmentError("Could not capture that clipboard image. Use Attach files to choose an image instead.");
    }
  };

  const dataTransferHasImage = (dataTransfer: DataTransfer): boolean => {
    const files = Array.from(dataTransfer.files ?? []);
    if (files.some((file) => file.type.toLowerCase().startsWith("image/"))) return true;
    return Array.from(dataTransfer.items ?? []).some(
      (item) => item.kind === "file" && item.type.toLowerCase().startsWith("image/"),
    );
  };

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | null = null;
    listen<{ paths?: string[]; position?: { x?: number; y?: number } }>("tauri://drag-drop", (event) => {
      if (disposed || disabledReason || isSubmitting || !event.payload?.paths?.length) return;
      const position = event.payload.position;
      const bounds = composerRef.current?.getBoundingClientRect();
      if (position && bounds && isNativeFileDropInsideBounds(position, bounds, window.devicePixelRatio)) {
        if (!shouldAcceptFileDrop(event.payload.paths)) return;
        addAttachmentPaths(event.payload.paths);
        resetFileDragState();
      }
    }).then((dispose) => {
      if (disposed) dispose();
      else unlisten = dispose;
    }).catch((error) => console.warn("Failed to listen for chat file drops:", error));
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [attachments, disabledReason, isSubmitting]);

  useEffect(() => {
    const reset = () => resetFileDragState();
    document.addEventListener("drop", reset, true);
    document.addEventListener("dragend", reset, true);
    return () => {
      document.removeEventListener("drop", reset, true);
      document.removeEventListener("dragend", reset, true);
    };
  }, []);

  useEffect(() => {
    if (!autoFocus) {
      autoFocusConsumedRef.current = false;
      return;
    }
    if (!disabledReason && !autoFocusConsumedRef.current) {
      textareaRef.current?.focus();
      autoFocusConsumedRef.current = true;
      onAutoFocused?.();
    }
  }, [autoFocus, disabledReason, onAutoFocused]);

  useLayoutEffect(() => {
    const textarea = textareaRef.current;
    if (!textarea) return;
    textarea.style.height = "0px";
    const maxHeight = 112;
    const nextHeight = draft.length > 0 ? Math.min(textarea.scrollHeight, maxHeight) : 28;
    textarea.style.height = `${Math.max(nextHeight, 28)}px`;
    textarea.style.overflowY = textarea.scrollHeight > maxHeight ? "auto" : "hidden";
  }, [draft]);

  return (
    <form
      className={`chat-composer relative mx-2.5 mb-2.5 rounded-xl border bg-[var(--color-wardian-input-bg)] px-3 pb-1 pt-1.5 shadow-sm ${
        isFileDragOver
          ? "border-[var(--color-wardian-accent)] bg-[var(--color-wardian-accent)]/10"
          : "border-wardian-light"
      }`}
      data-testid="chat-composer"
      ref={composerRef}
      onDragEnter={handleFileDragEnter}
      onDragOver={(event) => {
        if (hasWardianFileDropData(event.dataTransfer)) {
          event.preventDefault();
          event.dataTransfer.dropEffect = "copy";
        }
      }}
      onDragLeave={handleFileDragLeave}
      onDrop={(event) => {
        const paths = getDroppedFilePaths(event.dataTransfer);
        resetFileDragState();
        if (paths.length > 0 && shouldAcceptFileDrop(paths)) {
          event.preventDefault();
          addAttachmentPaths(paths);
        }
      }}
      onPaste={(event) => {
        const paths = getDroppedFilePaths(event.clipboardData);
        if (paths.length > 0) {
          event.preventDefault();
          addAttachmentPaths(paths);
        } else if (dataTransferHasImage(event.clipboardData)) {
          event.preventDefault();
          void captureClipboardImage();
        }
      }}
      onSubmit={(event) => {
        event.preventDefault();
        onSubmit();
      }}
    >
      {attachments.length > 0 ? (
        <div className="mb-2 flex flex-wrap gap-1.5" aria-label="Attached files">
          {attachments.map((attachment) => (
            <span
              className="chat-attachment-chip inline-flex max-w-full items-center gap-1 rounded border border-wardian-light bg-[var(--color-wardian-card-bg-muted)] py-1 pl-2 pr-1 text-[11px] text-primary"
              key={`${attachment.path || "clipboard-image"}:${attachment.name}`}
              title={attachment.path || "Pasted image"}
            >
              {attachment.image ? (
                <ImageIcon className="h-3 w-3 shrink-0 text-muted-neutral" aria-hidden="true" />
              ) : (
                <FileText className="h-3 w-3 shrink-0 text-muted-neutral" aria-hidden="true" />
              )}
              <span className="max-w-[20ch] truncate">{attachment.name}</span>
              <button
                aria-label={`Remove ${attachment.name}`}
                className="rounded p-0.5 text-muted-neutral hover:bg-[var(--color-wardian-card)] hover:text-primary"
                disabled={Boolean(disabledReason) || isSubmitting}
                onClick={() => {
                  const identity = `${attachment.path || "clipboard-image"}:${attachment.name}`;
                  onAttachmentsChange(
                    attachments.filter((item) => `${item.path || "clipboard-image"}:${item.name}` !== identity),
                  );
                }}
                type="button"
              >
                <X className="h-3 w-3" aria-hidden="true" />
              </button>
            </span>
          ))}
        </div>
      ) : null}
      {attachmentError ? (
        <div className="mb-1 text-[11px] leading-4 text-[var(--color-wardian-error)]" role="alert">
          {attachmentError}
        </div>
      ) : null}
      {showSlashMenu ? (
        <ul
          aria-label="Slash commands"
          className="chat-slash-menu wardian-menu p-1"
          role="listbox"
        >
          {slashMatches.map((entry, index) => (
            <li key={entry.command}>
              <button
                type="button"
                role="option"
                aria-selected={index === slashIndex}
                className={`flex w-full items-baseline gap-2 rounded px-2 py-1.5 text-left text-[12px] leading-4 ${
                  index === slashIndex
                    ? "bg-[var(--color-wardian-card-bg-muted)] text-primary"
                    : "text-muted-neutral hover:text-primary"
                }`}
                onMouseDown={(mouseEvent) => {
                  mouseEvent.preventDefault();
                  applySlashCommand(entry.command);
                }}
              >
                <span className="shrink-0 font-mono font-semibold">{entry.command}</span>
                <span className="min-w-0 truncate">{entry.description}</span>
              </button>
            </li>
          ))}
        </ul>
      ) : null}
      <textarea
        aria-label="Message agent"
        aria-expanded={showSlashMenu}
        className="max-h-28 min-h-7 w-full resize-none bg-transparent px-0 py-0 text-[13px] leading-5 text-primary outline-none placeholder:text-muted-neutral disabled:cursor-not-allowed disabled:opacity-70"
        disabled={Boolean(disabledReason)}
        onChange={(event) => {
          setSlashDismissed(false);
          onChange(event.target.value);
        }}
        onKeyDown={handleComposerKeyDown}
        placeholder={placeholder}
        ref={textareaRef}
        rows={1}
        value={draft}
      />
      <div className="mt-1.5 flex min-h-7 flex-wrap items-center justify-between gap-x-2 gap-y-1">
        <div className="flex min-w-0 items-center gap-1">
        <button
          aria-label="Attach files"
          className="inline-flex h-7 w-7 shrink-0 items-center justify-center rounded-lg text-muted-neutral transition-colors hover:bg-[var(--color-wardian-card-bg-muted)] hover:text-primary disabled:cursor-not-allowed disabled:opacity-60"
          disabled={Boolean(disabledReason) || isSubmitting}
          onClick={() => void chooseAttachments()}
          title="Attach files"
          type="button"
        >
          <Plus className="h-4 w-4" aria-hidden="true" />
        </button>
        {hasActionRequired ? (
          <span className="inline-flex min-w-0 items-center gap-1 rounded-md px-1.5 text-[11px] text-muted-neutral" title="Agent is waiting for your approval">
            <Hand className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
            <span className="truncate">Ask for approval</span>
          </span>
        ) : null}
        </div>
        <div className="ml-auto flex min-w-0 items-center gap-1">
          <ChatModelSelection
            agent={agent}
            readOnly={readOnly}
            deferDiscovery={deferModelDiscovery}
            onAgentConfigUpdated={onAgentConfigUpdated}
            sessionId={sessionId}
          />
          <button
            aria-label={isInterrupting ? "Interrupting agent" : isSubmitting ? "Sending message" : isInterruptAction ? "Interrupt agent" : isExecuting ? "Queue message" : "Send message"}
            className="inline-flex h-7 w-7 shrink-0 items-center justify-center rounded-lg border border-[var(--color-wardian-accent)] bg-[var(--color-wardian-accent)] text-[var(--color-wardian-accent-contrast)] transition-colors hover:opacity-85 disabled:cursor-not-allowed disabled:border-transparent disabled:bg-transparent disabled:text-[var(--color-wardian-text-muted-neutral)] disabled:opacity-50"
            disabled={Boolean(disabledReason) || isInterrupting || isSubmitting || (!isExecuting && !canSubmit)}
            onClick={isInterruptAction ? onInterrupt : undefined}
            title={isInterruptAction ? "Interrupt agent" : isSubmitting ? "Sending message" : isExecuting ? "Queue message" : "Send message"}
            type={isInterruptAction ? "button" : "submit"}
          >
            {isInterrupting || isSubmitting ? (
              <Loader2 className="h-4 w-4 animate-spin" aria-hidden="true" />
            ) : isInterruptAction ? (
              <Square className="h-3.5 w-3.5 fill-current" aria-hidden="true" />
            ) : (
              <SendHorizontal className="h-4 w-4" aria-hidden="true" />
            )}
          </button>
        </div>
      </div>
      {submitError ? (
        <div className="mt-1 text-[11px] leading-4 text-[var(--color-wardian-error)]" role="alert">
          {submitError}
        </div>
      ) : null}
    </form>
  );
}

function ChatModelSelection({
  agent,
  readOnly,
  deferDiscovery,
  onAgentConfigUpdated,
  sessionId,
}: {
  agent?: Pick<AgentConfig, "session_name" | "agent_class" | "provider" | "model" | "provider_config">;
  readOnly: boolean;
  deferDiscovery: boolean;
  onAgentConfigUpdated?: (agent: AgentConfig) => void;
  sessionId: string;
}) {
  const provider = agent?.provider;
  const configuredEffort = reasoningEffortForConfig(agent ?? {});
  const [selection, setSelection] = useState<ModelSelection>(() => ({
    model: agent?.model,
    reasoning_effort: configuredEffort,
  }));
  const [saveError, setSaveError] = useState<string | null>(null);
  const [saveNotice, setSaveNotice] = useState<string | null>(null);
  const [isSaving, setIsSaving] = useState(false);
  // Rollback target for a failed save. A ref rather than render-scope state:
  // two rapid changes must roll back to the last persisted value, not to a
  // snapshot the first change already superseded.
  const selectionRef = useRef(selection);

  useEffect(() => {
    const nextSelection = {
      model: agent?.model,
      reasoning_effort: configuredEffort,
    };
    selectionRef.current = nextSelection;
    setSelection(nextSelection);
    setSaveError(null);
    setSaveNotice(null);
  }, [agent?.model, configuredEffort, sessionId]);

  if (!provider?.trim()) return null;

  const saveSelection = async (nextSelection: ModelSelection) => {
    if (readOnly) return;
    const previousSelection = selectionRef.current;
    selectionRef.current = nextSelection;
    let persisted = false;
    setSelection(nextSelection);
    setSaveError(null);
    setSaveNotice(null);
    setIsSaving(true);
    try {
      const result = await invoke<AgentModelSelectionUpdateResult>("update_agent_model_selection", {
        sessionId,
        model: nextSelection.model ?? null,
        reasoningEffort: nextSelection.reasoning_effort ?? null,
      });
      const saved = result.config;
      const savedSelection = {
        model: saved.model,
        reasoning_effort: reasoningEffortForConfig(saved),
      };
      selectionRef.current = savedSelection;
      setSelection(savedSelection);
      onAgentConfigUpdated?.(saved);
      persisted = true;
      if (result.live_application === "failed") {
        setSaveError(`Saved, but the live model could not be changed: ${result.live_error ?? "Codex did not confirm the selection."}`);
      } else if (result.live_application === "unknown") {
        setSaveError(`Saved, but the live model change is unconfirmed: ${result.live_error ?? "The provider runtime changed or stopped before acknowledgement."}`);
      } else if (result.live_application === "deferred") {
        setSaveNotice("Saved for the next start or restart.");
      } else if (result.model?.intent === "default" || result.reasoning_effort?.intent === "default") {
        const effectiveModel = result.model?.effective_value ?? "provider default";
        const effectiveEffort = result.reasoning_effort?.effective_value ?? "provider default";
        setSaveNotice(`Saved as provider defaults. Live runtime accepted ${effectiveModel} / ${effectiveEffort} for future turns.`);
      }
    } catch (reason) {
      if (!persisted) {
        selectionRef.current = previousSelection;
        setSelection(previousSelection);
      }
      setSaveError(persisted ? `Saved, but the live model could not be changed: ${errorMessage(reason)}` : errorMessage(reason));
    } finally {
      setIsSaving(false);
    }
  };

  return (
    <div className="min-w-0 shrink-0">
      <ProviderModelSelector
        compact
        deferDiscovery={readOnly || deferDiscovery}
        disabled={readOnly || isSaving}
        idPrefix={`chat-${sessionId}`}
        provider={provider}
        selection={selection}
        onSelectionChange={(nextSelection) => void saveSelection(nextSelection)}
      />
      {isSaving ? <span className="shrink-0 text-[10px] text-muted-neutral" role="status">Applying model…</span> : null}
      {saveNotice ? <p className="mt-1 text-[10px] text-muted-neutral" role="status">{saveNotice}</p> : null}
      {saveError ? <p className="mt-1 text-[10px] text-wardian-error" role="alert">{saveError}</p> : null}
    </div>
  );
}

function shouldSubmitComposerKey(event: KeyboardEvent<HTMLTextAreaElement>): boolean {
  if (event.shiftKey || event.nativeEvent.isComposing) return false;
  return event.key === "Enter" || event.key === "NumpadEnter" || event.code === "Enter" || event.code === "NumpadEnter";
}

function LoadingState() {
  return (
    <div className="flex h-full min-h-[160px] items-center justify-center text-[13px] text-muted-neutral">
      Loading transcript...
    </div>
  );
}

function WaitingState({ compact = false }: { compact?: boolean }) {
  return (
    <div role="status" className={`flex flex-col items-center justify-center gap-3 text-center ${compact ? "mb-2 px-3 py-2" : "h-full min-h-[160px]"}`}>
      <div>
        <div className="text-[13px] font-semibold text-primary">Waiting for transcript read</div>
        <div className="mt-1 max-w-[42ch] text-[12px] leading-5 text-muted-neutral">
          The transcript read is still running after 30 seconds. Waiting for it to settle.
        </div>
      </div>
    </div>
  );
}

function EmptyState() {
  return (
    <div className="flex h-full min-h-[160px] flex-col items-center justify-center gap-1 text-center">
      <div className="text-[13px] font-semibold text-primary">No chat transcript yet</div>
      <div className="max-w-[32ch] text-[12px] leading-5 text-muted-neutral">
        Messages and agent activity will appear here when the provider exposes normalized events.
      </div>
    </div>
  );
}

function ErrorState({ error, onRetry, compact = false }: { error: string | null; onRetry: () => void; compact?: boolean }) {
  return (
    <div role="alert" className={`flex flex-col items-center justify-center gap-3 text-center ${compact ? "mb-2 rounded border border-wardian-error/40 px-3 py-2" : "h-full min-h-[160px]"}`}>
      <div>
        <div className="text-[13px] font-semibold text-[var(--color-wardian-error)]">Unable to load transcript</div>
        <div className="mt-1 max-w-[42ch] text-[12px] leading-5 text-muted-neutral">{error ?? "The transcript command failed."}</div>
      </div>
      <button
        type="button"
        className="rounded border border-wardian-light px-3 py-1.5 text-[12px] font-semibold text-primary hover:border-[var(--color-wardian-accent)]"
        onClick={onRetry}
      >
        Retry
      </button>
    </div>
  );
}

function appendThinkingIndicator(
  events: AgentChatEvent[],
  sessionId: string,
  provider: string,
  showThinking: boolean,
): AgentChatEvent[] {
  if (!showThinking) return events;

  const sequence = pendingSequence(events, 0);
  return [
    ...events,
    {
      id: `thinking-${sessionId}`,
      session_id: sessionId,
      provider,
      kind: "status",
      role: null,
      text: "Working...",
      title: "Working...",
      status: "processing",
      turn_id: null,
      source: "chat_ui",
      command: null,
      exit_code: null,
      path: null,
      language: null,
      created_at: null,
      sequence,
      metadata: { chat_thinking_indicator: true },
    },
  ];
}


function clearAwaitingResponseWhenAnswered(
  events: AgentChatEvent[],
  marker: AwaitingResponseMarker | null,
): AwaitingResponseMarker | null {
  if (!marker) return null;
  return responseEvents(events).some((event) => !marker.response_ids.has(event.id) && event.metadata.chat_older_header !== true) ? null : marker;
}

function pendingSequence(events: AgentChatEvent[], offset: number): number {
  return maxSequence(events) + offset + 1;
}

function maxSequence(events: AgentChatEvent[]): number {
  return events.reduce((max, event) => (typeof event.sequence === "number" ? Math.max(max, event.sequence) : max), 0);
}

function providerFromEvents(events: AgentChatEvent[]): string {
  return events.find((event) => event.provider)?.provider ?? "unknown";
}

function responseEvents(events: AgentChatEvent[]): AgentChatEvent[] {
  return events.filter((event) => {
    if (event.kind === "message") return event.role === "assistant" || event.role === "system" || event.role === "tool";
    return event.kind === "tool_call" || event.kind === "tool_result" || event.kind === "approval" || event.kind === "terminal_output" || event.kind === "error";
  });
}

function inputDisabledReason(isSubmitting: boolean): string | null {
  if (isSubmitting) return "Sending...";
  return null;
}

function errorMessage(reason: unknown): string {
  if (reason instanceof Error) return reason.message;
  if (typeof reason === "string") return reason;
  return "The transcript command failed.";
}
