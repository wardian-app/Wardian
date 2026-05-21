import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { remoteClient } from "./remoteClient";
import { RemoteMobileApp } from "./RemoteMobileApp";
import { useRemoteStore } from "./useRemoteStore";

const fetchMock = vi.fn();
globalThis.fetch = fetchMock;

describe("RemoteMobileApp", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    useRemoteStore.setState({
      agents: [],
      workflows: [],
      status: "loading",
      selectedAgentIds: new Set(),
      sending: false,
    });
  });

  afterEach(() => {
    vi.restoreAllMocks();
    remoteClient.setCsrfNonce(null);
  });

  it("bootstraps the session nonce, requires an explicit target, and sends a CSRF-protected prompt", async () => {
    fetchMock.mockImplementation((url: string, init?: RequestInit) => {
      if (url === "/remote/api/session") {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              csrf_nonce: "csrf-1",
              expires_at: "2026-05-21T08:05:00.000Z",
              absolute_expires_at: "2026-05-21T20:00:00.000Z",
            }),
            { status: 200 },
          ),
        );
      }
      if (url === "/remote/api/agents") {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              agents: [
                {
                  session_id: "agent-1",
                  session_name: "Coder",
                  agent_class: "Coder",
                  provider: "codex",
                  workspace: "<absolute-workspace-path>",
                  status: "Idle",
                  latest_text: "Ready",
                },
              ],
            }),
            { status: 200 },
          ),
        );
      }
      if (url === "/remote/api/workflows") {
        return Promise.resolve(new Response(JSON.stringify({ workflows: [] }), { status: 200 }));
      }
      if (url === "/remote/api/agents/action" && init?.method === "POST") {
        return Promise.resolve(new Response(JSON.stringify({ ok: true }), { status: 200 }));
      }
      return Promise.resolve(new Response("{}", { status: 404 }));
    });

    render(<RemoteMobileApp />);

    await screen.findByText("Coder");
    expect(screen.getByTestId("remote-agent-list")).toHaveClass("grid-cols-1");
    expect(fetchMock).toHaveBeenCalledWith("/remote/api/session", expect.objectContaining({ method: "GET" }));

    await userEvent.type(screen.getByLabelText("Prompt"), "status please");
    expect(screen.getByRole("button", { name: "Send" })).toBeDisabled();

    await userEvent.click(screen.getByRole("button", { name: /Coder/ }));
    await userEvent.click(screen.getByRole("button", { name: "Send" }));

    await waitFor(() => {
      expect(fetchMock).toHaveBeenCalledWith(
        "/remote/api/agents/action",
        expect.objectContaining({
          credentials: "same-origin",
          method: "POST",
          headers: expect.objectContaining({
            "Content-Type": "application/json",
            "x-wardian-csrf": "csrf-1",
          }),
        }),
      );
    });

    const actionCall = fetchMock.mock.calls.find(([url]) => url === "/remote/api/agents/action");
    expect(JSON.parse(actionCall?.[1]?.body as string)).toEqual({
      action: "send_prompt",
      target: "agent-1",
      prompt: "status please",
    });
  });

  it("shows a re-authentication action when the remote session expires", async () => {
    fetchMock.mockImplementation((url: string) => {
      if (url === "/remote/api/session") {
        return Promise.resolve(new Response("{}", { status: 401 }));
      }
      return Promise.resolve(new Response("{}", { status: 500 }));
    });

    render(<RemoteMobileApp />);

    expect(await screen.findByText("Session expired. Re-authentication is required.")).toBeVisible();
    expect(screen.getByRole("button", { name: "Re-authenticate" })).toBeVisible();
  });

  it("requires confirmation before killing an agent", async () => {
    vi.spyOn(window, "confirm").mockReturnValue(false);
    fetchMock.mockImplementation((url: string, init?: RequestInit) => {
      if (url === "/remote/api/session") {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              csrf_nonce: "csrf-1",
              expires_at: "2026-05-21T08:05:00.000Z",
              absolute_expires_at: "2026-05-21T20:00:00.000Z",
            }),
            { status: 200 },
          ),
        );
      }
      if (url === "/remote/api/agents") {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              agents: [
                {
                  session_id: "agent-1",
                  session_name: "Coder",
                  agent_class: "Coder",
                  provider: "codex",
                  workspace: "<absolute-workspace-path>",
                  status: "Idle",
                  latest_text: "Ready",
                },
              ],
            }),
            { status: 200 },
          ),
        );
      }
      if (url === "/remote/api/workflows") {
        return Promise.resolve(new Response(JSON.stringify({ workflows: [] }), { status: 200 }));
      }
      if (url === "/remote/api/agents/action" && init?.method === "POST") {
        return Promise.resolve(new Response(JSON.stringify({ ok: true }), { status: 200 }));
      }
      return Promise.resolve(new Response("{}", { status: 404 }));
    });

    render(<RemoteMobileApp />);

    await screen.findByText("Coder");
    await userEvent.click(screen.getByRole("button", { name: "Kill" }));

    expect(window.confirm).toHaveBeenCalledWith("Kill Coder?");
    expect(fetchMock.mock.calls.some(([url]) => url === "/remote/api/agents/action")).toBe(false);
  });
});
