import { memo, useState } from 'react';
import type { Blueprint } from './blueprintTypes';
import { useUpstreamContextV2 } from './useUpstreamContextV2';

interface VariableAssistantV2Props {
  blueprint: Blueprint | null;
  selectedNodeId: string | null;
}

export const VariableAssistantV2 = memo(({ blueprint, selectedNodeId }: VariableAssistantV2Props) => {
  const [copied, setCopied] = useState<string | null>(null);
  const variables = useUpstreamContextV2(blueprint, selectedNodeId);

  const copyVariable = (token: string) => {
    navigator.clipboard?.writeText(token);
    setCopied(token);
    window.setTimeout(() => setCopied(null), 1500);
  };

  return (
    <div className="flex flex-col rounded-lg border border-wardian-border bg-[var(--color-wardian-card)]">
      <div className="border-b border-wardian-border px-3 py-2">
        <div className="text-[10px] font-bold tracking-wide text-muted">Variable Assistant</div>
      </div>
      <div className="flex flex-col gap-1 p-3">
        {variables.map((token) => (
          <button
            key={token}
            type="button"
            onClick={() => copyVariable(token)}
            className="flex items-center justify-between gap-3 rounded-md border border-wardian-border bg-[var(--color-wardian-bg)] px-2 py-1.5 text-left text-[10px] font-mono text-[var(--color-wardian-text)] transition-colors hover:border-[var(--color-wardian-accent)]"
            title={token}
          >
            <span className="truncate">{token}</span>
            <span className="shrink-0 text-[9px] font-bold text-[var(--color-wardian-accent)]">
              {copied === token ? 'Copied' : 'Copy'}
            </span>
          </button>
        ))}
      </div>
    </div>
  );
});
