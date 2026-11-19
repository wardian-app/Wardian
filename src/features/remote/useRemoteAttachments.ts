import { useEffect, useRef, useState } from "react";
import { remoteClient, RemoteRequestError } from "./remoteClient";

const mediaTypes: Record<string, string> = {
  png: "image/png", jpg: "image/jpeg", jpeg: "image/jpeg", webp: "image/webp",
  gif: "image/gif", bmp: "image/bmp", pdf: "application/pdf", txt: "text/plain",
  md: "text/markdown", csv: "text/csv", json: "application/json",
};
export const REMOTE_ATTACHMENT_ACCEPT = [
  ...Object.keys(mediaTypes).map((extension) => `.${extension}`),
  ...new Set(Object.values(mediaTypes)),
].join(",");

export interface AttachmentChip {
  id: string;
  file: File;
  preview?: string;
  state: "selected" | "uploading" | "validating" | "ready" | "failed";
  progress: number;
  error?: string;
  retryAt?: number;
  authRequired?: boolean;
  recover?: boolean;
}

/** Transient File ownership stays with the composer; the gateway owns durable uploads. */
export function useRemoteAttachments(sessionId: string) {
  const [chips, setChips] = useState<AttachmentChip[]>([]);
  const [error, setError] = useState("");
  const items = useRef<AttachmentChip[]>([]);
  const requests = useRef(new Map<string, AbortController>());
  const update = (next: AttachmentChip[]) => { items.current = next; setChips(next); };
  const patch = (id: string, changes: Partial<AttachmentChip>) =>
    update(items.current.map((item) => item.id === id ? { ...item, ...changes } : item));

  const upload = async (chip: AttachmentChip, recover = false) => {
    if (requests.current.has(chip.id)) return;
    const controller = new AbortController();
    requests.current.set(chip.id, controller);
    patch(chip.id, { state: "uploading", error: undefined, authRequired: false, progress: 0 });
    try {
      if (recover) {
        try {
          await remoteClient.getAttachment(sessionId, chip.id);
          if (!controller.signal.aborted) patch(chip.id, { state: "ready", progress: 100 });
          return;
        } catch (failure) {
          if (!(failure instanceof RemoteRequestError) || failure.status !== 404) throw failure;
        }
      }
      if (controller.signal.aborted) return;
      const extension = chip.file.name.split(".").pop()?.toLowerCase() ?? "";
      await remoteClient.uploadAttachment(sessionId, chip.id, chip.file, mediaTypes[extension],
        controller.signal, (progress) => {
          if (!controller.signal.aborted) patch(chip.id, { progress, state: progress === 100 ? "validating" : "uploading" });
        });
      if (!controller.signal.aborted) patch(chip.id, { state: "ready", progress: 100 });
    } catch (failure) {
      if (controller.signal.aborted) return;
      const authRequired = failure instanceof RemoteRequestError && [401, 403].includes(failure.status);
      patch(chip.id, { state: "failed", authRequired,
        retryAt: Date.now() + (failure instanceof RemoteRequestError ? failure.retryAfterMs ?? 0 : 0),
        error: authRequired ? "Reconnect before retrying." : failure instanceof RemoteRequestError
          ? `Upload failed (${failure.code ?? failure.status}).${failure.retryAfterMs ? ` Retry after ${Math.ceil(failure.retryAfterMs / 1000)} seconds.` : ""}` : "Upload interrupted. Check and retry.",
      });
    } finally { requests.current.delete(chip.id); }
  };

  const remove = (id: string, accepted = false) => {
    requests.current.get(id)?.abort();
    const chip = items.current.find((item) => item.id === id);
    if (chip?.preview) URL.revokeObjectURL(chip.preview);
    update(items.current.filter((item) => item.id !== id));
    if (!accepted) void remoteClient.deleteAttachment(sessionId, id).catch(() => { /* TTL covers lost cleanup. */ });
  };
  useEffect(() => {
    const available = Math.max(0, 2 - requests.current.size);
    for (const chip of items.current.filter((item) => item.state === "selected").slice(0, available)) void upload(chip, chip.recover);
  });
  useEffect(() => () => {
    for (const request of requests.current.values()) request.abort();
    for (const chip of items.current) if (chip.preview) URL.revokeObjectURL(chip.preview);
    items.current = [];
  }, []);

  return {
    chips, error,
    add(files: File[]) {
      setError("");
      for (const file of files) {
        const extension = file.name.split(".").pop()?.toLowerCase() ?? "";
        if (!mediaTypes[extension]) { setError("Unsupported format. Choose PNG, JPEG, WebP, GIF, BMP, PDF or UTF-8 text files."); continue; }
        if (file.size > 10 * 1024 * 1024) { setError("Each file must be 10 MiB or smaller."); continue; }
        if (items.current.length >= 4 || items.current.reduce((sum, item) => sum + item.file.size, file.size) > 25 * 1024 * 1024) {
          setError("Attach up to four files, totaling 25 MiB or less."); continue;
        }
        const chip: AttachmentChip = { id: crypto.randomUUID(), file, state: "selected", progress: 0,
          preview: mediaTypes[extension].startsWith("image/") && file.type === mediaTypes[extension]
            ? URL.createObjectURL(file) : undefined };
        update([...items.current, chip]);
      }
    },
    remove,
    expire() {
      for (const request of requests.current.values()) request.abort();
      update(items.current.map((chip) => ({ ...chip, state: "failed", retryAt: Infinity,
        error: "Attachment expired. Remove and attach it again." })));
    },
    clear() { for (const chip of [...items.current]) remove(chip.id, true); },
    retry(chip: AttachmentChip) {
      if (chip.authRequired || Date.now() < (chip.retryAt ?? 0)) return;
      patch(chip.id, { state: "selected", recover: true });
    },
  };
}
