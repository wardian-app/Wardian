import type { SourceAnalysis } from './lib/rust-source.mjs';

export interface Finding {
  key: string;
  file: string;
  line: number;
  name: string;
  parent?: string;
  kind: string;
  source: string;
}

export interface DiagnosticSpan {
  is_primary: boolean;
  label: string | null;
  file_name: string;
  line_start: number;
  text: Array<{ text: string; highlight_start: number; highlight_end: number }>;
}

export function readBaseline(text: string): Map<string, string>;
export function compareWithBaseline(
  findings: Finding[],
  baseline: Map<string, string>,
  isCheckable?: (entry: string) => boolean,
): { added: Finding[]; stale: string[] };
export function rewriteManifestPaths(
  text: string,
  manifestDir: string,
  isCopied: (resolved: string) => boolean,
): string;
export function rewriteVisibility(text: string, keepPublic?: string[]): string;
export function deadCodeFindings(
  message: { message: string; code: { code: string } | null; spans: DiagnosticSpan[] },
  toRepoPath: (fileName: string) => string,
): Finding[];
export function splitCfgGated(findings: Finding[], analyses: SourceAnalysis[]): { kept: Finding[]; gated: Finding[] };
export function registeredCommands(libSource: string): string[];
export function main(argv?: string[]): number;
