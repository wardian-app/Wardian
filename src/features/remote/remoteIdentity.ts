import type { AuthChallengeResponse } from "../../types";

const IDENTITY_DB_NAME = "wardian-remote-identity";
const IDENTITY_DB_VERSION = 1;
const IDENTITY_STORE_NAME = "identity";
const DEVICE_IDENTITY_KEY = "device";

export interface RemoteDeviceKeyPair {
  privateKey: CryptoKey;
  publicKeySpkiDerBase64: string;
}

export interface StoredRemoteDeviceIdentity {
  device_id: string;
  public_key_fingerprint: string;
  server_identity_fingerprint: string;
  origin: string;
  private_key: CryptoKey;
  paired_at: string | null;
  pending_pairing_request_id?: string;
}

const subtle = () => {
  if (!globalThis.crypto?.subtle) {
    throw new Error("webcrypto_unavailable");
  }
  return globalThis.crypto.subtle;
};

export async function createRemoteDeviceKeyPair(): Promise<RemoteDeviceKeyPair> {
  // WebCrypto still permits exporting the public key; `extractable: false` protects the private key.
  const keyPair = await subtle().generateKey(
    { name: "ECDSA", namedCurve: "P-256" },
    false,
    ["sign", "verify"],
  );
  const publicKeySpkiDer = await subtle().exportKey("spki", keyPair.publicKey);
  return {
    privateKey: keyPair.privateKey,
    publicKeySpkiDerBase64: arrayBufferToBase64(publicKeySpkiDer),
  };
}

export function authSignatureMessageBytes(challenge: AuthChallengeResponse): Uint8Array {
  return new TextEncoder().encode(
    `wardian.remote.auth.v1\norigin:${challenge.origin}\ndevice:${challenge.device_id}\nchallenge:${challenge.challenge_id}\nnonce:${challenge.nonce}`,
  );
}

export async function signRemoteAuthChallenge(
  privateKey: CryptoKey,
  challenge: AuthChallengeResponse,
): Promise<string> {
  const signature = await subtle().sign(
    { name: "ECDSA", hash: "SHA-256" },
    privateKey,
    authSignatureMessageBytes(challenge),
  );
  return arrayBufferToBase64(rawP256SignatureToDer(signature));
}

export function defaultRemoteDeviceLabel(): string {
  const platform = globalThis.navigator?.platform?.trim();
  if (platform) return `Phone ${platform}`.slice(0, 80);
  return "Phone";
}

export async function saveStoredRemoteIdentity(identity: StoredRemoteDeviceIdentity): Promise<void> {
  const store = await identityStore("readwrite");
  await requestToPromise(store.put(identity, DEVICE_IDENTITY_KEY));
}

export async function loadStoredRemoteIdentity(): Promise<StoredRemoteDeviceIdentity | null> {
  const store = await identityStore("readonly");
  const value = await requestToPromise<StoredRemoteDeviceIdentity | undefined>(
    store.get(DEVICE_IDENTITY_KEY),
  );
  return value ?? null;
}

export async function clearStoredRemoteIdentity(): Promise<void> {
  const store = await identityStore("readwrite");
  await requestToPromise(store.delete(DEVICE_IDENTITY_KEY));
}

async function identityStore(mode: IDBTransactionMode): Promise<IDBObjectStore> {
  if (!globalThis.indexedDB) {
    throw new Error("indexeddb_unavailable");
  }
  const db = await openIdentityDb();
  return db.transaction(IDENTITY_STORE_NAME, mode).objectStore(IDENTITY_STORE_NAME);
}

function openIdentityDb(): Promise<IDBDatabase> {
  return new Promise((resolve, reject) => {
    const request = globalThis.indexedDB.open(IDENTITY_DB_NAME, IDENTITY_DB_VERSION);
    request.onupgradeneeded = () => {
      const db = request.result;
      if (!db.objectStoreNames.contains(IDENTITY_STORE_NAME)) {
        db.createObjectStore(IDENTITY_STORE_NAME);
      }
    };
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error ?? new Error("indexeddb_open_failed"));
  });
}

function requestToPromise<T>(request: IDBRequest<T>): Promise<T> {
  return new Promise((resolve, reject) => {
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error ?? new Error("indexeddb_request_failed"));
  });
}

export function rawP256SignatureToDer(signature: ArrayBuffer): ArrayBuffer {
  const bytes = new Uint8Array(signature);
  if (bytes.byteLength === 64) {
    const r = encodeDerInteger(bytes.slice(0, 32));
    const s = encodeDerInteger(bytes.slice(32));
    const sequenceLength = r.byteLength + s.byteLength;
    const result = new Uint8Array(1 + derLengthBytes(sequenceLength).byteLength + sequenceLength);
    let offset = 0;
    result[offset++] = 0x30;
    const length = derLengthBytes(sequenceLength);
    result.set(length, offset);
    offset += length.byteLength;
    result.set(r, offset);
    offset += r.byteLength;
    result.set(s, offset);
    return result.buffer;
  }
  if (bytes[0] === 0x30) {
    return bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength);
  }
  throw new Error("invalid_p256_signature");
}

function encodeDerInteger(value: Uint8Array): Uint8Array {
  let start = 0;
  while (start < value.byteLength - 1 && value[start] === 0) start += 1;
  const trimmed = value.slice(start);
  const needsSignPadding = (trimmed[0] & 0x80) !== 0;
  const length = trimmed.byteLength + (needsSignPadding ? 1 : 0);
  const encoded = new Uint8Array(1 + derLengthBytes(length).byteLength + length);
  let offset = 0;
  encoded[offset++] = 0x02;
  const lengthBytes = derLengthBytes(length);
  encoded.set(lengthBytes, offset);
  offset += lengthBytes.byteLength;
  if (needsSignPadding) {
    encoded[offset++] = 0;
  }
  encoded.set(trimmed, offset);
  return encoded;
}

function derLengthBytes(length: number): Uint8Array {
  if (length < 0x80) return new Uint8Array([length]);
  const bytes: number[] = [];
  let remaining = length;
  while (remaining > 0) {
    bytes.unshift(remaining & 0xff);
    remaining >>= 8;
  }
  return new Uint8Array([0x80 | bytes.length, ...bytes]);
}

function arrayBufferToBase64(buffer: ArrayBuffer): string {
  const bytes = new Uint8Array(buffer);
  let binary = "";
  for (const byte of bytes) {
    binary += String.fromCharCode(byte);
  }
  return btoa(binary);
}
