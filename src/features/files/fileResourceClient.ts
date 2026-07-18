import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  CheckpointFileRecoveryRequestV1,
  DiscardFileRecoveryRequestV1,
  FileRecoveryCheckpointV1,
  FileRecoveryMergeResultV1,
  FileRecoverySummaryV1,
  FileRecoveryV1,
  FileResourceEventV1,
  FileResourceSaveAsResultV1,
  FileResourceSaveResultV1,
  FileResourceSnapshotV1,
  FileResourceTextV1,
  FileResourceTicketV1,
  GetFileRecoveryRequestV1,
  ListFileRecoveriesRequestV1,
  MergeFileRecoveryRequestV1,
  OpenFileResourceRequestV1,
  PickFileResourceSaveTargetRequestV1,
  SaveFileResourceAsTextRequestV1,
  SaveFileResourceTextRequestV1,
  SaveTargetGrantV1,
} from "../../types";
import { fileResourceUrlForWebview } from "./resourceTicketUrl.mjs";

export const FILE_RESOURCE_REVISION_EVENT = "file-resource://revision";

/** Typed Tauri adapter for one set of locally shared file subscriptions. */
export class FileResourceClient {
  readonly #closeBySubscription = new Map<string, Promise<void>>();

  async open(request: OpenFileResourceRequestV1): Promise<FileResourceSnapshotV1> {
    const snapshot = await invoke<FileResourceSnapshotV1>("open_file_resource", { request });
    return snapshot;
  }

  readText(
    owner: Pick<FileResourceSnapshotV1, "resource_id" | "subscription_id" | "revision">,
  ): Promise<FileResourceTextV1> {
    return invoke<FileResourceTextV1>("read_file_resource_text", {
      request: {
        resource_id: owner.resource_id,
        subscription_id: owner.subscription_id,
        revision: owner.revision,
      },
    });
  }

  saveText(request: SaveFileResourceTextRequestV1): Promise<FileResourceSaveResultV1> {
    return invoke<FileResourceSaveResultV1>("save_file_resource_text", { request });
  }

  pickSaveTarget(
    request: PickFileResourceSaveTargetRequestV1,
  ): Promise<SaveTargetGrantV1 | null> {
    return invoke<SaveTargetGrantV1 | null>("pick_file_resource_save_target", { request });
  }

  saveAsText(
    request: SaveFileResourceAsTextRequestV1,
  ): Promise<FileResourceSaveAsResultV1> {
    return invoke<FileResourceSaveAsResultV1>("save_file_resource_as_text", { request });
  }

  checkpointRecovery(
    request: CheckpointFileRecoveryRequestV1,
  ): Promise<FileRecoveryCheckpointV1> {
    return invoke<FileRecoveryCheckpointV1>("checkpoint_file_recovery", { request });
  }

  getRecovery(request: GetFileRecoveryRequestV1): Promise<FileRecoveryV1> {
    return invoke<FileRecoveryV1>("get_file_recovery", { request });
  }

  listRecoveries(
    request: ListFileRecoveriesRequestV1,
  ): Promise<FileRecoverySummaryV1[]> {
    return invoke<FileRecoverySummaryV1[]>("list_file_recoveries", { request });
  }

  discardRecovery(request: DiscardFileRecoveryRequestV1): Promise<void> {
    return invoke<void>("discard_file_recovery", { request });
  }

  mergeRecovery(
    request: MergeFileRecoveryRequestV1,
  ): Promise<FileRecoveryMergeResultV1> {
    return invoke<FileRecoveryMergeResultV1>("merge_file_recovery", { request });
  }

  async issueTicket(
    owner: Pick<FileResourceSnapshotV1, "resource_id" | "subscription_id" | "revision">,
    purpose: string,
  ): Promise<FileResourceTicketV1> {
    const ticket = await invoke<FileResourceTicketV1>("issue_file_resource_ticket", {
      request: {
        resource_id: owner.resource_id,
        subscription_id: owner.subscription_id,
        revision: owner.revision,
        renderer_lease_id: purpose,
      },
    });
    return {
      ...ticket,
      url: fileResourceUrlForWebview(ticket.url, convertFileSrc),
    };
  }

  async closeRendererLease(
    owner: Pick<FileResourceSnapshotV1, "resource_id" | "subscription_id">,
    renderer_lease_id: string,
  ): Promise<void> {
    await invoke<void>("close_file_renderer_lease", {
      request: {
        resource_id: owner.resource_id,
        subscription_id: owner.subscription_id,
        renderer_lease_id,
      },
    });
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
    }).finally(() => {
      if (this.#closeBySubscription.get(subscription_id) === closing) {
        this.#closeBySubscription.delete(subscription_id);
      }
    });
    this.#closeBySubscription.set(subscription_id, closing);
    return closing;
  }
}

export const fileResourceClient = new FileResourceClient();
