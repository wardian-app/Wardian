import { describe, expect, it, vi } from "vitest";
import type {
  RemoteTerminalBrokerEvent,
  RemoteTerminalRegisteredMessage,
  RemoteTerminalV2State,
  TerminalBrokerState,
  TerminalPresentationState,
  TerminalSnapshot,
} from "../../types";
import { RemoteTerminalSessionClient } from "./remoteTerminalSessionClient";

class TestSocket {
  readyState = WebSocket.OPEN;
  sent: string[] = [];

  send(payload: string) {
    this.sent.push(payload);
  }
}

const presentation: TerminalPresentationState = {
  presentation_id: "remote:presentation-1",
  client_kind: "remote",
  desired_geometry: { cols: 80, rows: 24 },
  visibility: "visible",
  render_state: "mounted",
  interaction_capability: "interactive",
  interaction_sequence: 1,
  requires_resync: false,
};

function brokerState(overrides: Partial<TerminalBrokerState> = {}): TerminalBrokerState {
  return {
    session_id: "agent-1",
    runtime_generation: 4,
    lease_epoch: 7,
    stream_sequence: 12,
    interaction_sequence: 1,
    geometry: { cols: 100, rows: 30 },
    owner_presentation_id: "desktop:presentation-1",
    pending_activation: null,
    runtime_state: "live",
    ...overrides,
  };
}

function snapshot(overrides: Partial<TerminalSnapshot> = {}): TerminalSnapshot {
  return {
    snapshot_id: "snapshot-1",
    session_id: "agent-1",
    runtime_generation: 4,
    sequence_barrier: 12,
    geometry: { cols: 100, rows: 30 },
    terminal_state_base64: btoa("ready"),
    visible_grid: "ready",
    scrollback: [],
    ...overrides,
  };
}

function registered(overrides: Partial<RemoteTerminalRegisteredMessage> = {}): RemoteTerminalRegisteredMessage {
  return {
    type: "registered",
    protocol_version: 2,
    presentation,
    broker_state: brokerState(),
    initial_snapshot: snapshot(),
    ...overrides,
  };
}

function parsed(socket: TestSocket) {
  return socket.sent.map((payload) => JSON.parse(payload) as Record<string, unknown>);
}

function harness(options: { applySnapshot?: () => void | Promise<void> } = {}) {
  const socket = new TestSocket();
  const states: RemoteTerminalV2State[] = [];
  const events: RemoteTerminalBrokerEvent[][] = [];
  const nonfatal = vi.fn();
  const client = new RemoteTerminalSessionClient(socket as unknown as WebSocket, {
    applySnapshot: options.applySnapshot ?? vi.fn(),
    applyEvents: (batch) => {
      events.push([...batch]);
    },
    onState: (state) => states.push(state),
    onNonfatalError: nonfatal,
  });
  return { client, events, nonfatal, socket, states };
}

describe("RemoteTerminalSessionClient", () => {
  it("registers protocol v2 passively and acknowledges only after snapshot application", async () => {
    const order: string[] = [];
    const socket = new TestSocket();
    socket.send = (payload) => {
      order.push(`send:${JSON.parse(payload).type}`);
      socket.sent.push(payload);
    };
    const client = new RemoteTerminalSessionClient(socket as unknown as WebSocket, {
      applySnapshot: async () => {
        order.push("snapshot:start");
        await Promise.resolve();
        order.push("snapshot:end");
      },
      applyEvents: vi.fn(),
      onState: vi.fn(),
    });

    await client.handleMessage(registered());

    expect(client.state.mode).toBe("mirror");
    expect(order).toEqual([
      "snapshot:start",
      "snapshot:end",
      "send:ack_events",
      "send:request_events",
    ]);
    expect(parsed(socket)[0]).toEqual({
      type: "ack_events",
      runtime_generation: 4,
      applied_sequence: 12,
    });
    expect(parsed(socket)).not.toContainEqual(expect.objectContaining({ type: "begin_activation" }));
  });

  it("applies the activation snapshot before acknowledging and becoming owner", async () => {
    let releaseSnapshot: (() => void) | undefined;
    const applied = new Promise<void>((resolve) => {
      releaseSnapshot = resolve;
    });
    const { client, socket } = harness({ applySnapshot: vi.fn().mockResolvedValueOnce(undefined).mockReturnValueOnce(applied) });
    await client.handleMessage(registered());
    socket.sent = [];

    expect(client.activate()).toBe(true);
    expect(parsed(socket)).toEqual([{
      type: "begin_activation",
      runtime_generation: 4,
      observed_lease_epoch: 7,
    }]);
    const handling = client.handleMessage({
      type: "activation_begin",
      result: {
        decision: {
          status: "accepted",
          reason: null,
          runtime_generation: 4,
          lease_epoch: 8,
          owner_presentation_id: "desktop:presentation-1",
        },
        activation_id: "activation-1",
        snapshot: snapshot({ sequence_barrier: 15 }),
        sequence_barrier: 15,
      },
    });
    await Promise.resolve();
    await Promise.resolve();
    expect(parsed(socket)).not.toContainEqual(expect.objectContaining({ type: "ack_activation" }));

    releaseSnapshot?.();
    await handling;
    expect(parsed(socket)).toContainEqual({
      type: "ack_activation",
      runtime_generation: 4,
      lease_epoch: 8,
      activation_id: "activation-1",
    });

    await client.handleMessage({
      type: "activation_ack",
      result: {
        decision: {
          status: "accepted",
          reason: null,
          runtime_generation: 4,
          lease_epoch: 8,
          owner_presentation_id: presentation.presentation_id,
        },
        broker_state: brokerState({
          lease_epoch: 8,
          owner_presentation_id: presentation.presentation_id,
        }),
        snapshot: null,
      },
    });
    expect(client.state.mode).toBe("owner");
  });

  it("reports mirror viewport without resizing and enables owner-only input and ordered resize", async () => {
    const { client, socket } = harness();
    await client.handleMessage(registered());
    socket.sent = [];

    expect(client.reportViewport(112, 31)).toBe(true);
    expect(client.resize(112, 31)).toBe(false);
    expect(client.sendText("hello")).toBe(false);
    expect(parsed(socket)).toEqual([{
      type: "report_viewport",
      runtime_generation: 4,
      cols: 112,
      rows: 31,
    }]);

    await client.handleMessage({
      type: "presentation_state",
      presentation,
      broker_state: brokerState({ owner_presentation_id: presentation.presentation_id }),
    });
    socket.sent = [];
    expect(client.resize(112, 31)).toBe(true);
    expect(client.resize(113, 32)).toBe(true);
    expect(client.sendText("hello")).toBe(true);
    expect(parsed(socket)).toEqual([
      {
        type: "resize",
        runtime_generation: 4,
        lease_epoch: 7,
        geometry_sequence: 1,
        cols: 112,
        rows: 31,
      },
      {
        type: "resize",
        runtime_generation: 4,
        lease_epoch: 7,
        geometry_sequence: 2,
        cols: 113,
        rows: 32,
      },
      { type: "input", runtime_generation: 4, lease_epoch: 7, data: "hello" },
    ]);
  });

  it("recovers from gaps with a snapshot barrier and ignores already applied events", async () => {
    const applySnapshot = vi.fn();
    const { client, events, socket } = harness({ applySnapshot });
    await client.handleMessage(registered());
    socket.sent = [];

    await client.handleMessage({
      type: "events",
      batch: {
        status: "gap",
        runtime_generation: 4,
        events: [],
        next_sequence: 20,
        available_from_sequence: 20,
        latest_sequence: 20,
        recovery_snapshot: snapshot({ sequence_barrier: 20 }),
      },
    });
    await client.handleMessage({
      type: "events",
      batch: {
        status: "events",
        runtime_generation: 4,
        events: [
          { type: "output", sequence: 19, runtime_generation: 4, bytes_base64: btoa("old") },
          { type: "output", sequence: 21, runtime_generation: 4, bytes_base64: btoa("new") },
        ],
        next_sequence: 21,
        available_from_sequence: 19,
        latest_sequence: 21,
        recovery_snapshot: null,
      },
    });

    expect(applySnapshot).toHaveBeenLastCalledWith(expect.objectContaining({ sequence_barrier: 20 }));
    expect(events).toEqual([[
      { type: "output", sequence: 21, runtime_generation: 4, bytes_base64: btoa("new") },
    ]]);
    expect(parsed(socket)).toContainEqual({
      type: "ack_events",
      runtime_generation: 4,
      applied_sequence: 21,
    });
  });

  it("applies an owner resync snapshot before acknowledging the resync", async () => {
    const order: string[] = [];
    const socket = new TestSocket();
    socket.send = (payload) => {
      order.push(`send:${JSON.parse(payload).type}`);
      socket.sent.push(payload);
    };
    const client = new RemoteTerminalSessionClient(socket as unknown as WebSocket, {
      applySnapshot: async () => {
        order.push("snapshot");
      },
      applyEvents: vi.fn(),
      onState: vi.fn(),
    });
    await client.handleMessage(registered({
      presentation: { ...presentation, requires_resync: true },
      broker_state: brokerState({ owner_presentation_id: presentation.presentation_id }),
    }));
    socket.sent = [];
    order.length = 0;

    expect(client.sendText("blocked while resyncing")).toBe(false);
    expect(client.sendBinary(btoa("blocked"))).toBe(false);
    expect(client.resize(120, 40)).toBe(false);
    expect(client.beginOwnerResync()).toBe(true);
    await client.handleMessage({
      type: "owner_resync_begin",
      result: {
        decision: {
          status: "accepted",
          reason: null,
          runtime_generation: 4,
          lease_epoch: 7,
          owner_presentation_id: presentation.presentation_id,
        },
        resync_id: "resync-1",
        snapshot: snapshot({ sequence_barrier: 18 }),
        sequence_barrier: 18,
      },
    });

    expect(order).toEqual([
      "send:begin_owner_resync",
      "snapshot",
      "send:ack_events",
      "send:ack_owner_resync",
    ]);
    expect(parsed(socket)).not.toContainEqual(expect.objectContaining({ type: "input" }));

    await client.handleMessage({
      type: "owner_resync_ack",
      result: {
        decision: {
          status: "accepted",
          reason: null,
          runtime_generation: 4,
          lease_epoch: 7,
          owner_presentation_id: presentation.presentation_id,
        },
        broker_state: brokerState({ owner_presentation_id: presentation.presentation_id }),
      },
    });
    expect(client.sendText("ready")).toBe(true);
    expect(parsed(socket)).toContainEqual({
      type: "input",
      runtime_generation: 4,
      lease_epoch: 7,
      data: "ready",
    });
  });

  it("keeps lease disagreement nonfatal and accepts a replacement registration generation", async () => {
    const applySnapshot = vi.fn();
    const { client, nonfatal, socket } = harness({ applySnapshot });
    await client.handleMessage(registered());
    socket.sent = [];

    await client.handleMessage({
      type: "error",
      code: "not_owner",
      fatal: false,
      decision: {
        status: "rejected",
        reason: "not_owner",
        runtime_generation: 4,
        lease_epoch: 9,
        owner_presentation_id: "desktop:presentation-2",
      },
    });
    expect(nonfatal).toHaveBeenCalledWith("not_owner", expect.objectContaining({ reason: "not_owner" }));
    expect(socket.readyState).toBe(WebSocket.OPEN);

    await client.handleMessage(registered({
      broker_state: brokerState({ runtime_generation: 5, lease_epoch: 1, stream_sequence: 3 }),
      initial_snapshot: snapshot({ runtime_generation: 5, sequence_barrier: 3 }),
    }));
    expect(client.state.broker_state?.runtime_generation).toBe(5);
    expect(client.state.applied_sequence).toBe(3);
    expect(applySnapshot).toHaveBeenLastCalledWith(expect.objectContaining({ runtime_generation: 5 }));
  });

  it("detaches once and ignores messages that race cleanup", async () => {
    const applySnapshot = vi.fn();
    const { client, socket } = harness({ applySnapshot });
    await client.handleMessage(registered());
    socket.sent = [];

    client.detach();
    client.detach();
    await client.handleMessage(registered());

    expect(parsed(socket)).toEqual([{ type: "detach" }]);
    expect(applySnapshot).toHaveBeenCalledTimes(1);
  });

  it("bounds slow-renderer ingress and recovers overflowing output from an authoritative snapshot", async () => {
    let releaseFirstBatch: (() => void) | undefined;
    const firstBatchBlocked = new Promise<void>((resolve) => {
      releaseFirstBatch = resolve;
    });
    const socket = new TestSocket();
    const appliedSequences: number[][] = [];
    let blockFirstBatch = true;
    let releaseRecoverySnapshot: (() => void) | undefined;
    const recoverySnapshotBlocked = new Promise<void>((resolve) => {
      releaseRecoverySnapshot = resolve;
    });
    const applySnapshot = vi.fn(async (nextSnapshot: TerminalSnapshot) => {
      if (nextSnapshot.sequence_barrier === 30) await recoverySnapshotBlocked;
    });
    const client = new RemoteTerminalSessionClient(socket as unknown as WebSocket, {
      applySnapshot,
      applyEvents: async (events) => {
        appliedSequences.push(events.map((event) => event.sequence));
        if (blockFirstBatch) {
          blockFirstBatch = false;
          await firstBatchBlocked;
        }
      },
      onState: vi.fn(),
    });
    await client.handleMessage(registered());
    socket.sent = [];
    const largeOutput = btoa("x".repeat(200 * 1024));
    const eventMessage = (sequence: number) => ({
      type: "events" as const,
      batch: {
        status: "events" as const,
        runtime_generation: 4,
        events: [{ type: "output" as const, sequence, runtime_generation: 4, bytes_base64: largeOutput }],
        next_sequence: sequence,
        available_from_sequence: sequence,
        latest_sequence: sequence,
        recovery_snapshot: null,
      },
    });

    const pending = [client.handleMessage(eventMessage(13))];
    await Promise.resolve();
    for (let sequence = 14; sequence <= 22; sequence += 1) {
      pending.push(client.handleMessage(eventMessage(sequence)));
    }
    releaseFirstBatch?.();
    await Promise.all(pending);

    expect(appliedSequences).toEqual([[13]]);
    expect(parsed(socket).filter((message) => message.type === "request_snapshot")).toHaveLength(1);

    const recovery = client.handleMessage({ type: "snapshot", snapshot: snapshot({ sequence_barrier: 30 }) });
    await Promise.resolve();
    const droppedDuringRecovery = client.handleMessage(eventMessage(31));
    expect(parsed(socket)).not.toContainEqual(expect.objectContaining({
      type: "request_events", after_sequence: 30,
    }));
    releaseRecoverySnapshot?.();
    await Promise.all([recovery, droppedDuringRecovery]);
    expect(parsed(socket)).toContainEqual({
      type: "request_events", runtime_generation: 4, after_sequence: 30,
    });

    // The server replays from the acknowledged snapshot barrier; only that
    // replay is rendered, not the incremental that arrived during recovery.
    await client.handleMessage(eventMessage(31));
    expect(applySnapshot).toHaveBeenLastCalledWith(expect.objectContaining({ sequence_barrier: 30 }));
    expect(appliedSequences).toEqual([[13], [31]]);
  });
});
