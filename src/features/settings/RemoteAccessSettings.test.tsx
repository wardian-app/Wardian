import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";
import QRCode from "qrcode";
import { RemoteAccessSettings } from "./RemoteAccessSettings";
import type { RemoteSetupCheckResult } from "../../types";

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

const readySetupCheck = {
  overall_status: "ready",
  checks: [
    {
      id: "wardian_config",
      label: "Wardian remote config",
      status: "ok",
      message: "Remote access is enabled in Wardian.",
      details: "127.0.0.1:41241",
    },
    {
      id: "tailscale_serve",
      label: "Tailscale Serve",
      status: "ok",
      message: "Tailscale Serve forwards HTTPS traffic to Wardian's local gateway.",
      details: "http://127.0.0.1:41241",
    },
    {
      id: "https_gateway",
      label: "HTTPS remote gateway",
      status: "ok",
      message: "The HTTPS remote gateway is reachable.",
      details: "https://wardian.tailnet.ts.net/remote/api/health",
    },
  ],
  inferred_origin: "https://wardian.tailnet.ts.net",
  serve_target: "http://127.0.0.1:41241",
  setup_command: {
    label: "Configure Tailscale Serve",
    command: "tailscale serve --bg --https=443 http://127.0.0.1:41241",
  },
} satisfies RemoteSetupCheckResult;

const missingServeSetupCheck = {
  ...readySetupCheck,
  overall_status: "needs_action",
  checks: [
    readySetupCheck.checks[0],
    {
      id: "tailscale_serve",
      label: "Tailscale Serve",
      status: "error",
      message: "Tailscale Serve is not forwarding to Wardian's configured gateway port.",
      details: "http://127.0.0.1:41241",
    },
  ],
  serve_target: null,
} satisfies RemoteSetupCheckResult;

const disabledSetupCheck = {
  overall_status: "disabled",
  checks: [],
  inferred_origin: null,
  serve_target: null,
  setup_command: null,
} satisfies RemoteSetupCheckResult;

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
        case "load_remote_setup_check":
          return readySetupCheck;
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

  it("shows remote setup as ready when diagnostics pass", async () => {
    render(<RemoteAccessSettings />);

    expect(await screen.findByText("Remote access setup is ready for pairing.")).toBeVisible();
    expect(screen.getByText("Tailscale Serve")).toBeVisible();
    expect(screen.getAllByText("Ready").length).toBeGreaterThanOrEqual(1);
  });

  it("warns when Tailscale Serve is missing or mismatched", async () => {
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
        case "load_remote_setup_check":
          return missingServeSetupCheck;
        default:
          return null;
      }
    });

    render(<RemoteAccessSettings />);

    expect(
      await screen.findByText(/Wardian detected setup steps that may prevent your phone from connecting/i),
    ).toBeVisible();
    expect(screen.getByText("Tailscale Serve")).toBeVisible();
    expect(screen.getByText("Missing")).toBeVisible();
    expect(screen.getByText("tailscale serve --bg --https=443 http://127.0.0.1:41241")).toBeVisible();
  });

  it("keeps settings usable while setup diagnostics are still loading", async () => {
    let resolveSetupCheck: (value: RemoteSetupCheckResult) => void = () => undefined;
    const setupCheckPromise = new Promise<RemoteSetupCheckResult>((resolve) => {
      resolveSetupCheck = resolve;
    });
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
        case "load_remote_setup_check":
          return setupCheckPromise;
        default:
          return null;
      }
    });

    render(<RemoteAccessSettings />);

    expect(await screen.findByText("Enabled")).toBeVisible();
    expect(screen.getByRole("button", { name: /create pairing code/i })).toBeEnabled();
    expect(screen.queryByText("Remote access setup is ready for pairing.")).not.toBeInTheDocument();

    resolveSetupCheck(readySetupCheck);

    expect(await screen.findByText("Remote access setup is ready for pairing.")).toBeVisible();
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
        case "load_remote_setup_check":
          return readySetupCheck;
        default:
          return null;
      }
    });

    render(<RemoteAccessSettings />);

    expect(await screen.findByText("Needs repair")).toBeVisible();
    expect(screen.getAllByText("127.0.0.1:41241").length).toBeGreaterThanOrEqual(1);
    expect(screen.getByRole("button", { name: /create pairing code/i })).toBeDisabled();
    expect(mockInvoke).toHaveBeenCalledWith("load_remote_access_status");
  });

  it("lets users configure remote access before creating a pairing code", async () => {
    let setupLoads = 0;
    mockInvoke.mockImplementation(async (command, args) => {
      switch (command) {
        case "load_remote_access_status":
          return "disabled";
        case "load_remote_gateway_config":
          return null;
        case "list_remote_devices":
          return [];
        case "list_pending_remote_pairing_requests":
          return [];
        case "load_remote_setup_check":
          setupLoads += 1;
          return setupLoads > 1 ? readySetupCheck : disabledSetupCheck;
        case "save_remote_gateway_config": {
          const saveArgs = args as { config: typeof enabledConfig };
          expect(args).toEqual({
            config: expect.objectContaining({
              schema_version: 1,
              enabled: true,
              canonical_origin: "https://wardian.tailnet.ts.net",
              loopback_host: "127.0.0.1",
              loopback_port: 41241,
              gateway_identity_public_key: expect.stringMatching(/^wardian-local-gateway-v1:/),
              gateway_identity_fingerprint: expect.stringMatching(/^[0-9a-f]{2}(:[0-9a-f]{2})+$/),
            }),
          });
          return {
            ...enabledConfig,
            gateway_identity_public_key: saveArgs.config.gateway_identity_public_key,
            gateway_identity_fingerprint: saveArgs.config.gateway_identity_fingerprint,
          };
        }
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

    render(<RemoteAccessSettings />);

    expect(await screen.findByText("Disabled")).toBeVisible();
    expect(screen.getAllByText("Not configured").length).toBeGreaterThanOrEqual(1);
    expect(screen.getByRole("button", { name: /create pairing code/i })).toBeDisabled();

    await userEvent.click(screen.getByRole("checkbox", { name: /enable remote access/i }));
    await userEvent.type(screen.getByLabelText(/tailscale https origin/i), "wardian.tailnet.ts.net");
    await userEvent.click(screen.getByRole("button", { name: /save gateway settings/i }));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("save_remote_gateway_config", expect.any(Object));
    });
    await waitFor(() => {
      expect(mockInvoke.mock.calls.filter(([command]) => command === "load_remote_setup_check").length).toBeGreaterThanOrEqual(2);
    });
    expect(screen.getByText("Enabled")).toBeVisible();
    expect(screen.getByRole("button", { name: /create pairing code/i })).toBeEnabled();
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
        case "load_remote_setup_check":
          return readySetupCheck;
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
    await waitFor(() => {
      expect(screen.queryByText("Phone")).not.toBeInTheDocument();
    });
    expect(screen.queryByText(/revoked/i)).not.toBeInTheDocument();
    expect(screen.getByText("No paired devices.")).toBeVisible();
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
        case "load_remote_setup_check":
          return readySetupCheck;
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

  it("refreshes pending pairing approvals while a pairing code is active", async () => {
    let pendingPolls = 0;
    mockInvoke.mockImplementation(async (command) => {
      switch (command) {
        case "load_remote_access_status":
          return "enabled";
        case "load_remote_gateway_config":
          return enabledConfig;
        case "list_remote_devices":
          return [];
        case "load_remote_setup_check":
          return readySetupCheck;
        case "list_pending_remote_pairing_requests":
          pendingPolls += 1;
          return pendingPolls > 2
            ? [
                {
                  request_id: "pairing-request-1",
                  device_label: "Pixel phone",
                  public_key_fingerprint: "fp:phone",
                  canonical_origin: "https://wardian.tailnet.ts.net",
                  submitted_at: "2026-05-21T00:01:00.000Z",
                  expires_at: "2026-05-21T00:02:00.000Z",
                },
              ]
            : [];
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

    render(<RemoteAccessSettings />);

    await screen.findByText("Remote Access");
    fireEvent.click(screen.getByRole("button", { name: /create pairing code/i }));

    await screen.findByText(/offer-1/);
    expect(screen.queryByText("Pixel phone")).not.toBeInTheDocument();

    await waitFor(() => expect(screen.getByText("Pixel phone")).toBeVisible(), { timeout: 2500 });
  });
});
