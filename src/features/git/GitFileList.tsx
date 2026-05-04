import React from "react";
import { GitFileEntry } from "../../types";

interface GitFileListProps {
  files: GitFileEntry[];
  onStage?: (path: string) => void;
  onUnstage?: (path: string) => void;
  onDiscard?: (path: string) => void;
  onDiff?: (path: string, staged: boolean) => void;
}

const STATUS_COLORS: Record<string, string> = {
  M: "text-[var(--color-wardian-warning)]",
  A: "text-[var(--color-wardian-success)]",
  D: "text-[var(--color-wardian-error)]",
  R: "text-[var(--color-wardian-processing)]",
  C: "text-[var(--color-wardian-processing)]",
  U: "text-[var(--color-wardian-warning)]",
  "?": "text-[var(--color-wardian-text-muted)]",
};

export const GitFileList: React.FC<GitFileListProps> = ({
  files,
  onStage,
  onUnstage,
  onDiscard,
  onDiff,
}) => {
  if (files.length === 0) return null;

  return (
    <ul className="flex flex-col gap-0.5">
      {files.map((file, i) => {
        const colorClass = STATUS_COLORS[file.status] || "text-primary";
        const filename = file.path.split("/").pop() || file.path;
        const dir = file.path.includes("/")
          ? file.path.substring(0, file.path.lastIndexOf("/"))
          : "";

        return (
          <li
            key={`${file.path}-${file.is_staged}-${i}`}
            className="flex items-center gap-1.5 px-1.5 py-1 rounded hover:bg-wardian-card-bg-muted group text-xs"
          >
            <button
              className="flex-1 min-w-0 text-left truncate text-primary hover:underline cursor-pointer"
              aria-label={`View diff for ${file.path}`}
              title={file.path}
              onClick={() => onDiff?.(file.path, file.is_staged)}
            >
              <span>{filename}</span>
              {dir && (
                <span className="ml-1.5 text-[var(--color-wardian-text-muted)]">{dir}</span>
              )}
            </button>
            <div className="flex items-center gap-0.5 opacity-0 group-hover:opacity-100 transition-opacity shrink-0">
              {file.is_staged && onUnstage && (
                <button
                  className="p-0.5 rounded hover:bg-wardian-card text-[var(--color-wardian-text-muted)] hover:text-primary transition-colors"
                  aria-label={`Unstage ${file.path}`}
                  title="Unstage"
                  onClick={() => onUnstage(file.path)}
                >
                  <svg className="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                    <path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M20 12H4" />
                  </svg>
                </button>
              )}
              {!file.is_staged && onStage && (
                <button
                  className="p-0.5 rounded hover:bg-wardian-card text-[var(--color-wardian-text-muted)] hover:text-primary transition-colors"
                  aria-label={`Stage ${file.path}`}
                  title="Stage"
                  onClick={() => onStage(file.path)}
                >
                  <svg className="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                    <path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M12 4v16m8-8H4" />
                  </svg>
                </button>
              )}
              {!file.is_staged && onDiscard && file.status !== "?" && (
                <button
                  className="p-0.5 rounded hover:bg-[color-mix(in_srgb,var(--color-wardian-error),transparent_80%)] text-[var(--color-wardian-text-muted)] hover:text-[var(--color-wardian-error)] transition-colors"
                  aria-label={`Discard changes to ${file.path}`}
                  title="Discard Changes"
                  onClick={() => onDiscard(file.path)}
                >
                  <svg className="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                    <path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M3 10h10a8 8 0 018 8v2M3 10l6 6m-6-6l6-6" />
                  </svg>
                </button>
              )}
            </div>
            <span className={`font-mono font-bold w-4 text-center shrink-0 ${colorClass}`}>
              {file.status}
            </span>
          </li>
        );
      })}
    </ul>
  );
};
