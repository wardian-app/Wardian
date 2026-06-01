import { memo, useState } from 'react';
import type { Blueprint } from './blueprintTypes';
import { useUpstreamContext } from './useUpstreamContext';

interface VariableAssistantProps {
  blueprint: Blueprint | null;
  selectedNodeId: string | null;
}

export const VariableAssistant = memo(({ blueprint, selectedNodeId }: VariableAssistantProps) => {
  const [copied, setCopied] = useState<string | null>(null);
  const variables = useUpstreamContext(blueprint, selectedNodeId);

  const copyVariable = (token: string) => {
    navigator.clipboard?.writeText(token);
    setCopied(token);
    window.setTimeout(() => setCopied(null), 1500);
  };

  return (
    <div className="grid gap-1.5">
      <div className="flex items-center justify-between gap-2">
        <div className="text-[10px] font-bold uppercase tracking-wide text-muted">Insert variable</div>
        <div className="text-[9px] font-mono text-muted">{variables.length}</div>
      </div>
      <div className="grid gap-1">
        {variables.map((token) => (
          <button
            key={token}
            type="button"
            onClick={() => copyVariable(token)}
            className="flex h-8 items-center justify-between gap-2 rounded-md border border-wardian-border bg-[var(--color-wardian-bg)] px-2 text-left text-[10px] font-mono text-[var(--color-wardian-text)] transition-colors hover:border-[var(--color-wardian-accent)]"
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
