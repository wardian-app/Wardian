import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";
import QRCode from "qrcode";
import { RemoteAccessSettings } from "./RemoteAccessSettings";

const mockInvoke = vi.mocked(invoke);
const mockToDataURL = vi.mocked(
  QRCode.toDataURL as unknown as (text: string, options?: unknown) => Promise<string>,
);

vi.mock("qrcode", () => ({
  default: {
    toDataURL: vi.fn(),
  },
}));

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
    mockToDataURL.mockResolvedValue("data:image/png;base64,qr");
    mockInvoke.mockImplementation(async (command) => {
      switch (command) {
        case "load_remote_access_status":
          return "enabled";
        case "load_remote_gateway_config":
          return enabledConfig;
        case "list_remote_devices":
          return [];
        case "list_pending_remote_pairing_requests":
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
    expect(screen.getAllByText(/offer-1/).length).toBeGreaterThanOrEqual(1);
    expect(screen.getByText(/expires/i)).toBeVisible();
    expect(await screen.findByRole("img", { name: /remote pairing qr code/i })).toHaveAttribute(
      "src",
      "data:image/png;base64,qr",
    );
    expect(mockToDataURL).toHaveBeenCalledWith(
      "https://wardian.tailnet.ts.net/remote?pairing_offer_id=offer-1&nonce=nonce&server_fingerprint=fp",
      expect.any(Object),
    );
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
        case "list_pending_remote_pairing_requests":
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
        case "list_pending_remote_pairing_requests":
          return [];
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

  it("requires explicit desktop approval for pending phone pairing requests", async () => {
    mockInvoke.mockImplementation(async (command, args) => {
      switch (command) {
        case "load_remote_access_status":
          return "enabled";
        case "load_remote_gateway_config":
          return enabledConfig;
        case "list_remote_devices":
          return [];
        case "list_pending_remote_pairing_requests":
          return [
            {
              request_id: "pairing-request-1",
              device_label: "Pixel phone",
              public_key_fingerprint: "fp:phone",
              canonical_origin: "https://wardian.tailnet.ts.net",
              submitted_at: "2026-05-21T00:01:00.000Z",
              expires_at: "2026-05-21T00:02:00.000Z",
            },
          ];
        case "approve_remote_pairing_request":
          expect(args).toEqual({ requestId: "pairing-request-1" });
          return [
            {
              device_id: "dev-1",
              label: "Pixel phone",
              public_key_spki_der_base64: "key",
              public_key_fingerprint: "fp:phone",
              created_at: "2026-05-21T00:01:05.000Z",
              last_used_at: null,
              revoked_at: null,
            },
          ];
        default:
          return null;
      }
    });

    render(<RemoteAccessSettings />);

    expect(await screen.findByText("Pixel phone")).toBeVisible();
    expect(screen.getByText("fp:phone")).toBeVisible();
    expect(screen.getAllByText("https://wardian.tailnet.ts.net").length).toBeGreaterThanOrEqual(1);
    expect(screen.getAllByText(/full remote control/i).length).toBeGreaterThanOrEqual(1);

    await userEvent.click(screen.getByRole("button", { name: /approve pixel phone/i }));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("approve_remote_pairing_request", {
        requestId: "pairing-request-1",
      });
    });
    expect(screen.getByText("Pixel phone")).toBeVisible();
  });
});
