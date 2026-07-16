import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type ReactNode,
} from "react";

import type {
  DirtySurfaceChoice,
  DirtySurfacePrompt,
  DirtySurfacePromptRequest,
} from "./dirtySurfaceGuards";

type PendingPrompt = {
  request: DirtySurfacePromptRequest;
  resolve: (choice: DirtySurfaceChoice) => void;
};

export type DirtySurfacePromptController = {
  prompt: DirtySurfacePrompt;
  dialog: ReactNode;
};

/**
 * Serializes dirty-resource prompts so repeated close gestures cannot strand a
 * navigation transaction or replace an unresolved dialog.
 */
export function useDirtySurfacePrompt(): DirtySurfacePromptController {
  const activeRef = useRef<PendingPrompt | null>(null);
  const queueRef = useRef<PendingPrompt[]>([]);
  const [active, setActive] = useState<PendingPrompt | null>(null);

  const prompt = useCallback<DirtySurfacePrompt>((request) => new Promise((resolve) => {
    const pending = { request, resolve };
    if (activeRef.current) {
      queueRef.current.push(pending);
      return;
    }
    activeRef.current = pending;
    setActive(pending);
  }), []);

  const settle = useCallback((choice: DirtySurfaceChoice) => {
    const current = activeRef.current;
    if (!current) return;

    current.resolve(choice);
    const next = queueRef.current.shift() ?? null;
    activeRef.current = next;
    setActive(next);
  }, []);

  useEffect(() => () => {
    activeRef.current?.resolve("cancel");
    activeRef.current = null;
    for (const pending of queueRef.current.splice(0)) pending.resolve("cancel");
  }, []);

  return {
    prompt,
    dialog: active ? (
      <div
        className="fixed inset-0 z-[11000] flex items-center justify-center"
        style={{ backgroundColor: "var(--color-wardian-overlay)" }}
        onClick={() => settle("cancel")}
        onKeyDown={(event) => {
          if (event.key === "Escape") settle("cancel");
        }}
      >
        <section
          aria-describedby="dirty-surface-prompt-message"
          aria-labelledby="dirty-surface-prompt-title"
          aria-modal="true"
          className="mx-4 w-full max-w-sm rounded-lg p-6 shadow-2xl"
          role="dialog"
          style={{
            background: "var(--color-wardian-sidebar-primary)",
            border: "1px solid var(--color-wardian-border-heavy)",
          }}
          onClick={(event) => event.stopPropagation()}
        >
          <h2 id="dirty-surface-prompt-title" className="mb-2 text-sm font-bold text-primary">
            Unsaved {active.request.title} changes
          </h2>
          <p id="dirty-surface-prompt-message" className="mb-6 text-sm leading-relaxed text-muted">
            {active.request.message}
          </p>
          <div className="flex justify-end gap-2">
            <button
              type="button"
              autoFocus
              className="rounded px-4 py-2 text-xs font-bold text-muted transition-colors hover:text-primary"
              style={{
                background: "var(--color-wardian-card-bg-muted)",
                border: "1px solid var(--color-wardian-border)",
              }}
              onClick={() => settle("cancel")}
            >
              Cancel
            </button>
            <button
              type="button"
              className="rounded px-4 py-2 text-xs font-bold text-primary transition-colors"
              style={{
                background: "var(--color-wardian-card-bg-muted)",
                border: "1px solid var(--color-wardian-border)",
              }}
              onClick={() => settle("discard")}
            >
              Discard
            </button>
            <button
              type="button"
              className="rounded px-4 py-2 text-xs font-bold transition-colors"
              style={{
                background: "var(--color-wardian-accent)",
                color: "var(--color-wardian-bg)",
              }}
              onClick={() => settle("save")}
            >
              Save
            </button>
          </div>
        </section>
      </div>
    ) : null,
  };
}
