import React, { useState, useEffect, useLayoutEffect, useRef, useCallback, useMemo } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { AgentConfig, AgentTelemetry, AgentClassDefinition } from "../types";
import type { AgentChatEvent, CloneMode, OpenSurfaceRequest, WorkbenchShellV1 } from "../types";
import "../styles/App.css";

import AgentWatchlist from "../layout/watchlist/AgentWatchlist";
import { deriveCurrentThought, getStatusColorClass } from "../utils/statusUtils";
import {
  getAgentsForList,
  addAgentsToList,
  removeAgentsFromList,
  normalizeWatchlistState,
  createTeamFromAgents,
  ungroupTeam,
  addAgentToTeam,
  removeAgentFromTeam,
  removeAgentFromTeamAtEntry,
  reorderTeamMember,
  removeDeletedAgentsFromWatchlistState,
} from "../layout/watchlist/watchlistUtils";
import type { Watchlist, WatchlistPrefs, AgentInteractions, AgentTeam, WatchlistState, WatchlistEntry } from "../layout/watchlist/types";
import {
  DEFAULT_DASHBOARD_PREFS,
  mergeDashboardPrefs,
  type DashboardPrefs,
} from "../features/telemetry/dashboardColumns";
import { DEFAULT_WATCHLIST_PREFS } from "../layout/watchlist/types";

import { ErrorBoundary } from "../components/ErrorBoundary";
import { OnboardingTour, OnboardingWelcome } from "../components/OnboardingTour";
import { useConfirm } from "../components/ConfirmDialog";
import { SidebarIconRail, SidebarTab } from "../layout/SidebarIconRail";
import { SidebarContentPane } from "../layout/SidebarContentPane";
import { CustomTitleBar } from "../layout/titlebar/CustomTitleBar";
import { UserTerminalPanel } from "../features/terminal/UserTerminalPanel";
import { useTerminalApplicationVisibility } from "../features/terminal/useTerminalApplicationVisibility";
import { SettingsModal } from "../features/settings/SettingsModal";
import { UpdateAvailableNotice } from "../features/settings/UpdateAvailableNotice";
import { useAppUpdate } from "../features/settings/useAppUpdate";
import { useSelectedAgentGitStatus } from "../features/git/useSelectedAgentGitStatus";
import { useQueueStore } from "../store/useQueueStore";
import { completionPreviewFromTranscript } from "../features/queue/completionPreview";
import { useLibraryStore } from "../store/useLibraryStore";
import { useSettingsStore } from "../store/useSettingsStore";
import { useLayoutStore } from "../store/useLayoutStore";
import { useOnboardingStore } from "../store/useOnboardingStore";
import { submitInboxProviderChoice, submitInputToAgent, submitInputToAgents } from "../utils/terminalInput";
import { CustomCloneModal } from "../features/agents/CustomCloneModal";
import { WorkbenchConflictDialog } from "../features/workbench/WorkbenchConflictDialog";
import { createWorkbenchInvokeAdapter } from "../features/workbench/workbenchPersistence";
import { useWorkbenchPersistence } from "../features/workbench/useWorkbenchPersistence";
import {
  canSplitWorkbenchGroup,
  WorkbenchHost,
} from "../layout/workbench/WorkbenchHost";
import { AppShell } from "../layout/AppShell";
import { AgentResourceContext } from "../features/agents/AgentResourceContext";
import {
  useAgentResourceController,
  type AgentStatusTransition,
  type AgentTurnCompletion,
} from "../features/agents/useAgentResourceController";
import { RosterProvider } from "../features/agents/RosterContext";
import { useRosterController } from "../features/agents/useRosterController";
import {
  AgentsOverviewSurface,
  normalizeAgentsOverviewSurfaceState,
  revealAgentInOverviewState,
} from "../features/workbench/surfaces/AgentsOverviewSurface";
import { createCoreWorkbenchSurfaceRegistry } from "../features/workbench/coreSurfaceRegistry";
import { findExistingSurface } from "../features/workbench/adjacentSurfaceTargeting";
import { createWorkbenchNavigationService } from "../features/workbench/navigationService";
import { SurfaceRecoveryPlaceholder } from "../features/workbench/SurfaceRecoveryPlaceholder";
import { AgentSessionSurface } from "../features/workbench/surfaces/AgentSessionSurface";
import {
  AnalyticsSurface,
  DashboardSurface,
  GardenSurface,
  GraphSurface,
  InboxSurface,
  normalizeAnalyticsSurfaceState,
  normalizeDashboardSurfaceState,
  normalizeGardenSurfaceState,
  surfaceConfiguredDashboard,
  normalizeGraphSurfaceState,
} from "../features/workbench/surfaces/coreSurfaceDefinitions";
import type { WorkbenchSurfaceRenderer } from "../layout/workbench/DockviewLayoutAdapter";
import { LibrarySurface } from "../features/workbench/surfaces/LibrarySurface";
import { WorkflowsSurface } from "../features/workbench/surfaces/WorkflowsSurface";
import { useDirtySurfacePrompt } from "../features/workbench/surfaces/DirtySurfacePromptDialog";
import { FilesSurface } from "../features/files/FilesSurface";
import { BrowserSurface } from "../features/browser/BrowserSurface";
import {
  closeBrowserSession,
  openBrowserSession,
  reopenBrowserSurfaceSession,
  subscribeToBrowserSurfaceOpens,
} from "../features/browser/browserSessionClient";
import { FileEditorControllerRegistry } from "../features/files/fileEditorController";
import { createFilesCloseAdapter } from "../features/files/filesCloseAdapter";
import { fileResourceClient } from "../features/files/fileResourceClient";
import { fileResourceKey } from "../features/files/fileResourceKey";
import { openPermanentFileSurface } from "../features/files/fileSurfaceNavigation";
import {
  fileOpenDestinationForKind,
  fileOpenKindForDescriptor,
  openFileWithSettings,
} from "../features/files/fileOpenRouting";
import { useArtifactEvents } from "../features/files/useArtifactEvents";
import {
  filesSurfaceMigrationCommands,
  isFilesSurfaceStateV1,
} from "../features/files/filesSurfaceState";
import type {
  BrowserSurfaceState,
  BrowserViewport,
  FilesSurfaceStateV2,
  OpenFileResourceRequestV1,
} from "../types";
import type { SurfaceResourceProvision } from "../features/workbench/coreSurfaceRegistry";

declare global {
  interface Window {
    __wardianAppDebug?: {
      snapshot: (sessionId: string) => {
        title: string;
        thought: string;
        metrics: AgentTelemetry | null;
        derivedStatus: string;
      } | null;
    };
  }
}


function normalizeCollapsedTeamIdsByList(value: unknown): Record<string, string[]> {
  if (!value || typeof value !== "object" || Array.isArray(value)) return {};

  return Object.fromEntries(
    Object.entries(value).flatMap(([listId, teamIds]) => {
      if (!Array.isArray(teamIds)) return [];
      return [[listId, teamIds.filter((teamId): teamId is string => typeof teamId === "string")]];
    }),
  );
}

async function canonicalizeFileOpenPath(
  path: string,
  sourceRequest?: OpenFileResourceRequestV1,
) {
  const snapshot = await fileResourceClient.open({
    ...(sourceRequest ?? {
      path,
      agent_id: null,
      user_file_capability_id: null,
    }),
    path,
  });
  try {
    return {
      path: snapshot.descriptor.canonical_path,
      kind: fileOpenKindForDescriptor(snapshot.descriptor),
      renderer_kind: snapshot.descriptor.renderer_kind,
    };
  } finally {
    await fileResourceClient.close(snapshot.subscription_id).catch(() => undefined);
  }
}

const NATIVE_WINDOW_WIDTH_VAR = "--wardian-native-window-width";
const NATIVE_WINDOW_HEIGHT_VAR = "--wardian-native-window-height";
const OUTER_WINDOW_FALLBACK_COOLDOWN_MS = 1_000;
const MIN_NATIVE_WINDOW_WIDTH_PX = 320;
const MIN_NATIVE_WINDOW_HEIGHT_PX = 240;
const WORKBENCH_PERSISTENCE_ADAPTER = createWorkbenchInvokeAdapter(
  (command, args) => invoke(command, args),
);
const WORKBENCH_SHELL_PROJECTION = {
  read: () => {
    const layout = useLayoutStore.getState();
    return {
      left_sidebar_collapsed: layout.leftSidebarCollapsed,
      left_sidebar_width: layout.leftSidebarWidth,
      right_sidebar_collapsed: layout.rightSidebarCollapsed,
      right_sidebar_width: layout.rightSidebarWidth,
      bottom_terminal_open: layout.userTerminalOpen,
      bottom_terminal_height: layout.userTerminalHeight,
    };
  },
  write: (shell: WorkbenchShellV1) => {
    useLayoutStore.setState({
      leftSidebarCollapsed: shell.left_sidebar_collapsed,
      leftSidebarWidth: shell.left_sidebar_width,
      rightSidebarCollapsed: shell.right_sidebar_collapsed,
      rightSidebarWidth: shell.right_sidebar_width,
      userTerminalOpen: shell.bottom_terminal_open,
      userTerminalHeight: shell.bottom_terminal_height,
    });
  },
  subscribe: (listener: () => void) => useLayoutStore.subscribe((state, previous) => {
    if (
      state.leftSidebarCollapsed !== previous.leftSidebarCollapsed
      || state.leftSidebarWidth !== previous.leftSidebarWidth
      || state.rightSidebarCollapsed !== previous.rightSidebarCollapsed
      || state.rightSidebarWidth !== previous.rightSidebarWidth
      || state.userTerminalOpen !== previous.userTerminalOpen
      || state.userTerminalHeight !== previous.userTerminalHeight
    ) listener();
  }),
};

type NativeWindowResizePayload = {
  width?: number;
  height?: number;
};

type WorkflowInboxUpdate = {
  workflow_id: string;
  run_instance_id: string;
  workflow_name: string;
  status: "awaiting_approval" | "completed" | "failed";
  error?: string;
  summary?: string;
};

type NativeWindowCssSize = {
  width: string;
  height: string;
};

let syntheticNativeResizeDepth = 0;
let lastNativeResizePayloadAtMs = 0;
let nativeResizeDispatchFrame: number | null = null;
let pendingNativeResizeNotification: NativeWindowCssSize | null = null;

function toCssPixelValue(value: number | undefined) {
  if (typeof value !== "number" || !Number.isFinite(value) || value <= 0) return null;
  const deviceScale = window.devicePixelRatio > 0 ? window.devicePixelRatio : 1;
  return Math.max(1, Math.round(value / deviceScale));
}

function toCssPixelLength(value: number) {
  return `${value}px`;
}

function hasTauriGlobal() {
  const tauriWindow = window as { __TAURI__?: unknown; __TAURI_INTERNALS__?: unknown };
  return Boolean(tauriWindow.__TAURI__ || tauriWindow.__TAURI_INTERNALS__);
}

function dispatchPendingNativeResizeNotification() {
  nativeResizeDispatchFrame = null;
  const size = pendingNativeResizeNotification;
  pendingNativeResizeNotification = null;
  if (!size) return;

  syntheticNativeResizeDepth += 1;
  try {
    window.dispatchEvent(new Event("resize"));
  } finally {
    syntheticNativeResizeDepth -= 1;
  }
  window.dispatchEvent(new CustomEvent("wardian-native-window-resized", {
    detail: size,
  }));
}

function scheduleNativeResizeNotification(size: NativeWindowCssSize) {
  pendingNativeResizeNotification = size;
  if (nativeResizeDispatchFrame !== null) return;

  if (typeof window.requestAnimationFrame === "function") {
    nativeResizeDispatchFrame = window.requestAnimationFrame(dispatchPendingNativeResizeNotification);
  } else {
    nativeResizeDispatchFrame = window.setTimeout(dispatchPendingNativeResizeNotification, 0);
  }
}

function cancelPendingNativeResizeNotification() {
  if (nativeResizeDispatchFrame === null) return;
  if (typeof window.cancelAnimationFrame === "function") {
    window.cancelAnimationFrame(nativeResizeDispatchFrame);
  } else {
    window.clearTimeout(nativeResizeDispatchFrame);
  }
  nativeResizeDispatchFrame = null;
  pendingNativeResizeNotification = null;
}

function setNativeWindowCssSize(size: NativeWindowCssSize) {
  const root = document.documentElement;
  const currentWidth = root.style.getPropertyValue(NATIVE_WINDOW_WIDTH_VAR);
  const currentHeight = root.style.getPropertyValue(NATIVE_WINDOW_HEIGHT_VAR);
  if (currentWidth === size.width && currentHeight === size.height) return;

  root.style.setProperty(NATIVE_WINDOW_WIDTH_VAR, size.width);
  root.style.setProperty(NATIVE_WINDOW_HEIGHT_VAR, size.height);
  scheduleNativeResizeNotification(size);
}

function applyNativeWindowSizeFromPayload(payload: NativeWindowResizePayload | undefined) {
  const width = toCssPixelValue(payload?.width);
  const height = toCssPixelValue(payload?.height);
  if (width && height) {
    // A valid native payload, including a transient minimized size, is
    // authoritative. Do not let the outer-window fallback publish a second
    // geometry while the native resize stream is settling.
    lastNativeResizePayloadAtMs = Date.now();
  }
  if (!width || !height) return false;
  if (width < MIN_NATIVE_WINDOW_WIDTH_PX || height < MIN_NATIVE_WINDOW_HEIGHT_PX) return false;

  setNativeWindowCssSize({ width: toCssPixelLength(width), height: toCssPixelLength(height) });
  return true;
}

function applyNativeWindowSizeFromOuterWindow() {
  if (syntheticNativeResizeDepth > 0) return;
  if (Date.now() - lastNativeResizePayloadAtMs < OUTER_WINDOW_FALLBACK_COOLDOWN_MS) return;
  if (!hasTauriGlobal()) return;

  const width = window.outerWidth;
  const height = window.outerHeight;
  if (!Number.isFinite(width) || !Number.isFinite(height) || width <= 0 || height <= 0) return;

  setNativeWindowCssSize({
    width: `${Math.max(1, Math.round(width))}px`,
    height: `${Math.max(1, Math.round(height))}px`,
  });
}

/** Scrolls only the Agents surface viewport, never Dockview's pane wrapper. */
export function scrollAgentCardWithinOverview(
  agentId: string,
  ownerDocument: Document = document,
): boolean {
  const card = ownerDocument.getElementById(`agent-card-${agentId}`);
  const scrollRegion = card?.closest<HTMLElement>('[data-testid="agents-overview-container"]');
  if (!card || !scrollRegion) return false;

  const cardBounds = card.getBoundingClientRect();
  const regionBounds = scrollRegion.getBoundingClientRect();
  const centeredTop = scrollRegion.scrollTop
    + cardBounds.top
    - regionBounds.top
    - ((scrollRegion.clientHeight - cardBounds.height) / 2);
  const top = Math.max(0, Math.min(
    centeredTop,
    Math.max(0, scrollRegion.scrollHeight - scrollRegion.clientHeight),
  ));
  if (typeof scrollRegion.scrollTo === "function") {
    scrollRegion.scrollTo({ top, behavior: "smooth" });
  } else {
    scrollRegion.scrollTop = top;
  }
  return true;
}

function App() {
  useTerminalApplicationVisibility();
  return (
    <ErrorBoundary>
      <AppBody />
    </ErrorBoundary>
  );
}

function AppBody() {
  const confirm = useConfirm();
  const pendingQueueFlushRef = React.useRef<Set<string>>(new Set());
  const workbenchRootRef = useRef<HTMLDivElement>(null);
  const sidebarIconRailRef = useRef<HTMLElement>(null);
  const agentWatchlistRef = useRef<HTMLElement>(null);
  const workbenchPersistence = useWorkbenchPersistence({
    enabled: true,
    adapter: WORKBENCH_PERSISTENCE_ADAPTER,
    legacy_storage: window.localStorage,
    viewport: () => ({ width: window.innerWidth, height: window.innerHeight }),
    shell_projection: WORKBENCH_SHELL_PROJECTION,
  });
  const workbenchResetPending = workbenchPersistence.store.getState().reset_pending;
  const dirtySurfacePrompt = useDirtySurfacePrompt();
  const filesEditorRegistry = useMemo(
    () => new FileEditorControllerRegistry(fileResourceClient),
    [],
  );
  const workbenchRegistry = useMemo(() => createCoreWorkbenchSurfaceRegistry({
    dirty_surface_prompt: dirtySurfacePrompt.prompt,
    files_close_adapter: createFilesCloseAdapter(
      filesEditorRegistry,
      dirtySurfacePrompt.prompt,
    ),
  }), [dirtySurfacePrompt.prompt, filesEditorRegistry]);
  useEffect(() => {
    if (workbenchPersistence.status !== "ready") return;
    const commands = filesSurfaceMigrationCommands(
      workbenchPersistence.store.getState().document,
    );
    if (commands.length === 0) return;
    const result = workbenchPersistence.store.getState().apply_commands(commands);
    if (result.accepted) void workbenchPersistence.flush();
  }, [
    workbenchPersistence.flush,
    workbenchPersistence.status,
    workbenchPersistence.store,
  ]);
  const [, setSurfaceRecoveryAttempt] = useState(0);
  const workbenchNavigation = useMemo(
    () => createWorkbenchNavigationService({
      registry: workbenchRegistry,
      store: workbenchPersistence.store,
      can_split_group: (groupId, direction) => canSplitWorkbenchGroup(
        workbenchRootRef.current,
        groupId,
        direction,
      ),
      reset_document: workbenchPersistence.reset,
    }),
    [workbenchPersistence.reset, workbenchPersistence.store, workbenchRegistry],
  );
  useArtifactEvents(workbenchNavigation, workbenchPersistence.status === "ready");
  const libraryNavigationRequest = useLibraryStore((s) => s.navigationRequest);
  const seenLibraryNavigationRequestRef = useRef(libraryNavigationRequest);
  const appendAgentEvent = useQueueStore((s) => s.appendAgentEvent);
  const flushAgentCompletion = useQueueStore((s) => s.flushAgentCompletion);
  const addActionNeeded = useQueueStore((s) => s.addActionNeeded);
  const addWorkflowCompletion = useQueueStore((s) => s.addWorkflowCompletion);
  const loadQueueItems = useQueueStore((s) => s.loadItems);
  const loadQueuePreferences = useQueueStore((s) => s.loadPreferences);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    let outerPollId: number | undefined;

    try {
      getCurrentWindow().onResized((event) => {
        const appliedPayload = applyNativeWindowSizeFromPayload(event.payload as NativeWindowResizePayload | undefined);
        if (!appliedPayload) {
          applyNativeWindowSizeFromOuterWindow();
        }
      }).then((cleanup) => {
        if (disposed) {
          cleanup();
          return;
        }
        unlisten = cleanup;
      }).catch((error) => {
        console.warn("Failed to listen for native window resize", error);
      });
    } catch (error) {
      console.warn("Failed to listen for native window resize", error);
    }

    applyNativeWindowSizeFromOuterWindow();
    window.addEventListener("resize", applyNativeWindowSizeFromOuterWindow);
    outerPollId = window.setInterval(applyNativeWindowSizeFromOuterWindow, 250);

    return () => {
      disposed = true;
      unlisten?.();
      window.removeEventListener("resize", applyNativeWindowSizeFromOuterWindow);
      if (outerPollId !== undefined) {
        window.clearInterval(outerPollId);
      }
      cancelPendingNativeResizeNotification();
      lastNativeResizePayloadAtMs = 0;
      document.documentElement.style.removeProperty(NATIVE_WINDOW_WIDTH_VAR);
      document.documentElement.style.removeProperty(NATIVE_WINDOW_HEIGHT_VAR);
    };
  }, []);

  const handleAgentTurnCompletion = useCallback((
    completion: AgentTurnCompletion,
  ) => {
    setChangeReviewTurnRevision((revision) => revision + 1);
    const { session_id: sessionId, agent } = completion;
    const agentName = agent?.session_name.trim();
    // Never emit a durable notification with a session UUID while the roster
    // is still loading. A later config load intentionally does not replay it.
    if (!agentName || pendingQueueFlushRef.current.has(sessionId)) return;

    pendingQueueFlushRef.current.add(sessionId);
    invoke<AgentChatEvent[]>("load_agent_chat_transcript", { sessionId })
      .then(completionPreviewFromTranscript)
      .then((preview) => {
        if (preview) {
          flushAgentCompletion(sessionId, agentName, preview.summary, preview.evidence_id);
        }
      })
      .catch(() => undefined)
      .finally(() => pendingQueueFlushRef.current.delete(sessionId));
  }, [flushAgentCompletion]);

  const maybeAddActionNeededQueueItem = useCallback((
    sessionId: string,
    currentStatus: string,
    previousStatus: string | undefined,
    agent: AgentConfig | undefined,
  ) => {
    if (currentStatus !== "Action Needed" || previousStatus === "Action Needed") return;
    addActionNeeded(
      sessionId,
      agent?.session_name ?? sessionId,
      "Action required",
    );
  }, [addActionNeeded]);

  // A library deep-link (e.g. "Manage skills" from the agent config panel)
  // bumps the store's navigationRequest; switch the main view to the
  // library whenever that happens, but not on initial mount.
  useEffect(() => {
    if (seenLibraryNavigationRequestRef.current === libraryNavigationRequest) return;
    seenLibraryNavigationRequestRef.current = libraryNavigationRequest;
    workbenchNavigation.open({ surface_type: "library" });
  }, [libraryNavigationRequest, workbenchNavigation]);

  const [broadcastMessage, setBroadcastMessage] = useState("");

  const [activeTab, setActiveTab] = useState<SidebarTab>("agent-config");
  const leftCollapsed = useLayoutStore((state) => state.leftSidebarCollapsed);
  const setLeftCollapsed = useLayoutStore((state) => state.setLeftSidebarCollapsed);
  const rightCollapsed = useLayoutStore((state) => state.rightSidebarCollapsed);
  const setRightCollapsed = useLayoutStore((state) => state.setRightSidebarCollapsed);
  const focusLeftDock = useCallback(() => {
    setLeftCollapsed(false);
    window.requestAnimationFrame(() => {
      const target = sidebarIconRailRef.current?.querySelector<HTMLButtonElement>(
        '[data-sidebar-active="true"]',
      ) ?? sidebarIconRailRef.current?.querySelector<HTMLButtonElement>("button");
      target?.focus();
    });
  }, [setLeftCollapsed]);
  const focusRightDock = useCallback(() => {
    setRightCollapsed(false);
    window.requestAnimationFrame(() => {
      const target = agentWatchlistRef.current?.querySelector<HTMLInputElement>(
        'input[type="search"]',
      ) ?? agentWatchlistRef.current;
      target?.focus();
    });
  }, [setRightCollapsed]);
  const [editingAgentId, setEditingAgentId] = useState<string | null>(null);
  const [tempName, setTempName] = useState("");
  const {
    theme,
    autoPatchGemini,
    titlebarTelemetryVisible,
    workbenchNewTabAction,
    fileOpenActions,
    externalEditor,
    externalEditorCustomExecutable,
    app_settings_loaded,
    loadAppSettings,
    settingsOpen,
    setSettingsOpen,
    toggleSettings,
  } = useSettingsStore();
  const onboardingHintsLoaded = useOnboardingStore((state) => state.hintsLoaded);
  const guidedTourState = useOnboardingStore((state) => state.guidedTourState);
  const loadOnboardingHints = useOnboardingStore((state) => state.loadOnboardingHints);
  const setGuidedTourState = useOnboardingStore((state) => state.setGuidedTourState);
  const [guidedTourMode, setGuidedTourMode] = useState<"setup" | "review">("setup");
  const [tourEvolverSessionId, setTourEvolverSessionId] = useState<string | null>(null);
  const appUpdate = useAppUpdate();
  const [updateNoticeDismissed, setUpdateNoticeDismissed] = useState(false);
  const resolvedTitlebarTelemetryVisible = app_settings_loaded && titlebarTelemetryVisible;
  const resolvedWorkbenchNewTabAction = app_settings_loaded ? workbenchNewTabAction : "home";

  useEffect(() => {
    setUpdateNoticeDismissed(false);
  }, [appUpdate.availableUpdate?.version]);

  const updateAvailable = appUpdate.status === "available" && appUpdate.availableUpdate !== null;
  const reviewUpdate = () => {
    setUpdateNoticeDismissed(true);
    setSettingsOpen(true);
  };

  const [watchlists, setWatchlists] = useState<Watchlist[]>([]);
  const [teams, setTeams] = useState<AgentTeam[]>([]);
  const [watchlistPrefs, setWatchlistPrefs] = useState<WatchlistPrefs>(DEFAULT_WATCHLIST_PREFS);
  const [dashboardPrefs, setDashboardPrefs] = useState<DashboardPrefs>(DEFAULT_DASHBOARD_PREFS);
  const [agentInteractions, setAgentInteractions] = useState<AgentInteractions>({});
  const [changeReviewTurnRevision, setChangeReviewTurnRevision] = useState(0);
  const agentInteractionsRef = useRef<AgentInteractions>({});
  const interactionSaveChainRef = useRef<Promise<unknown>>(Promise.resolve());
  const hasAutoPatched = useRef(false);

  const handleAgentStatusTransition = useCallback((transition: AgentStatusTransition) => {
    maybeAddActionNeededQueueItem(
      transition.session_id,
      transition.current_status,
      transition.previous_status,
      transition.agent,
    );
  }, [maybeAddActionNeededQueueItem]);

  const queueAgentInteractionSnapshot = useCallback((snapshot: AgentInteractions) => {
    interactionSaveChainRef.current = interactionSaveChainRef.current
      .catch(() => undefined)
      .then(() => invoke("save_agent_interactions", { interactions: snapshot }))
      .catch(() => undefined);
  }, []);

  const handleAgentInteractions = useCallback((updates: Readonly<Record<string, string>>) => {
    const updated = { ...agentInteractionsRef.current, ...updates };
    agentInteractionsRef.current = updated;
    setAgentInteractions(updated);
    queueAgentInteractionSnapshot(updated);
  }, [queueAgentInteractionSnapshot]);

  const agentResources = useAgentResourceController({
    on_agent_json_event: appendAgentEvent,
    on_agent_status_transition: handleAgentStatusTransition,
    on_agent_turn_completed: handleAgentTurnCompletion,
    on_agent_interactions: handleAgentInteractions,
  });
  const {
    agents,
    telemetry,
    app_telemetry: appTelemetry,
    terminal_titles: terminalTitles,
    current_thoughts: currentThoughts,
    off_agent_ids: offAgentIds,
    refresh_agents: fetchAgents,
    set_terminal_title: handleTitleChange,
  } = agentResources;
  const roster = useRosterController({ agents, watchlists, teams });
  const {
    activeWatchlistId: activeListId,
    setActiveWatchlistId: setActiveListId,
    activeWatchlist: activeList,
    filter: rosterFilter,
    setFilter: setRosterFilter,
    selectedAgentIds,
    setSelectedAgentIds,
    selectAgent,
  } = roster;
  const filteredAgents = useMemo(
    () => getAgentsForList(agents, activeList, teams),
    [activeList, agents, teams],
  );
  const recentAgentIds = useMemo(
    () => Object.entries(agentInteractions)
      .sort(([, left], [, right]) => right.localeCompare(left))
      .map(([agentId]) => agentId),
    [agentInteractions],
  );
  const sourceControlStatus = useSelectedAgentGitStatus(selectedAgentIds, agents);

  useEffect(() => {
    if (!app_settings_loaded) {
      void loadAppSettings();
    }
  }, [app_settings_loaded, loadAppSettings]);

  useEffect(() => {
    if (!onboardingHintsLoaded) {
      void loadOnboardingHints();
    }
  }, [loadOnboardingHints, onboardingHintsLoaded]);

  useEffect(() => {
    if (import.meta.env.DEV) {
      window.__wardianAppDebug = {
      snapshot: (sessionId: string) => {
        const metrics = telemetry[sessionId] ?? null;
        if (!metrics && !terminalTitles[sessionId] && !currentThoughts[sessionId]) {
          return null;
        }

        const title = terminalTitles[sessionId] || "";
        const thought = currentThoughts[sessionId] || "";
        const { status } = deriveCurrentThought(title, thought, metrics, offAgentIds.has(sessionId));

        return {
          title,
          thought,
          metrics,
          derivedStatus: status,
        };
      },
    };
    }

    return () => {
      delete window.__wardianAppDebug;
    };
  }, [telemetry, terminalTitles, currentThoughts, offAgentIds]);

  const loadWatchlistState = useCallback(async () => {
      try {
        const data = await invoke<unknown>("load_watchlists");
        const state = normalizeWatchlistState(data);
        setWatchlists(state.watchlists);
        setTeams(state.teams);
      } catch { /* first run */ }
  }, []);

  useEffect(() => {
    (async () => {
      await loadWatchlistState();
      try {
        const prefs = await invoke<WatchlistPrefs | null>("load_watchlist_prefs");
        if (prefs) {
          // Merge saved prefs with defaults so newly-added columns always appear
          const savedMap = new Map(prefs.columns.map(c => [c.id, c]));
          const collapsedTeamIds = Array.isArray(prefs.collapsed_team_ids)
            ? prefs.collapsed_team_ids.filter((teamId): teamId is string => typeof teamId === "string")
            : [];
          const collapsedTeamIdsByList = normalizeCollapsedTeamIdsByList(
            prefs.collapsed_team_ids_by_list,
          );
          if (!collapsedTeamIdsByList.all && collapsedTeamIds.length > 0) {
            collapsedTeamIdsByList.all = collapsedTeamIds;
          }
          setWatchlistPrefs({
            ...DEFAULT_WATCHLIST_PREFS,
            ...prefs,
            columns: DEFAULT_WATCHLIST_PREFS.columns.map(def => savedMap.get(def.id) ?? def),
            collapsed_team_ids: collapsedTeamIds,
            collapsed_team_ids_by_list: collapsedTeamIdsByList,
          });
        }
      } catch { /* first run */ }

      try {
        // Merged over the defaults, so a column added in a later release appears
        // without a migration and a stale file cannot hide one.
        setDashboardPrefs(mergeDashboardPrefs(await invoke("load_dashboard_prefs")));
      } catch { /* first run */ }

      try {
        const interactions = await invoke<AgentInteractions>("load_agent_interactions");
        if (interactions) {
          const preLoadUpdates = agentInteractionsRef.current;
          const merged = { ...interactions, ...preLoadUpdates };
          agentInteractionsRef.current = merged;
          setAgentInteractions(merged);
          if (Object.keys(preLoadUpdates).length > 0) {
            queueAgentInteractionSnapshot(merged);
          }
        }
      } catch { /* first run */ }
    })();

  }, [loadWatchlistState, queueAgentInteractionSnapshot]);

  useEffect(() => {
    if (app_settings_loaded && autoPatchGemini && !hasAutoPatched.current) {
      hasAutoPatched.current = true;
      invoke('run_gemini_patch').catch(e => console.error("Auto patch failed:", e));
    }
  }, [app_settings_loaded, autoPatchGemini]);

  const persistWatchlistState = useCallback(async (state: WatchlistState) => {
    const normalized = normalizeWatchlistState(state);
    setWatchlists(normalized.watchlists);
    setTeams(normalized.teams);
    try {
      await invoke("save_watchlists", { watchlists: normalized });
    } catch { /* non-critical */ }
  }, []);

  const persistWatchlists = useCallback(async (lists: Watchlist[]) => {
    await persistWatchlistState({ version: 2, watchlists: lists, teams });
  }, [persistWatchlistState, teams]);

  const persistWatchlistPrefs = useCallback(async (prefs: WatchlistPrefs) => {
    setWatchlistPrefs(prefs);
    try {
      await invoke("save_watchlist_prefs", { prefs });
    } catch { /* non-critical */ }
  }, []);

  /**
   * Written on every Dashboard change, with no save step — the same contract the
   * watchlist uses. This is the seed a Dashboard starts from; a failed write
   * costs the next session's layout, never the current one's.
   */
  const persistDashboardPrefs = useCallback(async (prefs: DashboardPrefs) => {
    setDashboardPrefs(prefs);
    try {
      await invoke("save_dashboard_prefs", { prefs });
    } catch { /* non-critical */ }
  }, []);

  const handleAddToList = async (listId: string, agentId: string) => {
    await handleAddAgentsToList(listId, [agentId]);
  };

  const handleAddAgentsToList = async (listId: string, agentIds: string[]) => {
    const updated = watchlists.map((l) =>
      l.id === listId ? addAgentsToList(l, agentIds, teams) : l,
    );
    await persistWatchlists(updated);
  };

  const handleRemoveFromList = async (listId: string, agentId: string) => {
    await handleRemoveAgentsFromList(listId, [agentId]);
  };

  const handleRemoveAgentsFromList = async (listId: string, agentIds: string[]) => {
    const updated = watchlists.map((l) =>
      l.id === listId ? removeAgentsFromList(l, agentIds, teams) : l,
    );
    await persistWatchlists(updated);
  };

  const handleCreateTeam = async (agentIds: string[]) => {
    const next = createTeamFromAgents(
      { version: 2, watchlists, teams },
      crypto.randomUUID(),
      agentIds,
    );
    await persistWatchlistState(next);
  };

  const handleUngroupTeam = async (teamId: string) => {
    await persistWatchlistState(ungroupTeam({ version: 2, watchlists, teams }, teamId));
  };

  const handleRenameTeam = async (teamId: string, newName: string) => {
    await persistWatchlistState({
      version: 2,
      watchlists,
      teams: teams.map((team) => team.id === teamId ? { ...team, name: newName } : team),
    });
  };

  const handleAddAgentToTeam = async (teamId: string, agentId: string) => {
    await persistWatchlistState(addAgentToTeam({ version: 2, watchlists, teams }, teamId, agentId));
  };

  const handleRemoveAgentFromTeam = async (teamId: string, agentId: string, targetAgentId?: string, position: "before" | "after" = "before") => {
    await persistWatchlistState(removeAgentFromTeam({ version: 2, watchlists, teams }, teamId, agentId, targetAgentId, position));
  };

  const handleRemoveAgentFromTeamAtEntry = async (
    teamId: string,
    agentId: string,
    targetEntry: WatchlistEntry,
    position: "before" | "after",
    targetListId: string,
  ) => {
    await persistWatchlistState(removeAgentFromTeamAtEntry(
      { version: 2, watchlists, teams },
      teamId,
      agentId,
      targetEntry,
      position,
      targetListId,
    ));
  };

  const handleReorderTeamMember = async (teamId: string, draggedAgentId: string, targetAgentId: string, position: "before" | "after" = "before") => {
    await persistWatchlistState(reorderTeamMember({ version: 2, watchlists, teams }, teamId, draggedAgentId, targetAgentId, position));
  };

  useEffect(() => {
    const mediaQuery = window.matchMedia("(prefers-color-scheme: light)");
    const applyTheme = () => {
      let effectiveTheme = theme;
      if (theme === "system") effectiveTheme = mediaQuery.matches ? "light" : "dark";
      document.documentElement.setAttribute("data-theme", effectiveTheme);
      invoke("sync_provider_theme_settings", { theme: effectiveTheme }).catch((error) => {
        console.error("Failed to sync provider theme settings:", error);
      });
    };
    applyTheme();
    mediaQuery.addEventListener("change", applyTheme);
    return () => mediaQuery.removeEventListener("change", applyTheme);
  }, [theme]);

  const leftSidebarWidth = useLayoutStore((s) => s.leftSidebarWidth);
  const rightSidebarWidth = useLayoutStore((s) => s.rightSidebarWidth);
  const userTerminalOpen = useLayoutStore((s) => s.userTerminalOpen);
  const userTerminalHeight = useLayoutStore((s) => s.userTerminalHeight);
  const setUserTerminalOpen = useLayoutStore((s) => s.setUserTerminalOpen);
  const setUserTerminalHeight = useLayoutStore((s) => s.setUserTerminalHeight);
  const toggleUserTerminal = useLayoutStore((s) => s.toggleUserTerminal);

  useLayoutEffect(() => {
    const root = document.documentElement;
    root.style.setProperty('--sidebar-content-width', `${leftSidebarWidth}px`);
    root.style.setProperty('--sidebar-secondary-width', `${rightSidebarWidth}px`);
  }, [leftSidebarWidth, rightSidebarWidth]);

  const [draggedAgentId, setDraggedAgentId] = useState<string | null>(null);
  const [dragOverAgentId, setDragOverAgentId] = useState<string | null>(null);
  const [customCloneSourceId, setCustomCloneSourceId] = useState<string | null>(null);
  const wasDraggingRef = useRef(false);

  const [agentClasses, setAgentClasses] = useState<AgentClassDefinition[]>([]);

  const lastClickRef = useRef<{ id: string; time: number } | null>(null);

  const handleAgentCardClick = useCallback((e: React.MouseEvent, agentId: string) => {
    const now = Date.now();
    const isDoubleClick = Boolean(
      lastClickRef.current
      && lastClickRef.current.id === agentId
      && now - lastClickRef.current.time < 450,
    );
    lastClickRef.current = { id: agentId, time: now };
    if (isDoubleClick && !(e.ctrlKey || e.metaKey || e.shiftKey)) {
      setSelectedAgentIds(new Set([agentId]));
      return;
    }
    selectAgent(agentId, {
      ctrlKey: e.ctrlKey,
      metaKey: e.metaKey,
      shiftKey: e.shiftKey,
      rangeAgentIds: filteredAgents.map((agent) => agent.session_id),
    });
  }, [filteredAgents, selectAgent, setSelectedAgentIds]);

  const handleMouseDown = (agentId: string) => setDraggedAgentId(agentId);
  const handleMouseEnterCard = (agentId: string) => {
    if (draggedAgentId && draggedAgentId !== agentId) setDragOverAgentId(agentId);
  };

  const reorderGlobalAgentsAroundTarget = async (
    agentId: string,
    targetAgentId: string,
    position: "before" | "after",
  ) => {
    const remainingAgents = agents.filter((agent) => agent.session_id !== agentId);
    const targetIndex = remainingAgents.findIndex((agent) => agent.session_id === targetAgentId);
    const draggedAgent = agents.find((agent) => agent.session_id === agentId);
    if (!draggedAgent || targetIndex === -1) return;

    const nextAgents = [...remainingAgents];
    nextAgents.splice(targetIndex + (position === "after" ? 1 : 0), 0, draggedAgent);
    try { await agentResources.reorder_agents(nextAgents.map((agent) => agent.session_id)); }
    catch (err) { console.error("Failed to reorder:", err); }
  };

  const handleMouseUp = async () => {
    if (draggedAgentId && dragOverAgentId && draggedAgentId !== dragOverAgentId) {
      const newDisplayList = [...filteredAgents];
      const fromIndex = newDisplayList.findIndex(a => a.session_id === draggedAgentId);
      const toIndex = newDisplayList.findIndex(a => a.session_id === dragOverAgentId);
      if (fromIndex !== -1 && toIndex !== -1) {
        const draggedTeam = teams.find((team) => team.agentIds.includes(draggedAgentId));
        const targetTeam = teams.find((team) => team.agentIds.includes(dragOverAgentId));
        const position = fromIndex < toIndex ? "after" : "before";
        const watchlistState = { version: 2 as const, watchlists, teams };
        if (draggedTeam && draggedTeam.id === targetTeam?.id) {
          await handleReorderTeamMember(draggedTeam.id, draggedAgentId, dragOverAgentId, position);
        } else if (targetTeam) {
          const next = reorderTeamMember(
            addAgentToTeam(watchlistState, targetTeam.id, draggedAgentId),
            targetTeam.id,
            draggedAgentId,
            dragOverAgentId,
            position,
          );
          await persistWatchlistState(next);
          if (activeListId === "all") {
            await reorderGlobalAgentsAroundTarget(draggedAgentId, dragOverAgentId, position);
          }
        } else if (draggedTeam) {
          const next = removeAgentFromTeam(
            watchlistState,
            draggedTeam.id,
            draggedAgentId,
            dragOverAgentId,
            position,
          );
          await persistWatchlistState(next);
          if (activeListId === "all") {
            await reorderGlobalAgentsAroundTarget(draggedAgentId, dragOverAgentId, position);
          }
        } else {
          const [draggedItem] = newDisplayList.splice(fromIndex, 1);
          newDisplayList.splice(toIndex, 0, draggedItem);
          const newOrder = newDisplayList.map(a => a.session_id);
          if (activeListId !== 'all') {
            const updatedWatchlists = watchlists.map(l => l.id === activeListId ? { ...l, entries: newOrder.map(agentId => ({ type: "agent" as const, agentId })) } : l);
            await persistWatchlists(updatedWatchlists);
          } else {
            try { await agentResources.reorder_agents(newOrder); } catch (err) { console.error("Failed to reorder:", err); }
          }
        }
        wasDraggingRef.current = true;
      }
    }
    setDraggedAgentId(null);
    setDragOverAgentId(null);
  };

  useEffect(() => {
    const cancelDrag = () => {
      if (draggedAgentId) {
        setDraggedAgentId(null);
        setDragOverAgentId(null);
      }
    };
    window.addEventListener("mouseup", cancelDrag);
    return () => window.removeEventListener("mouseup", cancelDrag);
  }, [draggedAgentId]);

  const scrollToAgent = useCallback((agentId: string) => {
    scrollAgentCardWithinOverview(agentId);
  }, []);

  const fetchAgentClasses = async () => {
    try {
      const list = await invoke<AgentClassDefinition[]>("list_agent_classes");
      setAgentClasses(list);
    } catch (e) { console.error("Failed to fetch classes:", e); }
  };

  useEffect(() => {
    fetchAgentClasses();
    loadQueueItems();
    loadQueuePreferences();
    const inboxPoll = window.setInterval(() => loadQueueItems(), 5_000);
    const unlistenWatchlists = listen("watchlists-updated", () => loadWatchlistState());
    const unlistenInbox = listen("inbox-updated", () => loadQueueItems());
    // Class definitions (create/delete/reset) are now managed exclusively
    // from the Library's ClassDetail panel, which reports changes through
    // the same `library-changed` event the library store listens for (see
    // useLibraryStore.subscribeToLibraryChanges / src-tauri/src/commands/library.rs).
    // The backend only ever emits `library_type: "library"` (it covers both
    // `library/` and `classes/` under one watch), so there's nothing finer
    // to filter on here — refetch classes unconditionally on any change.
    const unlistenLibrary = listen<{ library_type: string }>("library-changed", (event) => {
      if (event.payload.library_type !== "library") return;
      fetchAgentClasses();
    });
    return () => {
      unlistenWatchlists.then(fn => fn());
      unlistenInbox.then(fn => fn());
      unlistenLibrary.then(fn => fn());
      window.clearInterval(inboxPoll);
    };
  }, [loadQueueItems, loadQueuePreferences, loadWatchlistState]);

  useEffect(() => {
    const unlistenWorkflowInbox = listen<WorkflowInboxUpdate>("workflow-inbox-updated", (event) => {
      const update = event.payload;
      if (update.status === "awaiting_approval") {
        void loadQueueItems();
        return;
      }
      addWorkflowCompletion({
        workflow_id: update.workflow_id,
        run_instance_id: update.run_instance_id,
        status: update.status,
        error: update.error,
        summary: update.summary,
      }, update.workflow_name);
    });
    return () => {
      unlistenWorkflowInbox.then((cleanup) => cleanup());
    };
  }, [addWorkflowCompletion, loadQueueItems]);

  async function sendCommand(sessionId: string, cmd: string, inboxItemId?: string) {
    try {
      if (inboxItemId) {
        await submitInboxProviderChoice(sessionId, cmd, inboxItemId);
      } else {
        await submitInputToAgent(sessionId, cmd);
      }
      const timestamp = new Date().toISOString();
      handleAgentInteractions({ [sessionId]: timestamp });
    } catch (e) {
      console.error(e);
      if (inboxItemId) await loadQueueItems();
      throw e;
    }
  }

  async function renameAgent(sessionId: string, newName: string) {
    if (!newName.trim()) {
      setEditingAgentId(null);
      return;
    }
    const re = /^[a-zA-Z0-9_-]+$/;
    if (!re.test(newName)) {
      alert("Invalid agent name. Names must contain only alphanumeric characters, underscores, or hyphens (no spaces).");
      return;
    }
    try {
      await agentResources.rename_agent(sessionId, newName);
      setEditingAgentId(null);
    } catch (e: unknown) { 
      console.error(e);
      alert(e instanceof Error ? e.message : String(e));
    }
  }

  async function broadcastInput(e: React.FormEvent) {
    e.preventDefault();
    if (!broadcastMessage || selectedAgentIds.size === 0) return;

    try {
      await submitInputToAgents(selectedAgentIds, broadcastMessage);
      setBroadcastMessage("");
    } catch (e) { console.error(e); }
  }

  const onPause = async (id: string) => {
    try {
      await agentResources.pause_agent(id);
    } catch (e) {
      console.error(e);
    }
  };

  const onRestart = async (id: string) => {
    try {
      await agentResources.resume_agent(id);
    } catch (e) {
      console.error(e);
    }
  };

  const onClear = async (id: string) => {
    try {
      await agentResources.clear_agent(id);
    } catch (e) {
      console.error(e);
    }
  };

  const onClone = async (id: string, mode: CloneMode) => {
    if (mode === "custom") {
      setCustomCloneSourceId(id);
      return;
    }

    try {
      await agentResources.clone_agent(id, mode);
      await loadWatchlistState();
    } catch (e) {
      console.error(e);
      alert(`Failed to clone agent: ${e}`);
    }
  };

  const onDelete = async (id: string) => {
    if (await confirm('Delete this agent?')) {
      try {
        const deletedIds = await agentResources.delete_agents([id]);
        if (deletedIds.length === 0) return;
        await persistWatchlistState(removeDeletedAgentsFromWatchlistState(
          { version: 2, watchlists, teams },
          [...deletedIds],
        ));
        setSelectedAgentIds(prev => {
          const next = new Set(prev);
          for (const deletedId of deletedIds) next.delete(deletedId);
          return next;
        });
      } catch (e) {
        console.error(e);
      }
    }
  };

  const onDeleteAgents = async (ids: string[]) => {
    if (ids.length === 0) return;

    const message = ids.length === 1
      ? 'Delete this agent?'
      : `Delete ${ids.length} selected agents?`;

    if (!(await confirm(message))) return;

    const deletedIds = await agentResources.delete_agents(ids);
    if (deletedIds.length === 0) return;

    await persistWatchlistState(removeDeletedAgentsFromWatchlistState(
      { version: 2, watchlists, teams },
      [...deletedIds],
    ));

    setSelectedAgentIds(prev => {
      const next = new Set(prev);
      for (const id of deletedIds) next.delete(id);
      return next;
    });
  };

  const selectedUserTerminalWorkspace = selectedAgentIds.size === 1
    ? agents.find((agent) => agent.session_id === Array.from(selectedAgentIds)[0])?.folder?.trim() || null
    : null;
  const selectedWorkbenchResourceKey = selectedAgentIds.size === 1
    ? Array.from(selectedAgentIds)[0]
    : undefined;

  const openAuxiliarySurface = useCallback((request: OpenSurfaceRequest) => {
    workbenchNavigation.open(request);
  }, [workbenchNavigation]);

  const openWorkflowsView = useCallback(() => {
    openAuxiliarySurface({ surface_type: "workflows" });
  }, [openAuxiliarySurface]);

  const openAgent = useCallback((sessionId: string) => {
    workbenchNavigation.open({
      surface_type: "agent-session",
      resource_key: sessionId,
    });
  }, [workbenchNavigation]);

  const beginGuidedTour = useCallback((mode: "setup" | "review") => {
    setTourEvolverSessionId(null);
    setGuidedTourMode(mode);
    setSettingsOpen(false);
    void setGuidedTourState("in_progress");
  }, [setGuidedTourState, setSettingsOpen]);

  const leaveGuidedTour = useCallback(() => {
    setTourEvolverSessionId(null);
    void setGuidedTourState("skipped");
  }, [setGuidedTourState]);

  const completeGuidedTour = useCallback(() => {
    setTourEvolverSessionId(null);
    void setGuidedTourState("completed");
  }, [setGuidedTourState]);

  const prepareTourAgentCreation = useCallback(() => {
    setActiveTab("agent-config");
    setLeftCollapsed(false);
  }, [setLeftCollapsed]);

  const prepareTourEvolver = useCallback((agent: AgentConfig) => {
    setTourEvolverSessionId(agent.session_id);
    openAgent(agent.session_id);
  }, [openAgent]);

  const prepareTourGraph = useCallback(() => {
    openAuxiliarySurface({ surface_type: "graph" });
  }, [openAuxiliarySurface]);

  const prepareTourWorkflow = useCallback(() => {
    openWorkflowsView();
  }, [openWorkflowsView]);

  useEffect(() => {
    const startTour = () => beginGuidedTour("review");
    window.addEventListener("wardian:start-guided-tour", startTour);
    return () => window.removeEventListener("wardian:start-guided-tour", startTour);
  }, [beginGuidedTour]);

  const scheduleAgentOverviewScroll = useCallback((sessionId: string) => {
    requestAnimationFrame(() => {
      requestAnimationFrame(() => {
        scrollAgentCardWithinOverview(sessionId);
      });
    });
  }, []);

  const focusAgentInOverviewSurface = useCallback((surfaceId: string, sessionId: string): boolean => {
    const store = workbenchPersistence.store;
    const state = store.getState();
    const overviewSurface = state.document.surfaces[surfaceId];
    if (overviewSurface?.surface_type !== "agents-overview") return false;
    const overviewGroup = Object.values(state.document.groups).find((group) => (
      group.surface_ids.includes(overviewSurface.surface_id)
    ));
    if (!overviewGroup) return false;

    // An existing Agents view is the destination even when another pane is
    // zoomed. Restore the layout first so focusing the tab can reveal it.
    if (
      state.zoomed_group_id !== null
      && state.zoomed_group_id !== overviewGroup.group_id
    ) {
      state.set_zoomed_group_id(null);
    }

    setSelectedAgentIds(new Set([sessionId]));
    const currentState = normalizeAgentsOverviewSurfaceState(overviewSurface.state);
    const targetAgent = agents.find((agent) => agent.session_id === sessionId);
    const nextState = targetAgent
      ? revealAgentInOverviewState(
        currentState,
        targetAgent,
        deriveCurrentThought(
          terminalTitles[sessionId] ?? "",
          currentThoughts[sessionId] ?? "",
          telemetry[sessionId],
          offAgentIds.has(sessionId),
        ).status,
      )
      : { ...currentState, focused_agent_id: sessionId };
    workbenchNavigation.focus(overviewSurface.surface_id);
    store.getState().apply_commands([{
      type: "update_surface_state",
      surface_id: overviewSurface.surface_id,
      state_schema_version: overviewSurface.state_schema_version,
      state: nextState,
    }]);
    scheduleAgentOverviewScroll(sessionId);
    return true;
  }, [agents, currentThoughts, offAgentIds, scheduleAgentOverviewScroll, setSelectedAgentIds, telemetry, terminalTitles, workbenchNavigation, workbenchPersistence.store]);

  const openAgentFromSurface = useCallback((sourceSurfaceId: string, sessionId: string) => {
    const state = workbenchPersistence.store.getState();
    const overviewSurfaceId = findExistingSurface(
      state.document,
      state.surface_mru,
      "agents-overview",
    );
    if (overviewSurfaceId && focusAgentInOverviewSurface(overviewSurfaceId, sessionId)) {
      return;
    }

    void workbenchNavigation.open_contextually(sourceSurfaceId, {
      surface_type: "agent-session",
      resource_key: sessionId,
    }).catch((error) => {
      console.error("contextual agent open failed", error);
    });
  }, [focusAgentInOverviewSurface, workbenchNavigation, workbenchPersistence.store]);

  const openAgentToSide = useCallback((sessionId: string) => {
    workbenchNavigation.open_to_side({
      surface_type: "agent-session",
      resource_key: sessionId,
    });
  }, [workbenchNavigation]);

  const revealAgentInOverview = useCallback((sessionId: string) => {
    const store = workbenchPersistence.store;
    const snapshot = store.getState();
    const overviewSurfaceId = findExistingSurface(
      snapshot.document,
      snapshot.surface_mru,
      "agents-overview",
    );

    if (!overviewSurfaceId) {
      setSelectedAgentIds(new Set([sessionId]));
      workbenchNavigation.open({
        surface_type: "agents-overview",
        state: {
          ...normalizeAgentsOverviewSurfaceState(
            workbenchRegistry.default_state("agents-overview"),
          ),
          focused_agent_id: sessionId,
        },
      });
      scheduleAgentOverviewScroll(sessionId);
      return;
    }

    focusAgentInOverviewSurface(overviewSurfaceId, sessionId);
  }, [focusAgentInOverviewSurface, scheduleAgentOverviewScroll, setSelectedAgentIds, workbenchNavigation, workbenchPersistence.store, workbenchRegistry]);

  const workbenchNotice = [
    workbenchPersistence.notice,
    workbenchPersistence.safe_mode
      ? "Workbench safe mode is active; the durable document is preserved."
      : null,
    workbenchPersistence.save_error,
    workbenchPersistence.save_pending || workbenchPersistence.is_dirty
      ? "Saving workbench changes…"
      : null,
  ].filter((message): message is string => Boolean(message)).join(" ") || null;

  const exportLocalWorkbench = useCallback(() => {
    const exported = workbenchPersistence.export_local_json();
    const objectUrl = URL.createObjectURL(new Blob([exported.json], { type: exported.mime_type }));
    const anchor = document.createElement("a");
    anchor.href = objectUrl;
    anchor.download = exported.filename;
    anchor.click();
    URL.revokeObjectURL(objectUrl);
  }, [workbenchPersistence]);

  const openWorkbenchLauncher = useCallback(() => {
    workbenchPersistence.store.getState().set_launcher_open(true);
  }, [workbenchPersistence.store]);

  /**
   * Creates the runtime resource a provisioning surface needs before it opens.
   *
   * Returning null aborts the open, so a host with no browser installed leaves
   * no surface behind pointing at a session that was never created.
   */
  const provisionWorkbenchSurfaceResource = useCallback(
    async (surfaceType: string): Promise<SurfaceResourceProvision | null> => {
      if (surfaceType !== "browser") return null;
      try {
        // A Browser opened while one agent is selected takes that agent's
        // workspace, which is the same rule the user terminal already follows.
        // With nothing selected there is no workspace to read a declared port
        // out of, and detection falls back to the conventional ones.
        const session = await openBrowserSession({
          workspace: selectedUserTerminalWorkspace,
        });
        return {
          resource_key: session.browser_id,
          release: () => {
            void closeBrowserSession(session.browser_id).catch(() => {});
          },
        };
      } catch (error) {
        console.error("[Wardian] could not open a browser session", error);
        return null;
      }
    },
    [selectedUserTerminalWorkspace],
  );

  /** Replaces a dead browser surface's session with a fresh one at the same URL. */
  const reopenBrowserSurface = useCallback(
    async (
      surfaceId: string,
      url: string,
      viewport: BrowserViewport | null,
    ): Promise<void> => {
      // `rebind_resource`, not `canonicalize_resource`: the latter converges
      // identity while deliberately keeping the persisted state, which is
      // wrong when the old session is gone and a new one has replaced it.
      await reopenBrowserSurfaceSession(
        url,
        (session) => workbenchNavigation.rebind_resource(surfaceId, {
          surface_type: "browser",
          resource_key: session.browser_id,
          // The viewport is carried forward, not dropped: a restored surface
          // that silently reverts to the default size would reopen the page at
          // a width the operator never chose.
          state: { url: session.url || url, viewport: viewport ?? null },
        }),
        viewport,
      );
    },
    [workbenchNavigation],
  );

  // A CLI `wardian browser open` asks the app to surface the session it made.
  //
  // Held until the durable document has loaded. Opening into the provisional
  // one would acknowledge the request and then lose it: `adopt_durable_state`
  // replaces the working document outright, so the surface would vanish with
  // nothing left to replay it. Waiting costs nothing, because the backend
  // holds every open until it is acknowledged.
  const workbenchReady = workbenchPersistence.status === "ready";
  useEffect(() => {
    if (!workbenchReady) return undefined;
    return subscribeToBrowserSurfaceOpens((summary) => {
      // `focus_resource` makes this idempotent: a session that both the event
      // and the read report is focused rather than opened twice.
      workbenchNavigation.open({
        surface_type: "browser",
        resource_key: summary.browser_id,
        state: { url: summary.url, viewport: null },
      });
    });
  }, [workbenchNavigation, workbenchReady]);

  const renderWorkbenchSurface: WorkbenchSurfaceRenderer = (surface, lifecycle) => {
    const resolvedSurface = workbenchRegistry.resolve_surface(surface);
    const restoreResult = resolvedSurface.restore_result;
    if (resolvedSurface.missing_surface_type || !restoreResult.ok) {
      const error = resolvedSurface.missing_surface_type
        ? `Surface type “${resolvedSurface.missing_surface_type}” is not installed or registered. Its persisted state has been kept intact.`
        : `Wardian could not restore this persisted state: ${restoreResult.ok ? "Unknown restore failure" : restoreResult.error}`;
      const canRebind = surface.surface_type === "agent-session";
      return (
        <SurfaceRecoveryPlaceholder
          surface={surface}
          error={error}
          on_retry={async () => {
            if (canRebind) await fetchAgents();
            setSurfaceRecoveryAttempt((attempt) => attempt + 1);
          }}
          on_reset={async () => { await workbenchNavigation.reset_surface(surface.surface_id); }}
          on_close={async () => { await workbenchNavigation.close(surface.surface_id); }}
          rebind_options={canRebind ? agents.map((agent) => ({
            resource_key: agent.session_id,
            label: agent.session_name,
          })) : []}
          {...(canRebind ? {
            on_rebind: async (resourceKey: string) => {
              await workbenchNavigation.rebind_resource(
                surface.surface_id,
                { surface_type: "agent-session", resource_key: resourceKey },
              );
            },
          } : {})}
        />
      );
    }
    const restoredSurface = {
      ...surface,
      state: restoreResult.ok ? restoreResult.state : surface.state,
    };

    if (surface.surface_type === "agent-session") {
      const resourceKey = surface.resource_key ?? "";
      return (
        <AgentSessionSurface
          surface_id={surface.surface_id}
          resource_key={resourceKey}
          agent={agents.find((agent) => agent.session_id === resourceKey)}
          theme={theme}
          visibility={lifecycle?.visible === false ? "hidden" : "visible"}
          render_state={lifecycle?.visible === false ? "suspended" : "mounted"}
          auto_focus_terminal={resourceKey === tourEvolverSessionId}
          on_title_change={handleTitleChange}
          on_refresh_agents={() => { void fetchAgents(); }}
          rebind_candidates={agents}
          on_rebind_agent={(nextAgentId) => {
            void workbenchNavigation.rebind_resource(surface.surface_id, {
              surface_type: "agent-session",
              resource_key: nextAgentId,
            });
          }}
          on_reset_surface={() => {
            workbenchPersistence.store.getState().apply_commands([{
              type: "update_surface_state",
              surface_id: surface.surface_id,
              state_schema_version: 1,
              state: workbenchRegistry.default_state("agent-session"),
            }]);
          }}
          on_close_surface={() => { void workbenchNavigation.close(surface.surface_id); }}
        />
      );
    }

    const visibility = lifecycle?.visible === false ? "hidden" : "visible";
    if (surface.surface_type === "browser") {
      const browserState = restoredSurface.state as BrowserSurfaceState;
      return (
        <BrowserSurface
          surface_id={surface.surface_id}
          resource_key={surface.resource_key ?? ""}
          persisted_url={browserState.url}
          persisted_viewport={browserState.viewport}
          visibility={visibility}
          on_url_change={(surfaceId, url) => {
            // Persisting the page, not the session id, is what lets a cold
            // restart reopen the same address against a fresh runtime.
            if (url === browserState.url) return;
            workbenchPersistence.store.getState().apply_commands([{
              type: "update_surface_state",
              surface_id: surfaceId,
              state_schema_version: 1,
              state: { ...browserState, url },
            }]);
          }}
          on_reopen={(url, viewport) => reopenBrowserSurface(surface.surface_id, url, viewport)}
          on_close_surface={() => { void workbenchNavigation.close(surface.surface_id); }}
        />
      );
    }
    if (surface.surface_type === "files") {
      const filesState = restoredSurface.state as FilesSurfaceStateV2;
      const filesStateSnapshot = JSON.stringify(filesState);
      const legacyPresentationIntent = surface.state_schema_version === 1
        && isFilesSurfaceStateV1(surface.state)
        && surface.state.mode === "preview"
        ? "renderer_default"
        : undefined;
      return (
        <FilesSurface
          surface_id={surface.surface_id}
          resource_key={surface.resource_key ?? ""}
          state={filesState}
          lifecycle={{ visible: lifecycle?.visible !== false }}
          client={fileResourceClient}
          editor_registry={filesEditorRegistry}
          legacy_presentation_intent={legacyPresentationIntent}
          on_canonical_resource={async (resourceKey) => {
            return await workbenchNavigation.canonicalize_resource(surface.surface_id, {
              surface_type: "files",
              resource_key: resourceKey,
              state: filesState,
            });
          }}
          on_open_file={async (path, openInNewTab = false, sourceRequest) => {
            const authorizedFile = await canonicalizeFileOpenPath(path, sourceRequest);
            const destination = fileOpenDestinationForKind(authorizedFile.kind, fileOpenActions);
            if (destination !== "wardian") {
              await openFileWithSettings(authorizedFile.path, {
                navigation: workbenchNavigation,
                file_open_actions: fileOpenActions,
                external_editor: externalEditor,
                external_editor_custom_executable: externalEditorCustomExecutable,
                verified_renderer_kind: authorizedFile.renderer_kind,
              });
              return;
            }
            if (openInNewTab) {
              openPermanentFileSurface(workbenchNavigation, authorizedFile.path);
              return;
            }
            await workbenchNavigation.rebind_resource(surface.surface_id, {
              surface_type: "files",
              resource_key: fileResourceKey(authorizedFile.path),
              state: filesState,
            });
          }}
          on_state_change={(state) => {
            const store = workbenchPersistence.store.getState();
            const currentSurface = store.document.surfaces[surface.surface_id];
            if (
              currentSurface === undefined
              || (
                legacyPresentationIntent !== undefined
                && currentSurface.state_schema_version !== 1
              )
            ) return;
            const currentRestore = workbenchRegistry.resolve_surface(currentSurface).restore_result;
            if (!currentRestore.ok || JSON.stringify(currentRestore.state) !== filesStateSnapshot) return;
            const result = store.apply_commands([{
              type: "update_surface_state",
              surface_id: surface.surface_id,
              state_schema_version: 2,
              state,
            }]);
            if (result.accepted && legacyPresentationIntent !== undefined) {
              void workbenchPersistence.flush();
            }
          }}
        />
      );
    }

    if (surface.surface_type === "dashboard") {
      return (
        <DashboardSurface
          surface_id={surface.surface_id}
          state={normalizeDashboardSurfaceState(restoredSurface)}
          visibility={visibility}
          // Surface state first, global prefs as the seed. A Dashboard the
          // workbench restored keeps its own configuration even when the global
          // file is missing or failed to write; a freshly opened one inherits
          // what was last used rather than a cold default.
          prefs={
            surfaceConfiguredDashboard(restoredSurface)
              ? normalizeDashboardSurfaceState(restoredSurface).prefs
              : dashboardPrefs
          }
          onPrefsChange={(prefs) => {
            void persistDashboardPrefs(prefs);
            // Mirrored into the surface so the workbench owns a copy too. Both
            // writes happen on every change; neither is behind a save step.
            workbenchPersistence.store.getState().apply_commands([{
              type: "update_surface_state",
              surface_id: surface.surface_id,
              state_schema_version: 1,
              state: { prefs },
            }]);
          }}
          // The instant columns read live agent state; every rate comes from the
          // telemetry store. Joining them here keeps that boundary explicit.
          live={agents.map((agent) => ({
            session_id: agent.session_id,
            status: telemetry[agent.session_id]?.current_status ?? null,
            cpu_usage: telemetry[agent.session_id]?.cpu_usage ?? null,
            memory_mb: telemetry[agent.session_id]?.memory_mb ?? null,
          }))}
          onOpenAgent={(sessionId) => openAgentFromSurface(surface.surface_id, sessionId)}
          onOpenAnalytics={(sessionId) =>
            workbenchNavigation.open({
              surface_type: "analytics",
              // Carries the agent through, so the grid opens on that row
              // instead of a default the reader has to navigate away from.
              state: {
                horizon: "week",
                dimension: "agent",
                measure: "active_ms",
                focus_key: sessionId ?? null,
              },
            })
          }
        />
      );
    }

    if (surface.surface_type === "analytics") {
      return (
        <AnalyticsSurface
          surface_id={surface.surface_id}
          state={normalizeAnalyticsSurfaceState(restoredSurface)}
          visibility={visibility}
          onOpenAgent={(sessionId) => openAgentFromSurface(surface.surface_id, sessionId)}
        />
      );
    }

    if (surface.surface_type === "inbox") {
      return (
        <InboxSurface
          surface_id={surface.surface_id}
          state={{}}
          visibility={visibility}
          onOpenAgent={(sessionId) => openAgentFromSurface(surface.surface_id, sessionId)}
          onSendAgentPrompt={sendCommand}
        />
      );
    }

    if (surface.surface_type === "graph") {
      return (
        <GraphSurface
          surface_id={surface.surface_id}
          state={normalizeGraphSurfaceState(restoredSurface)}
          visibility={visibility}
          filteredAgents={filteredAgents}
          allAgents={agents}
          telemetry={telemetry}
          terminalTitles={terminalTitles}
          currentThoughts={currentThoughts}
          selectedAgentIds={selectedAgentIds}
          offAgentIds={offAgentIds}
          watchlists={watchlists}
          activeList={activeList}
          teams={teams}
          interactions={agentInteractions}
          onSelectionChange={setSelectedAgentIds}
          onOpenAgent={(sessionId) => openAgentFromSurface(surface.surface_id, sessionId)}
          on_state_change={(state) => {
            workbenchPersistence.store.getState().apply_commands([{
              type: "update_surface_state",
              surface_id: surface.surface_id,
              state_schema_version: 1,
              state,
            }]);
          }}
          onInitiateRename={(id) => {
            const agent = agents.find((candidate) => candidate.session_id === id);
            workbenchNavigation.open({ surface_type: "agents-overview" });
            setSelectedAgentIds(new Set([id]));
            setEditingAgentId(id);
            setTempName(agent?.session_name ?? "");
          }}
          onQuery={openAgent}
          onPause={onPause}
          onRestart={onRestart}
          onClear={onClear}
          onClone={onClone}
          onAddToList={handleAddToList}
          onRemoveFromList={handleRemoveFromList}
          onAddAgentsToList={handleAddAgentsToList}
          onRemoveAgentsFromList={handleRemoveAgentsFromList}
          onDelete={onDelete}
          onDeleteAgents={onDeleteAgents}
          deriveCurrentThought={deriveCurrentThought}
        />
      );
    }

    if (surface.surface_type === "garden") {
      return (
        <GardenSurface
          surface_id={surface.surface_id}
          state={normalizeGardenSurfaceState(restoredSurface)}
          visibility={visibility}
          filteredAgents={filteredAgents}
          telemetry={telemetry}
          teams={teams}
          activeList={activeList}
          interactions={agentInteractions}
          selectedAgentIds={selectedAgentIds}
          offAgentIds={offAgentIds}
          onSelectionChange={setSelectedAgentIds}
          onOpenAgent={(sessionId) => openAgentFromSurface(surface.surface_id, sessionId)}
          on_state_change={(state) => {
            workbenchPersistence.store.getState().apply_commands([{
              type: "update_surface_state",
              surface_id: surface.surface_id,
              state_schema_version: 1,
              state,
            }]);
          }}
        />
      );
    }

    if (surface.surface_type === "library") {
      return (
        <LibrarySurface
          surface_id={surface.surface_id}
          selectedAgentIds={selectedAgentIds}
          onOpenWorkflowsView={openWorkflowsView}
        />
      );
    }

    if (surface.surface_type === "workflows") {
      return (
        <WorkflowsSurface
          surface_id={surface.surface_id}
          theme={theme}
          visibility={visibility}
        />
      );
    }

    if (surface.surface_type !== "agents-overview") {
      return (
        <section className="wardian-workbench-placeholder">
          <h2>{surface.surface_type}</h2>
          <p>This registered surface will adopt its Wardian view in the next migration slice.</p>
        </section>
      );
    }

    return (
      <AgentsOverviewSurface
        surface_id={surface.surface_id}
        state={normalizeAgentsOverviewSurfaceState(restoredSurface.state)}
        agents={roster.filteredAgents}
        recentAgentIds={recentAgentIds}
        telemetry={telemetry}
        terminalTitles={terminalTitles}
        currentThoughts={currentThoughts}
        selectedAgentIds={selectedAgentIds}
        offAgentIds={offAgentIds}
        theme={theme}
        visibility={visibility}
        draggedAgentId={draggedAgentId}
        dragOverAgentId={dragOverAgentId}
        editingAgentId={editingAgentId}
        tempName={tempName}
        watchlists={watchlists}
        onCardClick={handleAgentCardClick}
        onDelete={onDelete}
        onRename={renameAgent}
        setEditingAgentId={setEditingAgentId}
        setTempName={setTempName}
        handleTitleChange={handleTitleChange}
        getStatusColorClass={getStatusColorClass}
        deriveCurrentThought={deriveCurrentThought}
        onMouseEnterCard={handleMouseEnterCard}
        onMouseDown={handleMouseDown}
        onMouseUp={handleMouseUp}
        onAddToList={handleAddToList}
        onRemoveFromList={handleRemoveFromList}
        onQuery={(agentId) => scrollToAgent(agentId)}
        onPause={onPause}
        onRestart={onRestart}
        onClear={onClear}
        onClone={onClone}
        on_state_change={(state) => {
          workbenchPersistence.store.getState().apply_commands([{
            type: "update_surface_state",
            surface_id: surface.surface_id,
            state_schema_version: 1,
            state,
          }]);
        }}
      />
    );
  };

  return (
    <AgentResourceContext.Provider value={agentResources}>
      <RosterProvider value={roster}>
        <AppShell
          navigation={workbenchNavigation}
          contentBusy={workbenchResetPending}
          titlebar={<CustomTitleBar
        workbenchBusy={workbenchResetPending}
        leftCollapsed={leftCollapsed}
        setLeftCollapsed={setLeftCollapsed}
        rightCollapsed={rightCollapsed}
        setRightCollapsed={setRightCollapsed}
        leftSidebarWidth={leftSidebarWidth}
        rightSidebarWidth={rightSidebarWidth}
        telemetry={telemetry}
        appTelemetry={appTelemetry}
        agents={agents}
        offAgentIds={offAgentIds}
        titlebarTelemetryVisible={resolvedTitlebarTelemetryVisible}
          />}

          status={workbenchNotice || (updateAvailable && !updateNoticeDismissed) ? (
            <>
              {workbenchNotice ? (
                <div
                  role="status"
                  aria-live="polite"
                  data-testid="workbench-persistence-notice"
                  className="pointer-events-none fixed right-4 top-12 z-40 max-w-md rounded border px-3 py-2 text-xs shadow-lg"
                  style={{
                    background: "var(--color-wardian-card)",
                    borderColor: "var(--color-wardian-border)",
                    color: "var(--color-wardian-text-muted)",
                  }}
                >
                  {workbenchNotice}
                </div>
              ) : null}
              {updateAvailable && !updateNoticeDismissed && appUpdate.availableUpdate ? (
                <UpdateAvailableNotice
                  update={appUpdate.availableUpdate}
                  onDismiss={() => setUpdateNoticeDismissed(true)}
                  onReview={reviewUpdate}
                />
              ) : null}
            </>
          ) : null}

          conflictDialog={(workbenchPersistence.conflict === "revision_conflict"
          || workbenchPersistence.conflict === "future_schema") && (
        <WorkbenchConflictDialog
          mode={workbenchPersistence.conflict}
          resolving={workbenchPersistence.resolving_conflict}
          on_use_disk={() => { void workbenchPersistence.use_disk(); }}
          on_replace_disk={() => { void workbenchPersistence.replace_disk(); }}
          on_export_local={exportLocalWorkbench}
        />
          )}

          leftRail={<SidebarIconRail
          ref={sidebarIconRailRef}
          activeTab={activeTab}
          setActiveTab={setActiveTab}
          setCollapsed={setLeftCollapsed}
          userTerminalOpen={userTerminalOpen}
          settingsOpen={settingsOpen}
          updateAvailable={updateAvailable}
          sourceControlChangeCount={sourceControlStatus.changeCount}
          sourceControlBusy={sourceControlStatus.loading}
          onToggleUserTerminal={toggleUserTerminal}
          onToggleSettings={toggleSettings}
          />}
          leftPane={<SidebarContentPane
          activeTab={activeTab}
          leftCollapsed={leftCollapsed}
          selectedAgentIds={selectedAgentIds}
          setSelectedAgentIds={setSelectedAgentIds}
          agents={agents}
          agentClasses={agentClasses}
          telemetry={telemetry}
          sourceControlStatus={sourceControlStatus}
          turnRevision={changeReviewTurnRevision}
          onAgentsUpdated={fetchAgents}
          broadcastMessage={broadcastMessage}
          setBroadcastMessage={setBroadcastMessage}
          onBroadcast={broadcastInput}
          onOpenSurface={openAuxiliarySurface}
          />}

          mainContent={(
            <WorkbenchHost
              store={workbenchPersistence.store}
              safe_mode={workbenchPersistence.safe_mode}
              registry={workbenchRegistry}
              navigation={workbenchNavigation}
              root_ref={workbenchRootRef}
              new_tab_action={resolvedWorkbenchNewTabAction}
              on_quick_open={openWorkbenchLauncher}
              on_focus_left_dock={focusLeftDock}
              on_focus_right_dock={focusRightDock}
              resource_key={selectedWorkbenchResourceKey}
              render_surface={renderWorkbenchSurface}
              provision_surface_resource={provisionWorkbenchSurfaceResource}
              surface_title={(surface) => {
                if (surface.surface_type === "agent-session" && surface.resource_key) {
                  return agents.find((agent) => agent.session_id === surface.resource_key)
                    ?.session_name ?? `Agent Session: ${surface.resource_key}`;
                }
                return workbenchRegistry.presentation(surface).title;
              }}
            />
          )}
          mainOverlays={<>
            {dirtySurfacePrompt.dialog}
            <CustomCloneModal
            sourceSessionId={customCloneSourceId ?? ""}
            agentClasses={agentClasses}
            isOpen={Boolean(customCloneSourceId)}
            onClose={() => setCustomCloneSourceId(null)}
            onCloned={async () => {
              setCustomCloneSourceId(null);
              await loadWatchlistState();
              await fetchAgents();
            }}
            />
            {userTerminalOpen && (
            <UserTerminalPanel
              theme={theme}
              height={userTerminalHeight}
              selectedWorkspace={selectedUserTerminalWorkspace}
              onHeightChange={setUserTerminalHeight}
              onHide={() => setUserTerminalOpen(false)}
            />
            )}
            {settingsOpen && (
            <SettingsModal appUpdate={appUpdate} isOpen={true} onClose={() => setSettingsOpen(false)} />
            )}
            {onboardingHintsLoaded && guidedTourState === "unseen" ? (
              <OnboardingWelcome onStart={() => beginGuidedTour("setup")} onSkip={leaveGuidedTour} />
            ) : null}
            {onboardingHintsLoaded && guidedTourState === "in_progress" ? (
              <OnboardingTour
                agents={agents}
                reviewMode={guidedTourMode === "review"}
                onClose={leaveGuidedTour}
                onComplete={completeGuidedTour}
                onPrepareAgentCreation={prepareTourAgentCreation}
                onPrepareEvolver={prepareTourEvolver}
                onPrepareGraph={prepareTourGraph}
                onPrepareWorkflow={prepareTourWorkflow}
              />
            ) : null}
          </>}

          roster={<AgentWatchlist
          focus_target_ref={agentWatchlistRef}
          agents={agents}
          telemetry={telemetry}
          terminalTitles={terminalTitles}
          currentThoughts={currentThoughts}
          selectedAgentIds={selectedAgentIds}
          offAgentIds={offAgentIds}
          onSelectionChange={setSelectedAgentIds}
          filter={rosterFilter}
          onFilterChange={setRosterFilter}
          onSelectAgent={selectAgent}
          onRevealAgent={revealAgentInOverview}
          onOpenAgent={openAgent}
          onOpenAgentToSide={openAgentToSide}
          onRename={renameAgent}
          onReorderAgents={async (newOrder) => {
            try { await agentResources.reorder_agents(newOrder); } catch (e) { console.error(e); }
          }}
          onQuery={scrollToAgent}
          onPause={onPause}
          onRestart={onRestart}
          onClear={onClear}
          onClone={onClone}
          onDelete={onDelete}
          onDeleteAgents={onDeleteAgents}
          onAddToList={handleAddToList}
          onRemoveFromList={handleRemoveFromList}
          onAddAgentsToList={handleAddAgentsToList}
          onRemoveAgentsFromList={handleRemoveAgentsFromList}
          collapsed={rightCollapsed}
          watchlists={watchlists}
          activeListId={activeListId}
          onActiveListChange={setActiveListId}
          onWatchlistsChange={persistWatchlists}
          teams={teams}
          onCreateTeam={handleCreateTeam}
          onUngroupTeam={handleUngroupTeam}
          onRenameTeam={handleRenameTeam}
          onAddAgentToTeam={handleAddAgentToTeam}
          onRemoveAgentFromTeam={handleRemoveAgentFromTeam}
          onRemoveAgentFromTeamAtEntry={handleRemoveAgentFromTeamAtEntry}
          onReorderTeamMember={handleReorderTeamMember}
          prefs={watchlistPrefs}
          onPrefsChange={persistWatchlistPrefs}
          interactions={agentInteractions}
          />}
        />
      </RosterProvider>
    </AgentResourceContext.Provider>
  );
}

export default App;
