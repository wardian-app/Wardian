import type { ProviderQuestion } from "../../types";

interface ProviderQuestionDetailsProps {
  question: ProviderQuestion;
}

export function ProviderQuestionDetails({ question }: ProviderQuestionDetailsProps) {
  return (
    <div
      className="mt-3 space-y-3 rounded-md border border-wardian-border bg-wardian-card-bg-muted p-3"
      data-testid="provider-question-details"
      data-provider={question.provider}
    >
      {question.questions.map((entry, index) => (
        <section key={`${entry.id ?? index}-${entry.prompt}`} className="space-y-1.5">
          {entry.header && <p className="text-[11px] font-semibold uppercase tracking-wide text-muted-neutral">{entry.header}</p>}
          <p className="text-[13px] leading-5 text-primary">{entry.prompt}</p>
          {entry.options.length > 0 && (
            <ul aria-label={`Options for question ${index + 1}`} className="list-disc space-y-1 pl-5 text-[12px] leading-5 text-muted">
              {entry.options.map((option) => (
                <li key={`${option.label}-${option.description ?? ""}`}>
                  <span className="font-medium text-primary">{option.label}</span>
                  {option.description && <span> — {option.description}</span>}
                </li>
              ))}
            </ul>
          )}
        </section>
      ))}
    </div>
  );
}
