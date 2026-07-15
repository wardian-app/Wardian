import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  TerminalActivationAckResult,
  TerminalActivationBeginResult,
  TerminalBrokerEvent,
  TerminalBrokerState,
  TerminalEventBatch,
  TerminalEventSubscriptionResult,
  TerminalEventsReady,
  TerminalGeometryCommitResult,
  TerminalLeaseDecision,
  TerminalOwnerResyncAckResult,
  TerminalOwnerResyncBeginResult,
  TerminalPresentationRegistration,
  TerminalPresentationRegistrationResult,
  TerminalPresentationState,
  TerminalPresentationUpdateResult,
  TerminalSessionLifecycleNotification,
  TerminalSnapshot,
} from "../../types";

const MAX_EVENTS_PER_BATCH = 256;
const MAX_BYTES_PER_BATCH = 256 * 1024;
const MAX_BATCHES_PER_TURN = 32;

export type TerminalPresentationCallbacks = {
  applySnapshot: (snapshot: TerminalSnapshot) => void | Promise<void>;
  applyEvents: (events: readonly TerminalBrokerEvent[]) => void | Promise<void>;
  onBrokerState?: (state: TerminalBrokerState) => void;
  onRegistrationRecovered?: (result: TerminalPresentationRegistrationResult) => void;
  onLeaseDecision?: (decision: TerminalLeaseDecision) => void;
  onLifecycle?: (notification: TerminalSessionLifecycleNotification) => void;
};

type PresentationBinding = {
  callbacks: TerminalPresentationCallbacks;
  registration: TerminalPresentationRegistration;
  state: TerminalPresentationState | null;
  runtimeGeneration: number;
  appliedSequence: number;
};

function consumerId(sessionId: string) {
  return `desktop:${sessionId}`;
}

function terminalLease(
  sessionId: string,
  presentationId: string,
  brokerState: TerminalBrokerState,
) {
  return {
    session_id: sessionId,
    presentation_id: presentationId,
    runtime_generation: brokerState.runtime_generation,
    lease_epoch: brokerState.lease_epoch,
  };
}

function isPresentationNotFound(error: unknown) {
  return String(error).includes("PresentationNotFound");
}

/**
 * One ordered desktop broker feed for a native terminal session.
 *
 * Presentations remain independent xterms. This client owns only the shared
 * consumer/cursor, serializes snapshot/event application, and fans canonical
 * broker events to every currently registered local presentation.
 */
export class TerminalSessionClient {
  readonly sessionId: string;
  readonly #consumerId: string;
  readonly #presentations = new Map<string, PresentationBinding>();
  #brokerState: TerminalBrokerState | null = null;
  #replacementOwnerCandidate: string | null = null;
  #lastOwnerPresentationId: string | null = null;
  #runtimeTransitionPending = false;
  #runtimeGeneration = 0;
  #cursor = 0;
  #subscription: Promise<TerminalEventSubscriptionResult> | null = null;
  #eventUnlisten: UnlistenFn | null = null;
  #lifecycleUnlisten: UnlistenFn | null = null;
  #listenerSetup: Promise<void> | null = null;
  #drainInFlight = false;
  #drainQueued = false;
  #destroyed = false;
  #operation = Promise.resolve();

  constructor(sessionId: string) {
    if (!sessionId) {
      throw new Error("Terminal session id is required");
    }
    this.sessionId = sessionId;
    this.#consumerId = consumerId(sessionId);
  }

  get brokerState() {
    return this.#brokerState;
  }

  get presentationCount() {
    return this.#presentations.size;
  }

  /** Refreshes a mounted view's callbacks without replaying broker registration. */
  rebindPresentation(
    presentationId: string,
    callbacks: TerminalPresentationCallbacks,
  ): boolean {
    const binding = this.#presentations.get(presentationId);
    if (!binding) return false;
    binding.callbacks = callbacks;
    return true;
  }

  async registerPresentation(
    registration: TerminalPresentationRegistration,
    callbacks: TerminalPresentationCallbacks,
  ) {
    if (registration.session_id !== this.sessionId) {
      throw new Error("Presentation registration targets a different terminal session");
    }
    return this.#serialize(async () => {
      await this.#ensureListeners();
      this.#destroyed = false;
      const binding: PresentationBinding = {
        callbacks,
        registration,
        state: null,
        runtimeGeneration: 0,
        appliedSequence: 0,
      };
      this.#presentations.set(registration.presentation_id, binding);
      try {
        const result = await invoke<TerminalPresentationRegistrationResult>(
          "register_terminal_presentation",
          { request: registration },
        );
        if (!result) {
          this.#presentations.delete(registration.presentation_id);
          throw new Error("TerminalSessionProtocolUnavailable");
        }
        if (this.#presentations.get(registration.presentation_id) !== binding) {
          return result;
        }
        binding.state = result.presentation;
        this.#setBrokerState(result.broker_state);
        await this.#applySnapshot(binding, result.initial_snapshot);
        await this.#ensureSubscription(result.broker_state.runtime_generation);
        return result;
      } catch (error) {
        // A restoring placeholder can register before its PTY exists. Keep the
        // logical binding and retry when the broker announces a newer runtime.
        if (!String(error).includes("SessionNotFound")) {
          this.#presentations.delete(registration.presentation_id);
        }
        throw error;
      }
    });
  }

  async updatePresentation(
    presentationId: string,
    update: Omit<
      TerminalPresentationRegistration,
      "presentation_id" | "session_id" | "client_kind"
    >,
  ) {
    return this.#serialize(async () => {
      const binding = this.#presentations.get(presentationId);
      if (!binding) {
        throw new Error(`Terminal presentation not registered: ${presentationId}`);
      }
      binding.registration = {
        ...binding.registration,
        ...update,
      };
      if (!this.#brokerState || this.#runtimeTransitionPending) {
        return null;
      }
      let result: TerminalPresentationUpdateResult;
      try {
        result = await invoke<TerminalPresentationUpdateResult>(
          "update_terminal_presentation",
          {
            request: {
              presentation_id: presentationId,
              session_id: this.sessionId,
              runtime_generation: this.#brokerState.runtime_generation,
              desired_geometry: update.desired_geometry,
              visibility: update.visibility,
              render_state: update.render_state,
              requested_interaction: update.requested_interaction,
              observed_lease_epoch: update.observed_lease_epoch,
            },
          },
        );
      } catch (error) {
        if (!isPresentationNotFound(error)) {
          throw error;
        }
        const recovered = await this.#recoverPresentation(binding);
        return {
          presentation: recovered.presentation,
          broker_state: recovered.broker_state,
        };
      }
      binding.state = result.presentation;
      this.#setBrokerState(result.broker_state);
      return result;
    });
  }

  async unregisterPresentation(presentationId: string) {
    return this.#serialize(async () => {
      const binding = this.#presentations.get(presentationId);
      this.#presentations.delete(presentationId);
      try {
        if (binding) {
          try {
            const state = await invoke<TerminalBrokerState>("unregister_terminal_presentation", {
              request: {
                session_id: this.sessionId,
                presentation_id: presentationId,
                runtime_generation: this.#brokerState?.runtime_generation ?? 0,
              },
            });
            this.#setBrokerState(state);
          } catch (error) {
            if (!String(error).includes("SessionNotFound")) {
              throw error;
            }
          }
        }
      } finally {
        if (this.#presentations.size === 0) {
          await this.destroy();
        }
      }
    });
  }

  /**
   * Requests a fresh snapshot and applies it only while the initiating
   * presentation generation still owns the restore. The guard is evaluated
   * after IPC resolves, immediately before callback dispatch, so a rebind
   * cannot redirect stale snapshot work into replacement callbacks.
   */
  async requestPresentationSnapshot(
    presentationId: string,
    shouldApply?: () => boolean,
  ) {
    const binding = this.#requiredPresentation(presentationId);
    return this.#serialize(async () => {
      if (this.#runtimeTransitionPending) {
        return null;
      }
      const snapshot = await invoke<TerminalSnapshot>("request_terminal_snapshot", {
        request: { session_id: this.sessionId },
      });
      await this.#applySnapshot(binding, snapshot, true, shouldApply);
      if (snapshot.runtime_generation > this.#runtimeGeneration) {
        this.#runtimeGeneration = snapshot.runtime_generation;
      }
      return snapshot;
    });
  }

  async activate(presentationId: string) {
    const binding = this.#requiredPresentation(presentationId);
    return this.#serialize(() => this.#activatePresentation(presentationId, binding));
  }

  async resyncOwner(presentationId: string) {
    const binding = this.#requiredPresentation(presentationId);
    return this.#serialize(async () => {
      const state = this.#requiredBrokerState();
      const begin = await invoke<TerminalOwnerResyncBeginResult>(
        "begin_terminal_owner_resync",
        {
          request: {
            session_id: this.sessionId,
            presentation_id: presentationId,
            runtime_generation: state.runtime_generation,
            lease_epoch: state.lease_epoch,
          },
        },
      );
      this.#notifyDecision(begin.decision);
      if (begin.decision.status !== "accepted" || !begin.resync_id || !begin.snapshot) {
        return { begin, ack: null };
      }
      await this.#applySnapshot(binding, begin.snapshot);
      const ack = await invoke<TerminalOwnerResyncAckResult>("ack_terminal_owner_resync", {
        request: {
          session_id: this.sessionId,
          presentation_id: presentationId,
          runtime_generation: begin.decision.runtime_generation,
          lease_epoch: begin.decision.lease_epoch,
          resync_id: begin.resync_id,
        },
      });
      this.#setBrokerState(ack.broker_state);
      this.#notifyDecision(ack.decision);
      this.queueDrain();
      return { begin, ack };
    });
  }

  async sendText(presentationId: string, input: string) {
    const state = this.#requiredBrokerState();
    const decision = await invoke<TerminalLeaseDecision>(
      "send_terminal_presentation_input",
      {
        request: {
          ...terminalLease(this.sessionId, presentationId, state),
          input,
        },
      },
    );
    this.#notifyDecision(decision);
    return decision;
  }

  async sendBinary(presentationId: string, input: readonly number[]) {
    const state = this.#requiredBrokerState();
    const decision = await invoke<TerminalLeaseDecision>(
      "send_terminal_presentation_binary",
      {
        request: {
          ...terminalLease(this.sessionId, presentationId, state),
          input: Array.from(input),
        },
      },
    );
    this.#notifyDecision(decision);
    return decision;
  }

  async resize(
    presentationId: string,
    geometrySequence: number,
    cols: number,
    rows: number,
  ) {
    return this.#serialize(async () => {
      const binding = this.#requiredPresentation(presentationId);
      const state = this.#requiredBrokerState();
      const result = await invoke<TerminalGeometryCommitResult>("resize_terminal_presentation", {
        request: {
          ...terminalLease(this.sessionId, presentationId, state),
          geometry_sequence: geometrySequence,
          cols,
          rows,
        },
      });
      this.#notifyDecision(result.decision);
      if (result.snapshot) {
        await this.#applySnapshot(binding, result.snapshot);
      }
      return result;
    });
  }

  async reportViewport(presentationId: string, cols: number, rows: number) {
    const state = this.#requiredBrokerState();
    const presentation = await invoke<TerminalPresentationState>(
      "report_terminal_presentation_viewport",
      {
        request: {
          session_id: this.sessionId,
          presentation_id: presentationId,
          runtime_generation: state.runtime_generation,
          cols,
          rows,
        },
      },
    );
    const binding = this.#presentations.get(presentationId);
    if (binding) {
      binding.state = presentation;
    }
    return presentation;
  }

  queueDrain() {
    if (this.#destroyed || this.#presentations.size === 0) {
      return;
    }
    if (this.#drainInFlight) {
      this.#drainQueued = true;
      return;
    }
    this.#drainInFlight = true;
    queueMicrotask(() => {
      void this.#serialize(() => this.#drain())
        .catch((error) => {
          if (!this.#destroyed) {
            console.warn("Terminal broker event drain failed; waiting for the next wake-up.", error);
          }
        })
        .finally(() => {
          this.#drainInFlight = false;
          if (this.#drainQueued) {
            this.#drainQueued = false;
            this.queueDrain();
          }
        });
    });
  }

  async destroy() {
    if (this.#destroyed) {
      return;
    }
    this.#destroyed = true;
    this.#drainQueued = false;
    const subscribed = this.#subscription !== null;
    this.#subscription = null;
    if (subscribed) {
      try {
        await invoke("unsubscribe_terminal_events", {
          request: { session_id: this.sessionId, consumer_id: this.#consumerId },
        });
      } catch {
        // The runtime may have ended before the final local presentation.
      }
    }
    this.#eventUnlisten?.();
    this.#lifecycleUnlisten?.();
    this.#eventUnlisten = null;
    this.#lifecycleUnlisten = null;
    this.#listenerSetup = null;
    terminalSessionClients.delete(this.sessionId);
  }

  async #ensureListeners() {
    if (this.#listenerSetup) {
      return this.#listenerSetup;
    }
    this.#listenerSetup = Promise.all([
      listen<TerminalEventsReady>("terminal-session-events-ready", (event) => {
        if (event.payload.session_id !== this.sessionId) {
          return;
        }
        if (event.payload.runtime_generation < this.#runtimeGeneration) {
          return;
        }
        this.queueDrain();
      }).then((unlisten) => {
        if (this.#destroyed) {
          unlisten();
        } else {
          this.#eventUnlisten = unlisten;
        }
      }),
      listen<TerminalSessionLifecycleNotification>(
        "terminal-session-lifecycle",
        (event) => {
          const notification = event.payload;
          if (notification.session_id !== this.sessionId) {
            return;
          }
          void this.#serialize(async () => {
            if (
              this.#destroyed ||
              notification.runtime_generation < this.#runtimeGeneration
            ) {
              return;
            }
            if (
              notification.runtime_generation === this.#runtimeGeneration &&
              (notification.lifecycle === "runtime_paused" ||
                notification.lifecycle === "runtime_replaced") &&
              !this.#runtimeTransitionPending
            ) {
              this.#replacementOwnerCandidate = this.#brokerState?.owner_presentation_id
                ?? this.#lastOwnerPresentationId;
              this.#runtimeTransitionPending = true;
              // The paused actor and its presentation registry are no longer a
              // valid IPC target. The next generation will establish a fresh
              // consumer after each local presentation re-registers.
              this.#subscription = null;
            }
            for (const binding of this.#presentations.values()) {
              binding.callbacks.onLifecycle?.(notification);
            }
            if (notification.runtime_generation > this.#runtimeGeneration) {
              this.#runtimeTransitionPending = true;
              const previousOwnerPresentationId = this.#replacementOwnerCandidate
                ?? this.#brokerState?.owner_presentation_id
                ?? this.#lastOwnerPresentationId
                ?? null;
              this.#replacementOwnerCandidate = null;
              this.#runtimeGeneration = notification.runtime_generation;
              this.#subscription = null;
              await this.#retryRegistrationsForGeneration(previousOwnerPresentationId);
            } else {
              this.queueDrain();
            }
          }).catch(() => undefined);
        },
      ).then((unlisten) => {
        if (this.#destroyed) {
          unlisten();
        } else {
          this.#lifecycleUnlisten = unlisten;
        }
      }),
    ]).then(() => undefined);
    return this.#listenerSetup;
  }

  async #retryRegistrationsForGeneration(previousOwnerPresentationId: string | null = null) {
    let recoveredPresentations = 0;
    for (const [presentationId, binding] of this.#presentations) {
      try {
        const result = await invoke<TerminalPresentationRegistrationResult>(
          "register_terminal_presentation",
          { request: binding.registration },
        );
        if (!this.#presentations.has(presentationId)) {
          continue;
        }
        binding.state = result.presentation;
        this.#setBrokerState(result.broker_state);
        await this.#applySnapshot(binding, result.initial_snapshot);
        binding.callbacks.onRegistrationRecovered?.(result);
        recoveredPresentations += 1;
      } catch {
        // A later lifecycle notification or explicit remount retries again.
      }
    }
    if (this.#brokerState) {
      await this.#ensureSubscription(this.#brokerState.runtime_generation).catch(() => undefined);
      await this.#restorePreviousOwner(previousOwnerPresentationId);
      this.queueDrain();
    }
    if (recoveredPresentations === this.#presentations.size) {
      this.#runtimeTransitionPending = false;
    }
  }

  async #recoverPresentation(
    binding: PresentationBinding,
  ) {
    this.#runtimeTransitionPending = true;
    const previousGeneration = this.#runtimeGeneration;
    const result = await invoke<TerminalPresentationRegistrationResult>(
      "register_terminal_presentation",
      { request: binding.registration },
    );
    if (result.broker_state.runtime_generation !== previousGeneration) {
      this.#subscription = null;
    }
    binding.state = result.presentation;
    this.#setBrokerState(result.broker_state);
    await this.#applySnapshot(binding, result.initial_snapshot);
    await this.#ensureSubscription(result.broker_state.runtime_generation);
    binding.callbacks.onRegistrationRecovered?.(result);
    await this.#restorePreviousOwner(this.#lastOwnerPresentationId);
    this.#runtimeTransitionPending = false;
    return result;
  }

  async #restorePreviousOwner(previousOwnerPresentationId: string | null) {
    const previousOwner = previousOwnerPresentationId === null
      ? null
      : this.#presentations.get(previousOwnerPresentationId);
    const previousOwnerState = previousOwner?.state;
    if (
      previousOwner &&
      previousOwnerState?.visibility === "visible" &&
      previousOwnerState.render_state === "mounted" &&
      previousOwnerState.interaction_capability === "interactive" &&
      this.#brokerState?.owner_presentation_id === null
    ) {
      // A replacement runtime intentionally starts with no owner. Restore the
      // exact presentation that owned the previous generation only after it
      // has re-registered and synchronized; never promote a mirror merely
      // because it happened to register first.
      await this.#activatePresentation(previousOwner.registration.presentation_id, previousOwner)
        .catch(() => undefined);
    }
  }

  async #activatePresentation(
    presentationId: string,
    binding: PresentationBinding,
  ) {
    const state = this.#requiredBrokerState();
    const begin = await invoke<TerminalActivationBeginResult>("begin_terminal_activation", {
      request: {
        session_id: this.sessionId,
        presentation_id: presentationId,
        runtime_generation: state.runtime_generation,
        observed_lease_epoch: state.lease_epoch,
      },
    });
    this.#notifyDecision(begin.decision);
    if (begin.decision.status !== "accepted" || !begin.activation_id || !begin.snapshot) {
      return { begin, ack: null };
    }
    await this.#applySnapshot(binding, begin.snapshot);
    const ack = await invoke<TerminalActivationAckResult>("ack_terminal_activation", {
      request: {
        session_id: this.sessionId,
        presentation_id: presentationId,
        runtime_generation: begin.decision.runtime_generation,
        lease_epoch: begin.decision.lease_epoch,
        activation_id: begin.activation_id,
      },
    });
    this.#setBrokerState(ack.broker_state);
    this.#notifyDecision(ack.decision);
    if (ack.snapshot) {
      await this.#applySnapshot(binding, ack.snapshot);
    }
    this.queueDrain();
    return { begin, ack };
  }

  async #ensureSubscription(runtimeGeneration: number) {
    if (this.#subscription && this.#runtimeGeneration === runtimeGeneration) {
      return this.#subscription;
    }
    this.#runtimeGeneration = runtimeGeneration;
    const subscription = invoke<TerminalEventSubscriptionResult>("subscribe_terminal_events", {
      request: {
        session_id: this.sessionId,
        consumer_id: this.#consumerId,
        client_kind: "desktop",
        runtime_generation: runtimeGeneration,
      },
    });
    this.#subscription = subscription;
    try {
      const result = await subscription;
      if (this.#subscription !== subscription) {
        return result;
      }
      this.#setBrokerState(result.broker_state);
      this.#cursor = result.initial_snapshot.sequence_barrier;
      await this.#applySnapshotToAll(result.initial_snapshot);
      return result;
    } catch (error) {
      if (this.#subscription === subscription) {
        this.#subscription = null;
      }
      throw error;
    }
  }

  async #drain() {
    const state = this.#brokerState;
    if (!state || this.#destroyed) {
      return;
    }
    await this.#ensureSubscription(state.runtime_generation);
    let batches = 0;
    while (!this.#destroyed && batches < MAX_BATCHES_PER_TURN) {
      batches += 1;
      const batch = await invoke<TerminalEventBatch>("read_terminal_events", {
        request: {
          session_id: this.sessionId,
          consumer_id: this.#consumerId,
          runtime_generation: this.#runtimeGeneration,
          after_sequence: this.#cursor,
          max_events: MAX_EVENTS_PER_BATCH,
          max_bytes: MAX_BYTES_PER_BATCH,
        },
      });
      if (batch.runtime_generation < this.#runtimeGeneration) {
        return;
      }
      if (batch.status === "gap" || batch.status === "generation_changed") {
        if (!batch.recovery_snapshot) {
          throw new Error(`Terminal ${batch.status} response omitted its recovery snapshot`);
        }
        await this.#applySnapshotToAll(batch.recovery_snapshot);
        this.#runtimeGeneration = batch.recovery_snapshot.runtime_generation;
        this.#cursor = batch.recovery_snapshot.sequence_barrier;
        if (batch.status === "generation_changed") {
          this.#subscription = null;
          await this.#ensureSubscription(this.#runtimeGeneration);
        }
        continue;
      }
      if (batch.status === "terminated") {
        return;
      }
      if (batch.events.length > 0) {
        this.#applyBrokerEventState(batch.events);
        await Promise.all(
          Array.from(this.#presentations.values(), (binding) =>
            this.#applyEvents(binding, batch.events),
          ),
        );
      }
      this.#cursor = Math.max(this.#cursor, batch.next_sequence);
      await invoke("ack_terminal_events", {
        request: {
          session_id: this.sessionId,
          consumer_id: this.#consumerId,
          runtime_generation: this.#runtimeGeneration,
          applied_sequence: this.#cursor,
        },
      });
      if (this.#cursor >= batch.latest_sequence || batch.events.length === 0) {
        return;
      }
    }
    this.#drainQueued = true;
  }

  async #applySnapshotToAll(snapshot: TerminalSnapshot) {
    await Promise.all(
      Array.from(this.#presentations.values(), (binding) =>
        this.#applySnapshot(binding, snapshot),
      ),
    );
  }

  async #applySnapshot(
    binding: PresentationBinding,
    snapshot: TerminalSnapshot,
    force = false,
    shouldApply?: () => boolean,
  ) {
    if (
      !force &&
      (snapshot.runtime_generation < binding.runtimeGeneration ||
        (snapshot.runtime_generation === binding.runtimeGeneration &&
          snapshot.sequence_barrier <= binding.appliedSequence))
    ) {
      return;
    }
    if (shouldApply && !shouldApply()) {
      return;
    }
    await binding.callbacks.applySnapshot(snapshot);
    binding.runtimeGeneration = snapshot.runtime_generation;
    binding.appliedSequence = snapshot.sequence_barrier;
  }

  async #applyEvents(
    binding: PresentationBinding,
    events: readonly TerminalBrokerEvent[],
  ) {
    const pending = events.filter(
      (event) =>
        event.runtime_generation === binding.runtimeGeneration &&
        event.sequence > binding.appliedSequence,
    );
    if (pending.length === 0) {
      return;
    }
    await binding.callbacks.applyEvents(pending);
    binding.appliedSequence = pending[pending.length - 1]?.sequence ?? binding.appliedSequence;
  }

  #applyBrokerEventState(events: readonly TerminalBrokerEvent[]) {
    if (!this.#brokerState) {
      return;
    }
    let next = this.#brokerState;
    for (const event of events) {
      if (event.runtime_generation !== next.runtime_generation) {
        continue;
      }
      if (event.type === "geometry") {
        next = { ...next, geometry: event.geometry, stream_sequence: event.sequence };
      } else if (event.type === "ownership") {
        next = {
          ...next,
          lease_epoch: event.lease_epoch,
          owner_presentation_id: event.owner_presentation_id,
          pending_activation: event.activation_id === null ? null : next.pending_activation,
          stream_sequence: event.sequence,
        };
      } else if (event.type === "lifecycle") {
        const runtimeState = event.lifecycle === "runtime_paused"
          ? "paused"
          : event.lifecycle === "runtime_terminated"
            ? "terminated"
            : "live";
        next = { ...next, runtime_state: runtimeState, stream_sequence: event.sequence };
      } else {
        next = { ...next, stream_sequence: event.sequence };
      }
    }
    if (next !== this.#brokerState) {
      this.#setBrokerState(next);
    }
  }

  #setBrokerState(state: TerminalBrokerState) {
    if (state.runtime_generation < this.#runtimeGeneration) {
      return;
    }
    this.#brokerState = state;
    this.#runtimeGeneration = state.runtime_generation;
    if (state.owner_presentation_id) {
      this.#lastOwnerPresentationId = state.owner_presentation_id;
    }
    for (const binding of this.#presentations.values()) {
      binding.callbacks.onBrokerState?.(state);
    }
  }

  #notifyDecision(decision: TerminalLeaseDecision) {
    if (this.#brokerState && decision.runtime_generation >= this.#runtimeGeneration) {
      this.#brokerState = {
        ...this.#brokerState,
        runtime_generation: decision.runtime_generation,
        lease_epoch: decision.lease_epoch,
        owner_presentation_id: decision.owner_presentation_id,
      };
      this.#runtimeGeneration = decision.runtime_generation;
      if (decision.owner_presentation_id) {
        this.#lastOwnerPresentationId = decision.owner_presentation_id;
      }
    }
    for (const binding of this.#presentations.values()) {
      binding.callbacks.onLeaseDecision?.(decision);
    }
  }

  #requiredPresentation(presentationId: string) {
    const binding = this.#presentations.get(presentationId);
    if (!binding) {
      throw new Error(`Terminal presentation not registered: ${presentationId}`);
    }
    return binding;
  }

  #requiredBrokerState() {
    if (!this.#brokerState) {
      throw new Error(`Terminal runtime is not available: ${this.sessionId}`);
    }
    return this.#brokerState;
  }

  #serialize<T>(operation: () => Promise<T>) {
    const result = this.#operation.then(operation, operation);
    this.#operation = result.then(
      () => undefined,
      () => undefined,
    );
    return result;
  }
}

const terminalSessionClients = new Map<string, TerminalSessionClient>();

export function terminalSessionClientFor(sessionId: string) {
  const existing = terminalSessionClients.get(sessionId);
  if (existing) {
    return existing;
  }
  const client = new TerminalSessionClient(sessionId);
  terminalSessionClients.set(sessionId, client);
  return client;
}

export async function resetTerminalSessionClientsForTesting() {
  const clients = Array.from(terminalSessionClients.values());
  terminalSessionClients.clear();
  await Promise.all(clients.map((client) => client.destroy()));
}

export const __terminalSessionClientTesting = {
  MAX_BATCHES_PER_TURN,
  MAX_BYTES_PER_BATCH,
  MAX_EVENTS_PER_BATCH,
  consumerId,
  clientCount: () => terminalSessionClients.size,
};
