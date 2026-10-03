import { afterEach, describe, expect, it, vi } from "vitest";
import { RemoteRequestError, remoteClient } from "./remoteClient";

describe("remoteClient error propagation", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
    remoteClient.setCsrfNonce(null);
  });

  it("surfaces the gateway detail message when an action fails", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn().mockResolvedValue(
        new Response(
          JSON.stringify({
            ok: false,
            code: "agent_action_failed",
            detail: "Timed out waiting for ses_1 OpenCode terminal to become ready",
          }),
          { status: 400 },
        ),
      ),
    );

    const error = await remoteClient
      .sendPrompt("agent-1", "hello")
      .then(
        () => {
          throw new Error("expected rejection");
        },
        (cause: unknown) => cause as RemoteRequestError,
      );

    expect(error).toBeInstanceOf(RemoteRequestError);
    expect(error.status).toBe(400);
    expect(error.code).toBe("agent_action_failed");
    expect(error.detail).toBe("Timed out waiting for ses_1 OpenCode terminal to become ready");
    expect(error.message).toBe(
      "Remote request failed: Timed out waiting for ses_1 OpenCode terminal to become ready",
    );
  });

  it("keeps the status-only message when the error body has no detail", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn().mockResolvedValue(
        new Response(JSON.stringify({ ok: false, code: "agent_terminal_failed" }), {
          status: 400,
        }),
      ),
    );

    const error = await remoteClient
      .loadAgentTerminal("agent-1")
      .then(
        () => {
          throw new Error("expected rejection");
        },
        (cause: unknown) => cause as RemoteRequestError,
      );

    expect(error.message).toBe("Remote request failed: 400");
    expect(error.code).toBe("agent_terminal_failed");
    expect(error.detail).toBeUndefined();
  });

  it("aborts a remote request that never produces a response", async () => {
    vi.useFakeTimers();
    try {
      const fetchMock = vi.fn((_path: string, init?: RequestInit) =>
        new Promise<Response>((_resolve, reject) => {
          init?.signal?.addEventListener("abort", () => {
            reject(Object.assign(new Error("The operation was aborted"), { name: "AbortError" }));
          }, { once: true });
        }),
      );
      vi.stubGlobal("fetch", fetchMock);

      const request = remoteClient.loadSession().then(
        () => null,
        (error: unknown) => error,
      );
      await vi.advanceTimersByTimeAsync(15_000);

      await expect(request).resolves.toMatchObject({ name: "AbortError" });
      expect(fetchMock).toHaveBeenCalledWith(
        "/remote/api/session",
        expect.objectContaining({ signal: expect.any(AbortSignal) }),
      );
    } finally {
      vi.useRealTimers();
    }
  });

  it("does not abort a mutating request at the read timeout boundary", async () => {
    vi.useFakeTimers();
    try {
      let resolveFetch: ((response: Response) => void) | undefined;
      const fetchMock = vi.fn((_path: string, init?: RequestInit) =>
        new Promise<Response>((resolve, reject) => {
          resolveFetch = resolve;
          init?.signal?.addEventListener("abort", () => {
            reject(Object.assign(new Error("The operation was aborted"), { name: "AbortError" }));
          }, { once: true });
        }),
      );
      vi.stubGlobal("fetch", fetchMock);

      const request = remoteClient.sendPrompt("agent-1", "hello");
      await Promise.resolve();
      await vi.advanceTimersByTimeAsync(15_000);

      const signal = vi.mocked(fetchMock).mock.calls[0]?.[1]?.signal;
      expect(signal?.aborted).toBe(false);

      resolveFetch?.(new Response(JSON.stringify({ ok: true }), { status: 200 }));
      await expect(request).resolves.toBeUndefined();
    } finally {
      vi.useRealTimers();
    }
  });

  it("allows a healthy Chat read to finish past the session read deadline", async () => {
    vi.useFakeTimers();
    try {
      let finish!: (response: Response) => void;
      let signal: AbortSignal | null | undefined;
      vi.stubGlobal("fetch", vi.fn((_path: string, init?: RequestInit) =>
        new Promise<Response>((resolve, reject) => {
          finish = resolve;
          signal = init?.signal;
          signal?.addEventListener("abort", () => reject(new DOMException("Aborted", "AbortError")));
        }),
      ));
      const request = remoteClient.loadAgentChatPage("agent-1").catch((error: unknown) => error);
      await vi.advanceTimersByTimeAsync(16_000);
      expect(signal?.aborted).toBe(false);
      finish(new Response(JSON.stringify({ events: [], has_older: false, next_before: null })));
      await expect(request).resolves.toEqual({ events: [], has_older: false, next_before: null });
    } finally {
      vi.useRealTimers();
    }
  });

  it("bounds Chat reads with a distinguishable local timeout", async () => {
    vi.useFakeTimers();
    try {
      vi.stubGlobal("fetch", vi.fn((_path: string, init?: RequestInit) =>
        new Promise<Response>((_resolve, reject) => {
          init?.signal?.addEventListener("abort", () => reject(new DOMException("Aborted", "AbortError")));
        }),
      ));
      const request = remoteClient.loadAgentChatPage("agent-1").catch((error: unknown) => error);
      await vi.advanceTimersByTimeAsync(60_000);
      await expect(request).resolves.toMatchObject({ name: "RemoteChatTimeoutError" });
    } finally {
      vi.useRealTimers();
    }
  });

  it("cancels an owned Chat HTTP request independently of the read deadline", async () => {
    const controller = new AbortController();
    vi.stubGlobal("fetch", vi.fn((_path: string, init?: RequestInit) =>
      new Promise<Response>((_resolve, reject) => {
        init?.signal?.addEventListener("abort", () => reject(new DOMException("Aborted", "AbortError")));
      }),
    ));
    const request = remoteClient.loadAgentChatPage("agent-1", 20, controller.signal).catch((error: unknown) => error);
    controller.abort();
    await expect(request).resolves.toMatchObject({ name: "AbortError" });
  });

  it("preserves a received Chat 401 when its error body reaches the deadline", async () => {
    vi.useFakeTimers();
    try {
      let signal: AbortSignal | null | undefined;
      const response = new Response(null, { status: 401 });
      vi.spyOn(response, "json").mockImplementation(() => new Promise((_resolve, reject) => {
        signal?.addEventListener("abort", () => reject(new DOMException("Aborted", "AbortError")));
      }));
      vi.stubGlobal("fetch", vi.fn((_path: string, init?: RequestInit) => {
        signal = init?.signal;
        return Promise.resolve(response);
      }));
      const request = remoteClient.loadAgentChatPage("agent-1").catch((error: unknown) => error);
      await vi.advanceTimersByTimeAsync(60_000);
      await expect(request).resolves.toMatchObject({ name: "RemoteRequestError", status: 401 });
    } finally {
      vi.useRealTimers();
    }
  });

  it("requests bounded automation monitor pages through server-owned offsets", async () => {
    const fetchMock = vi.fn().mockResolvedValue(
      new Response(JSON.stringify({
        schema_version: 1,
        generated_at: "2026-08-31T12:00:00.000Z",
        active_runs: [],
        active_runs_truncated: false,
        active_runs_next_offset: null,
        recent_runs: [],
        recent_runs_truncated: false,
        recent_runs_next_offset: null,
        schedules: [],
        schedules_truncated: false,
        schedules_next_offset: null,
      }), { status: 200 }),
    );
    vi.stubGlobal("fetch", fetchMock);

    await remoteClient.loadAutomationMonitor({ active_offset: 25, recent_offset: 50 });

    expect(fetchMock).toHaveBeenCalledWith(
      "/remote/api/automations/monitor?active_offset=25&recent_offset=50",
      expect.objectContaining({ method: "GET" }),
    );
  });
});
