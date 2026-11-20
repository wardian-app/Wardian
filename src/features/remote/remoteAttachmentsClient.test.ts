import { afterEach, describe, expect, it, vi } from "vitest";
import { remoteClient } from "./remoteClient";

class UploadRequest {
  static latest: UploadRequest;
  constructor() { UploadRequest.latest = this; }
  upload = { onprogress: null as ((event: { lengthComputable: boolean; loaded: number; total: number }) => void) | null, onload: null as (() => void) | null };
  open = vi.fn();
  setRequestHeader = vi.fn();
  send = vi.fn();
  getResponseHeader = vi.fn(() => "5");
  abort = vi.fn(() => this.onabort?.());
  withCredentials = false;
  timeout = 0;
  status = 200;
  responseText = "";
  onload: (() => void) | null = null;
  onerror: (() => void) | null = null;
  ontimeout: (() => void) | null = null;
  onabort: (() => void) | null = null;
}
describe("remote raw attachment transport", () => {
  afterEach(() => { vi.unstubAllGlobals(); remoteClient.setCsrfNonce(null); });
  it("sends raw bytes, encoded filename, csrf and credentials with transfer progress", async () => {
    vi.stubGlobal("XMLHttpRequest", UploadRequest);
    remoteClient.setCsrfNonce("nonce");
    const file = new File(["body"], "a #%.txt");
    const progress = vi.fn();
    const promise = remoteClient.uploadAttachment("agent/1", "id", file, "text/plain", new AbortController().signal, progress);
    const xhr = UploadRequest.latest;
    expect(xhr.open).toHaveBeenCalledWith("POST", "/remote/api/agents/agent%2F1/attachments/id");
    expect(xhr.withCredentials).toBe(true);
    expect(xhr.setRequestHeader).toHaveBeenCalledWith("x-wardian-filename", "a%20%23%25.txt");
    expect(xhr.setRequestHeader).toHaveBeenCalledWith("x-wardian-csrf", "nonce");
    expect(xhr.setRequestHeader).toHaveBeenCalledWith("Content-Type", "text/plain");
    expect(xhr.send).toHaveBeenCalledWith(file);
    xhr.upload.onprogress?.({ lengthComputable: true, loaded: 2, total: 4 });
    xhr.upload.onload?.();
    expect(progress.mock.calls).toEqual([[50], [100]]);
    xhr.responseText = JSON.stringify({ upload_id: "id", name: file.name, size_bytes: 4, media_type: "text/plain", expires_at: "later" });
    xhr.onload?.();
    await expect(promise).resolves.toMatchObject({ upload_id: "id" });
  });
  it("preserves auth status on an empty error body and respects Retry-After", async () => {
    vi.stubGlobal("XMLHttpRequest", UploadRequest);
    const promise = remoteClient.uploadAttachment("agent", "id", new File([], "x.txt"), "text/plain", new AbortController().signal, vi.fn());
    UploadRequest.latest.status = 401;
    UploadRequest.latest.onload?.();
    await expect(promise).rejects.toMatchObject({ status: 401, retryAfterMs: 5000 });
  });
  it("aborts the request through its cancellation signal", async () => {
    vi.stubGlobal("XMLHttpRequest", UploadRequest);
    const controller = new AbortController();
    const promise = remoteClient.uploadAttachment("agent", "id", new File([], "x.txt"), "text/plain", controller.signal, vi.fn());
    controller.abort();
    await expect(promise).rejects.toMatchObject({ name: "AbortError" });
    expect(UploadRequest.latest.abort).toHaveBeenCalledOnce();
  });
  it("uses authenticated no-store status lookups, csrf deletion and attachment IDs in sends", async () => {
    const fetcher = vi.fn().mockImplementation(async () => new Response("{}"));
    vi.stubGlobal("fetch", fetcher);
    remoteClient.setCsrfNonce("nonce");
    await remoteClient.getAttachment("agent", "upload");
    await remoteClient.getAttachmentSubmission("agent", "submission");
    await remoteClient.deleteAttachment("agent", "upload");
    await remoteClient.sendPrompt("agent", "inspect", "message", undefined, { attachment_ids: ["upload"], submission_id: "submission" });
    expect(fetcher.mock.calls[0][1]).toMatchObject({ credentials: "same-origin", cache: "no-store" });
    expect(fetcher.mock.calls[1][0]).toBe("/remote/api/agents/agent/attachment-submissions/submission");
    expect(fetcher.mock.calls[2][1]).toMatchObject({ method: "DELETE", headers: { "x-wardian-csrf": "nonce" } });
    expect(JSON.parse(fetcher.mock.calls[3][1].body)).toMatchObject({ attachment_ids: ["upload"], submission_id: "submission" });
  });
});
