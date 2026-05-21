export type RemoteAccessStatus = "disabled" | "enabled" | "needs_repair";

export interface RemoteGatewayConfig {
  schema_version: 1;
  enabled: boolean;
  canonical_origin: string;
  loopback_host: string;
  loopback_port: number;
  gateway_identity_public_key: string;
  gateway_identity_fingerprint: string;
}

export interface PairingQrPayload {
  gateway_origin: string;
  pairing_offer_id: string;
  expires_at: string;
  nonce: string;
  server_identity_fingerprint: string;
}

export interface RemoteDeviceRecord {
  device_id: string;
  label: string;
  public_key_spki_der_base64: string;
  public_key_fingerprint: string;
  created_at: string;
  last_used_at: string | null;
  revoked_at: string | null;
}
