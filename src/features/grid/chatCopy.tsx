import { Check, Copy, MoreHorizontal } from "lucide-react";
import { useEffect, useId, useRef, useState } from "react";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";

type CopyState = "idle" | "copied" | "error";

export interface ChatCopyAction {
  label: string;
  value: string;
}

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

function useCopyValue(value: string) {
  const [state, setState] = useState<CopyState>("idle");
  const resetTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const scheduleReset = (delay: number) => {
    if (resetTimer.current !== null) clearTimeout(resetTimer.current);
    resetTimer.current = setTimeout(() => {
      resetTimer.current = null;
      setState("idle");
    }, delay);
  };

  useEffect(
    () => () => {
      if (resetTimer.current !== null) clearTimeout(resetTimer.current);
    },
    [],
  );

  const copy = async () => {
    if (!value) return;
    try {
      await writeClipboardText(value);
      setState("copied");
      scheduleReset(1400);
    } catch {
      setState("error");
      scheduleReset(2200);
    }
  };

  return { copy, state };
}

function copyStateLabel(label: string, state: CopyState) {
  return state === "copied" ? `${label} copied` : state === "error" ? `${label} failed` : label;
}

export function CopyIconButton({ label, value }: { label: string; value: string }) {
  const { copy, state } = useCopyValue(value);

  return (
    <button
      type="button"
      aria-label={copyStateLabel(label, state)}
      title={state === "copied" ? "Copied" : state === "error" ? "Copy failed" : label}
      className={`chat-copy-icon inline-flex h-6 w-6 items-center justify-center rounded text-muted-neutral transition-colors ${
        state === "copied"
          ? "bg-[color-mix(in_srgb,var(--color-wardian-success),transparent_86%)] text-[var(--color-wardian-success)]"
          : state === "error"
            ? "bg-[color-mix(in_srgb,var(--color-wardian-error),transparent_88%)] text-[var(--color-wardian-error)]"
            : "hover:bg-[var(--color-wardian-card-bg-muted)] hover:text-primary"
      }`}
      onClick={() => void copy()}
    >
      {state === "copied" ? <Check className="h-3.5 w-3.5" aria-hidden="true" /> : <Copy className="h-3.5 w-3.5" aria-hidden="true" />}
    </button>
  );
}

function CopyMenuItem({ action }: { action: ChatCopyAction }) {
  const { copy, state } = useCopyValue(action.value);

  return (
    <button
      type="button"
      role="menuitem"
      className="chat-copy-menu-item flex w-full items-center gap-2 rounded px-2.5 py-2 text-left text-[11px] font-medium text-primary hover:bg-[var(--color-wardian-card-bg-muted)]"
      aria-label={copyStateLabel(action.label, state)}
      onClick={() => void copy()}
    >
      {state === "copied" ? <Check className="h-3.5 w-3.5 text-[var(--color-wardian-success)]" aria-hidden="true" /> : <Copy className="h-3.5 w-3.5 text-muted-neutral" aria-hidden="true" />}
      <span>{state === "copied" ? "Copied" : state === "error" ? "Copy failed" : action.label}</span>
    </button>
  );
}

export function ChatRowActions({
  actions,
  className,
  label = "Message actions",
}: {
  actions: ChatCopyAction[];
  className?: string;
  label?: string;
}) {
  const [menuOpen, setMenuOpen] = useState(false);
  const menuId = useId();
  const containerRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    if (!menuOpen) return;

    const handlePointerDown = (event: MouseEvent) => {
      if (!containerRef.current?.contains(event.target as Node)) setMenuOpen(false);
    };
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      setMenuOpen(false);
      if (containerRef.current?.contains(document.activeElement)) triggerRef.current?.focus();
    };

    document.addEventListener("mousedown", handlePointerDown);
    document.addEventListener("keydown", handleKeyDown);
    return () => {
      document.removeEventListener("mousedown", handlePointerDown);
      document.removeEventListener("keydown", handleKeyDown);
    };
  }, [menuOpen]);

  if (actions.length === 0) return null;

  return (
    <div
      ref={containerRef}
      className={`chat-row-actions ${className ?? ""}`}
      data-has-secondary={actions.length > 1 ? "true" : "false"}
    >
      <div className="chat-row-actions-direct">
        <CopyIconButton label={actions[0].label} value={actions[0].value} />
      </div>
      <button
        type="button"
        ref={triggerRef}
        className="chat-row-actions-menu-trigger inline-flex items-center justify-center rounded text-muted-neutral hover:bg-[var(--color-wardian-card-bg-muted)] hover:text-primary"
        aria-label={label}
        aria-controls={menuId}
        aria-expanded={menuOpen}
        aria-haspopup="menu"
        title={label}
        onClick={() => setMenuOpen((value) => !value)}
      >
        <MoreHorizontal className="h-4 w-4" aria-hidden="true" />
      </button>
      {menuOpen ? (
        <div id={menuId} className="chat-row-actions-menu wardian-menu p-1" role="menu" aria-label={label}>
          {actions.map((action) => (
            <CopyMenuItem action={action} key={action.label} />
          ))}
        </div>
      ) : null}
    </div>
  );
}
