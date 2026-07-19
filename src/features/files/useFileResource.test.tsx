import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi, type Mock } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { listen, type Event, type EventCallback } from "@tauri-apps/api/event";
import type {
  FileContentDescriptorV1,
  FileResourceEventV1,
  FileResourceSnapshotV1,
} from "../../types";
import { FileResourceClient } from "./fileResourceClient";
import { useFileResource } from "./useFileResource";

vi.mock("@tauri-apps/api/core", () => ({
  convertFileSrc: vi.fn((path: string) => path),
  invoke: vi.fn(),
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(),
}));

const mockInvoke = vi.mocked(invoke);
const mockListen = vi.mocked(listen);

const descriptor: FileContentDescriptorV1 = {
  schema: 1,
  canonical_path: "/work/report.md",
  display_name: "report.md",
  extension: "md",
  mime_type: "text/markdown",
  encoding: "utf-8",
  renderer_kind: "markdown",
  size_bytes: 10,
  line_count: 1,
  content_hash: "hash-1",
  modified_at_ms: 1_700_000_000_000,
  capabilities: {
    preview: true,
    changes: true,
    draft: true,
    stream: false,
  },
  unavailable_reason: null,
};

const snapshot: FileResourceSnapshotV1 = {
  resource_id: "file:/work/report.md",
  subscription_id: "subscription-1",
  revision: 1,
  descriptor,
};

const request = {
  path: "/work/report.md",
  agent_id: "agent-1",
  user_file_capability_id: null,
} as const;

let revisionListener: EventCallback<FileResourceEventV1> | undefined;
let unlisten: Mock<() => void>;

function emitRevision(payload: FileResourceEventV1) {
  revisionListener?.({
    event: "file-resource://revision",
    id: 1,
    payload,
  } as Event<FileResourceEventV1>);
}

beforeEach(() => {
  revisionListener = undefined;
  unlisten = vi.fn();
  mockInvoke.mockReset();
  mockListen.mockReset();
  mockListen.mockImplementation((event, callback) => {
    expect(event).toBe("file-resource://revision");
    revisionListener = callback as EventCallback<FileResourceEventV1>;
    return Promise.resolve(unlisten);
  });
  mockInvoke.mockImplementation((command) => {
    if (command === "open_file_resource") return Promise.resolve(snapshot);
    return Promise.resolve(undefined);
  });
});

describe("useFileResource", () => {
  it("reopens authoritatively when a newer revision arrives before the snapshot", async () => {
    const eventDescriptor = {
      ...descriptor,
      content_hash: "untrusted-event-hash",
      modified_at_ms: descriptor.modified_at_ms + 1,
    };
    const authoritativeDescriptor = {
      ...descriptor,
      content_hash: "authoritative-hash-2",
      modified_at_ms: descriptor.modified_at_ms + 2,
    };
    let openCount = 0;
    mockInvoke.mockImplementation((command) => {
      if (command === "open_file_resource") {
        openCount += 1;
        if (openCount === 1) {
          emitRevision({
            schema: 1,
            resource_id: snapshot.resource_id,
            revision: 2,
            descriptor: eventDescriptor,
          });
          return Promise.resolve(snapshot);
        }
        return Promise.resolve({
          ...snapshot,
          subscription_id: "subscription-2",
          revision: 2,
          descriptor: authoritativeDescriptor,
        });
      }
      return Promise.resolve(undefined);
    });
    const client = new FileResourceClient();
    const resource = renderHook(() => useFileResource(request, client));

    await waitFor(() => expect(resource.result.current.status).toBe("ready"));
    expect(resource.result.current.snapshot).toEqual({
      ...snapshot,
      subscription_id: "subscription-2",
      revision: 2,
      descriptor: authoritativeDescriptor,
    });
    expect(mockInvoke.mock.calls.filter(([command]) => command === "open_file_resource"))
      .toHaveLength(2);
    expect(mockInvoke).toHaveBeenCalledWith("close_file_resource", {
      request: { subscription_id: snapshot.subscription_id },
    });

    resource.unmount();
    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("close_file_resource", {
        request: { subscription_id: "subscription-2" },
      });
      expect(unlisten).toHaveBeenCalledOnce();
    });
  });

  it("fails closed after bounded stale opens and recovers from a fresh retry", async () => {
    let openCount = 0;
    mockInvoke.mockImplementation((command) => {
      if (command !== "open_file_resource") return Promise.resolve(undefined);
      openCount += 1;
      if (openCount <= 3) {
        const candidateRevision = openCount;
        emitRevision({
          schema: 1,
          resource_id: snapshot.resource_id,
          revision: candidateRevision + 1,
          descriptor: {
            ...descriptor,
            content_hash: `event-hash-${candidateRevision + 1}`,
          },
        });
        return Promise.resolve({
          ...snapshot,
          subscription_id: `candidate-${candidateRevision}`,
          revision: candidateRevision,
          descriptor: {
            ...descriptor,
            content_hash: `candidate-hash-${candidateRevision}`,
          },
        });
      }
      return Promise.resolve({
        ...snapshot,
        subscription_id: "subscription-retry",
        revision: 4,
        descriptor: { ...descriptor, content_hash: "authoritative-retry-hash" },
      });
    });
    const client = new FileResourceClient();
    const resource = renderHook(() => useFileResource(request, client));

    await waitFor(() => expect(resource.result.current.status).toBe("error"));
    expect(resource.result.current.snapshot).toBeNull();
    expect(resource.result.current.error?.message)
      .toMatch(/kept changing.*Retry.*authoritative snapshot/i);
    expect(mockInvoke.mock.calls.filter(([command]) => command === "open_file_resource"))
      .toHaveLength(3);
    for (let candidate = 1; candidate <= 3; candidate += 1) {
      expect(mockInvoke).toHaveBeenCalledWith("close_file_resource", {
        request: { subscription_id: `candidate-${candidate}` },
      });
    }

    await act(async () => resource.result.current.retry());
    await waitFor(() => expect(resource.result.current.status).toBe("ready"));
    expect(resource.result.current.snapshot).toEqual({
      ...snapshot,
      subscription_id: "subscription-retry",
      revision: 4,
      descriptor: { ...descriptor, content_hash: "authoritative-retry-hash" },
    });
    expect(mockInvoke.mock.calls.filter(([command]) => command === "open_file_resource"))
      .toHaveLength(4);

    resource.unmount();
    await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith("close_file_resource", {
      request: { subscription_id: "subscription-retry" },
    }));
  });

  it("keeps the live subscription when bounded refresh reconciliation fails", async () => {
    const client = new FileResourceClient();
    const resource = renderHook(() => useFileResource(request, client));
    await waitFor(() => expect(resource.result.current.status).toBe("ready"));

    let refreshOpenCount = 0;
    mockInvoke.mockImplementation((command) => {
      if (command !== "open_file_resource") return Promise.resolve(undefined);
      refreshOpenCount += 1;
      if (refreshOpenCount <= 3) {
        const candidateRevision = refreshOpenCount + 1;
        emitRevision({
          schema: 1,
          resource_id: snapshot.resource_id,
          revision: candidateRevision + 1,
          descriptor: {
            ...descriptor,
            content_hash: `refresh-event-${candidateRevision + 1}`,
          },
        });
        return Promise.resolve({
          ...snapshot,
          subscription_id: `refresh-candidate-${candidateRevision}`,
          revision: candidateRevision,
          descriptor: {
            ...descriptor,
            content_hash: `refresh-candidate-${candidateRevision}`,
          },
        });
      }
      return Promise.resolve({
        ...snapshot,
        subscription_id: "refresh-authoritative",
        revision: 5,
        descriptor: { ...descriptor, content_hash: "refresh-authoritative-hash" },
      });
    });

    act(() => emitRevision({
      schema: 1,
      resource_id: snapshot.resource_id,
      revision: 2,
      descriptor: { ...descriptor, content_hash: "refresh-trigger" },
    }));
    await waitFor(() => expect(resource.result.current.status).toBe("error"));
    expect(resource.result.current.snapshot).toEqual(snapshot);
    expect(refreshOpenCount).toBe(3);
    for (let candidateRevision = 2; candidateRevision <= 4; candidateRevision += 1) {
      expect(mockInvoke).toHaveBeenCalledWith("close_file_resource", {
        request: { subscription_id: `refresh-candidate-${candidateRevision}` },
      });
    }
    expect(mockInvoke).not.toHaveBeenCalledWith("close_file_resource", {
      request: { subscription_id: snapshot.subscription_id },
    });

    await act(async () => resource.result.current.retry());
    await waitFor(() => expect(resource.result.current.status).toBe("ready"));
    expect(resource.result.current.snapshot?.subscription_id).toBe("refresh-authoritative");
    expect(mockInvoke).toHaveBeenCalledWith("close_file_resource", {
      request: { subscription_id: snapshot.subscription_id },
    });

    resource.unmount();
    await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith("close_file_resource", {
      request: { subscription_id: "refresh-authoritative" },
    }));
  });

  it("consumes a pre-snapshot ABA signal when authoritative reopen fails", async () => {
    let openCount = 0;
    mockInvoke.mockImplementation((command) => {
      if (command !== "open_file_resource") return Promise.resolve(undefined);
      openCount += 1;
      if (openCount === 1) {
        emitRevision({
          schema: 1,
          resource_id: snapshot.resource_id,
          revision: 9,
          descriptor: { ...descriptor, content_hash: "old-incarnation-hash" },
        });
        return Promise.resolve({ ...snapshot, subscription_id: "subscription-new-1" });
      }
      if (openCount === 2) return Promise.reject(new Error("authoritative reopen failed"));
      return Promise.resolve({
        ...snapshot,
        subscription_id: "subscription-new-3",
        revision: 1,
        descriptor: { ...descriptor, content_hash: "new-incarnation-hash" },
      });
    });
    const client = new FileResourceClient();
    const resource = renderHook(() => useFileResource(request, client));

    await waitFor(() => expect(resource.result.current.status).toBe("error"));
    expect(resource.result.current.error?.message).toBe("authoritative reopen failed");
    expect(mockInvoke.mock.calls.filter(([command]) => command === "open_file_resource"))
      .toHaveLength(2);
    expect(mockInvoke).toHaveBeenCalledWith("close_file_resource", {
      request: { subscription_id: "subscription-new-1" },
    });

    await act(async () => resource.result.current.retry());
    await waitFor(() => expect(resource.result.current.status).toBe("ready"));
    expect(resource.result.current.snapshot).toEqual({
      ...snapshot,
      subscription_id: "subscription-new-3",
      revision: 1,
      descriptor: { ...descriptor, content_hash: "new-incarnation-hash" },
    });
    expect(mockInvoke.mock.calls.filter(([command]) => command === "open_file_resource"))
      .toHaveLength(3);

    resource.unmount();
    await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith("close_file_resource", {
      request: { subscription_id: "subscription-new-3" },
    }));
  });

  it("shares one controller, applies only matching newer revisions, and releases last", async () => {
    const client = new FileResourceClient();
    const first = renderHook(() => useFileResource(request, client));
    await waitFor(() => expect(first.result.current.status).toBe("ready"));

    const second = renderHook(() => useFileResource(request, client));
    await waitFor(() => expect(second.result.current.status).toBe("ready"));

    expect(mockListen).toHaveBeenCalledTimes(1);
    expect(mockInvoke.mock.calls.filter(([command]) => command === "open_file_resource"))
      .toHaveLength(1);
    expect(first.result.current.snapshot).toEqual(snapshot);
    expect(second.result.current.snapshot).toEqual(snapshot);

    act(() => emitRevision({
      schema: 1,
      resource_id: "file:/work/unrelated.md",
      revision: 9,
      descriptor: { ...descriptor, canonical_path: "/work/unrelated.md" },
    }));
    act(() => emitRevision({
      schema: 1,
      resource_id: snapshot.resource_id,
      revision: snapshot.revision,
      descriptor: { ...descriptor, content_hash: "same-revision" },
    }));
    expect(first.result.current.snapshot).toEqual(snapshot);

    const revisedDescriptor = {
      ...descriptor,
      size_bytes: 20,
      content_hash: "hash-2",
      modified_at_ms: descriptor.modified_at_ms + 1,
    };
    mockInvoke.mockImplementation((command) => {
      if (command === "open_file_resource") {
        return Promise.resolve({
          ...snapshot,
          subscription_id: "subscription-2",
          revision: 2,
          descriptor: revisedDescriptor,
        });
      }
      return Promise.resolve(undefined);
    });
    act(() => emitRevision({
      schema: 1,
      resource_id: snapshot.resource_id,
      revision: 2,
      descriptor: { ...revisedDescriptor, content_hash: "untrusted-event-payload" },
    }));
    await waitFor(() => {
      expect(mockInvoke.mock.calls.filter(([command]) => command === "open_file_resource"))
        .toHaveLength(2);
      expect(first.result.current.snapshot).toEqual({
        ...snapshot,
        subscription_id: "subscription-2",
        revision: 2,
        descriptor: revisedDescriptor,
      });
    });
    expect(second.result.current.snapshot?.revision).toBe(2);
    expect(mockInvoke).toHaveBeenCalledWith("close_file_resource", {
      request: { subscription_id: snapshot.subscription_id },
    });

    first.unmount();
    expect(mockInvoke.mock.calls.filter(([command]) => command === "close_file_resource"))
      .toHaveLength(1);
    second.unmount();
    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("close_file_resource", {
        request: { subscription_id: "subscription-2" },
      });
      expect(mockInvoke.mock.calls.filter(([command]) => command === "close_file_resource"))
        .toHaveLength(2);
      expect(unlisten).toHaveBeenCalledOnce();
    });
  });

  it("keeps the last ready snapshot visible while a revision refresh is pending", async () => {
    let openCount = 0;
    let resolveRefresh: ((value: FileResourceSnapshotV1) => void) | undefined;
    const refresh = new Promise<FileResourceSnapshotV1>((resolve) => {
      resolveRefresh = resolve;
    });
    mockInvoke.mockImplementation((command) => {
      if (command !== "open_file_resource") return Promise.resolve(undefined);
      openCount += 1;
      return openCount === 1 ? Promise.resolve(snapshot) : refresh;
    });
    const client = new FileResourceClient();
    const resource = renderHook(() => useFileResource(request, client));
    await waitFor(() => expect(resource.result.current.status).toBe("ready"));

    act(() => emitRevision({
      schema: 1,
      resource_id: snapshot.resource_id,
      revision: 2,
      descriptor: { ...descriptor, content_hash: "event-hash-2" },
    }));
    await waitFor(() => expect(openCount).toBe(2));
    expect(resource.result.current.status).toBe("ready");
    expect(resource.result.current.snapshot).toEqual(snapshot);

    const refreshed = {
      ...snapshot,
      subscription_id: "subscription-2",
      revision: 2,
      descriptor: { ...descriptor, content_hash: "authoritative-hash-2" },
    };
    await act(async () => resolveRefresh?.(refreshed));
    await waitFor(() => expect(resource.result.current.snapshot).toEqual(refreshed));
    resource.unmount();
  });

  it("keeps distinct POSIX paths with literal backslashes in separate controllers", async () => {
    mockInvoke.mockImplementation((command, args) => {
      if (command !== "open_file_resource") return Promise.resolve(undefined);
      const openRequest = (args as { request: typeof request }).request;
      return Promise.resolve({
        ...snapshot,
        resource_id: `file:${openRequest.path}`,
        subscription_id: `subscription-${openRequest.path}`,
        descriptor: { ...descriptor, canonical_path: openRequest.path },
      });
    });
    const client = new FileResourceClient();
    const literalBackslash = renderHook(() => useFileResource({
      ...request,
      path: "/tmp/a\\b.md",
    }, client));
    const nestedPath = renderHook(() => useFileResource({
      ...request,
      path: "/tmp/a/b.md",
    }, client));

    await waitFor(() => {
      expect(literalBackslash.result.current.status).toBe("ready");
      expect(nestedPath.result.current.status).toBe("ready");
    });
    expect(mockListen).toHaveBeenCalledTimes(2);
    expect(mockInvoke.mock.calls.filter(([command]) => command === "open_file_resource"))
      .toHaveLength(2);
    expect(literalBackslash.result.current.snapshot?.descriptor.canonical_path)
      .toBe("/tmp/a\\b.md");
    expect(nestedPath.result.current.snapshot?.descriptor.canonical_path)
      .toBe("/tmp/a/b.md");

    literalBackslash.unmount();
    nestedPath.unmount();
  });

  it("shares one controller for equivalent Windows absolute path spellings", async () => {
    const client = new FileResourceClient();
    const backslashPath = renderHook(() => useFileResource({
      ...request,
      path: "C:\\work\\report.md",
    }, client));
    await waitFor(() => expect(backslashPath.result.current.status).toBe("ready"));
    const slashPath = renderHook(() => useFileResource({
      ...request,
      path: "C:/work/report.md",
    }, client));
    await waitFor(() => expect(slashPath.result.current.status).toBe("ready"));

    expect(mockListen).toHaveBeenCalledTimes(1);
    expect(mockInvoke.mock.calls.filter(([command]) => command === "open_file_resource"))
      .toHaveLength(1);

    backslashPath.unmount();
    slashPath.unmount();
  });

  it("contains load errors and retries without replacing the shared listener", async () => {
    mockInvoke.mockRejectedValueOnce(new Error("resource unavailable"));
    const client = new FileResourceClient();
    const resource = renderHook(() => useFileResource(request, client));

    await waitFor(() => expect(resource.result.current.status).toBe("error"));
    expect(resource.result.current.snapshot).toBeNull();
    expect(resource.result.current.error).toBeInstanceOf(Error);
    expect(resource.result.current.error?.message).toBe("resource unavailable");

    mockInvoke.mockResolvedValueOnce(snapshot);
    await act(async () => resource.result.current.retry());
    await waitFor(() => expect(resource.result.current.status).toBe("ready"));
    expect(resource.result.current.snapshot).toEqual(snapshot);
    expect(resource.result.current.error).toBeNull();
    expect(mockListen).toHaveBeenCalledTimes(1);

    resource.unmount();
    await waitFor(() => expect(unlisten).toHaveBeenCalledOnce());
  });

  it("contains listener errors and retries listener setup before opening", async () => {
    mockListen.mockRejectedValueOnce(new Error("listener unavailable"));
    const client = new FileResourceClient();
    const resource = renderHook(() => useFileResource(request, client));

    await waitFor(() => expect(resource.result.current.status).toBe("error"));
    expect(resource.result.current.error?.message).toBe("listener unavailable");
    expect(mockInvoke.mock.calls.filter(([command]) => command === "open_file_resource"))
      .toHaveLength(0);

    mockListen.mockImplementationOnce((_event, callback) => {
      revisionListener = callback as EventCallback<FileResourceEventV1>;
      return Promise.resolve(unlisten);
    });
    await act(async () => resource.result.current.retry());
    await waitFor(() => expect(resource.result.current.status).toBe("ready"));
    expect(mockListen).toHaveBeenCalledTimes(2);
    expect(mockInvoke.mock.calls.filter(([command]) => command === "open_file_resource"))
      .toHaveLength(1);

    resource.unmount();
    await waitFor(() => expect(unlisten).toHaveBeenCalledOnce());
  });

  it("creates a fresh controller when a consumer returns during final close", async () => {
    let resolveClose: (() => void) | undefined;
    let openCount = 0;
    mockInvoke.mockImplementation((command) => {
      if (command === "open_file_resource") {
        openCount += 1;
        return Promise.resolve({
          ...snapshot,
          subscription_id: `subscription-${openCount}`,
        });
      }
      if (command === "close_file_resource") {
        return new Promise<void>((resolve) => {
          resolveClose = resolve;
        });
      }
      return Promise.resolve(undefined);
    });
    const client = new FileResourceClient();
    const first = renderHook(() => useFileResource(request, client));
    await waitFor(() => expect(first.result.current.status).toBe("ready"));
    first.unmount();
    await waitFor(() => {
      expect(mockInvoke.mock.calls.filter(([command]) => command === "close_file_resource"))
        .toHaveLength(1);
    });

    const returning = renderHook(() => useFileResource(request, client));
    await waitFor(() => {
      expect(mockInvoke.mock.calls.filter(([command]) => command === "open_file_resource"))
        .toHaveLength(2);
      expect(returning.result.current.snapshot?.subscription_id).toBe("subscription-2");
    });

    resolveClose?.();
    returning.unmount();
  });

  it("keeps an older controller usable after a newer authorization unmounts", async () => {
    let openCount = 0;
    mockInvoke.mockImplementation((command, args) => {
      if (command === "open_file_resource") {
        openCount += 1;
        return Promise.resolve({
          ...snapshot,
          subscription_id: `subscription-${openCount}`,
        });
      }
      if (command === "issue_file_resource_ticket") {
        const issue = args as { request: {
          resource_id: string;
          revision: number;
          renderer_lease_id: string;
        } };
        return Promise.resolve({
          schema: 1,
          ticket_id: `ticket-${issue.request.renderer_lease_id}`,
          url: `wardian-resource://localhost/ticket-${issue.request.renderer_lease_id}`,
          resource_id: issue.request.resource_id,
          revision: issue.request.revision,
          renderer_lease_id: issue.request.renderer_lease_id,
          expires_at_ms: Date.now() + 60_000,
        });
      }
      return Promise.resolve(undefined);
    });
    const client = new FileResourceClient();
    const olderRequest = { ...request, agent_id: "agent-older" };
    const newerRequest = {
      ...request,
      agent_id: "agent-newer",
      user_file_capability_id: null,
    };

    const older = renderHook(() => useFileResource(olderRequest, client));
    await waitFor(() => expect(older.result.current.status).toBe("ready"));
    const newer = renderHook(() => useFileResource(newerRequest, client));
    await waitFor(() => expect(newer.result.current.status).toBe("ready"));
    const olderSnapshot = older.result.current.snapshot!;
    const newerSnapshot = newer.result.current.snapshot!;

    await client.readText(olderSnapshot);
    await client.issueTicket(olderSnapshot, "preview-pane-older");
    await client.closeRendererLease(olderSnapshot, "preview-pane-older");
    await client.readText(newerSnapshot);
    await client.issueTicket(newerSnapshot, "preview-pane-newer");
    await client.closeRendererLease(newerSnapshot, "preview-pane-newer");

    newer.unmount();
    await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith("close_file_resource", {
      request: { subscription_id: "subscription-2" },
    }));
    expect(older.result.current.snapshot?.subscription_id).toBe("subscription-1");

    await client.readText(olderSnapshot);
    expect(mockInvoke).toHaveBeenCalledWith("read_file_resource_text", {
      request: {
        resource_id: snapshot.resource_id,
        subscription_id: "subscription-1",
        revision: snapshot.revision,
      },
    });
    expect(mockInvoke).toHaveBeenCalledWith("issue_file_resource_ticket", {
      request: {
        resource_id: snapshot.resource_id,
        subscription_id: "subscription-1",
        revision: snapshot.revision,
        renderer_lease_id: "preview-pane-older",
      },
    });

    older.unmount();
    await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith("close_file_resource", {
      request: { subscription_id: "subscription-1" },
    }));
  });
});
