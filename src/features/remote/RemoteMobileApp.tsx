import React, { useEffect } from "react";
import "../../styles/App.css";
import { RefreshCw, Smartphone } from "lucide-react";
import { RemoteAgentCard } from "./RemoteAgentCard";
import { RemoteCommandBar } from "./RemoteCommandBar";
import { RemotePairingView } from "./RemotePairingView";
import { RemoteQueueView } from "./RemoteQueueView";
import { RemoteWorkflowList } from "./RemoteWorkflowList";
import { useRemoteStore } from "./useRemoteStore";

export const RemoteMobileApp: React.FC = () => {
  const agents = useRemoteStore((state) => state.agents);
  const workflows = useRemoteStore((state) => state.workflows);
  const status = useRemoteStore((state) => state.status);
  const load = useRemoteStore((state) => state.load);
  const disconnectStatusStream = useRemoteStore((state) => state.disconnectStatusStream);

  useEffect(() => {
    void load();
  }, [load]);

  useEffect(() => {
    return () => disconnectStatusStream();
  }, [disconnectStatusStream]);

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

  if (status === "device_revoked") {
    return <RemotePairingView state="revoked" />;
  }

  if (status === "unreachable") {
    return <RemotePairingView state="unreachable" actionLabel="Retry" onAction={() => void load()} />;
  }

  return (
    <main className="min-h-screen bg-wardian-bg text-primary" data-testid="remote-mobile-app">
      <header className="sticky top-0 z-10 border-b border-wardian-border bg-wardian-bg/95 px-4 py-3 backdrop-blur">
        <div className="flex items-center justify-between gap-3">
          <div className="flex min-w-0 items-center gap-2">
            <Smartphone className="h-4 w-4 shrink-0 text-muted-neutral" aria-hidden="true" />
            <h1 className="truncate text-base font-semibold">Wardian</h1>
          </div>
          <button
            type="button"
            aria-label="Refresh remote roster"
            onClick={() => void load()}
            className="inline-flex h-9 w-9 shrink-0 items-center justify-center rounded-md border border-wardian-border text-muted-neutral transition-colors hover:border-[var(--color-wardian-accent)] hover:text-primary"
          >
            <RefreshCw className="h-4 w-4" aria-hidden="true" />
          </button>
        </div>
      </header>

      <section className="p-3">
        <div className="mb-3 flex items-center justify-between gap-3">
          <h2 className="text-xs font-semibold uppercase text-muted-neutral">Agents</h2>
          <span className="text-[11px] text-muted-neutral">{agents.length}</span>
        </div>
        {agents.length === 0 ? (
          <div className="rounded-md border border-dashed border-wardian-border px-3 py-4 text-xs text-muted-neutral">
            No remote agents available.
          </div>
        ) : (
          <div data-testid="remote-agent-list" className="grid grid-cols-1 gap-3">
            {agents.map((agent) => (
              <RemoteAgentCard key={agent.session_id} agent={agent} />
            ))}
          </div>
        )}
      </section>

      <RemoteQueueView />
      <RemoteWorkflowList workflows={workflows} />
      <RemoteCommandBar />
    </main>
  );
};
