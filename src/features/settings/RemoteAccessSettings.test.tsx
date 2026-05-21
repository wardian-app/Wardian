import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { RemoteAccessSettings } from "./RemoteAccessSettings";

const mockInvoke = vi.mocked(invoke);

const enabledConfig = {
  schema_version: 1,
  enabled: true,
  canonical_origin: "https://wardian.tailnet.ts.net",
  loopback_host: "127.0.0.1",
  loopback_port: 41241,
  gateway_identity_public_key: "pub",
  gateway_identity_fingerprint: "fp",
};

describe("RemoteAccessSettings", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockInvoke.mockImplementation(async (command) => {
      switch (command) {
        case "load_remote_access_status":
          return "enabled";
        case "load_remote_gateway_config":
          return enabledConfig;
        case "list_remote_devices":
          return [];
        case "create_remote_pairing_offer":
          return {
            gateway_origin: "https://wardian.tailnet.ts.net",
            pairing_offer_id: "offer-1",
            expires_at: "2026-05-21T00:02:00.000Z",
            nonce: "nonce",
            server_identity_fingerprint: "fp",
          };
        default:
          return null;
      }
    });
  });

  it("shows the full-control warning before creating a pairing offer", async () => {
    render(<RemoteAccessSettings />);

    await screen.findByText("Remote Access");
    expect(screen.getByText(/full remote control/i)).toBeVisible();

    await userEvent.click(screen.getByRole("button", { name: /create pairing code/i }));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("create_remote_pairing_offer");
    });
    expect(screen.getByText(/offer-1/)).toBeVisible();
    expect(screen.getByText(/expires/i)).toBeVisible();
  });

  it("uses the backend access status before enabling pairing", async () => {
    mockInvoke.mockImplementation(async (command) => {
      switch (command) {
        case "load_remote_access_status":
          return "needs_repair";
        case "load_remote_gateway_config":
          return {
            ...enabledConfig,
            canonical_origin: "http://wardian.tailnet.ts.net",
          };
        case "list_remote_devices":
          return [];
        default:
          return null;
      }
    });

    render(<RemoteAccessSettings />);

    expect(await screen.findByText("Needs repair")).toBeVisible();
    expect(screen.getByText("127.0.0.1:41241")).toBeVisible();
    expect(screen.getByRole("button", { name: /create pairing code/i })).toBeDisabled();
    expect(mockInvoke).toHaveBeenCalledWith("load_remote_access_status");
  });

  it("revokes a paired device through the remote device command", async () => {
    mockInvoke.mockImplementation(async (command) => {
      switch (command) {
        case "load_remote_access_status":
          return "enabled";
        case "load_remote_gateway_config":
          return enabledConfig;
        case "list_remote_devices":
          return [
            {
              device_id: "dev-1",
              label: "Phone",
              public_key_spki_der_base64: "key",
              public_key_fingerprint: "fp",
              created_at: "2026-05-21T00:00:00.000Z",
              last_used_at: null,
              revoked_at: null,
            },
          ];
        case "revoke_remote_device":
          return [
            {
              device_id: "dev-1",
              label: "Phone",
              public_key_spki_der_base64: "key",
              public_key_fingerprint: "fp",
              created_at: "2026-05-21T00:00:00.000Z",
              last_used_at: null,
              revoked_at: "2026-05-21T00:05:00.000Z",
            },
          ];
        default:
          return null;
      }
    });

    render(<RemoteAccessSettings />);

    expect(await screen.findByText("Phone")).toBeVisible();
    await userEvent.click(screen.getByRole("button", { name: /revoke phone/i }));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("revoke_remote_device", { deviceId: "dev-1" });
    });
    expect(screen.getByText(/revoked/i)).toBeVisible();
  });
});
