import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { Check, Copy } from "lucide-react";
import { useState } from "react";

type CopyState = "idle" | "copied" | "error";

async function writeClipboardText(value: string) {
  try {
    await writeText(value);
    return;
  } catch (nativeError) {
    const browserWriteText = typeof navigator === "undefined" ? undefined : navigator.clipboard?.writeText;
    if (!browserWriteText) throw nativeError;
    await browserWriteText.call(navigator.clipboard, value);
  }
}

export function CopyIconButton({ label, value }: { label: string; value: string }) {
  const [state, setState] = useState<CopyState>("idle");
  const copy = async () => {
    if (!value) return;
    try {
      await writeClipboardText(value);
      setState("copied");
      window.setTimeout(() => setState("idle"), 1400);
    } catch {
      setState("error");
      window.setTimeout(() => setState("idle"), 2200);
    }
  };

  return (
    <button
      type="button"
      aria-label={state === "copied" ? `${label} copied` : state === "error" ? `${label} failed` : label}
      title={state === "copied" ? "Copied" : state === "error" ? "Copy failed" : label}
      className={`inline-flex h-6 w-6 items-center justify-center rounded border text-muted-neutral transition-colors ${
        state === "copied"
          ? "border-[color-mix(in_srgb,var(--color-wardian-success),transparent_40%)] bg-[color-mix(in_srgb,var(--color-wardian-success),transparent_86%)] text-[var(--color-wardian-success)]"
          : state === "error"
            ? "border-[color-mix(in_srgb,var(--color-wardian-error),transparent_40%)] bg-[color-mix(in_srgb,var(--color-wardian-error),transparent_88%)] text-[var(--color-wardian-error)]"
            : "border-wardian-light bg-[var(--color-wardian-card-bg-muted)] hover:text-primary"
      }`}
      onClick={copy}
    >
      {state === "copied" ? <Check className="h-3.5 w-3.5" aria-hidden="true" /> : <Copy className="h-3.5 w-3.5" aria-hidden="true" />}
    </button>
  );
}
