import type { ReactNode } from "react";

export function CodePanel({ content, language, className = "" }: { content: string; language: string; className?: string }) {
  return (
    <pre
      className={`mt-2 max-h-[300px] overflow-auto whitespace-pre-wrap break-words rounded border border-wardian-light bg-[var(--color-wardian-sidebar-primary)] p-2 text-[12px] leading-5 text-primary ${className}`}
    >
      <code data-language={language}>{renderHighlightedCode(content, language)}</code>
    </pre>
  );
}

export function renderHighlightedCode(content: string, language: string): ReactNode {
  const normalized = language.toLowerCase();
  const lines = content.split("\n");
  return lines.flatMap((line, index) => {
    const tokens = highlightLine(line, normalized, index);
    return index === lines.length - 1 ? tokens : [...tokens, "\n"];
  });
}

function highlightLine(line: string, language: string, lineIndex: number): ReactNode[] {
  if (language === "diff") return highlightDiffLine(line, lineIndex);
  if (language === "json") return highlightJsonLine(line, lineIndex);
  if (language === "shell" || language === "powershell" || language === "batch" || language === "terminal") {
    return highlightShellLine(line, lineIndex);
  }
  if (language === "rust" || language === "typescript" || language === "javascript" || language === "python") {
    return highlightCodeLine(line, lineIndex);
  }
  return [line];
}

function highlightDiffLine(line: string, lineIndex: number): ReactNode[] {
  if (/^\+[^+]/.test(line)) return [<span className="text-[var(--color-wardian-success)]" data-token="diff-add" key={`diff-${lineIndex}`}>{line}</span>];
  if (/^-[^-]/.test(line)) return [<span className="text-[var(--color-wardian-error)]" data-token="diff-remove" key={`diff-${lineIndex}`}>{line}</span>];
  if (/^@@/.test(line)) return [<span className="text-[var(--color-wardian-accent)]" data-token="diff-hunk" key={`diff-${lineIndex}`}>{line}</span>];
  return [line];
}

function highlightShellLine(line: string, lineIndex: number): ReactNode[] {
  const prompt = /^(\s*(?:\$|>|PS [^>]+>)\s+)(.*)$/.exec(line);
  if (!prompt) return [line];
  return [
    <span className="text-[var(--color-wardian-accent)]" data-token="shell-prompt" key={`shell-prompt-${lineIndex}`}>
      {prompt[1]}
    </span>,
    <span data-token="shell-command" key={`shell-command-${lineIndex}`}>
      {prompt[2]}
    </span>,
  ];
}

function highlightCodeLine(line: string, lineIndex: number): ReactNode[] {
  const pattern = /\b(const|let|var|fn|pub|struct|enum|impl|use|import|from|return|if|else|for|while|async|await|class|def)\b/g;
  const tokens: ReactNode[] = [];
  let lastIndex = 0;
  let match: RegExpExecArray | null;

  while ((match = pattern.exec(line)) !== null) {
    if (match.index > lastIndex) tokens.push(line.slice(lastIndex, match.index));
    tokens.push(
      <span className="text-[var(--color-wardian-accent)]" data-token="keyword" key={`kw-${lineIndex}-${match.index}`}>
        {match[0]}
      </span>,
    );
    lastIndex = match.index + match[0].length;
  }

  if (lastIndex < line.length) tokens.push(line.slice(lastIndex));
  return tokens.length > 0 ? tokens : [line];
}

function highlightJsonLine(line: string, lineIndex: number): ReactNode[] {
  const pattern = /("(?:\\.|[^"\\])*")(\s*:)?|\b(true|false|null)\b|-?\b\d+(?:\.\d+)?\b/g;
  const tokens: ReactNode[] = [];
  let lastIndex = 0;
  let match: RegExpExecArray | null;

  while ((match = pattern.exec(line)) !== null) {
    if (match.index > lastIndex) tokens.push(line.slice(lastIndex, match.index));
    const value = match[0];
    const token = match[2] ? "json-key" : match[3] ? "json-literal" : /^-?\d/.test(value) ? "json-number" : "json-string";
    const className =
      token === "json-key"
        ? "text-[var(--color-wardian-accent)]"
        : token === "json-string"
          ? "text-[var(--color-wardian-success)]"
          : "text-[var(--color-wardian-warning)]";
    tokens.push(
      <span className={className} data-token={token} key={`json-${lineIndex}-${match.index}`}>
        {value}
      </span>,
    );
    lastIndex = match.index + value.length;
  }

  if (lastIndex < line.length) tokens.push(line.slice(lastIndex));
  return tokens.length > 0 ? tokens : [line];
}
