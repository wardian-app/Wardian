import React, { useEffect, useRef, useState } from "react";
import "../../styles/App.css";
import { RefreshCw } from "lucide-react";
import { RemoteAgentDetailView } from "./RemoteAgentDetailView";
import { RemoteBottomNav } from "./RemoteBottomNav";
import { RemotePairingView } from "./RemotePairingView";
import { RemoteQueueView } from "./RemoteQueueView";
import { RemoteSettingsView } from "./RemoteSettingsView";
import { RemoteWatchlistView } from "./RemoteWatchlistView";
import { useRemoteStore } from "./useRemoteStore";
import { useSettingsStore } from "../../store/useSettingsStore";

const REMOTE_HISTORY_SETTINGS_VIEW = "settings";

const currentHistoryStateObject = () =>
  typeof window.history.state === "object" && window.history.state !== null && !Array.isArray(window.history.state)
    ? window.history.state
    : {};

const isRemoteSettingsHistoryState = (state = window.history.state) =>
  typeof state === "object" &&
  state !== null &&
  !Array.isArray(state) &&
  (state as { wardian_remote_view?: unknown }).wardian_remote_view === REMOTE_HISTORY_SETTINGS_VIEW;

const resolveTheme = (theme: "system" | "light" | "dark") => {
  if (theme !== "system") return theme;
  return window.matchMedia?.("(prefers-color-scheme: light)")?.matches ? "light" : "dark";
};

export const RemoteMobileApp: React.FC = () => {
  const agents = useRemoteStore((state) => state.agents);
  const status = useRemoteStore((state) => state.status);
  const activeAgentId = useRemoteStore((state) => state.activeAgentId);
  const activeRemoteTab = useRemoteStore((state) => state.activeRemoteTab);
  const theme = useSettingsStore((state) => state.theme);
  const load = useRemoteStore((state) => state.load);
  const refresh = useRemoteStore((state) => state.refresh);
  const disconnectStatusStream = useRemoteStore((state) => state.disconnectStatusStream);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const settingsOpenRef = useRef(false);
  const resumeRefreshInFlightRef = useRef(false);

  useEffect(() => {
    settingsOpenRef.current = settingsOpen;
  }, [settingsOpen]);

  useEffect(() => {
    void load();
  }, [load]);

  useEffect(() => {
    return () => disconnectStatusStream();
  }, [disconnectStatusStream]);

  useEffect(() => {
    const closeOverlayOnHistoryBack = () => {
      if (settingsOpenRef.current) {
        setSettingsOpen(false);
        return;
      }
      const state = useRemoteStore.getState();
      if (state.activeAgentId) state.closeAgent({ syncHistory: false });
    };

    window.addEventListener("popstate", closeOverlayOnHistoryBack);
    return () => {
      window.removeEventListener("popstate", closeOverlayOnHistoryBack);
    };
  }, []);

  useEffect(() => {
    const mediaQuery = window.matchMedia?.("(prefers-color-scheme: light)");
    const applyTheme = () => {
      document.documentElement.setAttribute("data-theme", resolveTheme(theme));
    };
    applyTheme();
    mediaQuery?.addEventListener?.("change", applyTheme);
    return () => mediaQuery?.removeEventListener?.("change", applyTheme);
  }, [theme]);

  useEffect(() => {
    const refreshOnResume = () => {
      if (document.visibilityState === "hidden") return;
      if (useRemoteStore.getState().status !== "ready") return;
      if (resumeRefreshInFlightRef.current) return;
      resumeRefreshInFlightRef.current = true;
      void refresh().finally(() => {
        resumeRefreshInFlightRef.current = false;
      });
    };

    window.addEventListener("focus", refreshOnResume);
    window.addEventListener("pageshow", refreshOnResume);
    document.addEventListener("visibilitychange", refreshOnResume);
    return () => {
      window.removeEventListener("focus", refreshOnResume);
      window.removeEventListener("pageshow", refreshOnResume);
      document.removeEventListener("visibilitychange", refreshOnResume);
    };
  }, [refresh]);

  if (status === "loading") {
    return (
      <main className="flex min-h-screen items-center justify-center bg-wardian-bg p-4 text-primary">
        <div className="inline-flex items-center gap-2 text-sm text-muted-neutral">
          <RefreshCw className="h-4 w-4 animate-spin" aria-hidden="true" />
          Loading Wardian...
        </div>
      </main>
    );
  }

  if (status === "session_expired") {
    return <RemotePairingView state="session_expired" actionLabel="Re-authenticate" onAction={() => void load()} />;
  }

  if (status === "pairing_pending") {
    return <RemotePairingView state="pending" />;
  }

  if (status === "pairing_expired") {
    return <RemotePairingView state="expired" actionLabel="Retry" onAction={() => void load()} />;
  }

  if (status === "gateway_identity_changed") {
    return <RemotePairingView state="identity_changed" />;
  }

  if (status === "device_revoked") {
    return <RemotePairingView state="revoked" />;
  }

  if (status === "unreachable") {
    return <RemotePairingView state="unreachable" actionLabel="Retry" onAction={() => void load()} />;
  }

  const activeAgent = agents.find((agent) => agent.session_id === activeAgentId);
  if (activeAgent) {
    return <RemoteAgentDetailView agent={activeAgent} />;
  }

  const openSettings = () => {
    setSettingsOpen(true);
    try {
      window.history.pushState(
        {
          ...currentHistoryStateObject(),
          wardian_remote_view: REMOTE_HISTORY_SETTINGS_VIEW,
        },
        "",
        `${window.location.pathname}${window.location.search}${window.location.hash}`,
      );
    } catch {
      // Some embedded browsers restrict history writes; the in-app Back button remains available.
    }
  };

  const closeSettings = () => {
    if (isRemoteSettingsHistoryState()) {
      try {
        window.history.back();
        setSettingsOpen(false);
        return;
      } catch {
        // If browser history cannot move, still close the in-app settings view.
      }
    }
    setSettingsOpen(false);
  };

  if (settingsOpen) {
    return <RemoteSettingsView onClose={closeSettings} />;
  }

  return (
    <main className="flex h-dvh flex-col overflow-hidden bg-wardian-bg text-primary" data-testid="remote-mobile-app">
      {activeRemoteTab === "watchlist" ? (
        <RemoteWatchlistView onOpenSettings={openSettings} />
      ) : activeRemoteTab === "queue" ? (
        <RemoteQueueView />
      ) : (
        <RemotePlaceholderPanel tab={activeRemoteTab} />
      )}
      <RemoteBottomNav />
    </main>
  );
};

function RemotePlaceholderPanel({ tab }: { tab: "workflows" | "queue" | "garden" | "library" }) {
  const label =
    tab === "workflows" ? "Workflows" : tab === "queue" ? "Queue" : tab === "garden" ? "Garden" : "Library";

  return (
    <section className="flex min-h-0 flex-1 flex-col">
      <header className="shrink-0 border-b border-wardian-border bg-wardian-bg/95 px-4 py-3 backdrop-blur">
        <h1 className="truncate text-base font-semibold text-primary">{label}</h1>
      </header>
      <div className="flex flex-1 items-center justify-center px-6 text-center text-sm text-muted-neutral">
        {label} is not available in the mobile PWA yet.
      </div>
    </section>
  );
}
