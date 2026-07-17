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
  it("reconciles the highest revision observed before the open snapshot is published", async () => {
    const revisedDescriptor = {
      ...descriptor,
      content_hash: "hash-2",
      modified_at_ms: descriptor.modified_at_ms + 1,
    };
    mockInvoke.mockImplementation((command) => {
      if (command === "open_file_resource") {
        emitRevision({
          schema: 1,
          resource_id: snapshot.resource_id,
          revision: 2,
          descriptor: revisedDescriptor,
        });
        return Promise.resolve(snapshot);
      }
      return Promise.resolve(undefined);
    });
    const client = new FileResourceClient();
    const resource = renderHook(() => useFileResource(request, client));

    await waitFor(() => expect(resource.result.current.status).toBe("ready"));
    expect(resource.result.current.snapshot).toEqual({
      ...snapshot,
      revision: 2,
      descriptor: revisedDescriptor,
    });
    expect(mockInvoke.mock.calls.filter(([command]) => command === "open_file_resource"))
      .toHaveLength(1);

    resource.unmount();
    await waitFor(() => expect(unlisten).toHaveBeenCalledOnce());
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
    mockInvoke.mockImplementation((command) => {
      if (command === "open_file_resource") {
        openCount += 1;
        return Promise.resolve({
          ...snapshot,
          subscription_id: `subscription-${openCount}`,
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
