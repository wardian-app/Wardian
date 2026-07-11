import type { EventCallback } from "@tauri-apps/api/event";
import type {
  TerminalBrokerEvent,
  TerminalBrokerState,
  TerminalEventBatch,
  TerminalPresentationRegistration,
  TerminalSessionLifecycleNotification,
  TerminalSnapshot,
} from "../../types";

const tauri = vi.hoisted(() => ({
  invoke: vi.fn(),
  listeners: new Map<string, Set<EventCallback<unknown>>>(),
}));

vi.mock("@tauri-apps/api/core", () => ({ invoke: tauri.invoke }));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async (eventName: string, callback: EventCallback<unknown>) => {
    const callbacks = tauri.listeners.get(eventName) ?? new Set<EventCallback<unknown>>();
    callbacks.add(callback);
    tauri.listeners.set(eventName, callbacks);
    return () => callbacks.delete(callback);
  }),
}));

import {
  __terminalSessionClientTesting,
  resetTerminalSessionClientsForTesting,
  terminalSessionClientFor,
} from "./terminalSessionClient";

function geometry(cols = 80, rows = 24) {
  return { cols, rows };
}

function brokerState(generation = 1, sequence = 0): TerminalBrokerState {
  return {
    session_id: "agent-1",
    runtime_generation: generation,
    lease_epoch: 0,
    stream_sequence: sequence,
    interaction_sequence: 0,
    geometry: geometry(),
    owner_presentation_id: null,
    pending_activation: null,
    runtime_state: "live",
  };
}

function snapshot(generation = 1, barrier = 0): TerminalSnapshot {
  return {
    snapshot_id: `snapshot-${generation}-${barrier}`,
    session_id: "agent-1",
    runtime_generation: generation,
    sequence_barrier: barrier,
    geometry: geometry(),
    terminal_state_base64: "",
    visible_grid: "",
    scrollback: [],
  };
}

function registration(presentationId: string): TerminalPresentationRegistration {
  return {
    presentation_id: presentationId,
    session_id: "agent-1",
    client_kind: "desktop",
    desired_geometry: geometry(),
    visibility: "visible",
    render_state: "mounted",
    requested_interaction: "interactive",
    observed_lease_epoch: 0,
  };
}

function registeredResult(presentationId: string, generation = 1) {
  return {
    presentation: {
      presentation_id: presentationId,
      client_kind: "desktop" as const,
      desired_geometry: geometry(),
      visibility: "visible" as const,
      render_state: "mounted" as const,
      interaction_capability: "interactive" as const,
      interaction_sequence: 1,
      requires_resync: false,
    },
    broker_state: brokerState(generation),
    initial_snapshot: snapshot(generation),
  };
}

function eventsBatch(
  events: TerminalBrokerEvent[],
  nextSequence: number,
  latestSequence = nextSequence,
): TerminalEventBatch {
  return {
    status: "events",
    runtime_generation: 1,
    events,
    next_sequence: nextSequence,
    available_from_sequence: 1,
    latest_sequence: latestSequence,
    recovery_snapshot: null,
  };
}

function emit<T>(eventName: string, payload: T) {
  for (const callback of tauri.listeners.get(eventName) ?? []) {
    callback({ event: eventName, id: 1, payload } as never);
  }
}

async function settle() {
  await Promise.resolve();
  await Promise.resolve();
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((next) => {
    resolve = next;
  });
  return { promise, resolve };
}

describe("TerminalSessionClient", () => {
  beforeEach(async () => {
    await resetTerminalSessionClientsForTesting();
    tauri.invoke.mockReset();
    tauri.listeners.clear();
  });

  afterEach(async () => {
    await resetTerminalSessionClientsForTesting();
  });

  it("shares exactly one desktop consumer across independent presentations", async () => {
    const snapshotsA: string[] = [];
    const snapshotsB: string[] = [];
    tauri.invoke.mockImplementation(async (command: string, args?: unknown) => {
      const request = (args as { request?: { presentation_id?: string } } | undefined)?.request;
      if (command === "register_terminal_presentation") {
        return registeredResult(request?.presentation_id ?? "missing");
      }
      if (command === "subscribe_terminal_events") {
        return { broker_state: brokerState(), initial_snapshot: snapshot() };
      }
      if (command === "unregister_terminal_presentation") {
        return brokerState();
      }
      if (command === "unsubscribe_terminal_events") {
        return undefined;
      }
      throw new Error(`Unexpected command: ${command}`);
    });

    const first = terminalSessionClientFor("agent-1");
    const second = terminalSessionClientFor("agent-1");
    expect(second).toBe(first);
    await first.registerPresentation(registration("pane-a"), {
      applySnapshot: (value) => {
        snapshotsA.push(value.snapshot_id);
      },
      applyEvents: () => undefined,
    });
    await second.registerPresentation(registration("pane-b"), {
      applySnapshot: (value) => {
        snapshotsB.push(value.snapshot_id);
      },
      applyEvents: () => undefined,
    });

    expect(tauri.invoke).toHaveBeenCalledTimes(3);
    expect(tauri.invoke.mock.calls.filter(([command]) => command === "subscribe_terminal_events"))
      .toHaveLength(1);
    expect(first.presentationCount).toBe(2);
    expect(snapshotsA.length).toBeGreaterThan(0);
    expect(snapshotsB.length).toBeGreaterThan(0);

    await first.unregisterPresentation("pane-a");
    expect(tauri.invoke).not.toHaveBeenCalledWith("unsubscribe_terminal_events", expect.anything());
    await first.unregisterPresentation("pane-b");
    expect(tauri.invoke).toHaveBeenCalledWith("unsubscribe_terminal_events", {
      request: { session_id: "agent-1", consumer_id: "desktop:agent-1" },
    });
    expect(__terminalSessionClientTesting.clientCount()).toBe(0);
  });

  it("filters shared feed events through each presentation snapshot barrier", async () => {
    const appliedA: number[][] = [];
    const appliedB: number[][] = [];
    let registrations = 0;
    tauri.invoke.mockImplementation(async (command: string) => {
      if (command === "register_terminal_presentation") {
        registrations += 1;
        if (registrations === 1) {
          return registeredResult("pane-a");
        }
        const result = registeredResult("pane-b");
        result.broker_state = brokerState(1, 2);
        result.initial_snapshot = snapshot(1, 2);
        return result;
      }
      if (command === "subscribe_terminal_events") {
        return { broker_state: brokerState(1, 3), initial_snapshot: snapshot() };
      }
      if (command === "read_terminal_events") {
        return eventsBatch(
          [1, 2, 3].map((sequence) => ({
            sequence,
            runtime_generation: 1,
            type: "output" as const,
            bytes: [64 + sequence],
          })),
          3,
          3,
        );
      }
      if (command === "ack_terminal_events") {
        return { runtime_generation: 1, acknowledged_sequence: 3 };
      }
      if (command === "unsubscribe_terminal_events") {
        return undefined;
      }
      throw new Error(`Unexpected command: ${command}`);
    });

    const client = terminalSessionClientFor("agent-1");
    await client.registerPresentation(registration("pane-a"), {
      applySnapshot: () => undefined,
      applyEvents: (events) => {
        appliedA.push(events.map((event) => event.sequence));
      },
    });
    await client.registerPresentation(registration("pane-b"), {
      applySnapshot: () => undefined,
      applyEvents: (events) => {
        appliedB.push(events.map((event) => event.sequence));
      },
    });
    client.queueDrain();

    await vi.waitFor(() => expect(appliedA).toEqual([[1, 2, 3]]));
    expect(appliedB).toEqual([[3]]);
  });

  it("serializes unregister behind an in-flight registration and always destroys the last feed", async () => {
    const registrationGate = deferred<ReturnType<typeof registeredResult>>();
    const commands: string[] = [];
    tauri.invoke.mockImplementation(async (command: string) => {
      commands.push(command);
      if (command === "register_terminal_presentation") {
        return registrationGate.promise;
      }
      if (command === "subscribe_terminal_events") {
        return { broker_state: brokerState(), initial_snapshot: snapshot() };
      }
      if (command === "unregister_terminal_presentation") {
        return brokerState();
      }
      if (command === "unsubscribe_terminal_events") {
        return undefined;
      }
      throw new Error(`Unexpected command: ${command}`);
    });

    const client = terminalSessionClientFor("agent-1");
    const registering = client.registerPresentation(registration("pane-a"), {
      applySnapshot: () => undefined,
      applyEvents: () => undefined,
    });
    const unregistering = client.unregisterPresentation("pane-a");
    registrationGate.resolve(registeredResult("pane-a"));
    await Promise.all([registering, unregistering]);

    expect(commands.indexOf("unregister_terminal_presentation")).toBeGreaterThan(
      commands.indexOf("register_terminal_presentation"),
    );
    expect(commands).toContain("unsubscribe_terminal_events");
    expect(__terminalSessionClientTesting.clientCount()).toBe(0);
  });

  it("drains bounded batches to caught-up and acknowledges only applied sequences", async () => {
    const applied: number[][] = [];
    const reads: number[] = [];
    const acknowledgements: number[] = [];
    tauri.invoke.mockImplementation(async (command: string, args?: unknown) => {
      const request = (args as { request?: { after_sequence?: number; applied_sequence?: number } })
        .request;
      if (command === "register_terminal_presentation") {
        return registeredResult("pane-a");
      }
      if (command === "subscribe_terminal_events") {
        return { broker_state: brokerState(1, 2), initial_snapshot: snapshot() };
      }
      if (command === "read_terminal_events") {
        reads.push(request?.after_sequence ?? -1);
        return reads.length === 1
          ? eventsBatch([{ sequence: 1, runtime_generation: 1, type: "output", bytes: [65] }], 1, 2)
          : eventsBatch([{ sequence: 2, runtime_generation: 1, type: "output", bytes: [66] }], 2, 2);
      }
      if (command === "ack_terminal_events") {
        acknowledgements.push(request?.applied_sequence ?? -1);
        return { runtime_generation: 1, acknowledged_sequence: request?.applied_sequence ?? 0 };
      }
      if (command === "unsubscribe_terminal_events") {
        return undefined;
      }
      throw new Error(`Unexpected command: ${command}`);
    });

    const client = terminalSessionClientFor("agent-1");
    await client.registerPresentation(registration("pane-a"), {
      applySnapshot: () => undefined,
      applyEvents: (events) => {
        applied.push(events.map((event) => event.sequence));
      },
    });
    emit("terminal-session-events-ready", {
      session_id: "agent-1",
      runtime_generation: 1,
      latest_sequence: 2,
    });

    await vi.waitFor(() => expect(acknowledgements).toEqual([1, 2]));
    expect(reads).toEqual([0, 1]);
    expect(applied).toEqual([[1], [2]]);
    for (const call of tauri.invoke.mock.calls.filter(([command]) => command === "read_terminal_events")) {
      expect(call[1]).toMatchObject({
        request: {
          max_events: 256,
          max_bytes: 256 * 1024,
        },
      });
    }
  });

  it("applies a gap snapshot barrier before replaying later events", async () => {
    const order: string[] = [];
    let readCount = 0;
    tauri.invoke.mockImplementation(async (command: string) => {
      if (command === "register_terminal_presentation") {
        return registeredResult("pane-a");
      }
      if (command === "subscribe_terminal_events") {
        return { broker_state: brokerState(1, 6), initial_snapshot: snapshot() };
      }
      if (command === "read_terminal_events") {
        readCount += 1;
        if (readCount === 1) {
          return {
            status: "gap",
            runtime_generation: 1,
            events: [],
            next_sequence: 5,
            available_from_sequence: 6,
            latest_sequence: 6,
            recovery_snapshot: snapshot(1, 5),
          } satisfies TerminalEventBatch;
        }
        return eventsBatch(
          [{ sequence: 6, runtime_generation: 1, type: "output", bytes: [67] }],
          6,
          6,
        );
      }
      if (command === "ack_terminal_events") {
        return { runtime_generation: 1, acknowledged_sequence: 6 };
      }
      if (command === "unsubscribe_terminal_events") {
        return undefined;
      }
      throw new Error(`Unexpected command: ${command}`);
    });

    const client = terminalSessionClientFor("agent-1");
    await client.registerPresentation(registration("pane-a"), {
      applySnapshot: (value) => {
        order.push(`snapshot:${value.sequence_barrier}`);
      },
      applyEvents: (events) => {
        order.push(`events:${events[0]?.sequence}`);
      },
    });
    order.length = 0;
    client.queueDrain();

    await vi.waitFor(() => expect(order).toEqual(["snapshot:5", "events:6"]));
  });

  it("performs explicit activation and same-epoch owner resync handshakes", async () => {
    const commands: string[] = [];
    tauri.invoke.mockImplementation(async (command: string) => {
      commands.push(command);
      if (command === "register_terminal_presentation") {
        return registeredResult("pane-a");
      }
      if (command === "subscribe_terminal_events") {
        return { broker_state: brokerState(), initial_snapshot: snapshot() };
      }
      if (command === "begin_terminal_activation") {
        return {
          decision: { status: "accepted", reason: null, runtime_generation: 1, lease_epoch: 1, owner_presentation_id: null },
          activation_id: "activation-1",
          snapshot: snapshot(1, 1),
          sequence_barrier: 1,
        };
      }
      if (command === "ack_terminal_activation") {
        return {
          decision: { status: "accepted", reason: null, runtime_generation: 1, lease_epoch: 1, owner_presentation_id: "pane-a" },
          broker_state: { ...brokerState(), lease_epoch: 1, owner_presentation_id: "pane-a" },
          snapshot: snapshot(1, 1),
        };
      }
      if (command === "begin_terminal_owner_resync") {
        return {
          decision: { status: "accepted", reason: null, runtime_generation: 1, lease_epoch: 1, owner_presentation_id: "pane-a" },
          resync_id: "resync-1",
          snapshot: snapshot(1, 2),
          sequence_barrier: 2,
        };
      }
      if (command === "ack_terminal_owner_resync") {
        return {
          decision: { status: "accepted", reason: null, runtime_generation: 1, lease_epoch: 1, owner_presentation_id: "pane-a" },
          broker_state: { ...brokerState(), lease_epoch: 1, owner_presentation_id: "pane-a" },
        };
      }
      if (command === "read_terminal_events") {
        return eventsBatch([], 0, 0);
      }
      if (command === "ack_terminal_events") {
        return { runtime_generation: 1, acknowledged_sequence: 0 };
      }
      if (command === "unsubscribe_terminal_events") {
        return undefined;
      }
      throw new Error(`Unexpected command: ${command}`);
    });

    const client = terminalSessionClientFor("agent-1");
    await client.registerPresentation(registration("pane-a"), {
      applySnapshot: () => undefined,
      applyEvents: () => undefined,
    });
    await client.activate("pane-a");
    await client.resyncOwner("pane-a");

    expect(commands).toEqual(expect.arrayContaining([
      "begin_terminal_activation",
      "ack_terminal_activation",
      "begin_terminal_owner_resync",
      "ack_terminal_owner_resync",
    ]));
  });

  it("orders generation replacement snapshots after an in-flight old-generation apply", async () => {
    const applyGate = deferred<void>();
    const order: string[] = [];
    let registrations = 0;
    let reads = 0;
    tauri.invoke.mockImplementation(async (command: string) => {
      if (command === "register_terminal_presentation") {
        registrations += 1;
        return registeredResult("pane-a", registrations === 1 ? 1 : 2);
      }
      if (command === "subscribe_terminal_events") {
        const generation = registrations === 1 ? 1 : 2;
        return { broker_state: brokerState(generation), initial_snapshot: snapshot(generation) };
      }
      if (command === "read_terminal_events") {
        reads += 1;
        if (reads === 1) {
          return eventsBatch(
            [{ sequence: 1, runtime_generation: 1, type: "output", bytes: [65] }],
            1,
            1,
          );
        }
        return { ...eventsBatch([], 0, 0), runtime_generation: 2 };
      }
      if (command === "ack_terminal_events") {
        return { runtime_generation: 1, acknowledged_sequence: 1 };
      }
      if (command === "unsubscribe_terminal_events") {
        return undefined;
      }
      throw new Error(`Unexpected command: ${command}`);
    });

    const client = terminalSessionClientFor("agent-1");
    await client.registerPresentation(registration("pane-a"), {
      applySnapshot: (value) => {
        order.push(`snapshot:${value.runtime_generation}`);
      },
      applyEvents: async () => {
        order.push("events:start");
        await applyGate.promise;
        order.push("events:done");
      },
    });
    order.length = 0;
    client.queueDrain();
    await vi.waitFor(() => expect(order).toEqual(["events:start"]));

    emit<TerminalSessionLifecycleNotification>("terminal-session-lifecycle", {
      session_id: "agent-1",
      runtime_generation: 2,
      lifecycle: "runtime_replaced",
    });
    await settle();
    expect(registrations).toBe(1);

    applyGate.resolve();
    await vi.waitFor(() => expect(registrations).toBe(2));
    expect(order).toEqual(["events:start", "events:done", "snapshot:2"]);
  });

  it("ignores regressive lifecycle hints and re-registers once for a newer generation", async () => {
    let registrations = 0;
    const lifecycle: number[] = [];
    tauri.invoke.mockImplementation(async (command: string) => {
      if (command === "register_terminal_presentation") {
        registrations += 1;
        return registeredResult("pane-a", registrations === 1 ? 1 : 2);
      }
      if (command === "subscribe_terminal_events") {
        const generation = registrations === 1 ? 1 : 2;
        return { broker_state: brokerState(generation), initial_snapshot: snapshot(generation) };
      }
      if (command === "read_terminal_events") {
        return {
          ...eventsBatch([], 0, 0),
          runtime_generation: registrations === 1 ? 1 : 2,
        };
      }
      if (command === "ack_terminal_events") {
        return { runtime_generation: registrations === 1 ? 1 : 2, acknowledged_sequence: 0 };
      }
      if (command === "unsubscribe_terminal_events") {
        return undefined;
      }
      throw new Error(`Unexpected command: ${command}`);
    });

    const client = terminalSessionClientFor("agent-1");
    await client.registerPresentation(registration("pane-a"), {
      applySnapshot: () => undefined,
      applyEvents: () => undefined,
      onLifecycle: (value) => lifecycle.push(value.runtime_generation),
    });
    emit<TerminalSessionLifecycleNotification>("terminal-session-lifecycle", {
      session_id: "agent-1",
      runtime_generation: 2,
      lifecycle: "runtime_replaced",
    });
    await vi.waitFor(() => expect(registrations).toBe(2));
    emit<TerminalSessionLifecycleNotification>("terminal-session-lifecycle", {
      session_id: "agent-1",
      runtime_generation: 1,
      lifecycle: "runtime_replaced",
    });
    await settle();

    expect(lifecycle).toEqual([2]);
    expect(client.brokerState?.runtime_generation).toBe(2);
  });

  it("returns stale input and resize lease decisions without rejecting the presentation", async () => {
    const rejectedDecision = {
      status: "rejected" as const,
      reason: "not_owner" as const,
      runtime_generation: 1,
      lease_epoch: 2,
      owner_presentation_id: "pane-owner",
    };
    tauri.invoke.mockImplementation(async (command: string) => {
      if (command === "register_terminal_presentation") {
        return registeredResult("pane-mirror");
      }
      if (command === "subscribe_terminal_events") {
        return { broker_state: brokerState(), initial_snapshot: snapshot() };
      }
      if (command === "send_terminal_presentation_input") {
        return rejectedDecision;
      }
      if (command === "resize_terminal_presentation") {
        return {
          decision: rejectedDecision,
          geometry_sequence: 4,
          geometry: geometry(),
          snapshot: null,
        };
      }
      if (command === "unsubscribe_terminal_events") {
        return undefined;
      }
      throw new Error(`Unexpected command: ${command}`);
    });

    const client = terminalSessionClientFor("agent-1");
    await client.registerPresentation(registration("pane-mirror"), {
      applySnapshot: () => undefined,
      applyEvents: () => undefined,
    });

    await expect(client.sendText("pane-mirror", "ignored")).resolves.toEqual(rejectedDecision);
    await expect(client.resize("pane-mirror", 4, 80, 24)).resolves.toMatchObject({
      decision: rejectedDecision,
    });
    expect(client.brokerState).toMatchObject({
      lease_epoch: 2,
      owner_presentation_id: "pane-owner",
    });
  });

  it("contains a feed read failure and retries on the next wake-up", async () => {
    const warning = vi.spyOn(console, "warn").mockImplementation(() => undefined);
    let reads = 0;
    tauri.invoke.mockImplementation(async (command: string) => {
      if (command === "register_terminal_presentation") {
        return registeredResult("pane-a");
      }
      if (command === "subscribe_terminal_events") {
        return { broker_state: brokerState(), initial_snapshot: snapshot() };
      }
      if (command === "read_terminal_events") {
        reads += 1;
        if (reads === 1) {
          throw new Error("transient read failure");
        }
        return eventsBatch([], 0, 0);
      }
      if (command === "ack_terminal_events") {
        return { runtime_generation: 1, acknowledged_sequence: 0 };
      }
      if (command === "unsubscribe_terminal_events") {
        return undefined;
      }
      throw new Error(`Unexpected command: ${command}`);
    });

    const client = terminalSessionClientFor("agent-1");
    await client.registerPresentation(registration("pane-a"), {
      applySnapshot: () => undefined,
      applyEvents: () => undefined,
    });
    client.queueDrain();
    await vi.waitFor(() => expect(warning).toHaveBeenCalledOnce());

    client.queueDrain();
    await vi.waitFor(() => expect(reads).toBe(2));
    warning.mockRestore();
  });
});
