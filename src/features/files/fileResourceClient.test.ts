import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import type {
  FileContentDescriptorV1,
  FileResourceSnapshotV1,
  FileResourceTextV1,
  FileResourceTicketV1,
} from "../../types";
import { FileResourceClient } from "./fileResourceClient";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(),
}));

const mockInvoke = vi.mocked(invoke);

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
    await expect(client.readText(snapshot.resource_id, snapshot.revision)).resolves.toEqual(text);
    await expect(
      client.issueTicket(snapshot.resource_id, snapshot.revision, "preview-pane-1"),
    ).resolves.toEqual(ticket);
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
      ["close_file_resource", {
        request: { subscription_id: snapshot.subscription_id },
      }],
    ]);
  });

  it("closes a subscription idempotently, including concurrent callers", async () => {
    let resolveClose: (() => void) | undefined;
    mockInvoke.mockImplementation((command) => {
      if (command === "open_file_resource") return Promise.resolve(snapshot);
      if (command === "close_file_resource") {
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
      .toHaveLength(1);
  });

  it("keeps the newest open subscription when responses resolve out of order", async () => {
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
      .mockResolvedValueOnce({
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

    resolveSecond?.({ ...snapshot, subscription_id: "subscription-2", revision: 5 });
    await second;
    resolveFirst?.(snapshot);
    await first;
    await client.readText(snapshot.resource_id, 5);

    expect(mockInvoke).toHaveBeenLastCalledWith("read_file_resource_text", {
      request: {
        resource_id: snapshot.resource_id,
        subscription_id: "subscription-2",
        revision: 5,
      },
    });
  });
});
