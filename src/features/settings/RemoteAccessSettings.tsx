import React, { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { KeyRound, QrCode, RefreshCw, ShieldAlert, Smartphone, Trash2 } from "lucide-react";
import type { PairingQrPayload, RemoteAccessStatus, RemoteDeviceRecord, RemoteGatewayConfig } from "../../types";

const actionButtonClass =
  "inline-flex items-center justify-center gap-2 rounded-md border border-wardian-border bg-wardian-bg px-3 py-2 text-xs font-semibold text-primary transition-colors hover:border-[var(--color-wardian-accent)] disabled:cursor-not-allowed disabled:opacity-60";

const iconButtonClass =
  "inline-flex h-8 w-8 items-center justify-center rounded-md border border-wardian-border text-muted-neutral transition-colors hover:border-[var(--color-wardian-accent)] hover:text-primary disabled:cursor-not-allowed disabled:opacity-50";

export const RemoteAccessSettings: React.FC = () => {
  const [status, setStatus] = useState<RemoteAccessStatus | null>(null);
  const [config, setConfig] = useState<RemoteGatewayConfig | null>(null);
  const [devices, setDevices] = useState<RemoteDeviceRecord[]>([]);
  const [pairing, setPairing] = useState<PairingQrPayload | null>(null);
  const [loading, setLoading] = useState(true);
  const [creatingPairing, setCreatingPairing] = useState(false);
  const [revokingDeviceId, setRevokingDeviceId] = useState<string | null>(null);
  const [error, setError] = useState("");

  const refresh = useCallback(async () => {
    setError("");
    setLoading(true);
    try {
      const [loadedStatus, loadedConfig, loadedDevices] = await Promise.all([
        invoke<RemoteAccessStatus>("load_remote_access_status"),
        invoke<RemoteGatewayConfig | null>("load_remote_gateway_config"),
        invoke<RemoteDeviceRecord[]>("list_remote_devices"),
      ]);
      setStatus(loadedStatus);
      setConfig(loadedConfig);
      setDevices(loadedDevices);
    } catch (err) {
      setError(`Unable to load remote access settings: ${String(err)}`);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const createPairingOffer = async () => {
    setError("");
    setCreatingPairing(true);
    try {
      setPairing(await invoke<PairingQrPayload>("create_remote_pairing_offer"));
    } catch (err) {
      setError(`Unable to create pairing code: ${String(err)}`);
    } finally {
      setCreatingPairing(false);
    }
  };

  const revokeDevice = async (device: RemoteDeviceRecord) => {
    setError("");
    setRevokingDeviceId(device.device_id);
    try {
      setDevices(await invoke<RemoteDeviceRecord[]>("revoke_remote_device", { deviceId: device.device_id }));
    } catch (err) {
      setError(`Unable to revoke ${device.label}: ${String(err)}`);
    } finally {
      setRevokingDeviceId(null);
    }
  };

  const remoteReady = status === "enabled";
  const originLabel = config?.canonical_origin || "Not configured";
  const loopbackLabel = config ? `${config.loopback_host}:${config.loopback_port}` : "Not configured";
  const statusLabel =
    status === "enabled" ? "Enabled" : status === "needs_repair" ? "Needs repair" : status === "disabled" ? "Disabled" : "Loading";
  const statusClass =
    status === "enabled" ? "mt-1 text-wardian-success" : status === "needs_repair" ? "mt-1 text-wardian-warning" : "mt-1 text-muted-neutral";

  return (
    <section aria-label="Remote Access" className="px-4 py-4">
      <div className="grid gap-4 lg:grid-cols-[minmax(0,1fr)_auto] lg:items-start">
        <div className="min-w-0">
          <div className="flex items-center gap-2">
            <Smartphone className="h-4 w-4 text-muted-neutral" aria-hidden="true" />
            <h4 className="text-sm font-semibold text-primary">Remote Access</h4>
          </div>
          <p className="mt-2 max-w-2xl text-xs leading-relaxed text-muted-neutral">
            Pair a phone through the local HTTPS gateway. A paired phone can send prompts, run workflows, and stop agents with full remote control.
          </p>
          <div className="mt-3 flex items-start gap-2 text-xs leading-relaxed text-wardian-warning">
            <ShieldAlert className="mt-0.5 h-4 w-4 shrink-0" aria-hidden="true" />
            <span>Only create a pairing code while your phone is on your trusted private network.</span>
          </div>
        </div>

        <div className="flex flex-wrap items-center gap-2 lg:justify-end">
          <button
            type="button"
            aria-label="Refresh remote access settings"
            onClick={() => void refresh()}
            disabled={loading}
            className={iconButtonClass}
          >
            <RefreshCw className="h-4 w-4" aria-hidden="true" />
          </button>
          <button
            type="button"
            onClick={() => void createPairingOffer()}
            disabled={!remoteReady || creatingPairing}
            className={actionButtonClass}
          >
            <QrCode className="h-4 w-4" aria-hidden="true" />
            {creatingPairing ? "Creating..." : "Create pairing code"}
          </button>
        </div>
      </div>

      <dl className="mt-4 grid gap-3 text-xs sm:grid-cols-3">
        <div>
          <dt className="text-muted-neutral">Tailscale origin</dt>
          <dd className="mt-1 font-mono text-primary">{originLabel}</dd>
        </div>
        <div>
          <dt className="text-muted-neutral">Local gateway</dt>
          <dd className="mt-1 font-mono text-primary">{loopbackLabel}</dd>
        </div>
        <div>
          <dt className="text-muted-neutral">Status</dt>
          <dd className={statusClass}>{statusLabel}</dd>
        </div>
      </dl>

      {pairing && (
        <div className="mt-4 border-t border-wardian-border pt-4 text-xs">
          <div className="flex items-center gap-2 font-semibold text-primary">
            <KeyRound className="h-4 w-4 text-muted-neutral" aria-hidden="true" />
            <span>Pairing offer {pairing.pairing_offer_id}</span>
          </div>
          <div className="mt-2 grid gap-2 text-muted-neutral sm:grid-cols-2">
            <div>Origin: <span className="font-mono text-primary">{pairing.gateway_origin}</span></div>
            <div>Expires: <span className="font-mono text-primary">{pairing.expires_at}</span></div>
          </div>
          <div className="mt-2 break-all font-mono text-muted-neutral">Nonce: {pairing.nonce}</div>
        </div>
      )}

      <div className="mt-4 border-t border-wardian-border pt-4">
        <div className="mb-2 text-xs font-semibold text-primary">Paired devices</div>
        {devices.length === 0 ? (
          <div className="text-xs text-muted-neutral">No paired devices.</div>
        ) : (
          <div className="divide-y divide-wardian-border">
            {devices.map((device) => {
              const revoked = Boolean(device.revoked_at);
              return (
                <div key={device.device_id} className="flex items-center justify-between gap-3 py-2 text-xs">
                  <div className="min-w-0">
                    <div className="truncate font-medium text-primary">{device.label}</div>
                    <div className="mt-0.5 truncate font-mono text-muted-neutral">{device.public_key_fingerprint}</div>
                    <div className={revoked ? "mt-0.5 text-wardian-error" : "mt-0.5 text-muted-neutral"}>
                      {revoked ? `Revoked ${device.revoked_at}` : `Paired ${device.created_at}`}
                    </div>
                  </div>
                  <button
                    type="button"
                    aria-label={`Revoke ${device.label}`}
                    onClick={() => void revokeDevice(device)}
                    disabled={revoked || revokingDeviceId === device.device_id}
                    className={iconButtonClass}
                  >
                    <Trash2 className="h-4 w-4" aria-hidden="true" />
                  </button>
                </div>
              );
            })}
          </div>
        )}
      </div>

      {error && <div className="mt-3 text-xs text-wardian-error">{error}</div>}
    </section>
  );
};
