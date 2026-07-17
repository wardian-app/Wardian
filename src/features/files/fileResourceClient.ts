import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  FileResourceEventV1,
  FileResourceSnapshotV1,
  FileResourceTextV1,
  FileResourceTicketV1,
  OpenFileResourceRequestV1,
} from "../../types";

export const FILE_RESOURCE_REVISION_EVENT = "file-resource://revision";

/** Typed Tauri adapter for one set of locally shared file subscriptions. */
export class FileResourceClient {
  readonly #subscriptionsByResource = new Map<string, Map<string, number>>();
  readonly #resourceBySubscription = new Map<string, string>();
  readonly #rendererLeaseOwners = new Map<string, Map<string, string>>();
  readonly #closeBySubscription = new Map<string, Promise<void>>();
  #openSequence = 0;

  async open(request: OpenFileResourceRequestV1): Promise<FileResourceSnapshotV1> {
    const open_sequence = ++this.#openSequence;
    const snapshot = await invoke<FileResourceSnapshotV1>("open_file_resource", { request });
    let subscriptions = this.#subscriptionsByResource.get(snapshot.resource_id);
    if (!subscriptions) {
      subscriptions = new Map();
      this.#subscriptionsByResource.set(snapshot.resource_id, subscriptions);
    }
    subscriptions.set(snapshot.subscription_id, open_sequence);
    this.#resourceBySubscription.set(snapshot.subscription_id, snapshot.resource_id);
    return snapshot;
  }

  readText(resource_id: string, revision: number): Promise<FileResourceTextV1> {
    return invoke<FileResourceTextV1>("read_file_resource_text", {
      request: {
        resource_id,
        subscription_id: this.#subscription(resource_id),
        revision,
      },
    });
  }

  async issueTicket(
    resource_id: string,
    revision: number,
    purpose: string,
  ): Promise<FileResourceTicketV1> {
    const subscription_id = this.#subscription(resource_id);
    const ticket = await invoke<FileResourceTicketV1>("issue_file_resource_ticket", {
      request: {
        resource_id,
        subscription_id,
        revision,
        renderer_lease_id: purpose,
      },
    });
    let owners = this.#rendererLeaseOwners.get(resource_id);
    if (!owners) {
      owners = new Map();
      this.#rendererLeaseOwners.set(resource_id, owners);
    }
    owners.set(purpose, subscription_id);
    return ticket;
  }

  async closeRendererLease(resource_id: string, renderer_lease_id: string): Promise<void> {
    const owners = this.#rendererLeaseOwners.get(resource_id);
    const subscription_id = owners?.get(renderer_lease_id) ?? this.#subscription(resource_id);
    await invoke<void>("close_file_renderer_lease", {
      request: {
        resource_id,
        subscription_id,
        renderer_lease_id,
      },
    });
    if (owners?.get(renderer_lease_id) === subscription_id) {
      owners.delete(renderer_lease_id);
      if (owners.size === 0) {
        this.#rendererLeaseOwners.delete(resource_id);
      }
    }
  }

  listenForRevisions(
    callback: (event: FileResourceEventV1) => void,
  ): Promise<UnlistenFn> {
    return listen<FileResourceEventV1>(FILE_RESOURCE_REVISION_EVENT, (event) => {
      callback(event.payload);
    });
  }

  close(subscription_id: string): Promise<void> {
    const existing = this.#closeBySubscription.get(subscription_id);
    if (existing) return existing;

    let closing: Promise<void>;
    closing = invoke<void>("close_file_resource", {
      request: { subscription_id },
    }).then(() => {
      const resource_id = this.#resourceBySubscription.get(subscription_id);
      if (resource_id) {
        const subscriptions = this.#subscriptionsByResource.get(resource_id);
        subscriptions?.delete(subscription_id);
        if (subscriptions?.size === 0) {
          this.#subscriptionsByResource.delete(resource_id);
        }
      }
      this.#resourceBySubscription.delete(subscription_id);
      for (const [lease_resource_id, owners] of this.#rendererLeaseOwners) {
        for (const [renderer_lease_id, owner] of owners) {
          if (owner === subscription_id) {
            owners.delete(renderer_lease_id);
          }
        }
        if (owners.size === 0) {
          this.#rendererLeaseOwners.delete(lease_resource_id);
        }
      }
    }).catch((error) => {
      if (this.#closeBySubscription.get(subscription_id) === closing) {
        this.#closeBySubscription.delete(subscription_id);
      }
      throw error;
    });
    this.#closeBySubscription.set(subscription_id, closing);
    return closing;
  }

  #subscription(resource_id: string) {
    const subscriptions = this.#subscriptionsByResource.get(resource_id);
    if (!subscriptions?.size) {
      throw new Error(`File resource is not open: ${resource_id}`);
    }
    let selected_id: string | undefined;
    let selected_sequence = -1;
    for (const [subscription_id, open_sequence] of subscriptions) {
      if (open_sequence > selected_sequence) {
        selected_id = subscription_id;
        selected_sequence = open_sequence;
      }
    }
    if (!selected_id) {
      throw new Error(`File resource is not open: ${resource_id}`);
    }
    return selected_id;
  }
}

export const fileResourceClient = new FileResourceClient();
