import React from "react";
import { ShieldAlert } from "lucide-react";

type RemotePairingState =
  | "expired"
  | "identity_changed"
  | "pending"
  | "revoked"
  | "session_expired"
  | "unreachable";

const labels: Record<RemotePairingState, string> = {
  expired: "Pairing expired.",
  identity_changed: "Gateway identity changed. Scan a fresh QR code to re-pair.",
  pending: "Waiting for desktop approval.",
  revoked: "This device has been revoked.",
  session_expired: "Session expired. Re-authentication is required.",
  unreachable: "Desktop unreachable.",
};

interface RemotePairingViewProps {
  state: RemotePairingState;
  actionLabel?: string;
  onAction?: () => void;
}

export const RemotePairingView: React.FC<RemotePairingViewProps> = ({ state, actionLabel, onAction }) => (
  <main className="flex min-h-screen items-center justify-center bg-wardian-bg p-4 text-primary">
    <div className="w-full max-w-sm rounded-md border border-wardian-border bg-wardian-card p-4 text-sm">
      <div className="mb-3 flex items-center gap-2 text-wardian-warning">
        <ShieldAlert className="h-4 w-4" aria-hidden="true" />
        <span className="font-semibold">Remote Access</span>
      </div>
      <p className="leading-relaxed text-muted-neutral">{labels[state]}</p>
      {actionLabel && onAction && (
        <button
          type="button"
          onClick={onAction}
          className="mt-4 inline-flex min-h-10 w-full items-center justify-center rounded-md border border-wardian-border px-3 py-2 text-sm font-semibold text-primary transition-colors hover:border-[var(--color-wardian-accent)]"
        >
          {actionLabel}
        </button>
      )}
    </div>
  </main>
);
