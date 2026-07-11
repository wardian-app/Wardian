import type {
  RemoteTerminalBrokerEvent,
  RemoteTerminalStreamMessage,
  RemoteTerminalV2ClientMessage,
  RemoteTerminalV2State,
  TerminalBrokerState,
  TerminalLeaseDecision,
  TerminalSnapshot,
} from "../../types";

export type RemoteTerminalSessionCallbacks = {
  applySnapshot: (snapshot: TerminalSnapshot) => void | Promise<void>;
  applyEvents: (events: readonly RemoteTerminalBrokerEvent[]) => void | Promise<void>;
  onState: (state: RemoteTerminalV2State) => void;
  onLeaseDecision?: (decision: TerminalLeaseDecision) => void;
  onNonfatalError?: (code: string, decision?: TerminalLeaseDecision) => void;
  onFatalError?: (code: string) => void;
};

const MAX_QUEUED_EVENT_BATCHES = 8;
const MAX_QUEUED_EVENT_BYTES = 512 * 1024;

type IngressItem = {
  message: RemoteTerminalStreamMessage | null;
  eventBytes: number;
  resolve: () => void;
  reject: (error: unknown) => void;
};

function isSocketOpen(socket: WebSocket) {
  return socket.readyState === WebSocket.OPEN;
}

/**
 * Ordered protocol-v2 adapter for one authenticated remote broker presentation.
 *
 * It deliberately keeps socket transport separate from xterm rendering. All
 * snapshot/event writes are serialized and acknowledged only after the renderer
 * callback completes, which makes activation and recovery barriers explicit.
 */
export class RemoteTerminalSessionClient {
  readonly #socket: WebSocket;
  readonly #callbacks: RemoteTerminalSessionCallbacks;
  #state: RemoteTerminalV2State = {
    presentation: null,
    broker_state: null,
    mode: "connecting",
    applied_sequence: 0,
  };
  #ingressQueue: IngressItem[] = [];
  #ingressDraining = false;
  #queuedEventBatches = 0;
  #queuedEventBytes = 0;
  #recoveryQueued = false;
  #awaitingRecoverySnapshot = false;
  #detached = false;
  #geometrySequence = 0;

  constructor(socket: WebSocket, callbacks: RemoteTerminalSessionCallbacks) {
    this.#socket = socket;
    this.#callbacks = callbacks;
  }

  get state() {
    return this.#state;
  }

  handleMessage(message: RemoteTerminalStreamMessage) {
    if (this.#detached) return Promise.resolve();
    const isIncrementalBatch = message.type === "events" && message.batch.status === "events";
    const eventBytes = isIncrementalBatch ? this.#eventBatchBytes(message) : 0;
    if (isIncrementalBatch && this.#awaitingRecoverySnapshot) {
      return Promise.resolve();
    }
    return new Promise<void>((resolve, reject) => {
      if (
        isIncrementalBatch
        && (this.#queuedEventBatches + 1 > MAX_QUEUED_EVENT_BATCHES
          || this.#queuedEventBytes + eventBytes > MAX_QUEUED_EVENT_BYTES)
      ) {
        const recoveryAlreadyQueued = this.#dropQueuedEventBatches();
        this.#awaitingRecoverySnapshot = true;
        if (!recoveryAlreadyQueued && !this.#recoveryQueued) {
          this.#recoveryQueued = true;
          this.#ingressQueue.push({ message: null, eventBytes: 0, resolve: () => undefined, reject });
        }
        resolve();
        this.#drainIngress();
        return;
      }
      this.#ingressQueue.push({ message, eventBytes, resolve, reject });
      if (isIncrementalBatch) {
        this.#queuedEventBatches += 1;
        this.#queuedEventBytes += eventBytes;
      }
      this.#drainIngress();
    });
  }

  reportViewport(cols: number, rows: number) {
    const brokerState = this.#state.broker_state;
    if (!brokerState) return false;
    return this.#send({
      type: "report_viewport",
      runtime_generation: brokerState.runtime_generation,
      cols,
      rows,
    });
  }

  setPresentationState(
    visibility: "visible" | "hidden",
    renderState: "mounted" | "suspended",
    requestedInteraction: "interactive" | "read_only",
    viewport?: { cols: number; rows: number },
  ) {
    const brokerState = this.#state.broker_state;
    if (!brokerState) return false;
    return this.#send({
      type: "set_presentation_state",
      runtime_generation: brokerState.runtime_generation,
      observed_lease_epoch: brokerState.lease_epoch,
      visibility,
      render_state: renderState,
      requested_interaction: requestedInteraction,
      ...(viewport ? { cols: viewport.cols, rows: viewport.rows } : {}),
    });
  }

  activate() {
    const brokerState = this.#state.broker_state;
    if (!brokerState || this.#state.mode === "owner") return false;
    return this.#send({
      type: "begin_activation",
      runtime_generation: brokerState.runtime_generation,
      observed_lease_epoch: brokerState.lease_epoch,
    });
  }

  beginOwnerResync() {
    const brokerState = this.#state.broker_state;
    if (!brokerState || this.#state.mode !== "owner") return false;
    return this.#send({
      type: "begin_owner_resync",
      runtime_generation: brokerState.runtime_generation,
      lease_epoch: brokerState.lease_epoch,
    });
  }

  sendText(data: string) {
    const brokerState = this.#ownedBrokerState();
    if (!brokerState || !data) return false;
    return this.#send({
      type: "input",
      runtime_generation: brokerState.runtime_generation,
      lease_epoch: brokerState.lease_epoch,
      data,
    });
  }

  sendBinary(dataBase64: string) {
    const brokerState = this.#ownedBrokerState();
    if (!brokerState || !dataBase64) return false;
    return this.#send({
      type: "binary",
      runtime_generation: brokerState.runtime_generation,
      lease_epoch: brokerState.lease_epoch,
      data_base64: dataBase64,
    });
  }

  resize(cols: number, rows: number) {
    const brokerState = this.#ownedBrokerState();
    if (!brokerState) return false;
    this.#geometrySequence += 1;
    return this.#send({
      type: "resize",
      runtime_generation: brokerState.runtime_generation,
      lease_epoch: brokerState.lease_epoch,
      geometry_sequence: this.#geometrySequence,
      cols,
      rows,
    });
  }

  requestSnapshot() {
    return this.#send({ type: "request_snapshot" });
  }

  detach() {
    if (this.#detached) return;
    this.#send({ type: "detach" });
    this.#detached = true;
    for (const item of this.#ingressQueue.splice(0)) item.resolve();
    this.#queuedEventBatches = 0;
    this.#queuedEventBytes = 0;
  }

  async #handleMessage(message: RemoteTerminalStreamMessage) {
    if (this.#detached) return;

    if (message.type === "registered") {
      this.#awaitingRecoverySnapshot = false;
      this.#state = {
        presentation: message.presentation,
        broker_state: message.broker_state,
        mode: this.#mode(message.presentation.presentation_id, message.broker_state),
        applied_sequence: 0,
      };
      this.#geometrySequence = 0;
      await this.#applySnapshot(message.initial_snapshot);
      this.#publishState();
      this.#requestEventsIfNeeded();
      return;
    }

    if (message.type === "presentation_state") {
      const brokerState = message.broker_state ?? this.#state.broker_state;
      this.#state = {
        ...this.#state,
        presentation: message.presentation,
        broker_state: brokerState,
        mode: brokerState
          ? this.#mode(message.presentation.presentation_id, brokerState)
          : "connecting",
      };
      this.#publishState();
      return;
    }

    if (message.type === "activation_begin") {
      const { result } = message;
      this.#notifyDecision(result.decision);
      if (
        result.decision.status === "accepted"
        && result.activation_id
        && result.snapshot
      ) {
        await this.#applySnapshot(result.snapshot);
        this.#send({
          type: "ack_activation",
          runtime_generation: result.decision.runtime_generation,
          lease_epoch: result.decision.lease_epoch,
          activation_id: result.activation_id,
        });
      }
      return;
    }

    if (message.type === "activation_ack") {
      this.#notifyDecision(message.result.decision);
      if (message.result.snapshot) {
        await this.#applySnapshot(message.result.snapshot);
      }
      this.#replaceBrokerState(message.result.broker_state);
      return;
    }

    if (message.type === "owner_resync_begin") {
      const { result } = message;
      this.#notifyDecision(result.decision);
      if (result.decision.status === "accepted" && result.resync_id && result.snapshot) {
        await this.#applySnapshot(result.snapshot);
        this.#send({
          type: "ack_owner_resync",
          runtime_generation: result.decision.runtime_generation,
          lease_epoch: result.decision.lease_epoch,
          resync_id: result.resync_id,
        });
      }
      return;
    }

    if (message.type === "owner_resync_ack") {
      this.#notifyDecision(message.result.decision);
      if (message.result.decision.status === "accepted" && this.#state.presentation) {
        this.#state = {
          ...this.#state,
          presentation: { ...this.#state.presentation, requires_resync: false },
        };
      }
      this.#replaceBrokerState(message.result.broker_state);
      return;
    }

    if (message.type === "input_result") {
      this.#notifyDecision(message.decision);
      return;
    }

    if (message.type === "resize_result") {
      this.#notifyDecision(message.result.decision);
      if (message.result.snapshot) {
        await this.#applySnapshot(message.result.snapshot);
      }
      return;
    }

    if (message.type === "snapshot" && "snapshot" in message) {
      await this.#applySnapshot(message.snapshot);
      this.#awaitingRecoverySnapshot = false;
      this.#requestEventsIfNeeded();
      return;
    }

    if (message.type === "events") {
      await this.#applyEventBatch(message.batch);
      return;
    }

    if (message.type === "error") {
      if (message.decision) this.#notifyDecision(message.decision);
      if (message.fatal) {
        this.#callbacks.onFatalError?.(message.code);
      } else {
        this.#callbacks.onNonfatalError?.(message.code, message.decision);
      }
    }
  }

  async #applyEventBatch(batch: Extract<RemoteTerminalStreamMessage, { type: "events" }>["batch"]) {
    const currentGeneration = this.#state.broker_state?.runtime_generation ?? 0;
    if (batch.runtime_generation < currentGeneration) return;

    if (batch.status === "gap" || batch.status === "generation_changed") {
      if (!batch.recovery_snapshot) {
        this.#callbacks.onNonfatalError?.(`terminal_${batch.status}_without_snapshot`);
        this.requestSnapshot();
        return;
      }
      await this.#applySnapshot(batch.recovery_snapshot);
      this.#awaitingRecoverySnapshot = false;
      this.#requestEventsIfNeeded();
      return;
    }

    if (batch.status === "terminated") return;

    const events = batch.events.filter(
      (event) => event.sequence > this.#state.applied_sequence
        && event.runtime_generation === batch.runtime_generation,
    );
    if (events.length > 0) {
      await this.#callbacks.applyEvents(events);
      this.#applyBrokerEventState(events);
    }
    this.#state = {
      ...this.#state,
      applied_sequence: Math.max(this.#state.applied_sequence, batch.next_sequence),
    };
    this.#publishState();
    this.#ackAppliedSequence(batch.runtime_generation, this.#state.applied_sequence);
    if (this.#state.applied_sequence < batch.latest_sequence) {
      this.#requestEventsIfNeeded();
    }
  }

  async #applySnapshot(snapshot: TerminalSnapshot) {
    await this.#callbacks.applySnapshot(snapshot);
    const brokerState = this.#state.broker_state;
    this.#state = {
      ...this.#state,
      broker_state: brokerState
        ? {
            ...brokerState,
            runtime_generation: snapshot.runtime_generation,
            stream_sequence: Math.max(brokerState.stream_sequence, snapshot.sequence_barrier),
            geometry: snapshot.geometry,
          }
        : brokerState,
      applied_sequence: snapshot.sequence_barrier,
    };
    this.#ackAppliedSequence(snapshot.runtime_generation, snapshot.sequence_barrier);
  }

  #applyBrokerEventState(events: readonly RemoteTerminalBrokerEvent[]) {
    let brokerState = this.#state.broker_state;
    if (!brokerState) return;
    for (const event of events) {
      if (event.type === "geometry") {
        brokerState = { ...brokerState, geometry: event.geometry };
      } else if (event.type === "ownership") {
        brokerState = {
          ...brokerState,
          owner_presentation_id: event.owner_presentation_id,
          lease_epoch: event.lease_epoch,
        };
      } else if (event.type === "lifecycle") {
        brokerState = {
          ...brokerState,
          runtime_generation: event.runtime_generation,
          runtime_state: event.lifecycle === "runtime_terminated"
            ? "terminated"
            : event.lifecycle === "runtime_paused"
              ? "paused"
              : "live",
        };
      }
      brokerState = {
        ...brokerState,
        stream_sequence: Math.max(brokerState.stream_sequence, event.sequence),
      };
    }
    this.#replaceBrokerState(brokerState);
  }

  #replaceBrokerState(brokerState: TerminalBrokerState) {
    this.#state = {
      ...this.#state,
      broker_state: brokerState,
      mode: this.#state.presentation
        ? this.#mode(this.#state.presentation.presentation_id, brokerState)
        : "connecting",
    };
    this.#publishState();
  }

  #mode(presentationId: string, brokerState: TerminalBrokerState) {
    return brokerState.owner_presentation_id === presentationId ? "owner" as const : "mirror" as const;
  }

  #ownedBrokerState() {
    return this.#state.mode === "owner" && !this.#state.presentation?.requires_resync
      ? this.#state.broker_state
      : null;
  }

  #notifyDecision(decision: TerminalLeaseDecision) {
    this.#callbacks.onLeaseDecision?.(decision);
    const brokerState = this.#state.broker_state;
    if (!brokerState) return;
    const generationAdvanced = decision.runtime_generation > brokerState.runtime_generation;
    this.#replaceBrokerState({
      ...brokerState,
      runtime_generation: Math.max(brokerState.runtime_generation, decision.runtime_generation),
      lease_epoch: generationAdvanced
        ? decision.lease_epoch
        : Math.max(brokerState.lease_epoch, decision.lease_epoch),
      owner_presentation_id: decision.owner_presentation_id,
    });
  }

  #ackAppliedSequence(runtimeGeneration: number, appliedSequence: number) {
    this.#send({
      type: "ack_events",
      runtime_generation: runtimeGeneration,
      applied_sequence: appliedSequence,
    });
  }

  #requestEventsIfNeeded() {
    const brokerState = this.#state.broker_state;
    if (!brokerState) return;
    this.#send({
      type: "request_events",
      runtime_generation: brokerState.runtime_generation,
      after_sequence: this.#state.applied_sequence,
    });
  }

  #send(message: RemoteTerminalV2ClientMessage) {
    if (this.#detached || !isSocketOpen(this.#socket)) return false;
    this.#socket.send(JSON.stringify(message));
    return true;
  }

  #publishState() {
    this.#callbacks.onState(this.#state);
  }

  #eventBatchBytes(message: Extract<RemoteTerminalStreamMessage, { type: "events" }>) {
    return message.batch.events.reduce((total, event) => {
      if (event.type !== "output") return total + 64;
      return total + Math.ceil(event.bytes_base64.length * 0.75);
    }, 0);
  }

  #dropQueuedEventBatches() {
    const retained: IngressItem[] = [];
    let recoveryAlreadyQueued = false;
    for (const item of this.#ingressQueue) {
      if (item.message?.type === "events" && item.message.batch.status === "events") {
        item.resolve();
      } else {
        retained.push(item);
        recoveryAlreadyQueued ||= item.message === null
          || item.message?.type === "registered"
          || item.message?.type === "snapshot"
          || (item.message?.type === "events"
            && (item.message.batch.status === "gap"
              || item.message.batch.status === "generation_changed"));
      }
    }
    this.#ingressQueue = retained;
    this.#queuedEventBatches = 0;
    this.#queuedEventBytes = 0;
    return recoveryAlreadyQueued;
  }

  #drainIngress() {
    if (this.#ingressDraining) return;
    this.#ingressDraining = true;
    void (async () => {
      while (!this.#detached) {
        const item = this.#ingressQueue.shift();
        if (!item) break;
        if (item.message?.type === "events" && item.message.batch.status === "events") {
          this.#queuedEventBatches = Math.max(0, this.#queuedEventBatches - 1);
          this.#queuedEventBytes = Math.max(0, this.#queuedEventBytes - item.eventBytes);
        }
        try {
          if (item.message) {
            await this.#handleMessage(item.message);
          } else {
            this.#recoveryQueued = false;
            this.requestSnapshot();
          }
          item.resolve();
        } catch (error) {
          item.reject(error);
        }
      }
    })().finally(() => {
      this.#ingressDraining = false;
      if (this.#ingressQueue.length > 0 && !this.#detached) this.#drainIngress();
    });
  }
}
