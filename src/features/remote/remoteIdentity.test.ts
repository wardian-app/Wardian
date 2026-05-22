import { createPublicKey, createVerify } from "node:crypto";
import { describe, expect, it } from "vitest";
import {
  authSignatureMessageBytes,
  createRemoteDeviceKeyPair,
  rawP256SignatureToDer,
  signRemoteAuthChallenge,
} from "./remoteIdentity";

describe("remoteIdentity", () => {
  it("creates a non-extractable P-256 device key and signs the backend challenge format", async () => {
    const identity = await createRemoteDeviceKeyPair();
    const challenge = {
      challenge_id: "challenge-1",
      device_id: "dev-1",
      origin: "https://wardian.tailnet.ts.net",
      server_identity_fingerprint: "desktop-fp",
      nonce: "nonce-1",
      expires_at: "2026-05-21T08:01:00.000Z",
      audience: "wardian_remote_pwa",
    };

    expect(identity.privateKey.extractable).toBe(false);
    expect(authSignatureMessageBytes(challenge)).toEqual(
      new TextEncoder().encode(
        "wardian.remote.auth.v1\norigin:https://wardian.tailnet.ts.net\ndevice:dev-1\nchallenge:challenge-1\nnonce:nonce-1",
      ),
    );

    const signatureDerBase64 = await signRemoteAuthChallenge(identity.privateKey, challenge);
    const verifier = createVerify("SHA256");
    verifier.update(Buffer.from(authSignatureMessageBytes(challenge)));

    expect(
      verifier.verify(
        createPublicKey({
          key: Buffer.from(identity.publicKeySpkiDerBase64, "base64"),
          type: "spki",
          format: "der",
        }),
        Buffer.from(signatureDerBase64, "base64"),
      ),
    ).toBe(true);
  });

  it("DER-encodes raw P-256 signatures that start with a sequence byte", () => {
    const rawSignature = new Uint8Array(64);
    rawSignature[0] = 0x30;
    rawSignature[31] = 0x01;
    rawSignature[63] = 0x02;

    const derSignature = new Uint8Array(rawP256SignatureToDer(rawSignature.buffer));

    expect(Array.from(derSignature.slice(0, 4))).toEqual([0x30, 0x25, 0x02, 0x20]);
  });
});
