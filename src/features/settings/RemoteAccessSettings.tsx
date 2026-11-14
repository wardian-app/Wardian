import React, { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Check, KeyRound, QrCode, RefreshCw, Save, ShieldAlert, Smartphone, Trash2, X } from "lucide-react";
import QRCode from "qrcode";
import type {
  PairingQrPayload,
  RemoteClientPairingQrPayload,
  RemoteAccessStatus,
  RemoteDeviceRecord,
  RemoteGatewayConfig,
  RemotePendingPairingRequest,
  RemoteSetupCheck,
  RemoteSetupCheckResult,
} from "../../types";

const actionButtonClass =
  "inline-flex items-center justify-center gap-2 rounded-md border border-wardian-border bg-wardian-bg px-3 py-2 text-xs font-semibold text-primary transition-colors hover:border-[var(--color-wardian-accent)] disabled:cursor-not-allowed disabled:opacity-60";

const iconButtonClass =
  "inline-flex h-8 w-8 items-center justify-center rounded-md border border-wardian-border text-muted-neutral transition-colors hover:border-[var(--color-wardian-accent)] hover:text-primary disabled:cursor-not-allowed disabled:opacity-50";

const inputClass =
  "mt-1 w-full rounded-md border border-wardian-border bg-wardian-bg px-3 py-2 text-sm text-primary outline-none transition-colors focus:border-[var(--color-wardian-accent)] disabled:cursor-not-allowed disabled:opacity-60";

type PairingDisplayPayload = PairingQrPayload | RemoteClientPairingQrPayload;

const pairingUrlForOffer = (pairing: PairingDisplayPayload): string => {
  if ("credential_kind" in pairing) return JSON.stringify(pairing);
  const url = new URL("/remote", pairing.gateway_origin);
  url.searchParams.set("pairing_offer_id", pairing.pairing_offer_id);
  url.searchParams.set("nonce", pairing.nonce);
  url.searchParams.set("server_fingerprint", pairing.server_identity_fingerprint);
  return url.toString();
};

const originList = (value: string): string[] =>
  value.split(/\r?\n/).map((origin) => origin.trim()).filter(Boolean);

const createGatewayIdentity = (): Pick<
  RemoteGatewayConfig,
  "gateway_identity_public_key" | "gateway_identity_fingerprint"
> => {
  const bytes = new Uint8Array(32);
  if (globalThis.crypto?.getRandomValues) {
    globalThis.crypto.getRandomValues(bytes);
  } else {
    for (let index = 0; index < bytes.length; index += 1) {
      bytes[index] = Math.floor(Math.random() * 256);
    }
  }
  const hex = Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");
  return {
    gateway_identity_public_key: `wardian-local-gateway-v1:${hex}`,
    gateway_identity_fingerprint: hex.match(/.{1,2}/g)?.join(":") ?? hex,
  };
};

const normalizeRemoteOriginInput = (value: string) => {
  const trimmed = value.trim();
  if (!trimmed || trimmed.includes("://")) return trimmed;
  return `https://${trimmed}`;
};

const activeRemoteDevices = (records: RemoteDeviceRecord[] | null | undefined): RemoteDeviceRecord[] =>
  (records ?? []).filter((device) => !device.revoked_at);

export const RemoteAccessSettings: React.FC = () => {
  const [status, setStatus] = useState<RemoteAccessStatus | null>(null);
  const [config, setConfig] = useState<RemoteGatewayConfig | null>(null);
  const [setupCheck, setSetupCheck] = useState<RemoteSetupCheckResult | null>(null);
  const [devices, setDevices] = useState<RemoteDeviceRecord[]>([]);
  const [pendingPairings, setPendingPairings] = useState<RemotePendingPairingRequest[]>([]);
  const [pairing, setPairing] = useState<PairingDisplayPayload | null>(null);
  const [pairingQrDataUrl, setPairingQrDataUrl] = useState("");
  const [remoteEnabledInput, setRemoteEnabledInput] = useState(false);
  const [originInput, setOriginInput] = useState("");
  const [clientOriginAllowlistInput, setClientOriginAllowlistInput] = useState("");
  const [pairingClientOrigin, setPairingClientOrigin] = useState("");
  const [portInput, setPortInput] = useState("41241");
  const [loading, setLoading] = useState(true);
  const [savingConfig, setSavingConfig] = useState(false);
  const [creatingPairing, setCreatingPairing] = useState(false);
  const [revokingDeviceId, setRevokingDeviceId] = useState<string | null>(null);
  const [reviewingPairingId, setReviewingPairingId] = useState<string | null>(null);
  const [error, setError] = useState("");

  const refreshSetupCheck = useCallback(async (delayMs = 0) => {
    if (delayMs > 0) {
      await new Promise((resolve) => window.setTimeout(resolve, delayMs));
    }
    try {
      setSetupCheck(await invoke<RemoteSetupCheckResult>("load_remote_setup_check"));
    } catch (err) {
      setSetupCheck(null);
      setError((current) => current || `Unable to load remote setup checks: ${String(err)}`);
    }
  }, []);

  const refresh = useCallback(async () => {
    setError("");
    setLoading(true);
    try {
      const [loadedStatus, loadedConfig, loadedDevices, loadedPendingPairings] = await Promise.all([
        invoke<RemoteAccessStatus>("load_remote_access_status"),
        invoke<RemoteGatewayConfig | null>("load_remote_gateway_config"),
        invoke<RemoteDeviceRecord[] | null>("list_remote_devices"),
        invoke<RemotePendingPairingRequest[] | null>("list_pending_remote_pairing_requests"),
      ]);
      setStatus(loadedStatus);
      setConfig(loadedConfig);
      setRemoteEnabledInput(loadedConfig?.enabled ?? false);
      setOriginInput(loadedConfig?.canonical_origin ?? "");
      const clientOrigins = loadedConfig?.client_origin_allowlist ?? [];
      setClientOriginAllowlistInput(clientOrigins.join("\n"));
      setPairingClientOrigin((current) =>
        clientOrigins.includes(current) ? current : clientOrigins[0] ?? "",
      );
      setPortInput(loadedConfig?.loopback_port ? String(loadedConfig.loopback_port) : "41241");
      setDevices(activeRemoteDevices(loadedDevices));
      setPendingPairings(loadedPendingPairings ?? []);
      void refreshSetupCheck();
    } catch (err) {
      setError(`Unable to load remote access settings: ${String(err)}`);
    } finally {
      setLoading(false);
    }
  }, [refreshSetupCheck]);

  const refreshPendingPairings = useCallback(async () => {
    try {
      setPendingPairings(
        (await invoke<RemotePendingPairingRequest[] | null>("list_pending_remote_pairing_requests")) ?? [],
      );
    } catch (err) {
      setError(`Unable to refresh pending pairings: ${String(err)}`);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  useEffect(() => {
    if (!pairing) return;
    void refreshPendingPairings();
    const intervalId = window.setInterval(() => {
      void refreshPendingPairings();
    }, 1_000);
    return () => window.clearInterval(intervalId);
  }, [pairing, refreshPendingPairings]);

  useEffect(() => {
    if (!pairing) {
      setPairingQrDataUrl("");
      return;
    }
    let cancelled = false;
    QRCode.toDataURL(pairingUrlForOffer(pairing), {
      errorCorrectionLevel: "M",
      margin: 1,
      width: 192,
    })
      .then((dataUrl) => {
        if (!cancelled) setPairingQrDataUrl(dataUrl);
      })
      .catch((err) => {
        if (!cancelled) setError(`Unable to render pairing QR code: ${String(err)}`);
      });
    return () => {
      cancelled = true;
    };
  }, [pairing]);

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

  const createClientPairingOffer = async () => {
    setError("");
    setCreatingPairing(true);
    try {
      setPairing(
        await invoke<RemoteClientPairingQrPayload>("create_remote_client_pairing_offer", {
          clientOrigin: pairingClientOrigin,
        }),
      );
    } catch (err) {
      setError(`Unable to create paired-client code: ${String(err)}`);
    } finally {
      setCreatingPairing(false);
    }
  };

  const saveGatewayConfig = async () => {
    setError("");
    setSavingConfig(true);
    try {
      const trimmedOrigin = normalizeRemoteOriginInput(originInput);
      const parsedPort = Number(portInput);
      if (remoteEnabledInput && !trimmedOrigin) {
        throw new Error("Enter the HTTPS Tailscale origin before enabling remote access.");
      }
      if (remoteEnabledInput && !trimmedOrigin.toLowerCase().startsWith("https://")) {
        throw new Error("Remote access requires an HTTPS Tailscale origin.");
      }
      if (!Number.isInteger(parsedPort) || parsedPort < 1 || parsedPort > 65535) {
        throw new Error("Local gateway port must be between 1 and 65535.");
      }

      const identity =
        config?.gateway_identity_public_key && config.gateway_identity_fingerprint
          ? {
              gateway_identity_public_key: config.gateway_identity_public_key,
              gateway_identity_fingerprint: config.gateway_identity_fingerprint,
            }
          : createGatewayIdentity();
      const saved = await invoke<RemoteGatewayConfig>("save_remote_gateway_config", {
        config: {
          schema_version: 1,
          enabled: remoteEnabledInput,
          canonical_origin: trimmedOrigin,
          client_origin_allowlist: originList(clientOriginAllowlistInput),
          loopback_host: "127.0.0.1",
          loopback_port: parsedPort,
          ...identity,
        },
      });

      setConfig(saved);
      setRemoteEnabledInput(saved.enabled);
      setOriginInput(saved.canonical_origin);
      setClientOriginAllowlistInput(saved.client_origin_allowlist.join("\n"));
      setPairingClientOrigin((current) =>
        saved.client_origin_allowlist.includes(current)
          ? current
          : saved.client_origin_allowlist[0] ?? "",
      );
      setPortInput(String(saved.loopback_port || parsedPort));
      setStatus(saved.enabled ? "enabled" : "disabled");
      setSetupCheck(null);
      void refreshSetupCheck(saved.enabled ? 500 : 0);
      if (!saved.enabled) {
        setPairing(null);
        setPendingPairings([]);
      }
    } catch (err) {
      setError(`Unable to save remote access settings: ${String(err)}`);
    } finally {
      setSavingConfig(false);
    }
  };

  const revokeDevice = async (device: RemoteDeviceRecord) => {
    setError("");
    setRevokingDeviceId(device.device_id);
    try {
      setDevices(
        activeRemoteDevices(await invoke<RemoteDeviceRecord[]>("revoke_remote_device", { deviceId: device.device_id })),
      );
    } catch (err) {
      setError(`Unable to revoke ${device.label}: ${String(err)}`);
    } finally {
      setRevokingDeviceId(null);
    }
  };

  const approvePairing = async (request: RemotePendingPairingRequest) => {
    setError("");
    setReviewingPairingId(request.request_id);
    try {
      setDevices(
        activeRemoteDevices(await invoke<RemoteDeviceRecord[]>("approve_remote_pairing_request", { requestId: request.request_id })),
      );
      setPendingPairings((requests) => requests.filter((candidate) => candidate.request_id !== request.request_id));
    } catch (err) {
      setError(`Unable to approve ${request.device_label}: ${String(err)}`);
    } finally {
      setReviewingPairingId(null);
    }
  };

  const rejectPairing = async (request: RemotePendingPairingRequest) => {
    setError("");
    setReviewingPairingId(request.request_id);
    try {
      setPendingPairings(
        await invoke<RemotePendingPairingRequest[]>("reject_remote_pairing_request", { requestId: request.request_id }),
      );
    } catch (err) {
      setError(`Unable to reject ${request.device_label}: ${String(err)}`);
    } finally {
      setReviewingPairingId(null);
    }
  };

  const remoteReady = status === "enabled";
  const originLabel = config?.canonical_origin || "Not configured";
  const loopbackLabel = config ? `${config.loopback_host}:${config.loopback_port}` : "Not configured";
  const statusLabel =
    status === "enabled" ? "Enabled" : status === "needs_repair" ? "Needs repair" : status === "disabled" ? "Disabled" : "Loading";
  const statusClass =
    status === "enabled" ? "mt-1 text-wardian-success" : status === "needs_repair" ? "mt-1 text-wardian-warning" : "mt-1 text-muted-neutral";
  const setupNeedsAction = setupCheck?.overall_status === "needs_action";
  const setupStatusLabel = (entryStatus: RemoteSetupCheck["status"]) => {
    switch (entryStatus) {
      case "ok":
        return "Ready";
      case "warning":
        return "Check";
      case "error":
        return "Missing";
      default:
        return "Check";
    }
  };
  const setupStatusClass = (entryStatus: RemoteSetupCheck["status"]) => {
    switch (entryStatus) {
      case "ok":
        return "text-wardian-success";
      case "warning":
        return "text-wardian-warning";
      case "error":
        return "text-wardian-error";
      default:
        return "text-muted-neutral";
    }
  };

  return (
    <section aria-label="Remote Access" className="px-4 py-4">
      <div className="grid gap-4 lg:grid-cols-[minmax(0,1fr)_auto] lg:items-start">
        <div className="min-w-0">
          <div className="flex items-center gap-2">
            <Smartphone className="h-4 w-4 text-muted-neutral" aria-hidden="true" />
            <h4 className="text-sm font-semibold text-primary">Remote Access</h4>
          </div>
          <p className="mt-2 max-w-2xl text-xs leading-relaxed text-muted-neutral">
            Pair a phone for full remote control, or allow a companion client to read the agent roster and chat. Companion clients need an exact origin in the allowlist below.
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
          <label className="text-xs font-medium text-muted-neutral">
            Client origin
            <select
              aria-label="Paired client origin"
              value={pairingClientOrigin}
              onChange={(event) => setPairingClientOrigin(event.target.value)}
              disabled={!config?.client_origin_allowlist?.length}
              className={inputClass}
            >
              <option value="">Choose an allowed origin</option>
              {(config?.client_origin_allowlist ?? []).map((origin) => (
                <option key={origin} value={origin}>{origin}</option>
              ))}
            </select>
          </label>
          <button
            type="button"
            onClick={() => void createClientPairingOffer()}
            disabled={!remoteReady || creatingPairing || !pairingClientOrigin}
            className={actionButtonClass}
          >
            <QrCode className="h-4 w-4" aria-hidden="true" />
            {creatingPairing ? "Creating..." : "Create client pairing code"}
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

      {config?.enabled && setupCheck ? (
        <div className="mt-4 border-t border-wardian-border pt-4" aria-label="Remote setup checks">
          <div
            className={`flex items-start gap-2 text-xs leading-relaxed ${
              setupNeedsAction ? "text-wardian-warning" : "text-wardian-success"
            }`}
            role="status"
          >
            {setupNeedsAction ? (
              <ShieldAlert className="mt-0.5 h-4 w-4 shrink-0" aria-hidden="true" />
            ) : (
              <Check className="mt-0.5 h-4 w-4 shrink-0" aria-hidden="true" />
            )}
            <span>
              {setupNeedsAction
                ? "Remote access is enabled, but Wardian detected setup steps that may prevent your phone from connecting."
                : "Remote access setup is ready for pairing."}
            </span>
          </div>

          <div className="mt-3 divide-y divide-wardian-border rounded-md border border-wardian-border">
            {setupCheck.checks.map((entry) => (
              <div key={entry.id} className="grid gap-2 px-3 py-2 text-xs sm:grid-cols-[minmax(0,1fr)_auto] sm:items-start">
                <div className="min-w-0">
                  <div className="font-semibold text-primary">{entry.label}</div>
                  <div className="mt-1 leading-relaxed text-muted-neutral">{entry.message}</div>
                  {entry.details ? <div className="mt-1 break-all font-mono text-[11px] text-muted-neutral">{entry.details}</div> : null}
                </div>
                <div className={`font-semibold ${setupStatusClass(entry.status)}`}>{setupStatusLabel(entry.status)}</div>
              </div>
            ))}
          </div>

          {setupNeedsAction && setupCheck.setup_command ? (
            <div className="mt-3 text-xs text-muted-neutral">
              <div className="font-semibold text-primary">{setupCheck.setup_command.label}</div>
              <div className="mt-1 break-all font-mono text-[11px] text-muted-neutral">{setupCheck.setup_command.command}</div>
            </div>
          ) : null}
        </div>
      ) : null}

      <div className="mt-4 border-t border-wardian-border pt-4">
        <div className="mb-3 flex items-center justify-between gap-3">
          <div>
            <div className="text-xs font-semibold text-primary">Gateway configuration</div>
            <div className="mt-1 text-xs leading-relaxed text-muted-neutral">
              Tailscale Serve should forward the HTTPS origin to the local Wardian gateway.
            </div>
          </div>
          <label className="flex shrink-0 items-center gap-2 text-xs font-semibold text-primary">
            <input
              type="checkbox"
              checked={remoteEnabledInput}
              onChange={(event) => setRemoteEnabledInput(event.target.checked)}
              className="h-4 w-4 rounded border-wardian-border accent-[var(--color-wardian-accent)]"
            />
            Enable remote access
          </label>
        </div>

        <div className="grid gap-3 sm:grid-cols-[minmax(0,1fr)_11rem]">
          <label className="min-w-0 text-xs font-medium text-muted-neutral">
            Tailscale HTTPS origin
            <input
              type="url"
              value={originInput}
              onChange={(event) => setOriginInput(event.target.value)}
              placeholder="https://machine.tailnet.ts.net"
              className={inputClass}
            />
          </label>
          <label className="text-xs font-medium text-muted-neutral">
            Local gateway port
            <input
              type="number"
              min={1}
              max={65535}
              value={portInput}
              onChange={(event) => setPortInput(event.target.value)}
              className={inputClass}
            />
          </label>
        </div>

        <label className="mt-3 block text-xs font-medium text-muted-neutral">
          Paired-client origin allowlist
          <textarea
            value={clientOriginAllowlistInput}
            onChange={(event) => setClientOriginAllowlistInput(event.target.value)}
            placeholder={"https://client.example\nhttp://localhost:3000"}
            rows={3}
            className={inputClass}
          />
          <span className="mt-1 block leading-relaxed">
            One exact browser origin per line. HTTPS is required except for HTTP loopback origins. An empty list disables paired clients.
          </span>
        </label>

        <div className="mt-3 flex flex-wrap items-center justify-between gap-3">
          <div className="text-xs text-muted-neutral">
            Wardian binds v1 remote access to <span className="font-mono text-primary">127.0.0.1</span>; expose that port through Tailscale Serve.
          </div>
          <button
            type="button"
            onClick={() => void saveGatewayConfig()}
            disabled={savingConfig}
            className={actionButtonClass}
          >
            <Save className="h-4 w-4" aria-hidden="true" />
            {savingConfig ? "Saving..." : "Save gateway settings"}
          </button>
        </div>
      </div>

      {pairing && (
        <div className="mt-4 border-t border-wardian-border pt-4 text-xs">
          <div className="flex items-center gap-2 font-semibold text-primary">
            <KeyRound className="h-4 w-4 text-muted-neutral" aria-hidden="true" />
            <span>Pairing offer {pairing.pairing_offer_id}</span>
          </div>
          <div className="mt-2 grid gap-2 text-muted-neutral sm:grid-cols-2">
            <div>Origin: <span className="font-mono text-primary">{pairing.gateway_origin}</span></div>
            <div>Expires: <span className="font-mono text-primary">{pairing.expires_at}</span></div>
            <div>Credential: <span className="font-mono text-primary">{"credential_kind" in pairing ? "client" : "pwa"}</span></div>
            {"credential_kind" in pairing ? (
              <div>Client origin: <span className="font-mono text-primary">{pairing.client_origin}</span></div>
            ) : null}
            {"credential_kind" in pairing && pairing.read_grants.length > 0 ? (
              <div>Grants: <span className="font-mono text-primary">{pairing.read_grants.join(", ")}</span></div>
            ) : null}
          </div>
          {pairingQrDataUrl && (
            <div className="mt-3 flex flex-wrap items-start gap-3">
              <img
                src={pairingQrDataUrl}
                alt="Remote pairing QR code"
                className="h-48 w-48 rounded-md border border-wardian-border bg-white p-2"
              />
              <div className="min-w-0 flex-1 break-all font-mono text-[11px] text-muted-neutral">
                {pairingUrlForOffer(pairing)}
              </div>
            </div>
          )}
          {"credential_kind" in pairing ? (
            <label className="mt-3 block text-xs text-muted-neutral">
              Client pairing JSON
              <textarea
                aria-label="Client pairing JSON"
                readOnly
                value={JSON.stringify(pairing)}
                onFocus={(event) => event.currentTarget.select()}
                rows={5}
                className={inputClass}
              />
              <span className="mt-1 block">Copy this JSON into the HUD's Wardian connection settings. It expires after one use.</span>
            </label>
          ) : null}
          <div className="mt-2 break-all font-mono text-muted-neutral">Nonce: {pairing.nonce}</div>
        </div>
      )}

      {pendingPairings.length > 0 && (
        <div className="mt-4 border-t border-wardian-border pt-4">
          <div className="mb-2 text-xs font-semibold text-primary">Pending pairing approvals</div>
          <div className="divide-y divide-wardian-border">
            {pendingPairings.map((request) => (
              <div key={request.request_id} className="flex items-center justify-between gap-3 py-2 text-xs">
                <div className="min-w-0">
                  <div className="truncate font-medium text-primary">{request.device_label}</div>
                  <div className="mt-0.5 truncate font-mono text-muted-neutral">{request.public_key_fingerprint}</div>
                  <div className="mt-0.5 truncate font-mono text-muted-neutral">{request.canonical_origin}</div>
                  <div className="mt-0.5 text-muted-neutral">
                    {request.credential_kind === "client" ? "Paired client" : "PWA"}
                    {request.client_origin ? ` · ${request.client_origin}` : ""}
                    {request.read_grants.length ? ` · ${request.read_grants.join(", ")}` : ""}
                    {request.send_grants.length ? ` · ${request.send_grants.join(", ")}` : ""}
                  </div>
                  <div className="mt-0.5 text-wardian-warning">
                    {request.credential_kind === "client"
                      ? request.send_grants.includes("chat:send")
                        ? `${request.read_grants.length ? "Read and " : ""}chat send access pending`
                        : "Read-only access pending"
                      : "Full remote control pending"} until {request.expires_at}
                  </div>
                </div>
                <div className="flex shrink-0 items-center gap-2">
                  <button
                    type="button"
                    aria-label={`Approve ${request.device_label}`}
                    onClick={() => void approvePairing(request)}
                    disabled={reviewingPairingId === request.request_id}
                    className={iconButtonClass}
                  >
                    <Check className="h-4 w-4" aria-hidden="true" />
                  </button>
                  <button
                    type="button"
                    aria-label={`Reject ${request.device_label}`}
                    onClick={() => void rejectPairing(request)}
                    disabled={reviewingPairingId === request.request_id}
                    className={iconButtonClass}
                  >
                    <X className="h-4 w-4" aria-hidden="true" />
                  </button>
                </div>
              </div>
            ))}
          </div>
        </div>
      )}

      <div className="mt-4 border-t border-wardian-border pt-4">
        <div className="mb-2 text-xs font-semibold text-primary">Paired devices</div>
        {devices.length === 0 ? (
          <div className="text-xs text-muted-neutral">No paired devices.</div>
        ) : (
          <div className="divide-y divide-wardian-border">
            {devices.map((device) => (
              <div key={device.device_id} className="flex items-center justify-between gap-3 py-2 text-xs">
                <div className="min-w-0">
                  <div className="truncate font-medium text-primary">{device.label}</div>
                  <div className="mt-0.5 truncate font-mono text-muted-neutral">{device.public_key_fingerprint}</div>
                  <div className="mt-0.5 text-muted-neutral">
                    {device.credential_kind === "client" ? "Paired client" : "PWA"}
                    {device.client_origin ? ` · ${device.client_origin}` : ""}
                    {device.read_grants.length ? ` · ${device.read_grants.join(", ")}` : ""}
                    {device.send_grants.length ? ` · ${device.send_grants.join(", ")}` : ""}
                  </div>
                  <div className="mt-0.5 text-muted-neutral">Paired {device.created_at}</div>
                </div>
                <button
                  type="button"
                  aria-label={`Revoke ${device.label}`}
                  onClick={() => void revokeDevice(device)}
                  disabled={revokingDeviceId === device.device_id}
                  className={iconButtonClass}
                >
                  <Trash2 className="h-4 w-4" aria-hidden="true" />
                </button>
              </div>
            ))}
          </div>
        )}
      </div>

      {error && <div className="mt-3 text-xs text-wardian-error">{error}</div>}
    </section>
  );
};
