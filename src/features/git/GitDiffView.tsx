import React from "react";

export interface GitDiffAction {
  label: string;
  onClick: () => void;
}

export interface GitDiffHunkAction {
  label: string;
  onClick: (patch: string) => void;
}

interface GitDiffViewProps {
  diff: string;
  filePath: string;
  onClose: () => void;
  actions?: GitDiffAction[];
  hunkActions?: GitDiffHunkAction[];
}

const buildHunkPatches = (lines: string[]) => {
  const hunkPatches = new Map<number, string>();
  const firstHunkIndex = lines.findIndex((line) => line.startsWith("@@"));
  if (firstHunkIndex === -1) {
    return hunkPatches;
  }

  const fileHeader = lines.slice(0, firstHunkIndex);
  for (let i = firstHunkIndex; i < lines.length; i += 1) {
    if (!lines[i].startsWith("@@")) {
      continue;
    }

    let nextHunkIndex = lines.length;
    for (let j = i + 1; j < lines.length; j += 1) {
      if (lines[j].startsWith("@@")) {
        nextHunkIndex = j;
        break;
      }
    }

    hunkPatches.set(i, `${[...fileHeader, ...lines.slice(i, nextHunkIndex)].join("\n")}\n`);
  }

  return hunkPatches;
};

export const GitDiffView: React.FC<GitDiffViewProps> = ({
  diff,
  filePath,
  onClose,
  actions = [],
  hunkActions = [],
}) => {
  const lines = diff.split("\n");
  const hunkPatches = hunkActions.length > 0 ? buildHunkPatches(lines) : new Map<number, string>();

  return (
    <div
      data-testid="git-diff-backdrop"
      className="fixed inset-0 z-[100] bg-black/60 flex items-center justify-center p-8 backdrop-blur-sm animate-in fade-in"
      onClick={onClose}
    >
      <div
        className="bg-wardian-card border border-wardian-border shadow-2xl rounded-2xl w-full max-w-4xl max-h-[90vh] flex flex-col font-mono text-sm overflow-hidden animate-in zoom-in-95 duration-200"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex justify-between items-center p-4 border-b border-wardian-border shrink-0 bg-wardian-bg/50">
          <h3 className="font-bold text-lg text-[var(--color-wardian-accent)] truncate flex-1 mr-4">
            {filePath}
          </h3>
          {actions.length > 0 && (
            <div className="flex shrink-0 items-center gap-1.5 mr-2">
              {actions.map((action) => (
                <button
                  key={action.label}
                  type="button"
                  onClick={action.onClick}
                  className="rounded border border-wardian-border px-2 py-1 text-[11px] font-medium text-primary hover:bg-wardian-card-bg-muted transition-colors"
                >
                  {action.label}
                </button>
              ))}
            </div>
          )}
          <button
            onClick={onClose}
            aria-label="Close diff"
            title="Close diff"
            className="text-[var(--color-wardian-text-muted)] hover:text-wardian-error font-bold transition-colors w-8 h-8 flex items-center justify-center rounded-md hover:bg-wardian-error/10"
          >
            ✕
          </button>
        </div>
        <div className="p-0 overflow-y-auto flex-1 bg-wardian-bg/30 cursor-text">
          {diff.trim() ? (
            <pre className="text-xs leading-relaxed">
              {lines.map((line, i) => {
                let lineClass = "px-4 py-0";
                let lineStyle: React.CSSProperties = {};
                if (line.startsWith("+") && !line.startsWith("+++")) {
                  lineStyle = {
                    backgroundColor: "color-mix(in srgb, var(--color-wardian-success), transparent 90%)",
                    color: "var(--color-wardian-success)",
                  };
                } else if (line.startsWith("-") && !line.startsWith("---")) {
                  lineStyle = {
                    backgroundColor: "color-mix(in srgb, var(--color-wardian-error), transparent 90%)",
                    color: "var(--color-wardian-error)",
                  };
                } else if (line.startsWith("@@")) {
                  lineStyle = {
                    backgroundColor: "color-mix(in srgb, var(--color-wardian-processing), transparent 90%)",
                    color: "var(--color-wardian-processing)",
                  };
                } else {
                  lineClass += " text-primary";
                }
                return (
                  <div key={i} className={lineClass} style={lineStyle}>
                    {line.startsWith("@@") && hunkActions.length > 0 ? (
                      <span className="flex min-h-[1.375rem] items-center justify-between gap-3">
                        <span>{line}</span>
                        <span className="flex shrink-0 items-center gap-1.5 font-sans">
                          {hunkActions.map((action) => (
                            <button
                              key={action.label}
                              type="button"
                              onClick={() => {
                                const patch = hunkPatches.get(i);
                                if (patch) {
                                  action.onClick(patch);
                                }
                              }}
                              className="rounded border border-wardian-border px-2 py-0.5 text-[11px] font-medium text-primary hover:bg-wardian-card-bg-muted transition-colors"
                            >
                              {action.label}
                            </button>
                          ))}
                        </span>
                      </span>
                    ) : (
                      line
                    )}
                  </div>
                );
              })}
            </pre>
          ) : (
            <div className="p-6 text-[var(--color-wardian-text-muted)] italic">
              No differences to display.
            </div>
          )}
        </div>
      </div>
    </div>
  );
};
