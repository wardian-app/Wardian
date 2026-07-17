import { useEffect, useMemo, useSyncExternalStore } from "react";
import type {
  FileResourceEventV1,
  FileResourceSnapshotV1,
  OpenFileResourceRequestV1,
} from "../../types";
import { FileResourceClient, fileResourceClient } from "./fileResourceClient";

export type FileResourceStatus = "loading" | "ready" | "error";

export type UseFileResourceResult = {
  status: FileResourceStatus;
  snapshot: FileResourceSnapshotV1 | null;
  error: Error | null;
  retry: () => Promise<void>;
};

type FileResourceState = Omit<UseFileResourceResult, "retry">;
type StateListener = () => void;

const controllersByClient = new WeakMap<
  FileResourceClient,
  Map<string, FileResourceController>
>();
const MAX_PRE_SNAPSHOT_REOPENS = 2;
const PRE_SNAPSHOT_RECONCILIATION_ERROR =
  "The file kept changing while Wardian opened it. Retry to load an authoritative snapshot.";

function requestIdentity(request: OpenFileResourceRequestV1) {
  return JSON.stringify([
    request.path.replace(/\\/g, "/"),
    request.agent_id,
    request.user_file_capability_id,
  ]);
}

function errorFrom(error: unknown) {
  if (error instanceof Error) return error;
  if (typeof error === "object" && error !== null && "message" in error) {
    return new Error(String(error.message));
  }
  return new Error(String(error));
}

class FileResourceController {
  readonly #listeners = new Set<StateListener>();
  readonly #client: FileResourceClient;
  readonly #request: OpenFileResourceRequestV1;
  readonly #identity: string;
  #state: FileResourceState = { status: "loading", snapshot: null, error: null };
  #consumerCount = 0;
  #started = false;
  #disposed = false;
  #loadGeneration = 0;
  #listenerSetup: Promise<void> | null = null;
  #unlisten: (() => void) | null = null;
  #preSnapshotRevisions = new Map<string, FileResourceEventV1>();

  constructor(
    client: FileResourceClient,
    request: OpenFileResourceRequestV1,
    identity: string,
  ) {
    this.#client = client;
    this.#request = request;
    this.#identity = identity;
  }

  getSnapshot = () => this.#state;

  subscribe = (listener: StateListener) => {
    this.#listeners.add(listener);
    return () => this.#listeners.delete(listener);
  };

  acquire() {
    this.#consumerCount += 1;
    if (!this.#started) {
      this.#started = true;
      void this.#start();
    }
    return () => {
      this.#consumerCount = Math.max(0, this.#consumerCount - 1);
      if (this.#consumerCount === 0) {
        queueMicrotask(() => {
          if (this.#consumerCount === 0) void this.#dispose();
        });
      }
    };
  }

  retry = async () => {
    try {
      await this.#ensureListener();
      await this.#load();
    } catch (error) {
      this.#fail(error);
    }
  };

  async #start() {
    try {
      await this.#ensureListener();
      await this.#load();
    } catch (error) {
      this.#fail(error);
    }
  }

  #ensureListener() {
    if (this.#listenerSetup) return this.#listenerSetup;
    const setup = this.#client.listenForRevisions((event) => {
      this.#applyRevision(event);
    }).then((unlisten) => {
      if (this.#disposed) {
        unlisten();
      } else {
        this.#unlisten = unlisten;
      }
    });
    this.#listenerSetup = setup;
    return setup.catch((error) => {
      if (this.#listenerSetup === setup) this.#listenerSetup = null;
      throw error;
    });
  }

  async #load() {
    const generation = ++this.#loadGeneration;
    this.#publish({ ...this.#state, status: "loading", error: null });
    try {
      for (let reopenAttempt = 0; ; reopenAttempt += 1) {
        const snapshot = await this.#client.open(this.#request);
        if (this.#disposed || generation !== this.#loadGeneration) {
          await this.#client.close(snapshot.subscription_id).catch(() => undefined);
          return;
        }
        const observed = this.#preSnapshotRevisions.get(snapshot.resource_id);
        if (observed) this.#preSnapshotRevisions.delete(snapshot.resource_id);
        if (observed && observed.revision > snapshot.revision) {
          await this.#client.close(snapshot.subscription_id).catch(() => undefined);
          if (this.#disposed || generation !== this.#loadGeneration) return;
          if (reopenAttempt < MAX_PRE_SNAPSHOT_REOPENS) continue;
          this.#preSnapshotRevisions.clear();
          throw new Error(PRE_SNAPSHOT_RECONCILIATION_ERROR);
        }

        const previousSubscription = this.#state.snapshot?.subscription_id;
        this.#preSnapshotRevisions.clear();
        this.#publish({ status: "ready", snapshot, error: null });
        if (previousSubscription && previousSubscription !== snapshot.subscription_id) {
          await this.#client.close(previousSubscription).catch(() => undefined);
        }
        return;
      }
    } catch (error) {
      if (!this.#disposed && generation === this.#loadGeneration) {
        this.#preSnapshotRevisions.clear();
        this.#fail(error);
      }
    }
  }

  #applyRevision(event: FileResourceEventV1) {
    if (this.#disposed || this.#state.status === "error") return;
    const snapshot = this.#state.snapshot;
    if (this.#state.status === "loading") {
      if (
        !snapshot
        || (event.resource_id === snapshot.resource_id && event.revision > snapshot.revision)
      ) {
        this.#rememberPreSnapshotRevision(event);
      }
      return;
    }
    if (!snapshot) {
      this.#rememberPreSnapshotRevision(event);
      return;
    }
    if (
      event.resource_id !== snapshot.resource_id
      || event.revision <= snapshot.revision
    ) {
      return;
    }
    this.#rememberPreSnapshotRevision(event);
    void this.#load();
  }

  #rememberPreSnapshotRevision(event: FileResourceEventV1) {
    const previous = this.#preSnapshotRevisions.get(event.resource_id);
    if (!previous || event.revision > previous.revision) {
      this.#preSnapshotRevisions.set(event.resource_id, event);
      if (this.#preSnapshotRevisions.size > 64) {
        const oldest = this.#preSnapshotRevisions.keys().next().value;
        if (oldest !== undefined) this.#preSnapshotRevisions.delete(oldest);
      }
    }
  }

  #fail(error: unknown) {
    this.#publish({ ...this.#state, status: "error", error: errorFrom(error) });
  }

  #publish(state: FileResourceState) {
    this.#state = state;
    for (const listener of this.#listeners) listener();
  }

  async #dispose() {
    if (this.#disposed) return;
    this.#disposed = true;
    this.#loadGeneration += 1;
    this.#unlisten?.();
    this.#unlisten = null;
    const controllers = controllersByClient.get(this.#client);
    if (controllers?.get(this.#identity) === this) {
      controllers.delete(this.#identity);
      if (controllers.size === 0) controllersByClient.delete(this.#client);
    }
    const subscription_id = this.#state.snapshot?.subscription_id;
    if (subscription_id) {
      await this.#client.close(subscription_id).catch(() => undefined);
    }
  }
}

function controllerFor(
  request: OpenFileResourceRequestV1,
  client: FileResourceClient,
) {
  let controllers = controllersByClient.get(client);
  if (!controllers) {
    controllers = new Map();
    controllersByClient.set(client, controllers);
  }
  const identity = requestIdentity(request);
  let controller = controllers.get(identity);
  if (!controller) {
    controller = new FileResourceController(client, request, identity);
    controllers.set(identity, controller);
  }
  return controller;
}

/**
 * Shares a backend subscription and revision listener for identical consumers,
 * containing transport failures in hook state instead of the React boundary.
 */
export function useFileResource(
  request: OpenFileResourceRequestV1,
  client: FileResourceClient = fileResourceClient,
): UseFileResourceResult {
  const identity = requestIdentity(request);
  const controller = useMemo(
    () => controllerFor(request, client),
    // The normalized identity includes every request field used by the backend.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [client, identity],
  );
  const state = useSyncExternalStore(
    controller.subscribe,
    controller.getSnapshot,
    controller.getSnapshot,
  );

  useEffect(() => controller.acquire(), [controller]);

  return useMemo(() => ({ ...state, retry: controller.retry }), [controller, state]);
}
