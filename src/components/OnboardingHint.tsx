import { useEffect, useState, type PropsWithChildren, type ReactNode } from "react";
import { X } from "lucide-react";

const storagePrefix = "wardian:onboarding-hint:";

interface OnboardingHintProps extends PropsWithChildren {
  id: string;
  title: string;
  actions?: ReactNode;
}

function storageKey(id: string) {
  return `${storagePrefix}${id}`;
}

function isStoredDismissed(id: string) {
  return localStorage.getItem(storageKey(id)) === "dismissed";
}

export function OnboardingHint({ id, title, actions, children }: OnboardingHintProps) {
  const [isDismissed, setIsDismissed] = useState(() => isStoredDismissed(id));

  useEffect(() => {
    setIsDismissed(isStoredDismissed(id));
  }, [id]);

  if (isDismissed) {
    return null;
  }

  const dismiss = () => {
    localStorage.setItem(storageKey(id), "dismissed");
    setIsDismissed(true);
  };

  return (
    <section className="rounded-lg border border-wardian-border bg-wardian-card-bg-muted px-3 py-3 text-xs text-muted">
      <div className="flex items-start gap-3">
        <div className="min-w-0 flex-1">
          <h3 className="text-xs font-bold text-primary">{title}</h3>
          <div className="mt-1 leading-5 text-muted">{children}</div>
          {actions && <div className="mt-2 flex flex-wrap items-center gap-3">{actions}</div>}
        </div>
        <button
          type="button"
          aria-label={`Dismiss ${title}`}
          title="Dismiss"
          onClick={dismiss}
          className="shrink-0 rounded-md p-1 text-muted-neutral hover:bg-wardian-card-bg hover:text-bright-neutral transition-colors"
        >
          <X className="h-3.5 w-3.5" aria-hidden="true" />
        </button>
      </div>
    </section>
  );
}
