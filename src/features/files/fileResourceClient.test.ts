import { beforeEach, describe, expect, it, vi } from "vitest";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import type {
  FileContentDescriptorV1,
  FileResourceSnapshotV1,
  FileResourceTextV1,
  FileResourceTicketV1,
} from "../../types";
import { FileResourceClient } from "./fileResourceClient";

vi.mock("@tauri-apps/api/core", () => ({
  convertFileSrc: vi.fn((path: string, protocol: string) => `http://${protocol}.localhost/${path}`),
  invoke: vi.fn(),
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(),
}));

const mockInvoke = vi.mocked(invoke);
const mockConvertFileSrc = vi.mocked(convertFileSrc);

const descriptor: FileContentDescriptorV1 = {
  schema: 1,
  canonical_path: "C:/work/Notes.md",
  display_name: "Notes.md",
  extension: "md",
  mime_type: "text/markdown",
  encoding: "utf-8",
  renderer_kind: "markdown",
  size_bytes: 12,
  line_count: 2,
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
  resource_id: "file:C:/work/Notes.md",
  subscription_id: "subscription-1",
  revision: 4,
  descriptor,
};

beforeEach(() => {
  mockInvoke.mockReset();
  mockConvertFileSrc.mockClear();
});

describe("FileResourceClient", () => {
  it("uses exact request-wrapped snake_case invoke payloads", async () => {
    const text: FileResourceTextV1 = {
      schema: 1,
      resource_id: snapshot.resource_id,
      revision: snapshot.revision,
      text: "# Notes\n",
    };
    const ticket: FileResourceTicketV1 = {
      schema: 1,
      ticket_id: "ticket-1",
      url: "wardian-resource://localhost/ticket-1",
      resource_id: snapshot.resource_id,
      revision: snapshot.revision,
      renderer_lease_id: "preview-pane-1",
      expires_at_ms: 1_700_000_060_000,
    };
    mockInvoke.mockImplementation(async (command) => {
      if (command === "open_file_resource") return snapshot;
      if (command === "read_file_resource_text") return text;
      if (command === "issue_file_resource_ticket") return ticket;
      return undefined;
    });

    const client = new FileResourceClient();
    await expect(client.open({
      path: "C:/work/Notes.md",
      agent_id: "agent-1",
      user_file_capability_id: null,
    })).resolves.toEqual(snapshot);
    await expect(client.readText(snapshot)).resolves.toEqual(text);
    await expect(
      client.issueTicket(snapshot, "preview-pane-1"),
    ).resolves.toEqual({
      ...ticket,
      url: "http://wardian-resource.localhost/ticket-1",
    });
    expect(mockConvertFileSrc).toHaveBeenCalledOnce();
    expect(mockConvertFileSrc).toHaveBeenCalledWith("ticket-1", "wardian-resource");
    await client.closeRendererLease(snapshot, "preview-pane-1");
    await client.close(snapshot.subscription_id);

    expect(mockInvoke.mock.calls).toEqual([
      ["open_file_resource", {
        request: {
          path: "C:/work/Notes.md",
          agent_id: "agent-1",
          user_file_capability_id: null,
        },
      }],
      ["read_file_resource_text", {
        request: {
          resource_id: snapshot.resource_id,
          subscription_id: snapshot.subscription_id,
          revision: snapshot.revision,
        },
      }],
      ["issue_file_resource_ticket", {
        request: {
          resource_id: snapshot.resource_id,
          subscription_id: snapshot.subscription_id,
          revision: snapshot.revision,
          renderer_lease_id: "preview-pane-1",
        },
      }],
      ["close_file_renderer_lease", {
        request: {
          resource_id: snapshot.resource_id,
          subscription_id: snapshot.subscription_id,
          renderer_lease_id: "preview-pane-1",
        },
      }],
      ["close_file_resource", {
        request: { subscription_id: snapshot.subscription_id },
      }],
    ]);
  });

  it("deduplicates concurrent subscription closes without retaining settled tombstones", async () => {
    let resolveClose: (() => void) | undefined;
    let closeCalls = 0;
    mockInvoke.mockImplementation((command) => {
      if (command === "open_file_resource") return Promise.resolve(snapshot);
      if (command === "close_file_resource") {
        closeCalls += 1;
        if (closeCalls > 1) return Promise.resolve(undefined);
        return new Promise<void>((resolve) => {
          resolveClose = resolve;
        });
      }
      return Promise.resolve(undefined);
    });
    const client = new FileResourceClient();
    await client.open({
      path: descriptor.canonical_path,
      agent_id: "agent-1",
      user_file_capability_id: null,
    });

    const first = client.close(snapshot.subscription_id);
    const second = client.close(snapshot.subscription_id);
    expect(mockInvoke.mock.calls.filter(([command]) => command === "close_file_resource"))
      .toHaveLength(1);
    resolveClose?.();
    await expect(Promise.all([first, second])).resolves.toEqual([undefined, undefined]);
    await client.close(snapshot.subscription_id);
    expect(mockInvoke.mock.calls.filter(([command]) => command === "close_file_resource"))
      .toHaveLength(2);
  });

  it("evicts every successful close after it settles", async () => {
    mockInvoke.mockResolvedValue(undefined);
    const client = new FileResourceClient();
    const subscriptionIds = Array.from(
      { length: 100 },
      (_, index) => `subscription-${index}`,
    );

    await Promise.all(subscriptionIds.map((subscriptionId) => client.close(subscriptionId)));
    await Promise.all(subscriptionIds.map((subscriptionId) => client.close(subscriptionId)));

    expect(mockInvoke.mock.calls.filter(([command]) => command === "close_file_resource"))
      .toHaveLength(subscriptionIds.length * 2);
  });

  it("does not let cleanup from an older close evict a newer in-flight close", async () => {
    let settleFirst: (() => void) | undefined;
    let replayFirstCleanup: (() => void) | undefined;
    let resolveSecond: (() => void) | undefined;
    mockInvoke
      .mockImplementationOnce(() => ({
        finally: (cleanup: () => void) => {
          replayFirstCleanup = cleanup;
          return new Promise<void>((resolve) => {
            settleFirst = () => {
              cleanup();
              resolve();
            };
          });
        },
      }) as Promise<void>)
      .mockImplementationOnce(() => new Promise<void>((resolve) => {
        resolveSecond = resolve;
      }));
    const client = new FileResourceClient();

    const first = client.close(snapshot.subscription_id);
    settleFirst?.();
    await first;

    const second = client.close(snapshot.subscription_id);
    replayFirstCleanup?.();
    const duplicate = client.close(snapshot.subscription_id);
    expect(second).toBe(duplicate);
    expect(mockInvoke.mock.calls.filter(([command]) => command === "close_file_resource"))
      .toHaveLength(2);

    resolveSecond?.();
    await expect(Promise.all([second, duplicate])).resolves.toEqual([undefined, undefined]);
  });

  it("keeps each owner bound to its own subscription when opens resolve out of order", async () => {
    let resolveFirst: ((value: FileResourceSnapshotV1) => void) | undefined;
    let resolveSecond: ((value: FileResourceSnapshotV1) => void) | undefined;
    const firstResponse = new Promise<FileResourceSnapshotV1>((resolve) => {
      resolveFirst = resolve;
    });
    const secondResponse = new Promise<FileResourceSnapshotV1>((resolve) => {
      resolveSecond = resolve;
    });
    mockInvoke
      .mockImplementationOnce(() => firstResponse)
      .mockImplementationOnce(() => secondResponse)
      .mockResolvedValue({
        schema: 1,
        resource_id: snapshot.resource_id,
        revision: 5,
        text: "newest",
      } satisfies FileResourceTextV1);
    const client = new FileResourceClient();
    const first = client.open({
      path: descriptor.canonical_path,
      agent_id: "agent-1",
      user_file_capability_id: null,
    });
    const second = client.open({
      path: descriptor.canonical_path,
      agent_id: "agent-1",
      user_file_capability_id: null,
    });

    const secondSnapshot = { ...snapshot, subscription_id: "subscription-2", revision: 5 };
    resolveSecond?.(secondSnapshot);
    await expect(second).resolves.toEqual(secondSnapshot);
    resolveFirst?.(snapshot);
    await expect(first).resolves.toEqual(snapshot);
    await client.readText(secondSnapshot);
    await client.readText(snapshot);

    expect(mockInvoke.mock.calls.slice(-2)).toEqual([
      ["read_file_resource_text", {
        request: {
          resource_id: snapshot.resource_id,
          subscription_id: secondSnapshot.subscription_id,
          revision: secondSnapshot.revision,
        },
      }],
      ["read_file_resource_text", {
        request: {
          resource_id: snapshot.resource_id,
          subscription_id: snapshot.subscription_id,
          revision: snapshot.revision,
        },
      }],
    ]);
  });

  it("keeps simultaneous controllers isolated when either subscription closes", async () => {
    const older = { ...snapshot, subscription_id: "subscription-older" };
    const newer = { ...snapshot, subscription_id: "subscription-newer" };
    mockInvoke
      .mockResolvedValueOnce(older)
      .mockResolvedValueOnce(newer)
      .mockResolvedValueOnce({
        schema: 1,
        resource_id: snapshot.resource_id,
        revision: snapshot.revision,
        text: "older remains authorized",
      } satisfies FileResourceTextV1)
      .mockResolvedValueOnce({
        schema: 1,
        ticket_id: "ticket-older",
        url: "wardian-resource://localhost/ticket-older",
        resource_id: snapshot.resource_id,
        revision: snapshot.revision,
        renderer_lease_id: "preview-pane-older",
        expires_at_ms: 1_700_000_060_000,
      } satisfies FileResourceTicketV1)
      .mockResolvedValueOnce(undefined)
      .mockResolvedValueOnce({
        schema: 1,
        resource_id: snapshot.resource_id,
        revision: snapshot.revision,
        text: "older remains authorized",
      } satisfies FileResourceTextV1)
      .mockResolvedValueOnce(undefined);

    const client = new FileResourceClient();
    await client.open({
      path: descriptor.canonical_path,
      agent_id: "agent-older",
      user_file_capability_id: null,
    });
    await client.open({
      path: descriptor.canonical_path,
      agent_id: "agent-newer",
      user_file_capability_id: null,
    });

    await client.readText(older);
    await client.issueTicket(older, "preview-pane-older");
    await client.close(newer.subscription_id);
    await client.readText(older);

    expect(mockInvoke).toHaveBeenNthCalledWith(3, "read_file_resource_text", {
      request: {
        resource_id: snapshot.resource_id,
        subscription_id: older.subscription_id,
        revision: snapshot.revision,
      },
    });
    expect(mockInvoke).toHaveBeenNthCalledWith(4, "issue_file_resource_ticket", {
      request: {
        resource_id: snapshot.resource_id,
        subscription_id: older.subscription_id,
        revision: snapshot.revision,
        renderer_lease_id: "preview-pane-older",
      },
    });

    expect(mockInvoke).toHaveBeenNthCalledWith(6, "read_file_resource_text", {
      request: {
        resource_id: snapshot.resource_id,
        subscription_id: older.subscription_id,
        revision: snapshot.revision,
      },
    });
    await client.close(older.subscription_id);
  });

  it("closes a renderer lease with the subscription that successfully issued it", async () => {
    const issuing = { ...snapshot, subscription_id: "subscription-issuing" };
    const newer = { ...snapshot, subscription_id: "subscription-newer" };
    const ticket = {
      schema: 1,
      ticket_id: "ticket-issued",
      url: "wardian-resource://localhost/ticket-issued",
      resource_id: snapshot.resource_id,
      revision: snapshot.revision,
      renderer_lease_id: "preview-pane-owned",
      expires_at_ms: 1_700_000_060_000,
    } satisfies FileResourceTicketV1;
    mockInvoke
      .mockResolvedValueOnce(issuing)
      .mockResolvedValueOnce(ticket)
      .mockResolvedValueOnce(newer)
      .mockRejectedValueOnce(new Error("lease reissue denied"))
      .mockRejectedValueOnce(new Error("transient lease close failure"))
      .mockResolvedValueOnce(undefined);

    const client = new FileResourceClient();
    await client.open({
      path: descriptor.canonical_path,
      agent_id: "agent-issuing",
      user_file_capability_id: null,
    });
    await client.issueTicket(issuing, "preview-pane-owned");
    await client.open({
      path: descriptor.canonical_path,
      agent_id: "agent-newer",
      user_file_capability_id: null,
    });
    await expect(
      client.issueTicket(newer, "preview-pane-owned"),
    ).rejects.toThrow("lease reissue denied");

    await expect(client.closeRendererLease(issuing, "preview-pane-owned"))
      .rejects.toThrow("transient lease close failure");
    await expect(client.closeRendererLease(issuing, "preview-pane-owned"))
      .resolves.toBeUndefined();

    const leaseCloses = mockInvoke.mock.calls.filter(
      ([command]) => command === "close_file_renderer_lease",
    );
    expect(leaseCloses).toEqual([
      ["close_file_renderer_lease", {
        request: {
          resource_id: snapshot.resource_id,
          subscription_id: issuing.subscription_id,
          renderer_lease_id: "preview-pane-owned",
        },
      }],
      ["close_file_renderer_lease", {
        request: {
          resource_id: snapshot.resource_id,
          subscription_id: issuing.subscription_id,
          renderer_lease_id: "preview-pane-owned",
        },
      }],
    ]);
  });

  it("never retargets renderer cleanup after its owning subscription closes", async () => {
    const issuing = { ...snapshot, subscription_id: "subscription-issuing" };
    const newer = { ...snapshot, subscription_id: "subscription-newer" };
    mockInvoke
      .mockResolvedValueOnce(issuing)
      .mockResolvedValueOnce({
        schema: 1,
        ticket_id: "ticket-issued",
        url: "wardian-resource://localhost/ticket-issued",
        resource_id: snapshot.resource_id,
        revision: snapshot.revision,
        renderer_lease_id: "preview-pane-owned",
        expires_at_ms: 1_700_000_060_000,
      } satisfies FileResourceTicketV1)
      .mockResolvedValueOnce(newer)
      .mockRejectedValueOnce(new Error("transient resource close failure"))
      .mockResolvedValueOnce(undefined)
      .mockResolvedValueOnce(undefined);

    const client = new FileResourceClient();
    await client.open({
      path: descriptor.canonical_path,
      agent_id: "agent-issuing",
      user_file_capability_id: null,
    });
    await client.issueTicket(issuing, "preview-pane-owned");
    await client.open({
      path: descriptor.canonical_path,
      agent_id: "agent-newer",
      user_file_capability_id: null,
    });

    await expect(client.close(issuing.subscription_id))
      .rejects.toThrow("transient resource close failure");
    await client.closeRendererLease(issuing, "preview-pane-owned");
    expect(mockInvoke).toHaveBeenLastCalledWith("close_file_renderer_lease", {
      request: {
        resource_id: snapshot.resource_id,
        subscription_id: issuing.subscription_id,
        renderer_lease_id: "preview-pane-owned",
      },
    });

    await client.close(issuing.subscription_id);
    await client.closeRendererLease(issuing, "preview-pane-owned");
    expect(mockInvoke).toHaveBeenLastCalledWith("close_file_renderer_lease", {
      request: {
        resource_id: snapshot.resource_id,
        subscription_id: issuing.subscription_id,
        renderer_lease_id: "preview-pane-owned",
      },
    });
  });

  it("retries a rejected close after evicting the failed operation", async () => {
    mockInvoke
      .mockResolvedValueOnce(snapshot)
      .mockRejectedValueOnce(new Error("transient close failure"))
      .mockResolvedValueOnce({
        schema: 1,
        resource_id: snapshot.resource_id,
        revision: snapshot.revision,
        text: "still active after rejected close",
      } satisfies FileResourceTextV1)
      .mockResolvedValueOnce(undefined)
      .mockResolvedValueOnce(undefined);
    const client = new FileResourceClient();
    await client.open({
      path: descriptor.canonical_path,
      agent_id: "agent-1",
      user_file_capability_id: null,
    });

    await expect(client.close(snapshot.subscription_id))
      .rejects.toThrow("transient close failure");
    await client.readText(snapshot);
    expect(mockInvoke).toHaveBeenCalledWith("read_file_resource_text", {
      request: {
        resource_id: snapshot.resource_id,
        subscription_id: snapshot.subscription_id,
        revision: snapshot.revision,
      },
    });
    await expect(client.close(snapshot.subscription_id)).resolves.toBeUndefined();
    await expect(client.close(snapshot.subscription_id)).resolves.toBeUndefined();

    expect(mockInvoke.mock.calls.filter(([command]) => command === "close_file_resource"))
      .toHaveLength(3);
  });
});
