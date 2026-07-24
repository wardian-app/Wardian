import { useState } from "react";
import { ArrowLeft, ArrowRight, X } from "lucide-react";
import { DocsLink } from "./DocsLink";

interface OnboardingTourProps {
  onClose: () => void;
}

const STEPS = [
  {
    title: "Start with a reliable agent",
    detail: "Verify a provider, choose a workspace, and spawn one agent before scaling out.",
    docsPath: "/guide/getting-started",
    docsLabel: "First-run guide",
  },
  {
    title: "Keep the roster readable",
    detail: "Use agents, teams, and watchlists to monitor work without opening every terminal.",
    docsPath: "/guide/watchlists",
    docsLabel: "Watchlist guide",
  },
  {
    title: "Coordinate deliberately",
    detail: "Select recipients before sending a command, then use the Graph to inspect and adjust communication boundaries.",
    docsPath: "/guide/graph",
    docsLabel: "Graph guide",
  },
  {
    title: "Turn repeatable work into workflows",
    detail: "Author a blueprint, validate it, then run and observe it from the workflow surface.",
    docsPath: "/guide/workflows",
    docsLabel: "Workflow guide",
  },
] as const;

export function OnboardingTour({ onClose }: OnboardingTourProps) {
  const [stepIndex, setStepIndex] = useState(0);
  const step = STEPS[stepIndex];
  const isFirst = stepIndex === 0;
  const isLast = stepIndex === STEPS.length - 1;

  return (
    <section
      data-testid="onboarding-tour"
      aria-labelledby="onboarding-tour-title"
      className="mt-4 rounded-lg border border-[var(--color-wardian-accent)]/45 bg-[color-mix(in_srgb,var(--color-wardian-accent),transparent_94%)] p-4"
    >
      <div className="flex items-start justify-between gap-4">
        <div>
          <p className="text-[10px] font-bold uppercase tracking-wide text-[var(--color-wardian-accent)]">
            Guided tour · {stepIndex + 1} of {STEPS.length}
          </p>
          <h4 id="onboarding-tour-title" className="mt-1 text-sm font-semibold text-primary">
            {step.title}
          </h4>
        </div>
        <button
          type="button"
          aria-label="Close guided tour"
          onClick={onClose}
          className="rounded-md p-1 text-muted-neutral transition-colors hover:bg-wardian-card-bg hover:text-primary"
        >
          <X className="h-4 w-4" aria-hidden="true" />
        </button>
      </div>
      <p className="mt-2 max-w-2xl text-xs leading-5 text-muted">{step.detail}</p>
      <div className="mt-3 flex flex-wrap items-center justify-between gap-3">
        <DocsLink path={step.docsPath}>{step.docsLabel}</DocsLink>
        <div className="flex items-center gap-2">
          <button
            type="button"
            onClick={() => setStepIndex((index) => Math.max(0, index - 1))}
            disabled={isFirst}
            className="inline-flex items-center gap-1 rounded-md border border-wardian-border px-2 py-1 text-[11px] font-semibold text-muted-neutral transition-colors hover:text-primary disabled:cursor-not-allowed disabled:opacity-40"
          >
            <ArrowLeft className="h-3.5 w-3.5" aria-hidden="true" />
            Back
          </button>
          <button
            type="button"
            onClick={() => isLast ? onClose() : setStepIndex((index) => index + 1)}
            className="inline-flex items-center gap-1 rounded-md border border-[var(--color-wardian-accent)] px-2 py-1 text-[11px] font-semibold text-[var(--color-wardian-accent)] transition-colors hover:bg-[color-mix(in_srgb,var(--color-wardian-accent),transparent_88%)]"
          >
            {isLast ? "Done" : "Next"}
            {!isLast && <ArrowRight className="h-3.5 w-3.5" aria-hidden="true" />}
          </button>
        </div>
      </div>
    </section>
  );
}
