import React from "react";

interface GitDiffViewProps {
  diff: string;
  filePath: string;
  onClose: () => void;
}

export const GitDiffView: React.FC<GitDiffViewProps> = ({ diff, filePath, onClose }) => {
  const lines = diff.split("\n");

  return (
    <div
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
          <button
            onClick={onClose}
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
                    {line}
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
