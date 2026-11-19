import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { RemoteAgentSummary } from "../../types";
import { RemoteAttachmentComposer } from "./RemoteAttachmentComposer";
import { remoteClient, RemoteRequestError } from "./remoteClient";

const agent = { session_id: "agent-1", session_name: "Coder" } as RemoteAgentSummary;
const metadata = { upload_id: "upload", name: "note.txt", size_bytes: 4, media_type: "text/plain", expires_at: "later" };
const sendText = vi.fn().mockResolvedValue(undefined);
const onAccepted = vi.fn();
function setup(disabledReason: string | null = null) {
  return render(<RemoteAttachmentComposer agent={agent} disabledReason={disabledReason} sendText={sendText} onAccepted={onAccepted} />);
}
function choose(file = new File(["note"], "note.txt", { type: "text/plain" })) {
  fireEvent.change(screen.getByLabelText("Choose files"), { target: { files: [file] } });
}
describe("remote attachment composer", () => {
  beforeEach(() => {
    vi.spyOn(remoteClient, "uploadAttachment").mockResolvedValue(metadata);
    vi.spyOn(remoteClient, "deleteAttachment").mockResolvedValue(undefined);
    vi.spyOn(remoteClient, "getAttachment").mockRejectedValue(new RemoteRequestError("missing", 404));
    vi.spyOn(remoteClient, "sendPrompt").mockResolvedValue({ ok: true, state: "accepted" });
    vi.stubGlobal("URL", Object.assign(URL, { createObjectURL: vi.fn(() => "blob:preview"), revokeObjectURL: vi.fn() }));
  });
  afterEach(() => { vi.restoreAllMocks(); vi.clearAllMocks(); vi.unstubAllGlobals(); });
  it("offers compact Files and Camera pickers with a rear camera hint", () => {
    setup();
    fireEvent.click(screen.getByRole("button", { name: "Attach file" }));
    expect(screen.getByRole("button", { name: "Files" })).toBeVisible();
    expect(screen.getByRole("button", { name: "Camera" })).toBeVisible();
    expect(screen.getByLabelText("Take photo")).toHaveAttribute("capture", "environment");
    expect(screen.getByLabelText("Choose files")).toHaveAttribute("accept", expect.stringContaining(".pdf"));
    const accepted = screen.getByLabelText("Choose files").getAttribute("accept")?.split(",");
    expect(accepted).toEqual(expect.arrayContaining(["image/png", "image/jpeg", "image/webp", "image/gif", "image/bmp",
      "application/pdf", "text/plain", "text/markdown", "text/csv", "application/json"]));
    expect(screen.getByLabelText("Take photo").getAttribute("accept")).not.toContain("image/*");
  });
  it.each([
    ["note.md", "text/plain", "text/markdown"],
    ["data.csv", "application/vnd.ms-excel", "text/csv"],
  ])("uses the canonical extension MIME for %s despite browser metadata", async (name, browserType, canonicalType) => {
    setup();
    choose(new File(["UTF-8 content"], name, { type: browserType }));
    await screen.findByText("Temporary · ready");
    expect(remoteClient.uploadAttachment).toHaveBeenCalledWith("agent-1", expect.any(String), expect.any(File),
      canonicalType, expect.any(AbortSignal), expect.any(Function));
  });
  it("shows progress, waits for validation and sends attachment-only defaults with UUIDs", async () => {
    let finish!: (value: typeof metadata) => void;
    vi.mocked(remoteClient.uploadAttachment).mockImplementation((_session, _id, _file, _type, _signal, progress) => {
      progress(45);
      return new Promise((resolve) => { finish = resolve; });
    });
    setup(); choose();
    expect(await screen.findByRole("progressbar")).toHaveAttribute("value", "45");
    expect(screen.getByRole("button", { name: "Send prompt" })).toBeDisabled();
    const progress = vi.mocked(remoteClient.uploadAttachment).mock.calls[0][5];
    act(() => progress(100));
    expect(screen.getByText("Validating…")).toBeVisible();
    await act(async () => finish(metadata));
    fireEvent.click(screen.getByRole("button", { name: "Send prompt" }));
    await waitFor(() => expect(onAccepted).toHaveBeenCalledOnce());
    expect(remoteClient.sendPrompt).toHaveBeenCalledWith("agent-1", "Please inspect the attached files.", "message", undefined,
      { attachment_ids: [expect.stringMatching(/^[a-f0-9-]{36}$/)], submission_id: expect.stringMatching(/^\d+\.[a-f0-9-]{36}$/) });
    expect(screen.queryByText("note.txt")).not.toBeInTheDocument();
  });
  it("aborts removal, deletes the upload and revokes image previews", async () => {
    vi.mocked(remoteClient.uploadAttachment).mockImplementation(() => new Promise(() => {}));
    setup(); choose(new File(["png"], "photo.png", { type: "image/png" }));
    await waitFor(() => expect(remoteClient.uploadAttachment).toHaveBeenCalled());
    const signal = vi.mocked(remoteClient.uploadAttachment).mock.calls[0][4];
    expect(screen.getByRole("presentation")).toHaveAttribute("src", "blob:preview");
    fireEvent.click(screen.getByRole("button", { name: "Remove photo.png" }));
    expect(signal.aborted).toBe(true);
    expect(remoteClient.deleteAttachment).toHaveBeenCalledOnce();
    expect(URL.revokeObjectURL).toHaveBeenCalledWith("blob:preview");
  });
  it("retains failed uploads and retries explicitly after checking server status", async () => {
    vi.mocked(remoteClient.uploadAttachment).mockRejectedValueOnce(new Error("lost"));
    setup(); choose();
    fireEvent.click(await screen.findByRole("button", { name: "Retry note.txt" }));
    await screen.findByText("Temporary · ready");
    expect(remoteClient.getAttachment).toHaveBeenCalledOnce();
    expect(remoteClient.uploadAttachment).toHaveBeenCalledTimes(2);
    expect(vi.mocked(remoteClient.uploadAttachment).mock.calls[0][1]).toBe(vi.mocked(remoteClient.uploadAttachment).mock.calls[1][1]);
  });
  it("requires reconnect for auth failures and blocks sending", async () => {
    vi.mocked(remoteClient.uploadAttachment).mockRejectedValue(new RemoteRequestError("expired", 401));
    setup(); choose();
    await screen.findByText("Reconnect before retrying.");
    expect(screen.queryByRole("button", { name: "Retry note.txt" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Send prompt" })).toBeDisabled();
  });
  it("preserves an uncertain send and recovers its receipt without replay", async () => {
    vi.mocked(remoteClient.sendPrompt).mockRejectedValue(new TypeError("lost response"));
    const lookup = vi.spyOn(remoteClient, "getAttachmentSubmission").mockResolvedValue({ submission_id: "same", state: "accepted" });
    setup(); choose();
    await screen.findByText("Temporary · ready");
    fireEvent.change(screen.getByRole("textbox"), { target: { value: "inspect" } });
    fireEvent.click(screen.getByRole("button", { name: "Send prompt" }));
    fireEvent.click(await screen.findByRole("button", { name: "Check delivery status" }));
    await waitFor(() => expect(onAccepted).toHaveBeenCalledOnce());
    expect(lookup).toHaveBeenCalledWith("agent-1", vi.mocked(remoteClient.sendPrompt).mock.calls[0][4]?.submission_id);
    expect(remoteClient.sendPrompt).toHaveBeenCalledOnce();
  });
  it("keeps a definite failed draft and files for an explicit send retry", async () => {
    vi.mocked(remoteClient.sendPrompt).mockRejectedValue(new RemoteRequestError("quota", 413));
    setup(); choose(); await screen.findByText("Temporary · ready");
    fireEvent.change(screen.getByRole("textbox"), { target: { value: "/inspect" } });
    fireEvent.click(screen.getByRole("button", { name: "Send prompt" }));
    await screen.findByText("Send failed. Your draft is preserved.");
    expect(screen.getByRole("textbox")).toHaveValue("/inspect");
    expect(screen.getByText("note.txt")).toBeVisible();
    expect(vi.mocked(remoteClient.sendPrompt).mock.calls[0][2]).toBe("message");
  });
  it("rejects unsupported formats and disables unavailable targets", () => {
    setup("Agent is off"); choose(new File(["heic"], "photo.heic"));
    expect(screen.getByRole("alert")).toHaveTextContent("Unsupported format");
    expect(remoteClient.uploadAttachment).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: "Send prompt" })).toBeDisabled();
  });
  it("queues at most two uploads and limits the retained batch to four files", async () => {
    const finishes: Array<(value: typeof metadata) => void> = [];
    vi.mocked(remoteClient.uploadAttachment).mockImplementation(() => new Promise((resolve) => finishes.push(resolve)));
    setup();
    fireEvent.change(screen.getByLabelText("Choose files"), { target: { files:
      Array.from({ length: 5 }, (_, index) => new File(["note"], `${index}.txt`)) } });
    await waitFor(() => expect(remoteClient.uploadAttachment).toHaveBeenCalledTimes(2));
    expect(screen.getByRole("alert")).toHaveTextContent("up to four files");
    await act(async () => finishes[0](metadata));
    await waitFor(() => expect(remoteClient.uploadAttachment).toHaveBeenCalledTimes(3));
    expect(screen.queryByText("4.txt")).not.toBeInTheDocument();
  });
  it("keeps uncertain state until a proven failed status allows a new submission ID", async () => {
    vi.mocked(remoteClient.sendPrompt).mockResolvedValueOnce({ ok: true, state: "submitted_unconfirmed" });
    const lookup = vi.spyOn(remoteClient, "getAttachmentSubmission")
      .mockResolvedValueOnce({ submission_id: "same", state: "dispatching" })
      .mockResolvedValueOnce({ submission_id: "same", state: "failed" });
    setup(); choose(); await screen.findByText("Temporary · ready");
    fireEvent.click(screen.getByRole("button", { name: "Send prompt" }));
    const check = await screen.findByRole("button", { name: "Check delivery status" });
    fireEvent.click(check);
    await screen.findByText("Delivery is still unconfirmed. Do not resend; check status again.");
    expect(screen.getByRole("button", { name: "Send prompt" })).toBeDisabled();
    fireEvent.click(check);
    await screen.findByText("Delivery failed before submission. You can retry Send.");
    fireEvent.click(screen.getByRole("button", { name: "Send prompt" }));
    await waitFor(() => expect(onAccepted).toHaveBeenCalledOnce());
    const calls = vi.mocked(remoteClient.sendPrompt).mock.calls;
    expect(calls[0][4]?.submission_id).not.toBe(calls[1][4]?.submission_id);
    expect(lookup).toHaveBeenCalledTimes(2);
  });
  it("honors Retry-After before an explicit retry", async () => {
    vi.mocked(remoteClient.uploadAttachment).mockRejectedValueOnce(new RemoteRequestError("busy", 429, "rate_limit", undefined, 5000));
    const clock = vi.spyOn(Date, "now").mockReturnValue(1000);
    setup(); choose();
    const retry = await screen.findByRole("button", { name: "Retry note.txt" });
    fireEvent.click(retry);
    expect(remoteClient.getAttachment).not.toHaveBeenCalled();
    clock.mockReturnValue(6001);
    fireEvent.click(retry);
    await screen.findByText("Temporary · ready");
    expect(remoteClient.getAttachment).toHaveBeenCalledOnce();
  });
  it("offers an explicit same-key retry after an unknown submission without changing its frozen payload", async () => {
    vi.mocked(remoteClient.sendPrompt).mockRejectedValueOnce(new TypeError("lost request"));
    vi.spyOn(remoteClient, "getAttachmentSubmission").mockRejectedValue(new RemoteRequestError("missing", 404));
    setup(); choose(); choose(new File(["second"], "second.md", { type: "text/plain" }));
    await waitFor(() => expect(screen.getAllByText("Temporary · ready")).toHaveLength(2));
    fireEvent.change(screen.getByRole("textbox"), { target: { value: "  inspect in order  " } });
    fireEvent.click(screen.getByRole("button", { name: "Send prompt" }));
    fireEvent.click(await screen.findByRole("button", { name: "Check delivery status" }));
    const retry = await screen.findByRole("button", { name: "Retry send" });
    expect(remoteClient.sendPrompt).toHaveBeenCalledOnce();
    expect(screen.getByRole("textbox")).toBeDisabled();
    expect(screen.getByRole("button", { name: "Remove note.txt" })).toBeDisabled();
    fireEvent.click(retry);
    await waitFor(() => expect(onAccepted).toHaveBeenCalledOnce());
    const calls = vi.mocked(remoteClient.sendPrompt).mock.calls;
    expect(calls[1]).toEqual(calls[0]);
    expect(calls[1][1]).toBe("inspect in order");
  });
  it.each(["status", "send"])("releases a permanently expired key from %s while preserving the draft", async (source) => {
    if (source === "status") {
      vi.mocked(remoteClient.sendPrompt).mockRejectedValueOnce(new TypeError("lost response"));
      vi.spyOn(remoteClient, "getAttachmentSubmission").mockRejectedValue(new RemoteRequestError("expired", 410, "submission_expired"));
    } else vi.mocked(remoteClient.sendPrompt).mockRejectedValueOnce(new RemoteRequestError("expired", 410, "submission_expired"));
    setup(); choose(); await screen.findByText("Temporary · ready");
    fireEvent.change(screen.getByRole("textbox"), { target: { value: "preserved intent" } });
    fireEvent.click(screen.getByRole("button", { name: "Send prompt" }));
    if (source === "status") fireEvent.click(await screen.findByRole("button", { name: "Check delivery status" }));
    await screen.findByText("Submission expired and cannot be delivered. Your draft is preserved; you can send it as a new message.");
    expect(screen.getByRole("textbox")).toHaveValue("preserved intent");
    expect(screen.getByRole("textbox")).toBeEnabled();
    expect(screen.getByText("note.txt")).toBeVisible();
    expect(remoteClient.sendPrompt).toHaveBeenCalledOnce();
    fireEvent.click(screen.getByRole("button", { name: "Send prompt" }));
    await waitFor(() => expect(onAccepted).toHaveBeenCalledOnce());
    const calls = vi.mocked(remoteClient.sendPrompt).mock.calls;
    expect(calls[1][4]?.submission_id).not.toBe(calls[0][4]?.submission_id);
  });
  it("keeps the original intent frozen when a same-key retry is rejected by authentication", async () => {
    vi.mocked(remoteClient.sendPrompt).mockRejectedValueOnce(new TypeError("lost original"))
      .mockRejectedValueOnce(new RemoteRequestError("expired session", 401));
    vi.spyOn(remoteClient, "getAttachmentSubmission").mockRejectedValue(new RemoteRequestError("missing", 404));
    setup(); choose(); await screen.findByText("Temporary · ready");
    fireEvent.click(screen.getByRole("button", { name: "Send prompt" }));
    fireEvent.click(await screen.findByRole("button", { name: "Check delivery status" }));
    fireEvent.click(await screen.findByRole("button", { name: "Retry send" }));
    await screen.findByText("Reconnect before continuing.");
    expect(screen.getByRole("button", { name: "Send prompt" })).toBeDisabled();
    expect(screen.getByRole("textbox")).toBeDisabled();
    expect(screen.getByRole("button", { name: "Check delivery status" })).toBeEnabled();
    expect(vi.mocked(remoteClient.sendPrompt).mock.calls[1]).toEqual(vi.mocked(remoteClient.sendPrompt).mock.calls[0]);
  });
  it.each([
    [410, "attachment_expired"],
    [404, "attachment_not_found"],
  ])("requires removal and reattachment after send returns %s %s", async (status, code) => {
    vi.mocked(remoteClient.sendPrompt).mockRejectedValueOnce(new RemoteRequestError("expired file", status, code));
    setup(); choose(); await screen.findByText("Temporary · ready");
    fireEvent.change(screen.getByRole("textbox"), { target: { value: "Keep this draft" } });
    fireEvent.click(screen.getByRole("button", { name: "Send prompt" }));
    await screen.findByText("Attachment expired. Remove and attach it again.");
    expect(screen.getByRole("textbox")).toHaveValue("Keep this draft");
    expect(screen.getByText("note.txt")).toBeVisible();
    expect(screen.getByRole("button", { name: "Send prompt" })).toBeDisabled();
    expect(screen.queryByRole("button", { name: "Retry note.txt" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Retry send" })).not.toBeInTheDocument();
    expect(remoteClient.uploadAttachment).toHaveBeenCalledOnce();
    expect(remoteClient.sendPrompt).toHaveBeenCalledOnce();
    const originalId = vi.mocked(remoteClient.uploadAttachment).mock.calls[0][1];
    fireEvent.click(screen.getByRole("button", { name: "Remove note.txt" }));
    expect(screen.queryByText("note.txt")).not.toBeInTheDocument();
    choose(); await screen.findByText("Temporary · ready");
    expect(vi.mocked(remoteClient.uploadAttachment).mock.calls[1][1]).not.toBe(originalId);
    expect(screen.getByRole("button", { name: "Send prompt" })).toBeEnabled();
    fireEvent.click(screen.getByRole("button", { name: "Send prompt" }));
    await waitFor(() => expect(onAccepted).toHaveBeenCalledOnce());
    expect(vi.mocked(remoteClient.sendPrompt).mock.calls[1][4]?.attachment_ids).not.toContain(originalId);
  });
});
