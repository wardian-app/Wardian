import { useLayoutEffect, useRef, useState } from "react";
import { File, Plus, Send, X } from "lucide-react";
import type { RemoteAgentSummary } from "../../types";
import { remoteClient, RemoteRequestError } from "./remoteClient";
import { REMOTE_ATTACHMENT_ACCEPT, useRemoteAttachments } from "./useRemoteAttachments";

const control = "inline-flex min-h-10 min-w-10 shrink-0 items-center justify-center rounded-md border border-wardian-border px-2 text-primary disabled:opacity-50";
const composerControl = "inline-flex h-11 w-11 shrink-0 items-center justify-center rounded-full focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[var(--color-wardian-accent)]";
const expiredMessage = "Submission expired and cannot be delivered. Your draft is preserved; you can send it as a new message.";

interface FrozenSubmission {
  submission_id: string;
  prompt: string;
  attachment_ids: string[];
}

export function RemoteAttachmentComposer({ agent, disabledReason, sendText, onAccepted }: {
  agent: RemoteAgentSummary;
  disabledReason: string | null;
  sendText: (text: string, mode: "message" | "command") => Promise<void>;
  onAccepted: () => void;
}) {
  const attachments = useRemoteAttachments(agent.session_id);
  const [prompt, setPrompt] = useState("");
  const [menu, setMenu] = useState(false);
  const [busy, setBusy] = useState(false);
  const [uncertain, setUncertain] = useState<FrozenSubmission | null>(null);
  const [retryMissing, setRetryMissing] = useState(false);
  const [error, setError] = useState("");
  const locked = useRef(false);
  const files = useRef<HTMLInputElement>(null);
  const camera = useRef<HTMLInputElement>(null);
  const textarea = useRef<HTMLTextAreaElement>(null);
  const frozen = busy || Boolean(uncertain);
  useLayoutEffect(() => {
    const input = textarea.current;
    if (!input) return;
    // Reset before measuring so accepted sends and shorter drafts shrink again.
    input.style.height = "auto";
    input.style.height = `${input.scrollHeight}px`;
  }, [prompt]);
  const accepted = () => { attachments.clear(); setPrompt(""); setUncertain(null); setRetryMissing(false); onAccepted(); };
  const handleExpiry = (failure: unknown, sending = false) => {
    if (!(failure instanceof RemoteRequestError)) return false;
    const fileExpired = failure.status === 410 && failure.code === "attachment_expired"
      || sending && failure.status === 404 && failure.code === "attachment_not_found";
    const keyExpired = failure.status === 410 && failure.code === "submission_expired";
    if (!fileExpired && !keyExpired) return false;
    setUncertain(null); setRetryMissing(false);
    if (fileExpired) { attachments.expire(); setError(""); }
    else setError(expiredMessage);
    return true;
  };
  const submit = async (retry?: FrozenSubmission) => {
    if (locked.current || disabledReason || (uncertain && (!retryMissing || retry !== uncertain))
      || attachments.chips.some((chip) => chip.state !== "ready")) return;
    if (!prompt.trim() && !attachments.chips.length) return;
    locked.current = true;
    setBusy(true);
    setError("");
    setRetryMissing(false);
    // A status lookup can race a late original request. Explicit retries must
    // retain this exact key and payload so server admission stays idempotent.
    const attempt = retry ?? (attachments.chips.length ? {
      submission_id: `${Date.now()}.${crypto.randomUUID()}`,
      prompt: prompt.trim() || "Please inspect the attached files.",
      attachment_ids: attachments.chips.map((chip) => chip.id),
    } : null);
    try {
      if (attempt) {
        const receipt = await remoteClient.sendPrompt(agent.session_id, attempt.prompt, "message", undefined,
          { attachment_ids: attempt.attachment_ids, submission_id: attempt.submission_id });
        if (receipt.state === "failed") { setUncertain(null); setError("Delivery failed before submission. You can retry Send."); return; }
        if (receipt.state !== "accepted") { setUncertain(attempt); setError("Delivery is unconfirmed. Check status before continuing."); return; }
      } else await sendText(prompt.trim(), prompt.trim().startsWith("/") ? "command" : "message");
      accepted();
    } catch (failure) {
      if (attempt && handleExpiry(failure, true)) return;
      // Transport loss and server failures cannot establish absence of provider effects.
      const definite = failure instanceof RemoteRequestError && failure.status >= 400 && failure.status < 500
        && (!retry || failure.status === 410)
        && ![404, 408, 409, 410].includes(failure.status) && !failure.code?.includes("unconfirmed");
      setUncertain(attempt && !definite ? attempt : null);
      setError(failure instanceof RemoteRequestError && [401, 403].includes(failure.status)
          ? "Reconnect before continuing." : attempt && !definite ? "Delivery is unconfirmed. Check status before continuing." : "Send failed. Your draft is preserved.");
    } finally { setBusy(false); locked.current = false; }
  };
  const recover = async () => {
    if (!uncertain || locked.current) return;
    locked.current = true; setBusy(true); setRetryMissing(false);
    try {
      const result = await remoteClient.getAttachmentSubmission(agent.session_id, uncertain.submission_id);
      if (result.state === "accepted") { accepted(); setError(""); }
      else if (result.state === "failed") { setUncertain(null); setError("Delivery failed before submission. You can retry Send."); }
      else setError("Delivery is still unconfirmed. Do not resend; check status again.");
    } catch (failure) {
      if (handleExpiry(failure)) return;
      if (failure instanceof RemoteRequestError && failure.status === 404) {
        setRetryMissing(true);
        setError("Submission was not found. Retry send will use the same submission and unchanged message.");
      } else setError("Could not confirm delivery. Reconnect and check status again.");
    }
    finally { locked.current = false; setBusy(false); }
  };
  return <form className="chat-composer shrink-0 border-t border-wardian-border bg-wardian-bg/95 p-3 backdrop-blur"
    onSubmit={(event) => { event.preventDefault(); void submit(); }}>
    {(error || attachments.error) && <p role="alert" className="mb-2 text-xs text-wardian-error">{error || attachments.error}</p>}
    {uncertain && <button type="button" className={`${control} mb-2`} disabled={busy} onClick={() => void recover()}>Check delivery status</button>}
    {uncertain && retryMissing && <button type="button" className={`${control} mb-2 ml-2`} disabled={busy || Boolean(disabledReason)} onClick={() => void submit(uncertain)}>Retry send</button>}
    <div data-testid="remote-composer-field" className="rounded-[28px] border border-wardian-border bg-wardian-card p-1 focus-within:border-[var(--color-wardian-accent)]">
      {attachments.chips.length > 0 && <div aria-label="Temporary attachments" className="flex gap-2 overflow-x-auto px-2 pb-2 pt-2">
        {attachments.chips.map((chip) => <div key={chip.id} className="flex shrink-0 items-center gap-1 rounded-md border border-wardian-border bg-wardian-card px-1 text-xs">
          {chip.preview ? <img src={chip.preview} alt="" className="h-8 w-8 rounded object-cover" /> : <File aria-hidden="true" className="h-5 w-5" />}
          <div className="max-w-36"><p className="truncate" title={chip.file.name}>{chip.file.name}</p>
            <span role="status">{chip.error ?? (chip.state === "ready" ? "Temporary · ready" : chip.state === "validating" ? "Validating…" : `${chip.progress}%`)}</span>
            {chip.state === "uploading" && <progress aria-label={`Uploading ${chip.file.name}`} value={chip.progress} max={100} className="block w-full" />}
          </div>
          {chip.state === "failed" && !chip.authRequired && chip.retryAt !== Infinity && <button type="button" className={control} disabled={frozen}
            aria-label={`Retry ${chip.file.name}`} onClick={() => attachments.retry(chip)}>Retry</button>}
          <button type="button" className={control} aria-label={`Remove ${chip.file.name}`} disabled={frozen} onClick={() => attachments.remove(chip.id)}><X className="h-4 w-4" /></button>
        </div>)}
      </div>}
      <div className="flex items-end gap-1">
        <div className="relative">
          <button type="button" aria-label="Attach file" aria-expanded={menu} disabled={frozen || Boolean(disabledReason)} className={`${composerControl} text-muted hover:bg-wardian-card-bg-muted disabled:opacity-50`} onClick={() => setMenu(!menu)}><Plus aria-hidden="true" className="h-5 w-5" /></button>
          {menu && <div aria-label="Attachment sources" className="absolute bottom-full left-0 mb-1 rounded-md border border-wardian-border bg-wardian-card p-1" onKeyDown={(event) => { if (event.key === "Escape") setMenu(false); }}>
            <button type="button" className={control} disabled={frozen || Boolean(disabledReason)} onClick={() => { setMenu(false); files.current?.click(); }}>Files</button>
            <button type="button" className={control} disabled={frozen || Boolean(disabledReason)} onClick={() => { setMenu(false); camera.current?.click(); }}>Camera</button>
          </div>}
        </div>
        <input ref={files} hidden type="file" disabled={frozen || Boolean(disabledReason)} multiple accept={REMOTE_ATTACHMENT_ACCEPT} aria-label="Choose files" onChange={(event) => { attachments.add(Array.from(event.target.files ?? [])); event.target.value = ""; }} />
        <input ref={camera} hidden type="file" disabled={frozen || Boolean(disabledReason)} accept="image/png,image/jpeg,image/webp,image/gif,image/bmp" capture="environment" aria-label="Take photo" onChange={(event) => { attachments.add(Array.from(event.target.files ?? [])); event.target.value = ""; }} />
        <textarea ref={textarea} aria-label={`Prompt ${agent.session_name}`} value={prompt} onChange={(event) => setPrompt(event.target.value)} disabled={frozen || Boolean(disabledReason)} rows={1}
          className="max-h-40 min-h-11 min-w-0 flex-1 resize-none overflow-y-auto bg-transparent px-1 py-2.5 text-base leading-6 text-primary outline-none disabled:opacity-70" placeholder={disabledReason ?? "Prompt agent"} />
        <button type="submit" aria-label="Send prompt" disabled={frozen || Boolean(disabledReason) || (!prompt.trim() && !attachments.chips.length) || attachments.chips.some((chip) => chip.state !== "ready")}
          className={`${composerControl} bg-[var(--color-wardian-accent)] text-[var(--color-wardian-accent-contrast)] disabled:bg-wardian-card-bg-muted disabled:text-[var(--color-wardian-text-muted)]`}><Send aria-hidden="true" className="h-5 w-5" /></button>
      </div>
    </div>
  </form>;
}
